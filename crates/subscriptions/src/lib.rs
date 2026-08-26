pub mod bridge;
pub mod fetch;
pub mod model;
pub mod parser;
pub mod repository;
pub mod service;

pub use bridge::{ProviderBridge, ProviderBridgeHandle};
pub use fetch::{FetchOptions, SubscriptionFetcher};
pub use model::{CatalogNode, DownloadMode, ProviderDiff, RefreshResult, SubscriptionRecord};
pub use parser::{DetectedSubscription, NormalizedNode, SubscriptionFormat, detect_and_normalize};
pub use repository::SubscriptionRepository;
pub use service::SubscriptionService;
