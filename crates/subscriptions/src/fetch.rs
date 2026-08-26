use futures_util::StreamExt;
use node2socks_domain::{AppError, AppResult, ErrorCode};
use reqwest::{Client, header::HeaderMap, redirect::Policy};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Url;

pub const DEFAULT_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub connect_timeout: Duration,
    pub total_timeout: Duration,
    pub max_body_bytes: usize,
    pub max_redirects: usize,
    pub user_agent: String,
    pub headers: HeaderMap,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            total_timeout: Duration::from_secs(30),
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            max_redirects: 5,
            user_agent: format!("Node2Socks/{}", env!("CARGO_PKG_VERSION")),
            headers: HeaderMap::new(),
        }
    }
}

pub struct SubscriptionFetcher {
    client: Client,
    max_body_bytes: usize,
}

impl SubscriptionFetcher {
    pub fn direct(options: FetchOptions) -> AppResult<Self> {
        Self::build(options, None)
    }

    pub fn proxied(options: FetchOptions, proxy_url: &str) -> AppResult<Self> {
        Self::build(options, Some(proxy_url))
    }

    fn build(options: FetchOptions, proxy_url: Option<&str>) -> AppResult<Self> {
        let max_body_bytes = options.max_body_bytes;
        let mut builder = Client::builder()
            .no_proxy()
            .connect_timeout(options.connect_timeout)
            .timeout(options.total_timeout)
            .redirect(Policy::limited(options.max_redirects))
            .user_agent(options.user_agent)
            .default_headers(options.headers);
        if let Some(proxy_url) = proxy_url {
            builder = builder.proxy(reqwest::Proxy::all(proxy_url).map_err(proxy_error)?);
        }
        let client = builder.build().map_err(fetch_error)?;
        Ok(Self {
            client,
            max_body_bytes,
        })
    }
    pub async fn fetch(&self, url: &str) -> AppResult<Vec<u8>> {
        self.fetch_cancellable(url, &CancellationToken::new()).await
    }

    pub async fn fetch_cancellable(
        &self,
        url: &str,
        cancellation: &CancellationToken,
    ) -> AppResult<Vec<u8>> {
        let parsed = Url::parse(url).map_err(|error| {
            AppError::new(
                ErrorCode::InvalidConfiguration,
                format!("invalid subscription URL: {error}"),
            )
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "subscription URL must use http or https",
            ));
        }
        let request = self.client.get(parsed).send();
        let response = tokio::select! {
            response = request => response.map_err(fetch_error)?,
            _ = cancellation.cancelled() => return Err(cancelled()),
        }
        .error_for_status()
        .map_err(fetch_error)?;
        if response
            .content_length()
            .is_some_and(|length| length > self.max_body_bytes as u64)
        {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "subscription body exceeds configured limit",
            ));
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        loop {
            let next = tokio::select! { value = stream.next() => value, _ = cancellation.cancelled() => return Err(cancelled()) };
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(fetch_error)?;
            if body.len().saturating_add(chunk.len()) > self.max_body_bytes {
                return Err(AppError::new(
                    ErrorCode::InvalidConfiguration,
                    "subscription body exceeds configured limit",
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

fn fetch_error(error: reqwest::Error) -> AppError {
    let message = if error.is_timeout() {
        "subscription request timed out"
    } else if error.is_connect() {
        "subscription connection failed"
    } else if error.status().is_some() {
        "subscription server returned an error status"
    } else {
        "subscription request failed"
    };
    AppError::new(ErrorCode::SubscriptionFetchFailed, message)
}

fn proxy_error(error: reqwest::Error) -> AppError {
    AppError::new(
        ErrorCode::InvalidConfiguration,
        format!("invalid custom subscription proxy: {error}"),
    )
}

fn cancelled() -> AppError {
    AppError::new(
        ErrorCode::OperationCancelled,
        "subscription fetch cancelled",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_scheme_is_rejected() {
        let fetcher = SubscriptionFetcher::direct(FetchOptions::default()).unwrap();
        let error = fetcher.fetch("file:///secret").await.unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfiguration);
    }
}
