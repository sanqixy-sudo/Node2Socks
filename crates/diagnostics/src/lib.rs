//! Read-only Windows coexistence diagnostics. Never modifies Clash or system networking.

use node2socks_domain::{AppError, AppResult, ErrorCode};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ClashMode {
    NotDetected,
    SystemProxy,
    TunSuspected,
    ProcessOnly,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkAdapter {
    pub name: String,
    pub description: String,
    pub physical: bool,
    pub up: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoexistenceReport {
    pub clash_mode: ClashMode,
    pub clash_processes: Vec<String>,
    pub system_proxy: Option<String>,
    pub tun_adapters: Vec<String>,
    pub physical_adapters: Vec<NetworkAdapter>,
    pub recommended_interface: Option<String>,
    pub warning: Option<String>,
}

pub fn inspect_windows() -> AppResult<CoexistenceReport> {
    #[cfg(windows)]
    {
        let processes = processes()?;
        let proxy = system_proxy();
        let adapters = adapters()?;
        let tun: Vec<_> = adapters
            .iter()
            .filter(|a| looks_virtual(&a.description) || looks_virtual(&a.name))
            .map(|a| a.name.clone())
            .collect();
        let physical: Vec<_> = adapters
            .iter()
            .filter(|a| a.physical && a.up)
            .cloned()
            .collect();
        let has_clash = !processes.is_empty();
        let mode = if !tun.is_empty() && has_clash {
            ClashMode::TunSuspected
        } else if proxy.is_some() && has_clash {
            ClashMode::SystemProxy
        } else if has_clash {
            ClashMode::ProcessOnly
        } else {
            ClashMode::NotDetected
        };
        let warning=(mode==ClashMode::TunSuspected).then(||"检测到透明代理可能影响 Node2Socks 出站。请验证出口，必要时在 Clash 为 node2socks-mihomo.exe 设置 DIRECT。".into());
        let recommended_interface = physical.first().map(|a| a.name.clone());
        Ok(CoexistenceReport {
            clash_mode: mode,
            clash_processes: processes,
            system_proxy: proxy,
            tun_adapters: tun,
            physical_adapters: physical,
            recommended_interface,
            warning,
        })
    }
    #[cfg(not(windows))]
    {
        Err(AppError::new(
            ErrorCode::InvalidConfiguration,
            "coexistence detection is Windows-only",
        ))
    }
}

#[cfg(windows)]
fn processes() -> AppResult<Vec<String>> {
    use std::os::windows::process::CommandExt;
    let output = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(io_error)?;
    let known = [
        "clash",
        "mihomo",
        "clash-verge",
        "clash verge",
        "mihomo party",
        "flclash",
    ];
    let mut names = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let name = line
            .split(',')
            .next()
            .unwrap_or_default()
            .trim_matches('"')
            .to_ascii_lowercase();
        if known.iter().any(|v| name.contains(v)) && !name.contains("node2socks-mihomo") {
            names.push(name)
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}
#[cfg(windows)]
fn system_proxy() -> Option<String> {
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
            return Some("已启用".into());
        }
        let len = buffer.iter().position(|v| *v == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..len]))
    }
}
#[cfg(windows)]
fn adapters() -> AppResult<Vec<NetworkAdapter>> {
    use std::os::windows::process::CommandExt;
    let script = "Get-NetAdapter -IncludeHidden | Select-Object Name,InterfaceDescription,HardwareInterface,Status | ConvertTo-Json -Compress";
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(io_error)?;
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(config_error)?;
    let values = value.as_array().cloned().unwrap_or_else(|| vec![value]);
    Ok(values
        .into_iter()
        .filter_map(|v| {
            Some(NetworkAdapter {
                name: v.get("Name")?.as_str()?.into(),
                description: v
                    .get("InterfaceDescription")
                    .and_then(|x| x.as_str())
                    .unwrap_or_default()
                    .into(),
                physical: v
                    .get("HardwareInterface")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
                up: v
                    .get("Status")
                    .and_then(|x| x.as_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case("Up")),
            })
        })
        .collect())
}
fn looks_virtual(value: &str) -> bool {
    Regex::new("(?i)(tun|tap|wintun|clash|mihomo|wireguard|tailscale|vpn|virtual)")
        .expect("static regex")
        .is_match(value)
}
#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn io_error(error: std::io::Error) -> AppError {
    AppError::new(ErrorCode::IoError, error.to_string())
}
fn config_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidConfiguration, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn virtual_adapter_detection_is_conservative() {
        assert!(looks_virtual("Meta Tunnel Wintun"));
        assert!(looks_virtual("Clash Verge Service"));
        assert!(!looks_virtual("Intel(R) Wi-Fi 6 AX201"));
    }
}
