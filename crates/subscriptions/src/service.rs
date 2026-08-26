use crate::{
    FetchOptions, ProviderBridge, RefreshResult, SubscriptionFetcher, SubscriptionRepository,
    detect_and_normalize,
};
use node2socks_domain::{AppError, AppResult, ErrorCode};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Semaphore, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub struct SubscriptionService {
    repository: SubscriptionRepository,
    bridge: ProviderBridge,
    concurrency: Arc<Semaphore>,
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
        })
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
            crate::DownloadMode::CustomHttp | crate::DownloadMode::CustomSocks5 => {
                let proxy_url = item.proxy_url.as_deref().ok_or_else(|| {
                    AppError::new(
                        ErrorCode::InvalidConfiguration,
                        "custom proxy URL is required",
                    )
                })?;
                SubscriptionFetcher::proxied(options, proxy_url)?
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
}
