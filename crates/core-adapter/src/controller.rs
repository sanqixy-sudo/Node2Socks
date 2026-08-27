use node2socks_domain::{AppError, AppResult, ErrorCode};
use reqwest::{Client, StatusCode};
use serde_json::json;
use std::time::Duration;

#[derive(Clone)]
pub struct MihomoController {
    base_url: String,
    secret: String,
    client: Client,
}

impl MihomoController {
    pub fn new(port: u16, secret: String) -> AppResult<Self> {
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|error| AppError::new(ErrorCode::InvalidConfiguration, error.to_string()))?;
        Ok(Self {
            base_url: format!("http://127.0.0.1:{port}"),
            secret,
            client,
        })
    }

    pub async fn select(&self, selector: &str, internal_node: &str) -> AppResult<()> {
        let response = self
            .client
            .put(format!("{}/proxies/{selector}", self.base_url))
            .bearer_auth(&self.secret)
            .json(&json!({ "name": internal_node }))
            .send()
            .await
            .map_err(network_error)?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(AppError::new(
                ErrorCode::CoreUnhealthy,
                format!("selector switch returned HTTP {}", response.status()),
            ));
        }
        Ok(())
    }

    pub async fn selected(&self, selector: &str) -> AppResult<String> {
        let value = self
            .client
            .get(format!("{}/proxies/{selector}", self.base_url))
            .bearer_auth(&self.secret)
            .send()
            .await
            .map_err(network_error)?
            .error_for_status()
            .map_err(network_error)?
            .json::<serde_json::Value>()
            .await
            .map_err(network_error)?;
        value["now"]
            .as_str()
            .map(ToOwned::to_owned)
            .ok_or_else(|| AppError::new(ErrorCode::CoreUnhealthy, "selector response omitted now"))
    }

    pub async fn delay(
        &self,
        internal_node: &str,
        test_url: &str,
        timeout_duration: Duration,
    ) -> AppResult<u64> {
        let url = self.delay_url(internal_node, test_url, timeout_duration)?;
        let mut last_error = None;
        for attempt in 0..4 {
            match self
                .client
                .get(url.clone())
                .bearer_auth(&self.secret)
                .timeout(timeout_duration + Duration::from_secs(1))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    let value = response
                        .json::<serde_json::Value>()
                        .await
                        .map_err(network_error)?;
                    return value["delay"].as_u64().ok_or_else(|| {
                        AppError::new(
                            ErrorCode::CoreUnhealthy,
                            "delay response omitted a valid delay",
                        )
                    });
                }
                Ok(response) => {
                    last_error = Some(format!("HTTP {}", response.status()));
                    if response.status() != StatusCode::NOT_FOUND || attempt == 3 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(350 * (attempt + 1) as u64)).await;
                }
                Err(error) => {
                    last_error = Some(error.to_string());
                    if attempt == 3 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250 * (attempt + 1) as u64)).await;
                }
            }
        }
        Err(AppError::new(
            ErrorCode::CoreUnhealthy,
            format!(
                "delay probe failed for {internal_node}: {}",
                last_error.unwrap_or_else(|| "unknown error".into())
            ),
        ))
    }
    fn delay_url(
        &self,
        internal_node: &str,
        test_url: &str,
        timeout_duration: Duration,
    ) -> AppResult<reqwest::Url> {
        let mut url = reqwest::Url::parse(&self.base_url)
            .map_err(|error| AppError::new(ErrorCode::InvalidConfiguration, error.to_string()))?;
        url.path_segments_mut()
            .map_err(|_| AppError::new(ErrorCode::InvalidConfiguration, "invalid controller URL"))?
            .extend(["proxies", internal_node, "delay"]);
        url.query_pairs_mut()
            .append_pair(
                "timeout",
                &timeout_duration
                    .as_millis()
                    .min(u128::from(u32::MAX))
                    .to_string(),
            )
            .append_pair("url", test_url);
        Ok(url)
    }

    /// Probe a Provider node through a dedicated selector. Mihomo does not
    /// expose Provider members under the direct delay endpoint.
    pub async fn delay_via_selector(
        &self,
        selector: &str,
        internal_node: &str,
        test_url: &str,
        timeout_duration: Duration,
    ) -> AppResult<u64> {
        self.select(selector, internal_node).await?;
        self.delay(selector, test_url, timeout_duration).await
    }

    pub async fn refresh_provider(&self, provider: &str) -> AppResult<()> {
        let response = self
            .client
            .put(format!("{}/providers/proxies/{provider}", self.base_url))
            .bearer_auth(&self.secret)
            .send()
            .await
            .map_err(network_error)?;
        if !response.status().is_success() {
            return Err(AppError::new(
                ErrorCode::CoreUnhealthy,
                format!("provider refresh returned HTTP {}", response.status()),
            ));
        }
        Ok(())
    }
}

fn network_error(error: reqwest::Error) -> AppError {
    AppError::new(ErrorCode::CoreUnhealthy, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_url_encodes_node_and_test_url() {
        let controller = MihomoController::new(9090, "secret".into()).unwrap();
        let url = controller
            .delay_url(
                "[provider] 香港 节点/01",
                "https://www.gstatic.com/generate_204",
                Duration::from_secs(5),
            )
            .unwrap();
        let segments: Vec<_> = url.path_segments().unwrap().collect();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], "proxies");
        assert_eq!(segments[2], "delay");
        assert!(!url.as_str().contains(' '));
        assert!(!url.as_str().contains("香港"));
        assert!(url.as_str().contains("%2F01"));
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "timeout")
                .map(|(_, value)| value.into_owned()),
            Some("5000".into())
        );
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "url")
                .map(|(_, value)| value.into_owned()),
            Some("https://www.gstatic.com/generate_204".into())
        );
    }
}
