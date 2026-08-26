//! Atomic local backup/restore and centrally redacted diagnostic export.

use node2socks_domain::{AppError, AppResult, ErrorCode};
use regex::Regex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct BackupManifest {
    pub version: u32,
    pub created_at: u64,
    pub database_sha256: String,
}
pub fn backup_database(source: &Path, destination_dir: &Path) -> AppResult<PathBuf> {
    fs::create_dir_all(destination_dir).map_err(io_error)?;
    let timestamp = now()?;
    let target = destination_dir.join(format!("node2socks-{timestamp}.db"));
    let source_db = Connection::open(source).map_err(db_error)?;
    source_db
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .map_err(db_error)?;
    fs::copy(source, &target).map_err(io_error)?;
    let bytes = fs::read(&target).map_err(io_error)?;
    let manifest = BackupManifest {
        version: 1,
        created_at: timestamp,
        database_sha256: hex(&Sha256::digest(&bytes)),
    };
    fs::write(
        target.with_extension("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(config_error)?,
    )
    .map_err(io_error)?;
    Ok(target)
}
pub fn restore_database(backup: &Path, target: &Path) -> AppResult<()> {
    Connection::open(backup)
        .and_then(|db| db.execute_batch("PRAGMA quick_check"))
        .map_err(db_error)?;
    let parent = target.parent().ok_or_else(|| {
        AppError::new(
            ErrorCode::InvalidConfiguration,
            "database target has no parent",
        )
    })?;
    fs::create_dir_all(parent).map_err(io_error)?;
    let temp = target.with_extension("restore.tmp");
    fs::copy(backup, &temp).map_err(io_error)?;
    if target.exists() {
        let previous = target.with_extension("before-restore.bak");
        fs::rename(target, &previous).map_err(io_error)?;
    }
    fs::rename(temp, target).map_err(io_error)
}
pub fn redact(input: &str) -> String {
    let patterns = [
        r"(?i)(Bearer\s+)[A-Za-z0-9._~+/=-]+",
        r"(?i)(token|secret|password|authorization)(\s*[=:]\s*)([^\s,;&]+)",
        r"(?i)(https?://[^\s?]+\?[^\s]*?(token|key|auth)=)([^&\s]+)",
        r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
    ];
    let mut output = input.to_owned();
    for pattern in patterns {
        let regex = Regex::new(pattern).expect("static redaction regex");
        output = if pattern.contains("Bearer\\s") {
            regex.replace_all(&output, "${1}[REDACTED]").into_owned()
        } else if pattern.starts_with("\\b") {
            regex.replace_all(&output, "[IP-REDACTED]").into_owned()
        } else {
            regex
                .replace_all(&output, "${1}${2}[REDACTED]")
                .into_owned()
        };
    }
    output
}
pub fn export_diagnostics(destination: &Path, sections: &[(&str, String)]) -> AppResult<()> {
    let mut output = String::from("Node2Socks diagnostic export\n");
    for (name, value) in sections {
        output.push_str(&format!("\n[{name}]\n{}\n", redact(value)));
    }
    fs::write(destination, output).map_err(io_error)
}
fn now() -> AppResult<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(config_error)
}
fn hex(value: &[u8]) -> String {
    value.iter().map(|v| format!("{v:02x}")).collect()
}
fn io_error(error: std::io::Error) -> AppError {
    AppError::new(ErrorCode::IoError, error.to_string())
}
fn db_error(error: rusqlite::Error) -> AppError {
    AppError::new(ErrorCode::DatabaseError, error.to_string())
}
fn config_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::InvalidConfiguration, error.to_string())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redaction_removes_tokens_credentials_and_ips() {
        let source = "url=https://x.test/sub?token=abc123 password=pw Authorization: Bearer very.secret 1.2.3.4";
        let clean = redact(source);
        assert!(!clean.contains("abc123"));
        assert!(!clean.contains("very.secret"));
        assert!(!clean.contains("1.2.3.4"));
    }
    #[test]
    fn backup_and_restore_preserve_database() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("app.db");
        drop(node2socks_storage::open_and_migrate(&source).unwrap());
        let backup = backup_database(&source, &dir.path().join("backups")).unwrap();
        fs::remove_file(&source).unwrap();
        restore_database(&backup, &source).unwrap();
        assert!(
            Connection::open(source)
                .unwrap()
                .query_row("SELECT count(*) FROM proxy_slots", [], |r| r
                    .get::<_, u64>(0))
                .is_ok()
        );
    }
}
