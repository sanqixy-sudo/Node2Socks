use crate::topology::{CoreTopology, DOWNLOAD_SELECTOR, slot_selector_name};
use node2socks_domain::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderSource {
    pub subscription_id: Uuid,
    pub url: String,
    pub bearer_token: String,
    pub interval_seconds: u64,
}

/// Builds Mihomo providers/listeners/selectors from domain state. Provider URLs must be localhost.
pub fn render_provider_topology(
    topology: &CoreTopology,
    providers: &[ProviderSource],
    controller_port: u16,
    secret: &str,
) -> AppResult<String> {
    topology.validate()?;
    for provider in providers {
        let parsed = reqwest::Url::parse(&provider.url).map_err(config_error)?;
        if !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost")) {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "provider bridge URL must be localhost",
            ));
        }
    }
    let mut out = format!(
        "allow-lan: false\nbind-address: 127.0.0.1\nmode: rule\nlog-level: info\nipv6: false\nexternal-controller: 127.0.0.1:{controller_port}\nsecret: \"{secret}\"\nproxy-providers:\n"
    );
    for provider in providers {
        let name = format!("provider-{}", provider.subscription_id);
        // Provider refresh is owned by Node2Socks; Mihomo must not independently\n        // fetch dynamic names and drift away from persisted Slot bindings.
        out.push_str(&format!("  {name}:\n    type: http\n    url: \"{}\"\n    path: ./providers/{name}.yaml\n    header:\n      Authorization:\n        - \"Bearer {}\"\n",provider.url,provider.bearer_token));
    }
    out.push_str("listeners:\n");
    for slot in &topology.slots {
        let selector = slot_selector_name(slot.id);
        out.push_str(&format!("  - name: {selector}-in\n    type: socks\n    listen: 127.0.0.1\n    port: {}\n    proxy: {selector}\n    udp: false\n",slot.local_port));
    }
    if let Some(port) = topology.download_port {
        out.push_str(&format!("  - name: {DOWNLOAD_SELECTOR}-in\n    type: socks\n    listen: 127.0.0.1\n    port: {port}\n    proxy: {DOWNLOAD_SELECTOR}\n    udp: false\n"));
    }
    out.push_str("proxies: []\nproxy-groups:\n");
    for slot in &topology.slots {
        let selector = slot_selector_name(slot.id);
        out.push_str(&format!(
            "  - name: {selector}\n    type: select\n    proxies:\n      - REJECT\n"
        ));
        if !providers.is_empty() {
            out.push_str("    use:\n");
            for provider in providers {
                out.push_str(&format!("      - provider-{}\n", provider.subscription_id));
            }
        }
        out.push_str(&format!(
            "    default-selected: \"{}\"\n    empty-fallback: REJECT\n",
            yaml_escape(slot.selected.as_deref().unwrap_or("REJECT"))
        ));
    }
    // Dedicated selector for subscription downloads via a node. Like the probe
    // selector it never changes Slot routing and fails closed on REJECT.
    if topology.download_port.is_some() {
        out.push_str(&format!(
            "  - name: {DOWNLOAD_SELECTOR}\n    type: select\n    proxies:\n      - REJECT\n"
        ));
        if !providers.is_empty() {
            out.push_str("    use:\n");
            for provider in providers {
                out.push_str(&format!("      - provider-{}\n", provider.subscription_id));
            }
        }
        out.push_str("    default-selected: REJECT\n    empty-fallback: REJECT\n");
    }
    // Hidden selectors used solely for latency tests. Each worker owns an
    // independent selector so probes can switch and measure concurrently;
    // none of them owns a listener or changes any user Slot route.
    for worker in 0..LATENCY_PROBE_WORKERS {
        let selector = latency_probe_selector(worker);
        out.push_str(&format!(
            "  - name: {selector}\n    type: select\n    proxies:\n      - REJECT\n"
        ));
        if !providers.is_empty() {
            out.push_str("    use:\n");
            for provider in providers {
                out.push_str(&format!("      - provider-{}\n", provider.subscription_id));
            }
        }
        out.push_str("    default-selected: REJECT\n    empty-fallback: REJECT\n");
    }
    out.push_str("rules: []\n");
    Ok(out)
}
pub const LATENCY_PROBE_WORKERS: usize = 6;

pub fn latency_probe_selector(worker: usize) -> String {
    format!("node2socks-probe-{worker}")
}

fn yaml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
fn config_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidConfiguration, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::CoreSlot;
    #[test]
    fn provider_bridge_is_local_only_and_token_stays_in_runtime_output() {
        let subscription = Uuid::new_v4();
        let node = "[abc12345] JP".to_owned();
        let config = render_provider_topology(
            &CoreTopology {
                slots: vec![CoreSlot {
                    id: Uuid::new_v4(),
                    local_port: 21001,
                    selected: Some(node.clone()),
                }],
                available_nodes: vec![node.clone()],
                download_port: None,
            },
            &[ProviderSource {
                subscription_id: subscription,
                url: format!("http://127.0.0.1:4567/provider/{subscription}"),
                bearer_token: "secret".into(),
                interval_seconds: 300,
            }],
            19090,
            "controller",
        )
        .unwrap();
        assert!(config.contains("listen: 127.0.0.1"));
        assert!(config.contains(&node));
        assert!(config.contains("Authorization"));
        assert!(config.contains(&format!("      - provider-{subscription}")));
        assert!(!config.contains(&format!("      - \"{node}\"")));
        assert!(!config.contains("0.0.0.0"));
        assert!(
            render_provider_topology(
                &CoreTopology::default(),
                &[ProviderSource {
                    subscription_id: subscription,
                    url: "https://evil.example/provider".into(),
                    bearer_token: "x".into(),
                    interval_seconds: 60
                }],
                19090,
                "s"
            )
            .is_err()
        );
    }

    #[test]
    fn download_lane_uses_every_provider_and_fails_closed() {
        let subscription = Uuid::new_v4();
        let config = render_provider_topology(
            &CoreTopology {
                slots: vec![],
                available_nodes: vec![],
                download_port: Some(22_051),
            },
            &[ProviderSource {
                subscription_id: subscription,
                url: format!("http://127.0.0.1:4567/provider/{subscription}"),
                bearer_token: "secret".into(),
                interval_seconds: 300,
            }],
            19090,
            "controller",
        )
        .unwrap();
        assert!(config.contains(&format!("  - name: {DOWNLOAD_SELECTOR}-in\n")));
        assert!(config.contains("    port: 22051\n"));
        assert!(config.contains(&format!(
            "  - name: {DOWNLOAD_SELECTOR}\n    type: select\n    proxies:\n      - REJECT\n    use:\n      - provider-{subscription}\n    default-selected: REJECT\n    empty-fallback: REJECT\n"
        )));
        // Without a download port no download listener or selector is rendered.
        let quiet = render_provider_topology(
            &CoreTopology::default(),
            &[ProviderSource {
                subscription_id: subscription,
                url: format!("http://127.0.0.1:4567/provider/{subscription}"),
                bearer_token: "secret".into(),
                interval_seconds: 300,
            }],
            19090,
            "controller",
        )
        .unwrap();
        assert!(!quiet.contains(DOWNLOAD_SELECTOR));
    }
}
