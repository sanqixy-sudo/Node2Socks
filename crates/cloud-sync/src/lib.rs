//! Offline-first encrypted outbox and configurable Cloud client.

use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use node2socks_crypto::{Envelope, SecretKey, encrypt};
use node2socks_domain::{AppError, AppResult, ErrorCode};
use rand::RngCore;
use reqwest::Client;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxRecord {
    pub id: i64,
    pub record_type: String,
    pub record_id: String,
    pub operation: String,
    pub base_version: u64,
    pub payload_cipher: Vec<u8>,
    pub attempts: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudDevice {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub last_seen_at: u64,
    pub current: bool,
}

#[derive(Debug, Serialize)]
struct AuthRequest<'a> {
    email: &'a str,
    password: &'a str,
    device_name: &'a str,
    platform: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullEvent {
    pub cursor: u64,
    pub record_type: String,
    pub record_id: String,
    pub version: u64,
    pub deleted: bool,
    pub ciphertext: String,
    pub nonce: String,
    pub aad_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultEnvelope {
    pub kdf: String,
    pub kdf_params_json: String,
    pub salt: String,
    pub wrapped_vault_key: String,
    pub nonce: String,
    pub version: u64,
}

pub struct Outbox {
    connection: Arc<Mutex<Connection>>,
}
impl Outbox {
    pub fn new(connection: Connection) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
        }
    }
    pub fn enqueue_json(
        &self,
        key: &SecretKey,
        record_type: &str,
        record_id: Uuid,
        base_version: u64,
        deleted: bool,
        payload: &serde_json::Value,
    ) -> AppResult<()> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let base_version = connection
            .query_row(
                "SELECT cloud_version FROM sync_versions WHERE record_type=?1 AND record_id=?2",
                params![record_type, record_id.to_string()],
                |row| row.get(0),
            )
            .unwrap_or(base_version);
        let aad = format!("{record_type}:{record_id}:{base_version}");
        let cipher = encrypt(
            key,
            &serde_json::to_vec(payload).map_err(config_error)?,
            aad.as_bytes(),
        )?;
        connection
            .execute(
                "DELETE FROM sync_outbox WHERE record_type=?1 AND record_id=?2",
                params![record_type, record_id.to_string()],
            )
            .map_err(db_error)?;
        connection.execute("INSERT INTO sync_outbox(record_type,record_id,operation,base_version,payload_cipher,created_at) VALUES(?1,?2,?3,?4,?5,strftime('%s','now'))",params![record_type,record_id.to_string(),if deleted{"delete"}else{"upsert"},base_version,cipher]).map_err(db_error)?;
        Ok(())
    }
    pub fn pending(&self, limit: usize) -> AppResult<Vec<OutboxRecord>> {
        let connection = self.connection.lock().map_err(lock_error)?;
        let mut statement=connection.prepare("SELECT id,record_type,record_id,operation,base_version,payload_cipher,attempts FROM sync_outbox ORDER BY id LIMIT ?1").map_err(db_error)?;
        statement
            .query_map([limit], |r| {
                Ok(OutboxRecord {
                    id: r.get(0)?,
                    record_type: r.get(1)?,
                    record_id: r.get(2)?,
                    operation: r.get(3)?,
                    base_version: r.get(4)?,
                    payload_cipher: r.get(5)?,
                    attempts: r.get(6)?,
                })
            })
            .map_err(db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_error)
    }
    fn acknowledge_versions(
        &self,
        pending: &[OutboxRecord],
        accepted: &[serde_json::Value],
    ) -> AppResult<()> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let tx = connection.transaction().map_err(db_error)?;
        for item in accepted {
            let Some(record_type) = item.get("record_type").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(record_id) = item.get("record_id").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(version) = item.get("version").and_then(|v| v.as_u64()) else {
                continue;
            };
            tx.execute("INSERT INTO sync_versions(record_type,record_id,cloud_version) VALUES(?1,?2,?3) ON CONFLICT(record_type,record_id) DO UPDATE SET cloud_version=excluded.cloud_version",params![record_type,record_id,version]).map_err(db_error)?;
            if let Some(pending) = pending.iter().find(|pending| {
                pending.record_type == record_type && pending.record_id == record_id
            }) {
                tx.execute("DELETE FROM sync_outbox WHERE id=?1", [pending.id])
                    .map_err(db_error)?;
            }
        }
        tx.commit().map_err(db_error)
    }
    pub fn acknowledge(&self, ids: &[i64]) -> AppResult<()> {
        let mut connection = self.connection.lock().map_err(lock_error)?;
        let tx = connection.transaction().map_err(db_error)?;
        for id in ids {
            tx.execute("DELETE FROM sync_outbox WHERE id=?1", [id])
                .map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(())
    }
    pub fn mark_failed(&self, id: i64, error: &AppError) -> AppResult<()> {
        self.connection
            .lock()
            .map_err(lock_error)?
            .execute(
                "UPDATE sync_outbox SET attempts=attempts+1,last_error=?2 WHERE id=?1",
                params![id, error.code.to_string()],
            )
            .map_err(db_error)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct CloudClient {
    base: Url,
    client: Client,
}
impl CloudClient {
    pub fn new(base: &str, development_http: bool) -> AppResult<Self> {
        let mut url = Url::parse(base).map_err(config_error)?;
        if url.query().is_some() || url.fragment().is_some() {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "Cloud base URL cannot contain query or fragment",
            ));
        }
        if url.scheme() != "https"
            && !(development_http && matches!(url.host_str(), Some("localhost" | "127.0.0.1")))
        {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "Cloud URL must use HTTPS except localhost development",
            ));
        }
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(config_error)?;
        Ok(Self { base: url, client })
    }
    pub async fn server_info(&self, cancel: &CancellationToken) -> AppResult<serde_json::Value> {
        let url = self.base.join("api/v1/server-info").map_err(config_error)?;
        let request = self.client.get(url).send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud request cancelled"))};
        response
            .error_for_status()
            .map_err(network_error)?
            .json()
            .await
            .map_err(network_error)
    }
    pub async fn register(
        &self,
        email: &str,
        password: &str,
        device_name: &str,
        cancel: &CancellationToken,
    ) -> AppResult<AuthTokens> {
        self.authenticate("api/v1/auth/register", email, password, device_name, cancel)
            .await
    }

    pub async fn login(
        &self,
        email: &str,
        password: &str,
        device_name: &str,
        cancel: &CancellationToken,
    ) -> AppResult<AuthTokens> {
        self.authenticate("api/v1/auth/login", email, password, device_name, cancel)
            .await
    }

    async fn authenticate(
        &self,
        path: &str,
        email: &str,
        password: &str,
        device_name: &str,
        cancel: &CancellationToken,
    ) -> AppResult<AuthTokens> {
        let url = self.base.join(path).map_err(config_error)?;
        let request = self
            .client
            .post(url)
            .json(&AuthRequest {
                email,
                password,
                device_name,
                platform: "windows",
            })
            .send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud request cancelled"))};
        response
            .error_for_status()
            .map_err(network_error)?
            .json()
            .await
            .map_err(network_error)
    }

    pub async fn refresh(
        &self,
        refresh_token: &str,
        cancel: &CancellationToken,
    ) -> AppResult<AuthTokens> {
        let url = self
            .base
            .join("api/v1/auth/refresh")
            .map_err(config_error)?;
        let request = self
            .client
            .post(url)
            .json(&serde_json::json!({"refresh_token": refresh_token}))
            .send();
        let response = tokio::select! {
            response = request => response.map_err(network_error)?,
            _ = cancel.cancelled() => return Err(AppError::new(
                ErrorCode::OperationCancelled,
                "cloud token refresh cancelled",
            )),
        };
        response
            .error_for_status()
            .map_err(network_error)?
            .json()
            .await
            .map_err(network_error)
    }

    pub async fn devices(
        &self,
        token: &str,
        cancel: &CancellationToken,
    ) -> AppResult<Vec<CloudDevice>> {
        let url = self.base.join("api/v1/devices").map_err(config_error)?;
        let request = self.client.get(url).bearer_auth(token).send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud request cancelled"))};
        response
            .error_for_status()
            .map_err(network_error)?
            .json()
            .await
            .map_err(network_error)
    }

    pub async fn revoke_device(
        &self,
        token: &str,
        device_id: &str,
        cancel: &CancellationToken,
    ) -> AppResult<()> {
        let path = format!("api/v1/devices/{device_id}");
        let url = self.base.join(&path).map_err(config_error)?;
        let request = self.client.delete(url).bearer_auth(token).send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud request cancelled"))};
        response.error_for_status().map_err(network_error)?;
        Ok(())
    }

    pub async fn create_vault(
        &self,
        token: &str,
        password: &str,
        vault_key: &SecretKey,
        cancel: &CancellationToken,
    ) -> AppResult<()> {
        let value = vault_envelope(password, vault_key)?;
        let url = self.base.join("api/v1/vault").map_err(config_error)?;
        let request = self.client.put(url).bearer_auth(token).json(&value).send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud request cancelled"))};
        response.error_for_status().map_err(network_error)?;
        Ok(())
    }

    pub async fn unlock_vault(
        &self,
        token: &str,
        password: &str,
        cancel: &CancellationToken,
    ) -> AppResult<SecretKey> {
        let url = self.base.join("api/v1/vault").map_err(config_error)?;
        let request = self.client.get(url).bearer_auth(token).send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud request cancelled"))};
        let value = response
            .error_for_status()
            .map_err(network_error)?
            .json::<VaultEnvelope>()
            .await
            .map_err(network_error)?;
        if value.kdf != "argon2id" || value.version != 1 {
            return Err(AppError::new(
                ErrorCode::CryptoError,
                "unsupported cloud vault format",
            ));
        }
        let salt = STANDARD_NO_PAD.decode(value.salt).map_err(config_error)?;
        let wrapping = SecretKey::derive(password.as_bytes(), &salt)?;
        let wrapped = serde_json::to_vec(&Envelope {
            version: 1,
            nonce: value.nonce,
            ciphertext: value.wrapped_vault_key,
        })
        .map_err(config_error)?;
        SecretKey::unwrap(&wrapping, &wrapped, b"vault-bootstrap-v1")
    }

    pub async fn change_password(
        &self,
        token: &str,
        current_password: &str,
        new_password: &str,
        vault_key: &SecretKey,
        cancel: &CancellationToken,
    ) -> AppResult<()> {
        let vault = vault_envelope(new_password, vault_key)?;
        let url = self
            .base
            .join("api/v1/auth/change-password")
            .map_err(config_error)?;
        let request = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(&serde_json::json!({
                "current_password": current_password,
                "new_password": new_password,
                "vault": vault,
            }))
            .send();
        let response = tokio::select! {
            response = request => response.map_err(network_error)?,
            _ = cancel.cancelled() => return Err(AppError::new(
                ErrorCode::OperationCancelled,
                "cloud password change cancelled",
            )),
        };
        response.error_for_status().map_err(network_error)?;
        Ok(())
    }

    pub async fn pull(
        &self,
        token: &str,
        cursor: u64,
        cancel: &CancellationToken,
    ) -> AppResult<Vec<PullEvent>> {
        let mut url = self.base.join("api/v1/sync/pull").map_err(config_error)?;
        url.query_pairs_mut()
            .append_pair("cursor", &cursor.to_string());
        let request = self.client.get(url).bearer_auth(token).send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud request cancelled"))};
        let value = response
            .error_for_status()
            .map_err(network_error)?
            .json::<serde_json::Value>()
            .await
            .map_err(network_error)?;
        serde_json::from_value(
            value
                .get("events")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
        )
        .map_err(config_error)
    }

    pub async fn logout(&self, token: &str, cancel: &CancellationToken) -> AppResult<()> {
        let url = self.base.join("api/v1/auth/logout").map_err(config_error)?;
        let request = self.client.post(url).bearer_auth(token).send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud request cancelled"))};
        response.error_for_status().map_err(network_error)?;
        Ok(())
    }
    pub async fn push_outbox(
        &self,
        token: &str,
        outbox: &Outbox,
        cancel: &CancellationToken,
    ) -> AppResult<usize> {
        let pending = outbox.pending(500)?;
        if pending.is_empty() {
            return Ok(0);
        }
        let records: Vec<_> = pending
            .iter()
            .map(|item| {
                let envelope: Envelope =
                    serde_json::from_slice(&item.payload_cipher).map_err(config_error)?;
                Ok(
                    serde_json::json!({"record_type":item.record_type,"record_id":item.record_id,
                "base_version":item.base_version,"deleted":item.operation=="delete",
                "ciphertext":envelope.ciphertext,"nonce":envelope.nonce,
                "aad_version":envelope.version}),
                )
            })
            .collect::<AppResult<Vec<_>>>()?;
        let url = self.base.join("api/v1/sync/push").map_err(config_error)?;
        let request = self
            .client
            .post(url)
            .bearer_auth(token)
            .json(&serde_json::json!({"records":records}))
            .send();
        let response = tokio::select! {response=request=>response.map_err(network_error)?,_=cancel.cancelled()=>return Err(AppError::new(ErrorCode::OperationCancelled,"cloud sync cancelled"))};
        let value = response
            .error_for_status()
            .map_err(network_error)?
            .json::<serde_json::Value>()
            .await
            .map_err(network_error)?;
        let accepted = value["accepted"].as_array().cloned().unwrap_or_default();
        outbox.acknowledge_versions(&pending, &accepted)?;
        if value["conflicts"].as_array().is_some_and(|v| !v.is_empty()) {
            return Err(AppError::new(
                ErrorCode::SyncConflict,
                "cloud rejected one or more base versions",
            ));
        }
        Ok(accepted.len())
    }
}
fn vault_envelope(password: &str, vault_key: &SecretKey) -> AppResult<VaultEnvelope> {
    let mut salt = [0_u8; 16];
    rand::rng().fill_bytes(&mut salt);
    let wrapping = SecretKey::derive(password.as_bytes(), &salt)?;
    let wrapped = vault_key.wrap(&wrapping, b"vault-bootstrap-v1")?;
    let envelope: Envelope = serde_json::from_slice(&wrapped).map_err(config_error)?;
    Ok(VaultEnvelope {
        kdf: "argon2id".into(),
        kdf_params_json: r#"{"memory_kib":65536,"iterations":3,"parallelism":1}"#.into(),
        salt: STANDARD_NO_PAD.encode(salt),
        wrapped_vault_key: envelope.ciphertext,
        nonce: envelope.nonce,
        version: 1,
    })
}
fn config_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidConfiguration, error.to_string())
}
fn network_error(error: reqwest::Error) -> AppError {
    AppError::new(ErrorCode::CloudUnreachable, error.to_string())
}
fn db_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::DatabaseError, error.to_string())
}
fn lock_error<T>(error: std::sync::PoisonError<T>) -> AppError {
    db_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use node2socks_storage::open_in_memory_and_migrate;
    #[test]
    fn offline_queue_preserves_upsert_and_tombstone() {
        let outbox = Outbox::new(open_in_memory_and_migrate().unwrap());
        let key = SecretKey::random();
        outbox
            .enqueue_json(
                &key,
                "slot",
                Uuid::new_v4(),
                0,
                false,
                &serde_json::json!({"port":21001}),
            )
            .unwrap();
        outbox
            .enqueue_json(
                &key,
                "slot",
                Uuid::new_v4(),
                1,
                true,
                &serde_json::json!({}),
            )
            .unwrap();
        let values = outbox.pending(10).unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[1].operation, "delete");
        outbox.acknowledge(&[values[0].id]).unwrap();
        assert_eq!(outbox.pending(10).unwrap().len(), 1)
    }
    #[test]
    fn cloud_base_url_requires_https_except_local_development() {
        assert!(CloudClient::new("http://sync.example.com", false).is_err());
        assert!(CloudClient::new("http://127.0.0.1:8080", true).is_ok());
        assert!(CloudClient::new("https://sync.example.com", false).is_ok())
    }

    #[test]
    fn outbox_uses_cloud_version_in_aad_and_replaces_same_object() {
        let connection = open_in_memory_and_migrate().unwrap();
        let id = Uuid::new_v4();
        connection
            .execute(
                "INSERT INTO sync_versions(record_type,record_id,cloud_version) VALUES('slot',?1,7)",
                [id.to_string()],
            )
            .unwrap();
        let key = SecretKey::random();
        let outbox = Outbox::new(connection);
        let payload = serde_json::json!({"port": 21001});
        outbox
            .enqueue_json(&key, "slot", id, 99, false, &payload)
            .unwrap();
        let first = outbox.pending(10).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].base_version, 7);
        let aad = format!("slot:{id}:7");
        let plain =
            node2socks_crypto::decrypt(&key, &first[0].payload_cipher, aad.as_bytes()).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&plain).unwrap(),
            payload
        );
        assert!(
            node2socks_crypto::decrypt(&key, &first[0].payload_cipher, b"slot:wrong:7").is_err()
        );

        outbox
            .enqueue_json(&key, "slot", id, 99, true, &serde_json::json!({}))
            .unwrap();
        let replaced = outbox.pending(10).unwrap();
        assert_eq!(replaced.len(), 1);
        assert_eq!(replaced[0].operation, "delete");
        assert_eq!(replaced[0].base_version, 7);
    }

    #[test]
    fn partial_acceptance_removes_only_confirmed_records() {
        let outbox = Outbox::new(open_in_memory_and_migrate().unwrap());
        let key = SecretKey::random();
        let accepted_id = Uuid::new_v4();
        let conflict_id = Uuid::new_v4();
        for id in [accepted_id, conflict_id] {
            outbox
                .enqueue_json(&key, "slot", id, 0, false, &serde_json::json!({}))
                .unwrap();
        }
        let pending = outbox.pending(10).unwrap();
        outbox
            .acknowledge_versions(
                &pending,
                &[serde_json::json!({
                    "record_type": "slot",
                    "record_id": accepted_id,
                    "version": 1
                })],
            )
            .unwrap();
        let remaining = outbox.pending(10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].record_id, conflict_id.to_string());
    }
}
