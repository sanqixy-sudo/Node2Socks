use crate::{
    FetchOptions, ProviderBridge, RefreshResult, SubscriptionFetcher, SubscriptionRepository,
    detect_and_normalize,
};
use node2socks_domain::{AppError, AppResult, ErrorCode};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::{sync::Semaphore, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Resolves a localhost SOCKS5 URL that exits through a specific catalog node.
/// Implemented by the desktop layer on top of the running Core: it switches the
/// dedicated download selector to the target node, then returns the listener URL.
pub trait NodeDownloadDialer: Send + Sync {
    fn proxy_url_for(
        &self,
        node_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = AppResult<String>> + Send + '_>>;
}

#[derive(Clone)]
pub struct SubscriptionService {
    repository: SubscriptionRepository,
    bridge: ProviderBridge,
    concurrency: Arc<Semaphore>,
    node_dialer: Option<Arc<dyn NodeDownloadDialer>>,
}

impl SubscriptionService {
    pub fn new(
        repository: SubscriptionRepository,
        bridge: ProviderBridge,
        concurrency: usize,
    ) -> AppResult<Self> {
        if concurrency == 0 {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "refresh concurrency must be positive",
            ));
        }
        Ok(Self {
            repository,
            bridge,
            concurrency: Arc::new(Semaphore::new(concurrency)),
            node_dialer: None,
        })
    }

    pub fn with_node_dialer(mut self, dialer: Arc<dyn NodeDownloadDialer>) -> Self {
        self.node_dialer = Some(dialer);
        self
    }

    pub async fn refresh(&self, id: Uuid, cancel: &CancellationToken) -> AppResult<RefreshResult> {
        let _permit = tokio::select! {result=self.concurrency.acquire()=>result.map_err(|_|AppError::new(ErrorCode::SubscriptionFetchFailed,"refresh queue closed"))?, _=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"subscription refresh cancelled"))};
        let item = self.repository.get(id)?.ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidConfiguration,
                "subscription does not exist",
            )
        })?;
        let mut options = FetchOptions::default();
        if let Some(agent) = item.user_agent.clone() {
            options.user_agent = agent;
        }
        options.headers = build_headers(&item.headers)?;
        let fetcher = match item.download_mode {
            crate::DownloadMode::Direct => SubscriptionFetcher::direct(options)?,
            crate::DownloadMode::System => {
                let proxy_url = crate::system_proxy_url().ok_or_else(|| {
                    AppError::new(
                        ErrorCode::InvalidConfiguration,
                        "system proxy is not enabled; turn it on in Windows settings or switch the subscription to direct download",
                    )
                })?;
                SubscriptionFetcher::proxied(options, &proxy_url)?
            }
            crate::DownloadMode::CustomHttp | crate::DownloadMode::CustomSocks5 => {
                let proxy_url = item.proxy_url.as_deref().ok_or_else(|| {
                    AppError::new(
                        ErrorCode::InvalidConfiguration,
                        "custom proxy URL is required",
                    )
                })?;
                SubscriptionFetcher::proxied(options, proxy_url)?
            }
            crate::DownloadMode::Node => {
                let node_id = item.download_node_id.ok_or_else(|| {
                    AppError::new(
                        ErrorCode::InvalidConfiguration,
                        "通过节点下载需要先在订阅设置中选择用于下载的节点",
                    )
                })?;
                let dialer = self.node_dialer.as_ref().ok_or_else(|| {
                    AppError::new(
                        ErrorCode::CoreNotRunning,
                        "通过节点下载需要 Core 运行，请先启动 Core",
                    )
                })?;
                let proxy_url = dialer.proxy_url_for(node_id).await?;
                SubscriptionFetcher::proxied(options, &proxy_url)?
            }
        };
        let body = match fetcher.fetch_cancellable(&item.url, cancel).await {
            Ok(value) => value,
            Err(error) => {
                self.repository.mark_error(id, &error)?;
                return Err(error);
            }
        };
        let parsed = match detect_and_normalize(id, &body) {
            Ok(value) => value,
            Err(error) => {
                self.repository.mark_error(id, &error)?;
                return Err(error);
            }
        };
        self.bridge
            .set_payload(id, parsed.mihomo_payload.clone())
            .await;
        let format = format!("{:?}", parsed.format).to_ascii_lowercase();
        let diff = self
            .repository
            .apply_nodes(id, &parsed.nodes, &body, &format)?;
        Ok(RefreshResult {
            subscription_id: id,
            node_count: parsed.nodes.len(),
            diff,
        })
    }

    pub async fn restore_bridge(&self) -> AppResult<()> {
        for item in self.repository.list()? {
            if let Some(payload) = self.repository.cached_payload(item.id)? {
                let parsed = detect_and_normalize(item.id, &payload)?;
                self.bridge
                    .set_payload(item.id, parsed.mihomo_payload)
                    .await;
            }
        }
        Ok(())
    }

    pub fn due(&self, epoch_seconds: u64) -> AppResult<Vec<Uuid>> {
        self.repository.due(epoch_seconds)
    }

    pub fn spawn_scheduler(self: Arc<Self>, cancel: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                tokio::select! {_=cancel.cancelled()=>break,_=interval.tick()=>{let now=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_secs()).unwrap_or(0); if let Ok(ids)=self.repository.due(now){for id in ids{let service=self.clone();let child=cancel.child_token();tokio::spawn(async move{let _=service.refresh(id,&child).await;});}}}}
            }
        })
    }
}

fn build_headers(values: &[(String, String)]) -> AppResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in values {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).map_err(config_error)?,
            HeaderValue::from_str(value).map_err(config_error)?,
        );
    }
    Ok(headers)
}
fn config_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidConfiguration, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DownloadMode, SubscriptionRecord};
    use node2socks_crypto::SecretKey;
    use node2socks_storage::open_in_memory_and_migrate;

    #[test]
    fn repository_crud_encrypts_sensitive_fields_and_diffs_nodes() {
        let repository =
            SubscriptionRepository::new(open_in_memory_and_migrate().unwrap(), SecretKey::random());
        let id = Uuid::new_v4();
        let item = SubscriptionRecord {
            id,
            name: "Airport".into(),
            url: "https://example.test/sub?token=secret".into(),
            enabled: true,
            refresh_interval_sec: 1800,
            next_refresh_at: None,
            last_success_at: None,
            last_error: None,
            download_mode: DownloadMode::Direct,
            user_agent: None,
            headers: vec![("X-Key".into(), "secret".into())],
            proxy_url: None,
            download_node_id: None,
            revision: 0,
        };
        repository.upsert(&item).unwrap();
        assert_eq!(repository.get(id).unwrap().unwrap().url, item.url);
        let first=detect_and_normalize(id,b"proxies:\n  - {name: JP, type: ss, server: 1.2.3.4, port: 443, password: p, cipher: aes-128-gcm}\n").unwrap();
        let diff = repository
            .apply_nodes(id, &first.nodes, b"payload", "provider_yaml")
            .unwrap();
        assert_eq!(diff.added.len(), 1);
        let second=detect_and_normalize(id,b"proxies:\n  - {name: US, type: ss, server: 5.6.7.8, port: 443, password: p, cipher: aes-128-gcm}\n").unwrap();
        let diff = repository
            .apply_nodes(id, &second.nodes, b"payload2", "provider_yaml")
            .unwrap();
        assert_eq!((diff.added.len(), diff.disappeared.len()), (1, 1));
        repository.delete(id).unwrap();
        assert!(repository.list().unwrap().is_empty());
    }

    fn node_mode_record(id: Uuid, node_id: Option<Uuid>) -> SubscriptionRecord {
        SubscriptionRecord {
            id,
            name: "Airport".into(),
            url: "https://example.test/sub".into(),
            enabled: true,
            refresh_interval_sec: 1800,
            next_refresh_at: None,
            last_success_at: None,
            last_error: None,
            download_mode: DownloadMode::Node,
            user_agent: None,
            headers: Vec::new(),
            proxy_url: None,
            download_node_id: node_id,
            revision: 0,
        }
    }

    fn service(
        repository: SubscriptionRepository,
        dialer: Option<Arc<dyn NodeDownloadDialer>>,
    ) -> SubscriptionService {
        let service = SubscriptionService::new(repository, ProviderBridge::new(), 4).unwrap();
        match dialer {
            Some(dialer) => service.with_node_dialer(dialer),
            None => service,
        }
    }

    #[test]
    fn node_mode_roundtrips_through_repository() {
        let repository =
            SubscriptionRepository::new(open_in_memory_and_migrate().unwrap(), SecretKey::random());
        let id = Uuid::new_v4();
        let node_id = Uuid::new_v4();
        repository
            .upsert(&node_mode_record(id, Some(node_id)))
            .unwrap();
        let loaded = repository.get(id).unwrap().unwrap();
        assert_eq!(loaded.download_mode, DownloadMode::Node);
        assert_eq!(loaded.download_node_id, Some(node_id));
        // Older sync payloads predate download_node_id and must still parse.
        let legacy = serde_json::json!({
            "id": id,
            "name": "Airport",
            "url": "https://example.test/sub",
            "enabled": true,
            "refresh_interval_sec": 1800,
            "next_refresh_at": null,
            "last_success_at": null,
            "last_error": null,
            "download_mode": "node",
            "user_agent": null,
            "headers": [],
            "revision": 0
        });
        let record: SubscriptionRecord = serde_json::from_value(legacy).unwrap();
        assert_eq!(record.download_mode, DownloadMode::Node);
        assert_eq!(record.download_node_id, None);
    }

    #[tokio::test]
    async fn node_mode_without_selected_node_is_a_structured_error() {
        let repository =
            SubscriptionRepository::new(open_in_memory_and_migrate().unwrap(), SecretKey::random());
        let id = Uuid::new_v4();
        repository.upsert(&node_mode_record(id, None)).unwrap();
        let error = service(repository, None)
            .refresh(id, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfiguration);
        assert!(error.message.contains("选择用于下载的节点"));
    }

    #[tokio::test]
    async fn node_mode_without_dialer_requires_a_running_core() {
        let repository =
            SubscriptionRepository::new(open_in_memory_and_migrate().unwrap(), SecretKey::random());
        let id = Uuid::new_v4();
        repository
            .upsert(&node_mode_record(id, Some(Uuid::new_v4())))
            .unwrap();
        let error = service(repository, None)
            .refresh(id, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::CoreNotRunning);
    }

    struct RecordingDialer {
        calls: std::sync::Mutex<Vec<Uuid>>,
    }

    impl NodeDownloadDialer for RecordingDialer {
        fn proxy_url_for(
            &self,
            node_id: Uuid,
        ) -> Pin<Box<dyn Future<Output = AppResult<String>> + Send + '_>> {
            self.calls.lock().unwrap().push(node_id);
            Box::pin(async move {
                Err(AppError::new(
                    ErrorCode::NodeUnavailable,
                    "dialer stopped here",
                ))
            })
        }
    }

    #[tokio::test]
    async fn node_mode_dials_through_the_injected_dialer() {
        let repository =
            SubscriptionRepository::new(open_in_memory_and_migrate().unwrap(), SecretKey::random());
        let id = Uuid::new_v4();
        let node_id = Uuid::new_v4();
        repository
            .upsert(&node_mode_record(id, Some(node_id)))
            .unwrap();
        let dialer = Arc::new(RecordingDialer {
            calls: std::sync::Mutex::new(Vec::new()),
        });
        let error = service(repository, Some(dialer.clone()))
            .refresh(id, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NodeUnavailable);
        assert_eq!(*dialer.calls.lock().unwrap(), vec![node_id]);
    }
}
