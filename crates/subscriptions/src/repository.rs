use crate::{CatalogNode, DownloadMode, NormalizedNode, ProviderDiff, SubscriptionRecord};
use node2socks_crypto::{SecretKey, decrypt, encrypt};
use node2socks_domain::{AppError, AppResult, ErrorCode};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

#[derive(Clone)]
pub struct SubscriptionRepository {
    connection: Arc<Mutex<Connection>>,
    key: Arc<SecretKey>,
}

impl SubscriptionRepository {
    pub fn new(connection: Connection, key: SecretKey) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
            key: Arc::new(key),
        }
    }

    pub fn upsert(&self, item: &SubscriptionRecord) -> AppResult<()> {
        let now = now()?.to_string();
        let aad = format!("subscription:{}", item.id);
        let url = encrypt(&self.key, item.url.as_bytes(), aad.as_bytes())?;
        let headers = encrypt(
            &self.key,
            &serde_json::to_vec(&item.headers).map_err(db_error)?,
            aad.as_bytes(),
        )?;
        let proxy_url = item
            .proxy_url
            .as_ref()
            .map(|value| encrypt(&self.key, value.as_bytes(), aad.as_bytes()))
            .transpose()?;
        self.connection.lock().map_err(lock_error)?.execute(
            "INSERT INTO subscriptions(id,name,url_cipher,refresh_interval_sec,headers_cipher,enabled,next_refresh_at,download_mode,user_agent,proxy_url_cipher,download_node_id,created_at,updated_at,sync_version) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?12,?13) ON CONFLICT(id) DO UPDATE SET name=excluded.name,url_cipher=excluded.url_cipher,refresh_interval_sec=excluded.refresh_interval_sec,headers_cipher=excluded.headers_cipher,enabled=excluded.enabled,next_refresh_at=excluded.next_refresh_at,download_mode=excluded.download_mode,user_agent=excluded.user_agent,proxy_url_cipher=excluded.proxy_url_cipher,download_node_id=excluded.download_node_id,updated_at=excluded.updated_at,sync_version=excluded.sync_version",
            params![item.id.to_string(),item.name,url,item.refresh_interval_sec,headers,item.enabled,item.next_refresh_at.map(|v|v.to_string()),mode_name(&item.download_mode),item.user_agent,proxy_url,item.download_node_id.map(|id|id.to_string()),now,item.revision]
        ).map_err(db_error)?;
        Ok(())
    }

    pub fn get(&self, id: Uuid) -> AppResult<Option<SubscriptionRecord>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        connection.query_row("SELECT id,name,url_cipher,enabled,refresh_interval_sec,next_refresh_at,last_success_at,last_error,download_mode,user_agent,headers_cipher,proxy_url_cipher,sync_version,download_node_id FROM subscriptions WHERE id=?1", [id.to_string()], |row| {
            let id_text: String = row.get(0)?;
            let id = Uuid::parse_str(&id_text).map_err(convert_error)?;
            let aad = format!("subscription:{id}");
            let url_cipher: Vec<u8> = row.get(2)?;
            let headers_cipher: Option<Vec<u8>> = row.get(10)?;
            let url = String::from_utf8(decrypt(&self.key, &url_cipher, aad.as_bytes()).map_err(convert_app_error)?).map_err(convert_error)?;
            let headers = match headers_cipher { Some(value) => serde_json::from_slice(&decrypt(&self.key,&value,aad.as_bytes()).map_err(convert_app_error)?).map_err(convert_error)?, None => Vec::new() };
            let proxy_cipher: Option<Vec<u8>> = row.get(11)?;
            let proxy_url = proxy_cipher.map(|value| String::from_utf8(decrypt(&self.key,&value,aad.as_bytes()).map_err(convert_app_error)?).map_err(convert_error)).transpose()?;
            let download_node_id = row.get::<_,Option<String>>(13)?.map(|value| Uuid::parse_str(&value).map_err(convert_error)).transpose()?;
            Ok(SubscriptionRecord { id, name:row.get(1)?, url, enabled:row.get::<_,i64>(3)? != 0, refresh_interval_sec:row.get(4)?, next_refresh_at:parse_time(row.get(5)?)?, last_success_at:parse_time(row.get(6)?)?, last_error:row.get(7)?, download_mode:parse_mode(&row.get::<_,String>(8)?)?, user_agent:row.get(9)?, headers, proxy_url, download_node_id, revision:row.get(12)? })
        }).optional().map_err(db_error)
    }

    pub fn list(&self) -> AppResult<Vec<SubscriptionRecord>> {
        let ids = {
            let connection = self.connection.lock().map_err(lock_error)?;
            let mut statement = connection
                .prepare("SELECT id FROM subscriptions ORDER BY name")
                .map_err(db_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_error)?
        };
        ids.into_iter()
            .map(|id| {
                Uuid::parse_str(&id)
                    .map_err(|e| AppError::new(ErrorCode::DatabaseError, e.to_string()))
                    .and_then(|id| {
                        self.get(id)?.ok_or_else(|| {
                            AppError::new(ErrorCode::DatabaseError, "subscription vanished")
                        })
                    })
            })
            .collect()
    }

    pub fn delete(&self, id: Uuid) -> AppResult<()> {
        self.connection
            .lock()
            .map_err(lock_error)?
            .execute("DELETE FROM subscriptions WHERE id=?1", [id.to_string()])
            .map_err(db_error)?;
        Ok(())
    }

    pub fn due(&self, epoch_seconds: u64) -> AppResult<Vec<Uuid>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement = connection.prepare("SELECT id FROM subscriptions WHERE enabled=1 AND refresh_interval_sec>0 AND (next_refresh_at IS NULL OR CAST(next_refresh_at AS INTEGER)<=?1)").map_err(db_error)?;
        statement
            .query_map([epoch_seconds], |row| {
                let value: String = row.get(0)?;
                Uuid::parse_str(&value).map_err(convert_error)
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)
    }

    pub fn apply_nodes(
        &self,
        subscription_id: Uuid,
        nodes: &[NormalizedNode],
        payload: &[u8],
        format: &str,
    ) -> AppResult<ProviderDiff> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let transaction = connection.transaction().map_err(db_error)?;
        let existing = {
            let mut statement = transaction.prepare("SELECT id,stable_key,internal_name,upstream_name,protocol,is_present FROM nodes WHERE subscription_id=?1").map_err(db_error)?;
            statement
                .query_map([subscription_id.to_string()], |row| {
                    Ok((
                        Uuid::parse_str(&row.get::<_, String>(0)?).map_err(convert_error)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)? != 0,
                    ))
                })
                .map_err(db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db_error)?
        };
        let by_key: HashMap<_, _> = existing
            .iter()
            .map(|item| (item.1.clone(), item.clone()))
            .collect();
        let incoming: HashSet<_> = nodes.iter().map(|node| node.stable_key.clone()).collect();
        let timestamp = now()?.to_string();
        let mut diff = ProviderDiff::default();
        for node in nodes {
            if let Some(old) = by_key.get(&node.stable_key) {
                transaction.execute("UPDATE nodes SET internal_name=?2,upstream_name=?3,protocol=?4,last_seen_at=?5,is_present=1,updated_at=?5 WHERE id=?1",params![old.0.to_string(),node.internal_name,node.display_name,node.protocol,timestamp]).map_err(db_error)?;
                if old.2 != node.internal_name
                    || old.3 != node.display_name
                    || old.4 != node.protocol
                    || !old.5
                {
                    diff.updated.push(old.0);
                } else {
                    diff.unchanged.push(old.0);
                }
            } else {
                let id = Uuid::new_v4();
                transaction.execute("INSERT INTO nodes(id,subscription_id,stable_key,internal_name,upstream_name,protocol,provider_name,last_seen_at,is_present,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,1,?8,?8)",params![id.to_string(),subscription_id.to_string(),node.stable_key,node.internal_name,node.display_name,node.protocol,format!("provider-{subscription_id}"),timestamp]).map_err(db_error)?;
                diff.added.push(id);
            }
        }
        for old in &existing {
            if !incoming.contains(&old.1) && old.5 {
                transaction
                    .execute(
                        "UPDATE nodes SET is_present=0,updated_at=?2 WHERE id=?1",
                        params![old.0.to_string(), timestamp],
                    )
                    .map_err(db_error)?;
                diff.disappeared.push(old.0);
            }
        }
        let aad = format!("subscription:{subscription_id}");
        let cached = encrypt(&self.key, payload, aad.as_bytes())?;
        let interval: u64 = transaction
            .query_row(
                "SELECT refresh_interval_sec FROM subscriptions WHERE id=?1",
                [subscription_id.to_string()],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        let current = now()?;
        transaction.execute("UPDATE subscriptions SET content_format=?2,cached_payload_cipher=?3,last_success_at=?4,last_error=NULL,next_refresh_at=?5,updated_at=?4 WHERE id=?1",params![subscription_id.to_string(),format,cached,current.to_string(),(interval > 0).then(|| current.saturating_add(interval).to_string())]).map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(diff)
    }

    pub fn cached_payload(&self, id: Uuid) -> AppResult<Option<Vec<u8>>> {
        let cipher: Option<Vec<u8>> = self
            .connection
            .lock()
            .map_err(lock_error)?
            .query_row(
                "SELECT cached_payload_cipher FROM subscriptions WHERE id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_error)?
            .flatten();
        cipher
            .map(|value| decrypt(&self.key, &value, format!("subscription:{id}").as_bytes()))
            .transpose()
    }

    pub fn mark_error(&self, id: Uuid, error: &AppError) -> AppResult<()> {
        let current = now()?;
        let interval: u64 = self
            .connection
            .lock()
            .map_err(lock_error)?
            .query_row(
                "SELECT refresh_interval_sec FROM subscriptions WHERE id=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .map_err(db_error)?;
        self.connection.lock().map_err(lock_error)?.execute("UPDATE subscriptions SET last_error=?2,next_refresh_at=?3,updated_at=?4 WHERE id=?1",params![id.to_string(),format!("{}",error.code),(interval > 0).then(|| current.saturating_add(interval.min(300)).to_string()),current.to_string()]).map_err(db_error)?;
        Ok(())
    }

    pub fn nodes(&self) -> AppResult<Vec<CatalogNode>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement=connection.prepare("SELECT id,subscription_id,stable_key,internal_name,upstream_name,protocol,is_present FROM nodes ORDER BY upstream_name").map_err(db_error)?;
        statement
            .query_map([], |r| {
                Ok(CatalogNode {
                    id: Uuid::parse_str(&r.get::<_, String>(0)?).map_err(convert_error)?,
                    subscription_id: Uuid::parse_str(&r.get::<_, String>(1)?)
                        .map_err(convert_error)?,
                    stable_key: r.get(2)?,
                    internal_name: r.get(3)?,
                    display_name: r.get(4)?,
                    protocol: r.get(5)?,
                    present: r.get::<_, i64>(6)? != 0,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)
    }
}

fn now() -> AppResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| AppError::new(ErrorCode::DatabaseError, e.to_string()))
}
fn mode_name(mode: &DownloadMode) -> &'static str {
    match mode {
        DownloadMode::Direct => "direct",
        DownloadMode::System => "system",
        DownloadMode::CustomHttp => "custom_http",
        DownloadMode::CustomSocks5 => "custom_socks5",
        DownloadMode::Node => "node",
    }
}
fn parse_mode(value: &str) -> rusqlite::Result<DownloadMode> {
    match value {
        "direct" => Ok(DownloadMode::Direct),
        "system" => Ok(DownloadMode::System),
        "custom_http" => Ok(DownloadMode::CustomHttp),
        "custom_socks5" => Ok(DownloadMode::CustomSocks5),
        "node" => Ok(DownloadMode::Node),
        _ => Err(convert_error(std::io::Error::other("invalid mode"))),
    }
}
fn parse_time(value: Option<String>) -> rusqlite::Result<Option<u64>> {
    value.map(|v| v.parse().map_err(convert_error)).transpose()
}
fn convert_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
fn convert_app_error(error: AppError) -> rusqlite::Error {
    convert_error(std::io::Error::other(error.to_string()))
}
fn db_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::DatabaseError, error.to_string())
}
fn lock_error<T>(error: std::sync::PoisonError<T>) -> AppError {
    db_error(error)
}
