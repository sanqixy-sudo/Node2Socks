//! Proxy-core abstraction and the Mihomo process-boundary adapter.

use async_trait::async_trait;
use node2socks_domain::{AppResult, CoreHealth};

pub mod controller;
pub mod mihomo;
pub mod provider;
pub mod topology;

#[async_trait]
pub trait ProxyCore: Send + Sync {
    async fn start(&self) -> AppResult<CoreHealth>;
    async fn stop(&self) -> AppResult<()>;
    async fn restart(&self) -> AppResult<CoreHealth>;
    async fn health(&self) -> AppResult<CoreHealth>;
}
