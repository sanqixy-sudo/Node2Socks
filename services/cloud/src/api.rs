use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const ACCESS_TTL: u64 = 15 * 60;
const REFRESH_TTL: u64 = 30 * 24 * 60 * 60;

#[derive(Clone)]
pub struct CloudState {
    db: Arc<Mutex<Connection>>,
    jwt_secret: Arc<Vec<u8>>,
}
impl CloudState {
    pub fn new(db: Connection, jwt_secret: Vec<u8>) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            jwt_secret: Arc::new(jwt_secret),
        }
    }
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    code: &'static str,
    message: String,
}
struct ApiError(StatusCode, &'static str, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(ApiErrorBody {
                code: self.1,
                message: self.2,
            }),
        )
            .into_response()
    }
}
type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    device_id: String,
    exp: usize,
}
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    email: String,
    password: String,
    device_name: String,
    platform: String,
}
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    email: String,
    password: String,
    device_name: String,
    platform: String,
}
#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    refresh_token: String,
}
#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
    vault: VaultEnvelope,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub device_id: String,
}
#[derive(Debug, Serialize)]
struct DeviceResponse {
    id: String,
    name: String,
    platform: String,
    last_seen_at: u64,
    current: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VaultEnvelope {
    pub kdf: String,
    pub kdf_params_json: String,
    pub salt: String,
    pub wrapped_vault_key: String,
    pub nonce: String,
    pub version: u64,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyncRecord {
    pub record_type: String,
    pub record_id: String,
    pub base_version: u64,
    pub deleted: bool,
    pub ciphertext: String,
    pub nonce: String,
    pub aad_version: u64,
}
#[derive(Debug, Deserialize)]
pub struct PushRequest {
    pub records: Vec<SyncRecord>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct PushedRecord {
    pub record_type: String,
    pub record_id: String,
    pub version: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub record_type: String,
    pub record_id: String,
    pub expected_version: u64,
    pub actual_version: u64,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct PushResponse {
    pub accepted: Vec<PushedRecord>,
    pub conflicts: Vec<ConflictRecord>,
    pub cursor: u64,
}
#[derive(Debug, Deserialize)]
pub struct PullQuery {
    #[serde(default)]
    cursor: u64,
}
#[derive(Debug, Serialize, Deserialize)]
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
#[derive(Debug, Serialize, Deserialize)]
pub struct PullResponse {
    pub events: Vec<PullEvent>,
    pub cursor: u64,
}

pub fn routes(state: CloudState) -> Router {
    Router::new()
        .route("/api/v1/auth/register", post(register))
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/auth/refresh", post(refresh))
        .route("/api/v1/auth/logout", post(logout))
        .route("/api/v1/auth/change-password", post(change_password))
        .route("/api/v1/devices", get(devices))
        .route("/api/v1/devices/{id}", delete(revoke_device))
        .route("/api/v1/vault", get(get_vault).put(put_vault))
        .route("/api/v1/sync/push", post(push_records))
        .route("/api/v1/sync/pull", get(pull_records))
        .route("/api/v1/sync/resolve", post(push_records))
        .with_state(state)
}

async fn register(
    State(state): State<CloudState>,
    Json(request): Json<RegisterRequest>,
) -> ApiResult<Json<TokenResponse>> {
    validate_password(&request.password)?;
    let user = Uuid::new_v4();
    let now = now();
    let hash = hash_password(&request.password)?;
    {
        let mut db = state.db.lock().map_err(lock_error)?;
        let tx = db.transaction().map_err(db_error)?;
        tx.execute("INSERT INTO users(id,email,password_hash,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",params![user.to_string(),request.email.trim(),hash,now]).map_err(|e|if e.to_string().contains("UNIQUE"){ApiError(StatusCode::CONFLICT,"ACCOUNT_EXISTS","account already exists".into())}else{db_error(e)})?;
        tx.commit().map_err(db_error)?;
    }
    issue_tokens(
        &state,
        &user.to_string(),
        &request.device_name,
        &request.platform,
    )
}
async fn login(
    State(state): State<CloudState>,
    Json(request): Json<LoginRequest>,
) -> ApiResult<Json<TokenResponse>> {
    let (user,hash)=state.db.lock().map_err(lock_error)?.query_row("SELECT id,password_hash FROM users WHERE email=?1 COLLATE NOCASE AND disabled_at IS NULL",[request.email.trim()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional().map_err(db_error)?.ok_or_else(auth_error)?;
    Argon2::default()
        .verify_password(
            request.password.as_bytes(),
            &PasswordHash::new(&hash).map_err(auth_internal)?,
        )
        .map_err(|_| auth_error())?;
    issue_tokens(&state, &user, &request.device_name, &request.platform)
}
async fn refresh(
    State(state): State<CloudState>,
    Json(request): Json<RefreshRequest>,
) -> ApiResult<Json<TokenResponse>> {
    let digest = Sha256::digest(request.refresh_token.as_bytes()).to_vec();
    let now = now();
    let (user,device)=state.db.lock().map_err(lock_error)?.query_row("SELECT r.user_id,r.device_id FROM refresh_tokens r JOIN devices d ON d.id=r.device_id WHERE r.token_hash=?1 AND r.revoked_at IS NULL AND r.expires_at>?2 AND d.revoked_at IS NULL",params![digest,now],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional().map_err(db_error)?.ok_or_else(auth_error)?;
    state
        .db
        .lock()
        .map_err(lock_error)?
        .execute(
            "UPDATE refresh_tokens SET revoked_at=?2 WHERE token_hash=?1",
            params![digest, now],
        )
        .map_err(db_error)?;
    issue_tokens_for_device(&state, &user, &device)
}
async fn logout(State(state): State<CloudState>, headers: HeaderMap) -> ApiResult<StatusCode> {
    let claims = authorize(&state, &headers)?;
    state
        .db
        .lock()
        .map_err(lock_error)?
        .execute(
            "UPDATE refresh_tokens SET revoked_at=?2 WHERE device_id=?1 AND revoked_at IS NULL",
            params![claims.device_id, now()],
        )
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn change_password(
    State(state): State<CloudState>,
    headers: HeaderMap,
    Json(request): Json<ChangePasswordRequest>,
) -> ApiResult<StatusCode> {
    validate_password(&request.new_password)?;
    let claims = authorize(&state, &headers)?;
    let old: String = state
        .db
        .lock()
        .map_err(lock_error)?
        .query_row(
            "SELECT password_hash FROM users WHERE id=?1",
            [&claims.sub],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    Argon2::default()
        .verify_password(
            request.current_password.as_bytes(),
            &PasswordHash::new(&old).map_err(auth_internal)?,
        )
        .map_err(|_| auth_error())?;
    if request.vault.kdf != "argon2id" || request.vault.version == 0 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "INVALID_VAULT",
            "unsupported vault envelope".into(),
        ));
    }
    let salt = decode_b64(&request.vault.salt)?;
    let wrapped_key = decode_b64(&request.vault.wrapped_vault_key)?;
    let nonce = decode_b64(&request.vault.nonce)?;
    if nonce.len() != 12 || wrapped_key.is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "INVALID_VAULT",
            "invalid vault nonce or wrapped key".into(),
        ));
    }
    let hash = hash_password(&request.new_password)?;
    let mut db = state.db.lock().map_err(lock_error)?;
    let tx = db.transaction().map_err(db_error)?;
    tx.execute(
        "UPDATE users SET password_hash=?2,updated_at=?3 WHERE id=?1",
        params![claims.sub, hash, now()],
    )
    .map_err(db_error)?;
    tx.execute(
        "UPDATE refresh_tokens SET revoked_at=?2 WHERE user_id=?1 AND revoked_at IS NULL",
        params![claims.sub, now()],
    )
    .map_err(db_error)?;
    tx.execute("INSERT INTO vault_bootstrap(user_id,kdf,kdf_params_json,salt,wrapped_vault_key,nonce,version,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(user_id) DO UPDATE SET kdf=excluded.kdf,kdf_params_json=excluded.kdf_params_json,salt=excluded.salt,wrapped_vault_key=excluded.wrapped_vault_key,nonce=excluded.nonce,version=excluded.version,updated_at=excluded.updated_at",
        params![claims.sub,request.vault.kdf,request.vault.kdf_params_json,salt,wrapped_key,nonce,request.vault.version,now()]
    ).map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn devices(
    State(state): State<CloudState>,
    headers: HeaderMap,
) -> ApiResult<Json<Vec<DeviceResponse>>> {
    let claims = authorize(&state, &headers)?;
    let db = state.db.lock().map_err(lock_error)?;
    let mut query=db.prepare("SELECT id,name,platform,last_seen_at FROM devices WHERE user_id=?1 AND revoked_at IS NULL ORDER BY last_seen_at DESC").map_err(db_error)?;
    let rows = query
        .query_map([&claims.sub], |r| {
            let id = r.get::<_, String>(0)?;
            Ok(DeviceResponse {
                current: id == claims.device_id,
                id,
                name: r.get(1)?,
                platform: r.get(2)?,
                last_seen_at: r.get(3)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    Ok(Json(rows))
}
async fn revoke_device(
    State(state): State<CloudState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let claims = authorize(&state, &headers)?;
    let now = now();
    let changed = state
        .db
        .lock()
        .map_err(lock_error)?
        .execute(
            "UPDATE devices SET revoked_at=?3 WHERE id=?1 AND user_id=?2 AND revoked_at IS NULL",
            params![id, claims.sub, now],
        )
        .map_err(db_error)?;
    if changed == 0 {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "DEVICE_NOT_FOUND",
            "device not found".into(),
        ));
    }
    state
        .db
        .lock()
        .map_err(lock_error)?
        .execute(
            "UPDATE refresh_tokens SET revoked_at=?2 WHERE device_id=?1 AND revoked_at IS NULL",
            params![id, now],
        )
        .map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn get_vault(
    State(state): State<CloudState>,
    headers: HeaderMap,
) -> ApiResult<Json<VaultEnvelope>> {
    let claims = authorize(&state, &headers)?;
    let row=state.db.lock().map_err(lock_error)?.query_row("SELECT kdf,kdf_params_json,salt,wrapped_vault_key,nonce,version FROM vault_bootstrap WHERE user_id=?1",[claims.sub],|r|Ok(VaultEnvelope{kdf:r.get(0)?,kdf_params_json:r.get(1)?,salt:STANDARD_NO_PAD.encode(r.get::<_,Vec<u8>>(2)?),wrapped_vault_key:STANDARD_NO_PAD.encode(r.get::<_,Vec<u8>>(3)?),nonce:STANDARD_NO_PAD.encode(r.get::<_,Vec<u8>>(4)?),version:r.get(5)?})).optional().map_err(db_error)?.ok_or_else(||ApiError(StatusCode::NOT_FOUND,"VAULT_NOT_FOUND","vault not initialized".into()))?;
    Ok(Json(row))
}
async fn put_vault(
    State(state): State<CloudState>,
    headers: HeaderMap,
    Json(value): Json<VaultEnvelope>,
) -> ApiResult<StatusCode> {
    let claims = authorize(&state, &headers)?;
    if value.kdf != "argon2id" || value.version == 0 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "INVALID_VAULT",
            "unsupported vault envelope".into(),
        ));
    }
    let salt = decode_b64(&value.salt)?;
    let key = decode_b64(&value.wrapped_vault_key)?;
    let nonce = decode_b64(&value.nonce)?;
    if nonce.len() != 12 || key.is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "INVALID_VAULT",
            "invalid vault nonce or wrapped key".into(),
        ));
    }
    state.db.lock().map_err(lock_error)?.execute("INSERT INTO vault_bootstrap(user_id,kdf,kdf_params_json,salt,wrapped_vault_key,nonce,version,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(user_id) DO UPDATE SET kdf=excluded.kdf,kdf_params_json=excluded.kdf_params_json,salt=excluded.salt,wrapped_vault_key=excluded.wrapped_vault_key,nonce=excluded.nonce,version=excluded.version,updated_at=excluded.updated_at",params![claims.sub,value.kdf,value.kdf_params_json,salt,key,nonce,value.version,now()]).map_err(db_error)?;
    Ok(StatusCode::NO_CONTENT)
}
async fn push_records(
    State(state): State<CloudState>,
    headers: HeaderMap,
    Json(request): Json<PushRequest>,
) -> ApiResult<Json<PushResponse>> {
    let claims = authorize(&state, &headers)?;
    if request.records.len() > 500 {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "TOO_MANY_RECORDS",
            "maximum 500 records".into(),
        ));
    }
    let mut db = state.db.lock().map_err(lock_error)?;
    let tx = db.transaction().map_err(db_error)?;
    let mut accepted = Vec::new();
    let mut conflicts = Vec::new();
    for record in request.records {
        let current:Option<u64>=tx.query_row("SELECT version FROM sync_objects WHERE user_id=?1 AND object_type=?2 AND object_id=?3",params![claims.sub,record.record_type,record.record_id],|r|r.get(0)).optional().map_err(db_error)?;
        let actual = current.unwrap_or(0);
        if actual != record.base_version {
            conflicts.push(ConflictRecord {
                record_type: record.record_type,
                record_id: record.record_id,
                expected_version: record.base_version,
                actual_version: actual,
            });
            continue;
        }
        let cipher = decode_b64(&record.ciphertext)?;
        let nonce = decode_b64(&record.nonce)?;
        if nonce.len() != 12 {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "INVALID_NONCE",
                "sync nonce must be 12 bytes".into(),
            ));
        }
        let version = actual + 1;
        let timestamp = now();
        tx.execute("INSERT INTO sync_objects(user_id,object_type,object_id,version,deleted,ciphertext,nonce,aad_version,updated_at,updated_by_device_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(user_id,object_type,object_id) DO UPDATE SET version=excluded.version,deleted=excluded.deleted,ciphertext=excluded.ciphertext,nonce=excluded.nonce,aad_version=excluded.aad_version,updated_at=excluded.updated_at,updated_by_device_id=excluded.updated_by_device_id",params![claims.sub,record.record_type,record.record_id,version,record.deleted,cipher,nonce,record.aad_version,timestamp,claims.device_id]).map_err(db_error)?;
        tx.execute("INSERT INTO sync_events(user_id,object_type,object_id,version,deleted,ciphertext,nonce,aad_version,created_at,device_id) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![claims.sub,record.record_type,record.record_id,version,record.deleted,cipher,nonce,record.aad_version,timestamp,claims.device_id]).map_err(db_error)?;
        accepted.push(PushedRecord {
            record_type: record.record_type,
            record_id: record.record_id,
            version,
        });
    }
    let cursor: u64 = tx
        .query_row(
            "SELECT COALESCE(MAX(seq),0) FROM sync_events WHERE user_id=?1",
            [claims.sub],
            |r| r.get(0),
        )
        .map_err(db_error)?;
    tx.commit().map_err(db_error)?;
    Ok(Json(PushResponse {
        accepted,
        conflicts,
        cursor,
    }))
}
async fn pull_records(
    State(state): State<CloudState>,
    headers: HeaderMap,
    Query(query): Query<PullQuery>,
) -> ApiResult<Json<PullResponse>> {
    let claims = authorize(&state, &headers)?;
    let db = state.db.lock().map_err(lock_error)?;
    let mut statement=db.prepare("SELECT seq,object_type,object_id,version,deleted,ciphertext,nonce,aad_version FROM sync_events WHERE user_id=?1 AND seq>?2 ORDER BY seq LIMIT 1000").map_err(db_error)?;
    let events = statement
        .query_map(params![claims.sub, query.cursor], |r| {
            Ok(PullEvent {
                cursor: r.get(0)?,
                record_type: r.get(1)?,
                record_id: r.get(2)?,
                version: r.get(3)?,
                deleted: r.get::<_, i64>(4)? != 0,
                ciphertext: STANDARD_NO_PAD.encode(r.get::<_, Vec<u8>>(5)?),
                nonce: STANDARD_NO_PAD.encode(r.get::<_, Vec<u8>>(6)?),
                aad_version: r.get(7)?,
            })
        })
        .map_err(db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error)?;
    let cursor = events.last().map(|e| e.cursor).unwrap_or(query.cursor);
    Ok(Json(PullResponse { events, cursor }))
}

fn issue_tokens_for_device(
    state: &CloudState,
    user: &str,
    device: &str,
) -> ApiResult<Json<TokenResponse>> {
    let now = now();
    let mut raw = [0_u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let refresh = STANDARD_NO_PAD.encode(raw);
    let digest = Sha256::digest(refresh.as_bytes()).to_vec();
    state.db.lock().map_err(lock_error)?.execute(
        "INSERT INTO refresh_tokens(id,user_id,device_id,token_hash,expires_at,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![Uuid::new_v4().to_string(),user,device,digest,now+REFRESH_TTL,now],
    ).map_err(db_error)?;
    let claims = Claims {
        sub: user.into(),
        device_id: device.into(),
        exp: (now + ACCESS_TTL) as usize,
    };
    let access = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(&state.jwt_secret),
    )
    .map_err(auth_internal)?;
    Ok(Json(TokenResponse {
        access_token: access,
        refresh_token: refresh,
        expires_in: ACCESS_TTL,
        device_id: device.into(),
    }))
}

fn issue_tokens(
    state: &CloudState,
    user: &str,
    name: &str,
    platform: &str,
) -> ApiResult<Json<TokenResponse>> {
    let device = Uuid::new_v4().to_string();
    let now = now();
    let mut raw = [0_u8; 32];
    rand::rng().fill_bytes(&mut raw);
    let refresh = STANDARD_NO_PAD.encode(raw);
    let digest = Sha256::digest(refresh.as_bytes()).to_vec();
    let refresh_id = Uuid::new_v4().to_string();
    let db = state.db.lock().map_err(lock_error)?;
    db.execute("INSERT INTO devices(id,user_id,name,platform,created_at,last_seen_at) VALUES(?1,?2,?3,?4,?5,?5)",params![device,user,name,platform,now]).map_err(db_error)?;
    db.execute("INSERT INTO refresh_tokens(id,user_id,device_id,token_hash,expires_at,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![refresh_id,user,device,digest,now+REFRESH_TTL,now]).map_err(db_error)?;
    let claims = Claims {
        sub: user.into(),
        device_id: device.clone(),
        exp: (now + ACCESS_TTL) as usize,
    };
    let access = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(&state.jwt_secret),
    )
    .map_err(auth_internal)?;
    Ok(Json(TokenResponse {
        access_token: access,
        refresh_token: refresh,
        expires_in: ACCESS_TTL,
        device_id: device,
    }))
}
fn authorize(state: &CloudState, headers: &HeaderMap) -> ApiResult<Claims> {
    let value = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(auth_error)?;
    let claims = decode::<Claims>(
        value,
        &DecodingKey::from_secret(&state.jwt_secret),
        &Validation::default(),
    )
    .map_err(|_| auth_error())?
    .claims;
    let active:bool=state.db.lock().map_err(lock_error)?.query_row("SELECT EXISTS(SELECT 1 FROM devices WHERE id=?1 AND user_id=?2 AND revoked_at IS NULL)",params![claims.device_id,claims.sub],|r|r.get(0)).map_err(db_error)?;
    if !active {
        return Err(auth_error());
    }
    Ok(claims)
}
fn hash_password(value: &str) -> ApiResult<String> {
    let mut salt = [0_u8; 16];
    rand::rng().fill_bytes(&mut salt);
    let salt = SaltString::encode_b64(&salt).map_err(auth_internal)?;
    Argon2::default()
        .hash_password(value.as_bytes(), &salt)
        .map(|v| v.to_string())
        .map_err(auth_internal)
}
fn validate_password(value: &str) -> ApiResult<()> {
    if value.len() < 10 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "WEAK_PASSWORD",
            "password must contain at least 10 characters".into(),
        ));
    }
    Ok(())
}
fn decode_b64(value: &str) -> ApiResult<Vec<u8>> {
    STANDARD_NO_PAD.decode(value).map_err(|_| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "INVALID_BASE64",
            "invalid encrypted record encoding".into(),
        )
    })
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
fn auth_error() -> ApiError {
    ApiError(
        StatusCode::UNAUTHORIZED,
        "AUTH_FAILED",
        "invalid credentials".into(),
    )
}
fn auth_internal(error: impl std::fmt::Display) -> ApiError {
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "AUTH_INTERNAL",
        error.to_string(),
    )
}
fn db_error(error: impl std::fmt::Display) -> ApiError {
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "DATABASE_ERROR",
        error.to_string(),
    )
}
fn lock_error<T>(error: std::sync::PoisonError<T>) -> ApiError {
    db_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;
    fn app() -> Router {
        let mut db = Connection::open_in_memory().unwrap();
        crate::migrate(&mut db).unwrap();
        routes(CloudState::new(
            db,
            b"test-secret-at-least-32-bytes-long".to_vec(),
        ))
    }
    async fn json(
        app: &Router,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }
    #[tokio::test]
    async fn auth_vault_sync_conflict_and_tombstone_roundtrip() {
        let app = app();
        let(status,value)=json(&app,"POST","/api/v1/auth/register",None,serde_json::json!({"email":"a@example.com","password":"long-password","device_name":"A","platform":"windows"})).await;
        assert_eq!(status, StatusCode::OK);
        let initial_device = value["device_id"].as_str().unwrap().to_owned();
        let refresh_token = value["refresh_token"].as_str().unwrap().to_owned();
        let (refresh_status, refreshed) = json(
            &app,
            "POST",
            "/api/v1/auth/refresh",
            None,
            serde_json::json!({"refresh_token": refresh_token}),
        )
        .await;
        assert_eq!(refresh_status, StatusCode::OK);
        assert_eq!(refreshed["device_id"], initial_device);
        let token = refreshed["access_token"].as_str().unwrap().to_owned();
        let rotated_refresh = refreshed["refresh_token"].as_str().unwrap().to_owned();
        assert_ne!(rotated_refresh, refresh_token);
        let vault = serde_json::json!({"kdf":"argon2id","kdf_params_json":"{}","salt":"AQID","wrapped_vault_key":"AQIDBA","nonce":"AQEBAQEBAQEBAQEB","version":1});
        assert_eq!(
            json(&app, "PUT", "/api/v1/vault", Some(&token), vault)
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        let record = serde_json::json!({"records":[{"record_type":"slot","record_id":"s1","base_version":0,"deleted":false,"ciphertext":"AQID","nonce":"AQEBAQEBAQEBAQEB","aad_version":1}]});
        let (_, pushed) = json(
            &app,
            "POST",
            "/api/v1/sync/push",
            Some(&token),
            record.clone(),
        )
        .await;
        assert_eq!(pushed["accepted"][0]["version"], 1);
        let (_, conflict) = json(&app, "POST", "/api/v1/sync/push", Some(&token), record).await;
        assert_eq!(conflict["conflicts"][0]["actual_version"], 1);
        let (_, pulled) = json(
            &app,
            "GET",
            "/api/v1/sync/pull?cursor=0",
            Some(&token),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(pulled["events"][0]["record_id"], "s1");
        let tombstone = serde_json::json!({"records":[{"record_type":"slot","record_id":"s1","base_version":1,"deleted":true,"ciphertext":"AQID","nonce":"AQEBAQEBAQEBAQEB","aad_version":1}]});
        let (_, deleted) = json(&app, "POST", "/api/v1/sync/push", Some(&token), tombstone).await;
        assert_eq!(deleted["accepted"][0]["version"], 2);
        let changed_vault = serde_json::json!({"kdf":"argon2id","kdf_params_json":"{\"updated\":true}","salt":"BAUG","wrapped_vault_key":"BQYHCA","nonce":"AgICAgICAgICAgIC","version":1});
        let (changed_status, _) = json(
            &app,
            "POST",
            "/api/v1/auth/change-password",
            Some(&token),
            serde_json::json!({
                "current_password": "long-password",
                "new_password": "new-long-password",
                "vault": changed_vault,
            }),
        )
        .await;
        assert_eq!(changed_status, StatusCode::NO_CONTENT);
        let (old_login, _) = json(&app,"POST","/api/v1/auth/login",None,serde_json::json!({"email":"a@example.com","password":"long-password","device_name":"old","platform":"windows"})).await;
        assert_eq!(old_login, StatusCode::UNAUTHORIZED);
        let (new_login, new_tokens) = json(&app,"POST","/api/v1/auth/login",None,serde_json::json!({"email":"a@example.com","password":"new-long-password","device_name":"new","platform":"windows"})).await;
        assert_eq!(new_login, StatusCode::OK);
        let new_access = new_tokens["access_token"].as_str().unwrap();
        let (vault_status, stored_vault) = json(
            &app,
            "GET",
            "/api/v1/vault",
            Some(new_access),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(vault_status, StatusCode::OK);
        assert_eq!(stored_vault["kdf_params_json"], "{\"updated\":true}");
        let (revoked_refresh, _) = json(
            &app,
            "POST",
            "/api/v1/auth/refresh",
            None,
            serde_json::json!({"refresh_token": rotated_refresh}),
        )
        .await;
        assert_eq!(revoked_refresh, StatusCode::UNAUTHORIZED);
    }
}
