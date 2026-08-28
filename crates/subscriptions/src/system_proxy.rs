//! Reads the Windows system proxy for subscription downloads.
//!
//! This is the ONLY place where the app honors the Windows system proxy, and
//! only when a subscription's download mode is explicitly set to "system"
//! (see AGENTS.md product invariants). The proxy core never sees this value.

/// Returns the configured Windows system proxy as a reqwest-compatible proxy
/// URL (e.g. `http://127.0.0.1:7890`), or `None` when the system proxy is
/// disabled, unreadable, or the platform is not Windows.
pub fn system_proxy_url() -> Option<String> {
    read_system_proxy().and_then(|raw| parse_proxy_server(&raw))
}

#[cfg(windows)]
fn read_system_proxy() -> Option<String> {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::core::PCWSTR;
    unsafe {
        let path = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");
        let enabled = wide("ProxyEnable");
        let mut flag = 0_u32;
        let mut size = 4_u32;
        if RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            PCWSTR(enabled.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut flag as *mut u32).cast()),
            Some(&mut size),
        )
        .is_err()
            || flag == 0
        {
            return None;
        }
        let key = wide("ProxyServer");
        let mut bytes = 2048_u32;
        let mut buffer = vec![0_u16; 1024];
        if RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            PCWSTR(key.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
        .is_err()
        {
            return None;
        }
        let len = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
        let value = String::from_utf16_lossy(&buffer[..len]);
        (!value.trim().is_empty()).then_some(value)
    }
}

#[cfg(not(windows))]
fn read_system_proxy() -> Option<String> {
    None
}

/// Parses the ProxyServer registry value into a proxy URL.
///
/// Supported shapes:
/// - `host:port` (single proxy for all protocols) -> `http://host:port`
/// - `http=host:port;https=host:port;socks=host:port` (per-protocol) ->
///   https entry preferred, then http, then socks (as `socks5://`)
fn parse_proxy_server(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if !raw.contains('=') {
        return with_scheme("http", raw);
    }
    let (mut https, mut http, mut socks) = (None, None, None);
    for part in raw.split(';') {
        let (scheme, address) = part.trim().split_once('=')?;
        let address = address.trim();
        if address.is_empty() {
            return None;
        }
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = Some(address),
            "http" => http = Some(address),
            "socks" => socks = Some(address),
            _ => {}
        }
    }
    if let Some(address) = https.or(http) {
        return with_scheme("http", address);
    }
    if let Some(address) = socks {
        return with_scheme("socks5", address);
    }
    None
}

fn with_scheme(scheme: &str, address: &str) -> Option<String> {
    let address = address
        .trim()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_start_matches("socks://")
        .trim_start_matches("socks5://");
    if address.is_empty() || address.contains('/') || !address.contains(':') {
        return None;
    }
    Some(format!("{scheme}://{address}"))
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_host_port() {
        assert_eq!(
            parse_proxy_server("127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
    }

    #[test]
    fn per_protocol_prefers_https() {
        assert_eq!(
            parse_proxy_server("http=10.0.0.1:8080;https=10.0.0.2:8443").as_deref(),
            Some("http://10.0.0.2:8443")
        );
    }

    #[test]
    fn per_protocol_falls_back_to_http_then_socks() {
        assert_eq!(
            parse_proxy_server("socks=127.0.0.1:1080;http=10.0.0.1:8080").as_deref(),
            Some("http://10.0.0.1:8080")
        );
        assert_eq!(
            parse_proxy_server("socks=127.0.0.1:1080").as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
    }

    #[test]
    fn strips_embedded_schemes() {
        assert_eq!(
            parse_proxy_server("http://127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_proxy_server(""), None);
        assert_eq!(parse_proxy_server("   "), None);
        assert_eq!(parse_proxy_server("no-port"), None);
        assert_eq!(parse_proxy_server("http=;https="), None);
    }
}
