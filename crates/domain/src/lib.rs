//! Product-level models. Mihomo-specific YAML and API payloads do not belong here.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use uuid::Uuid;

pub const DEFAULT_LISTEN_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT_START: u16 = 21_000;
pub const DEFAULT_PORT_END: u16 = 21_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    PortInUse,
    CoreBinaryMissing,
    CoreChecksumFailed,
    CoreStartFailed,
    CoreUnhealthy,
    CoreNotRunning,
    CoreShutdownFailed,
    DatabaseError,
    InvalidConfiguration,
    IoError,
    SubscriptionFetchFailed,
    SubscriptionParseFailed,
    NodeUnavailable,
    SlotOrphaned,
    CloudUnreachable,
    AuthFailed,
    SyncConflict,
    CryptoError,
    OperationCancelled,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", serde_name(*self))
    }
}

fn serde_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::PortInUse => "PORT_IN_USE",
        ErrorCode::CoreBinaryMissing => "CORE_BINARY_MISSING",
        ErrorCode::CoreChecksumFailed => "CORE_CHECKSUM_FAILED",
        ErrorCode::CoreStartFailed => "CORE_START_FAILED",
        ErrorCode::CoreUnhealthy => "CORE_UNHEALTHY",
        ErrorCode::CoreNotRunning => "CORE_NOT_RUNNING",
        ErrorCode::CoreShutdownFailed => "CORE_SHUTDOWN_FAILED",
        ErrorCode::DatabaseError => "DATABASE_ERROR",
        ErrorCode::InvalidConfiguration => "INVALID_CONFIGURATION",
        ErrorCode::IoError => "IO_ERROR",
        ErrorCode::SubscriptionFetchFailed => "SUBSCRIPTION_FETCH_FAILED",
        ErrorCode::SubscriptionParseFailed => "SUBSCRIPTION_PARSE_FAILED",
        ErrorCode::NodeUnavailable => "NODE_UNAVAILABLE",
        ErrorCode::SlotOrphaned => "SLOT_ORPHANED",
        ErrorCode::CloudUnreachable => "CLOUD_UNREACHABLE",
        ErrorCode::AuthFailed => "AUTH_FAILED",
        ErrorCode::SyncConflict => "SYNC_CONFLICT",
        ErrorCode::CryptoError => "CRYPTO_ERROR",
        ErrorCode::OperationCancelled => "OPERATION_CANCELLED",
    }
}

#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: Uuid,
    pub name: String,
    pub enabled: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub id: Uuid,
    pub subscription_id: Uuid,
    pub stable_key: String,
    pub display_name: String,
    pub protocol: String,
    pub is_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxySlot {
    pub id: Uuid,
    pub name: String,
    pub local_port: u16,
    pub listen_host: String,
    pub enabled: bool,
    pub revision: u64,
}

impl ProxySlot {
    pub fn new(name: impl Into<String>, local_port: u16) -> AppResult<Self> {
        if !(1..=65_535).contains(&local_port) {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "slot port must be between 1 and 65535",
            ));
        }
        Ok(Self {
            id: Uuid::new_v4(),
            name: name.into(),
            local_port,
            listen_host: DEFAULT_LISTEN_HOST.to_owned(),
            enabled: true,
            revision: 0,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotBindingState {
    Active,
    Orphaned,
    Unbound,
    Blocked,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotBinding {
    pub slot_id: Uuid,
    pub node_id: Option<Uuid>,
    pub state: SlotBindingState,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreState {
    Stopped,
    Starting,
    Running,
    Unhealthy,
    Crashed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreHealth {
    pub state: CoreState,
    pub pid: Option<u32>,
    pub controller_address: Option<String>,
    pub version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_default_to_localhost() {
        let slot = ProxySlot::new("Primary", 21_001).unwrap();
        assert_eq!(slot.listen_host, "127.0.0.1");
        assert_eq!(slot.local_port, 21_001);
    }
}
