use base64::{Engine, engine::general_purpose::STANDARD};
use node2socks_domain::{AppError, AppResult, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use url::Url;
use uuid::Uuid;

const URI_SCHEMES: &[&str] = &[
    "vless",
    "vmess",
    "trojan",
    "ss",
    "hysteria2",
    "hy2",
    "tuic",
    "socks",
    "socks5",
    "http",
    "https",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionFormat {
    ClashYaml,
    ProviderYaml,
    UriList,
    Base64UriList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedNode {
    pub stable_key: String,
    pub display_name: String,
    pub internal_name: String,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedSubscription {
    pub format: SubscriptionFormat,
    pub nodes: Vec<NormalizedNode>,
    pub mihomo_payload: String,
}

pub fn detect_and_normalize(
    subscription_id: Uuid,
    bytes: &[u8],
) -> AppResult<DetectedSubscription> {
    if bytes.is_empty() {
        return parse_error("subscription body is empty");
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|error| AppError::new(ErrorCode::InvalidConfiguration, error.to_string()))?;
    if let Ok(yaml) = serde_yaml::from_str::<Value>(text)
        && let Some((format, proxies)) = yaml_proxies(&yaml)
    {
        return normalize_yaml(subscription_id, format, proxies);
    }
    if looks_like_uri_list(text) {
        return normalize_uri_list(subscription_id, text, SubscriptionFormat::UriList);
    }
    let compact: String = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if let Ok(decoded) = STANDARD.decode(compact.as_bytes())
        && let Ok(decoded_text) = String::from_utf8(decoded)
        && looks_like_uri_list(&decoded_text)
    {
        return normalize_uri_list(
            subscription_id,
            &decoded_text,
            SubscriptionFormat::Base64UriList,
        );
    }
    parse_error("unsupported or invalid subscription content")
}

fn yaml_proxies(value: &Value) -> Option<(SubscriptionFormat, &[Value])> {
    let mapping = value.as_mapping()?;
    let proxies = mapping
        .get(Value::String("proxies".into()))?
        .as_sequence()?;
    let format = if mapping.contains_key(Value::String("proxy-groups".into()))
        || mapping.contains_key(Value::String("rules".into()))
    {
        SubscriptionFormat::ClashYaml
    } else {
        SubscriptionFormat::ProviderYaml
    };
    Some((format, proxies))
}

fn normalize_yaml(
    subscription_id: Uuid,
    format: SubscriptionFormat,
    proxies: &[Value],
) -> AppResult<DetectedSubscription> {
    if proxies.is_empty() {
        return parse_error("YAML proxies list is empty");
    }
    let prefix = short_prefix(subscription_id);
    let mut normalized = Vec::with_capacity(proxies.len());
    let mut output_proxies = Vec::with_capacity(proxies.len());
    for proxy in proxies {
        let mapping = proxy.as_mapping().ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidConfiguration,
                "YAML proxy entry is not a mapping",
            )
        })?;
        let display_name = string_field(mapping, "name")?;
        let protocol = string_field(mapping, "type")?.to_ascii_lowercase();
        let internal_name = format!("[{prefix}] {display_name}");
        let mut canonical = BTreeMap::new();
        for (key, value) in mapping {
            let Some(key) = key.as_str() else { continue };
            if key.eq_ignore_ascii_case("name") {
                continue;
            }
            canonical.insert(key.to_ascii_lowercase(), yaml_scalar(value));
        }
        let stable_key = hash_identity(subscription_id, &canonical)?;
        normalized.push(NormalizedNode {
            stable_key,
            display_name,
            internal_name: internal_name.clone(),
            protocol,
        });
        let mut rewritten = mapping.clone();
        rewritten.insert(Value::String("name".into()), Value::String(internal_name));
        output_proxies.push(Value::Mapping(rewritten));
    }
    let payload = serde_yaml::to_string(&serde_yaml::Value::Mapping(Mapping::from_iter([(
        Value::String("proxies".into()),
        Value::Sequence(output_proxies),
    )])))
    .map_err(|error| AppError::new(ErrorCode::InvalidConfiguration, error.to_string()))?;
    Ok(DetectedSubscription {
        format,
        nodes: normalized,
        mihomo_payload: payload,
    })
}

fn normalize_uri_list(
    subscription_id: Uuid,
    text: &str,
    format: SubscriptionFormat,
) -> AppResult<DetectedSubscription> {
    let prefix = short_prefix(subscription_id);
    let mut nodes = Vec::new();
    let mut output = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let scheme = line
            .split_once("://")
            .map(|value| value.0.to_ascii_lowercase());
        let Some(protocol) = scheme.filter(|value| URI_SCHEMES.contains(&value.as_str())) else {
            return parse_error("URI list contains an unsupported or malformed line");
        };
        let (identity, name) = uri_identity_and_name(line, &protocol)?;
        let display_name = name.unwrap_or_else(|| format!("{protocol} node {}", nodes.len() + 1));
        let internal_name = format!("[{prefix}] {display_name}");
        nodes.push(NormalizedNode {
            stable_key: hash_bytes(format!("{subscription_id}\0{identity}").as_bytes()),
            display_name,
            internal_name: internal_name.clone(),
            protocol,
        });
        let without_fragment = line.split('#').next().unwrap_or(line);
        output.push(format!(
            "{without_fragment}#{}",
            urlencoding(&internal_name)
        ));
    }
    if nodes.is_empty() {
        return parse_error("URI subscription contains no nodes");
    }
    Ok(DetectedSubscription {
        format,
        nodes,
        mihomo_payload: output.join("\n") + "\n",
    })
}

fn uri_identity_and_name(line: &str, protocol: &str) -> AppResult<(String, Option<String>)> {
    if protocol == "vmess" {
        let encoded = line
            .strip_prefix("vmess://")
            .ok_or_else(|| AppError::new(ErrorCode::InvalidConfiguration, "invalid VMess URI"))?
            .split('#')
            .next()
            .unwrap_or_default();
        if let Ok(decoded) = STANDARD.decode(encoded)
            && let Ok(mut value) = serde_json::from_slice::<JsonValue>(&decoded)
        {
            let name = value
                .get("ps")
                .and_then(JsonValue::as_str)
                .map(ToOwned::to_owned);
            if let Some(object) = value.as_object_mut() {
                object.remove("ps");
            }
            return Ok((format!("vmess:{value}"), name));
        }
    }
    let parsed = Url::parse(line).map_err(|error| {
        AppError::new(
            ErrorCode::InvalidConfiguration,
            format!("invalid proxy URI: {error}"),
        )
    })?;
    let name = parsed
        .fragment()
        .map(|value| percent_decode(value).unwrap_or_else(|| value.to_owned()));
    let mut identity = parsed;
    identity.set_fragment(None);
    Ok((identity.to_string(), name))
}

fn string_field(mapping: &Mapping, key: &str) -> AppResult<String> {
    mapping
        .get(Value::String(key.into()))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::InvalidConfiguration,
                format!("YAML proxy is missing {key}"),
            )
        })
}

fn yaml_scalar(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.trim().to_ascii_lowercase(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    }
}

fn hash_identity<T: Serialize>(subscription_id: Uuid, value: &T) -> AppResult<String> {
    let canonical = serde_json::to_vec(value)
        .map_err(|error| AppError::new(ErrorCode::InvalidConfiguration, error.to_string()))?;
    let mut bytes = subscription_id.as_bytes().to_vec();
    bytes.extend_from_slice(&canonical);
    Ok(hash_bytes(&bytes))
}

fn hash_bytes(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn short_prefix(subscription_id: Uuid) -> String {
    subscription_id.simple().to_string()[..8].to_owned()
}

fn looks_like_uri_list(text: &str) -> bool {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .any(|line| {
            URI_SCHEMES.iter().any(|scheme| {
                line.trim()
                    .to_ascii_lowercase()
                    .starts_with(&format!("{scheme}://"))
            })
        })
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn percent_decode(value: &str) -> Option<String> {
    let synthetic = format!("http://localhost/?value={value}");
    Url::parse(&synthetic)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == "value")
        .map(|(_, value)| value.into_owned())
}

fn parse_error<T>(message: &str) -> AppResult<T> {
    Err(AppError::new(ErrorCode::InvalidConfiguration, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_clash_yaml_and_stable_key_ignores_rename() {
        let id = Uuid::new_v4();
        let first = detect_and_normalize(
            id,
            b"proxies:\n  - {name: JP01, type: ss, server: 1.2.3.4, port: 443, password: p, cipher: aes-128-gcm}\nproxy-groups: []\n",
        )
        .unwrap();
        let renamed = detect_and_normalize(
            id,
            b"proxies:\n  - {name: Tokyo Premium, type: ss, server: 1.2.3.4, port: 443, password: p, cipher: aes-128-gcm}\nproxy-groups: []\n",
        )
        .unwrap();
        assert_eq!(first.format, SubscriptionFormat::ClashYaml);
        assert_eq!(first.nodes[0].stable_key, renamed.nodes[0].stable_key);
        assert_ne!(first.nodes[0].display_name, renamed.nodes[0].display_name);
    }

    #[test]
    fn endpoint_or_credential_change_changes_stable_key() {
        let id = Uuid::new_v4();
        let parse = |server: &str, password: &str| {
            detect_and_normalize(
                id,
                format!("proxies:\n  - {{name: x, type: ss, server: {server}, port: 443, password: {password}, cipher: aes-128-gcm}}\n").as_bytes(),
            )
            .unwrap()
            .nodes[0]
            .stable_key
            .clone()
        };
        assert_ne!(parse("1.2.3.4", "p"), parse("1.2.3.5", "p"));
        assert_ne!(parse("1.2.3.4", "p"), parse("1.2.3.4", "q"));
    }

    #[test]
    fn detects_uri_and_base64_uri_lists() {
        let id = Uuid::new_v4();
        let source =
            "vless://uuid@example.com:443?security=tls#JP%2001\ntrojan://pw@example.net:443#US\n";
        let direct = detect_and_normalize(id, source.as_bytes()).unwrap();
        let encoded = STANDARD.encode(source);
        let base64 = detect_and_normalize(id, encoded.as_bytes()).unwrap();
        assert_eq!(direct.format, SubscriptionFormat::UriList);
        assert_eq!(base64.format, SubscriptionFormat::Base64UriList);
        assert_eq!(direct.nodes.len(), 2);
        assert_eq!(direct.nodes[0].stable_key, base64.nodes[0].stable_key);
    }

    #[test]
    fn rejects_invalid_mixed_content() {
        let error = detect_and_normalize(
            Uuid::new_v4(),
            b"vless://uuid@example.com:443#ok\nnot-a-proxy\n",
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidConfiguration);
    }
}
