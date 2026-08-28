use node2socks_domain::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreSlot {
    pub id: Uuid,
    pub local_port: u16,
    pub selected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CoreTopology {
    pub slots: Vec<CoreSlot>,
    /// Mihomo internal proxy names exposed by current normalized providers.
    pub available_nodes: Vec<String>,
    /// Dedicated localhost SOCKS port for subscription downloads via a node.
    /// Present only when at least one subscription downloads via a node.
    #[serde(default)]
    pub download_port: Option<u16>,
}

impl CoreTopology {
    pub fn validate(&self) -> AppResult<()> {
        let mut ports = std::collections::HashSet::new();
        for slot in &self.slots {
            if slot.local_port == 0 || !ports.insert(slot.local_port) {
                return Err(AppError::new(
                    ErrorCode::InvalidConfiguration,
                    "Core topology contains a zero or duplicate Slot port",
                ));
            }
            if let Some(selected) = &slot.selected
                && !self.available_nodes.contains(selected)
            {
                return Err(AppError::new(
                    ErrorCode::InvalidConfiguration,
                    format!("Slot selected node is absent: {selected}"),
                ));
            }
        }
        if let Some(port) = self.download_port
            && (port == 0 || !ports.insert(port))
        {
            return Err(AppError::new(
                ErrorCode::InvalidConfiguration,
                "Core topology download port is zero or collides with a Slot port",
            ));
        }
        Ok(())
    }
}

pub fn slot_selector_name(id: Uuid) -> String {
    format!("slot-{id}")
}

/// Fixed selector routing subscription downloads through a chosen node.
/// Fail-closed: without an explicit selection it resolves to REJECT.
pub const DOWNLOAD_SELECTOR: &str = "n2s-download";

/// Build only Mihomo runtime output. Product state must never be parsed back from it.
pub fn render_topology(
    topology: &CoreTopology,
    controller_port: u16,
    secret: &str,
) -> AppResult<String> {
    topology.validate()?;
    let mut output = format!(
        concat!(
            "allow-lan: false\n",
            "bind-address: 127.0.0.1\n",
            "mode: rule\n",
            "log-level: info\n",
            "ipv6: false\n",
            "external-controller: 127.0.0.1:{}\n",
            "secret: \"{}\"\n",
        ),
        controller_port, secret
    );
    output.push_str("listeners:\n");
    for slot in &topology.slots {
        let selector = slot_selector_name(slot.id);
        output.push_str(&format!("  - name: {selector}-in\n"));
        output.push_str("    type: socks\n");
        output.push_str("    listen: 127.0.0.1\n");
        output.push_str(&format!("    port: {}\n", slot.local_port));
        output.push_str(&format!("    proxy: {selector}\n"));
        output.push_str("    udp: false\n");
    }
    if let Some(port) = topology.download_port {
        output.push_str(&format!("  - name: {DOWNLOAD_SELECTOR}-in\n"));
        output.push_str("    type: socks\n");
        output.push_str("    listen: 127.0.0.1\n");
        output.push_str(&format!("    port: {port}\n"));
        output.push_str(&format!("    proxy: {DOWNLOAD_SELECTOR}\n"));
        output.push_str("    udp: false\n");
    }
    output.push_str("proxies: []\nproxy-groups:\n");
    for slot in &topology.slots {
        let selector = slot_selector_name(slot.id);
        let selected = slot.selected.as_deref().unwrap_or("REJECT");
        output.push_str(&format!("  - name: {selector}\n"));
        output.push_str("    type: select\n");
        output.push_str("    proxies:\n      - REJECT\n      - DIRECT\n");
        output.push_str(&format!("    default-selected: {selected}\n"));
        output.push_str("    empty-fallback: REJECT\n");
    }
    if topology.download_port.is_some() {
        output.push_str(&format!("  - name: {DOWNLOAD_SELECTOR}\n"));
        output.push_str("    type: select\n");
        output.push_str("    proxies:\n      - REJECT\n");
        output.push_str("    default-selected: REJECT\n");
        output.push_str("    empty-fallback: REJECT\n");
    }
    output.push_str("rules: []\n");
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_is_localhost_only_and_has_one_selector_per_slot() {
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let yaml = render_topology(
            &CoreTopology {
                slots: vec![
                    CoreSlot {
                        id: first,
                        local_port: 21_001,
                        selected: None,
                    },
                    CoreSlot {
                        id: second,
                        local_port: 21_002,
                        selected: None,
                    },
                ],
                available_nodes: vec![],
                download_port: None,
            },
            19_090,
            "secret",
        )
        .unwrap();
        assert!(yaml.contains("listen: 127.0.0.1"));
        assert!(!yaml.contains("0.0.0.0"));
        assert!(yaml.contains(&slot_selector_name(first)));
        assert!(yaml.contains(&slot_selector_name(second)));
    }

    #[test]
    fn download_lane_is_localhost_fail_closed_and_never_collides_with_slots() {
        let topology = CoreTopology {
            slots: vec![CoreSlot {
                id: Uuid::new_v4(),
                local_port: 21_001,
                selected: None,
            }],
            available_nodes: vec![],
            download_port: Some(22_050),
        };
        let yaml = render_topology(&topology, 19_090, "secret").unwrap();
        assert!(yaml.contains(&format!("  - name: {DOWNLOAD_SELECTOR}-in\n")));
        assert!(yaml.contains("    port: 22050\n"));
        assert!(yaml.contains(&format!("  - name: {DOWNLOAD_SELECTOR}\n")));
        assert!(yaml.contains("    default-selected: REJECT\n"));
        let colliding = CoreTopology {
            download_port: Some(21_001),
            ..topology
        };
        assert!(render_topology(&colliding, 19_090, "secret").is_err());
    }
}
