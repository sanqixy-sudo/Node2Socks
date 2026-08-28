use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadMode {
    Direct,
    System,
    CustomHttp,
    CustomSocks5,
    Node,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscriptionRecord {
    pub id: Uuid,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub refresh_interval_sec: u64,
    pub next_refresh_at: Option<u64>,
    pub last_success_at: Option<u64>,
    pub last_error: Option<String>,
    pub download_mode: DownloadMode,
    pub user_agent: Option<String>,
    #[serde(default)]
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub download_node_id: Option<Uuid>,
    pub headers: Vec<(String, String)>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogNode {
    pub id: Uuid,
    pub subscription_id: Uuid,
    pub stable_key: String,
    pub internal_name: String,
    pub display_name: String,
    pub protocol: String,
    pub present: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDiff {
    pub added: Vec<Uuid>,
    pub updated: Vec<Uuid>,
    pub disappeared: Vec<Uuid>,
    pub unchanged: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshResult {
    pub subscription_id: Uuid,
    pub node_count: usize,
    pub diff: ProviderDiff,
}
