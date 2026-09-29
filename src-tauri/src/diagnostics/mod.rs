//! 本地诊断事件日志：`DiagnosticWriter` 把会话/轮次事件追加写入 JSONL，
//! `redact_text` 在落盘前抹掉疑似密钥、令牌等敏感片段。
//! 事件只增不改，供延迟分析与问题回放使用。

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use ts_rs::TS;

pub const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;
const RETENTION_DAYS: i64 = 14;
const MAX_MESSAGE_CHARS: usize = 2_000;

#[derive(Debug, Error)]
pub enum DiagnosticError {
    #[error("Diagnostic path is invalid")]
    InvalidPath,
    #[error("Diagnostic operation failed")]
    Operation,
}

impl DiagnosticError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPath => "DIAGNOSTICS_PATH_INVALID",
            Self::Operation => "DIAGNOSTICS_OPERATION_FAILED",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub area: String,
    pub code: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub snapshot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub retry_count: Option<u32>,
}

pub struct DiagnosticWriter {
    logs_directory: PathBuf,
    lock: Mutex<()>,
}

impl DiagnosticWriter {
    pub fn new(logs_directory: PathBuf) -> Result<Self, DiagnosticError> {
        fs::create_dir_all(&logs_directory).map_err(|_| DiagnosticError::Operation)?;
        Ok(Self {
            logs_directory,
            lock: Mutex::new(()),
        })
    }

    pub fn active_path(&self) -> PathBuf {
        self.logs_directory.join("diagnostics.ndjson")
    }
    pub fn rotated_path(&self) -> PathBuf {
        self.logs_directory.join("diagnostics.1.ndjson")
    }

    pub fn record(&self, event: &DiagnosticEvent) -> Result<(), DiagnosticError> {
        let _guard = self.lock.lock().map_err(|_| DiagnosticError::Operation)?;
        let mut line = serde_json::to_vec(event).map_err(|_| DiagnosticError::Operation)?;
        line.push(b'\n');
        if line.len() > MAX_FILE_BYTES {
            return Err(DiagnosticError::Operation);
        }
        let active = self.active_path();
        let current = fs::metadata(&active)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if current.saturating_add(line.len() as u64) > MAX_FILE_BYTES as u64 {
            let rotated = self.rotated_path();
            if rotated.exists() {
                fs::remove_file(&rotated).map_err(|_| DiagnosticError::Operation)?;
            }
            fs::rename(&active, rotated).map_err(|_| DiagnosticError::Operation)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(active)
            .map_err(|_| DiagnosticError::Operation)?;
        file.write_all(&line)
            .map_err(|_| DiagnosticError::Operation)?;
        file.sync_data().map_err(|_| DiagnosticError::Operation)
    }

    pub fn cleanup(&self, now: DateTime<Utc>) -> Result<(), DiagnosticError> {
        let _guard = self.lock.lock().map_err(|_| DiagnosticError::Operation)?;
        let cutoff = now - Duration::days(RETENTION_DAYS);
        for path in [self.active_path(), self.rotated_path()] {
            if !path.starts_with(&self.logs_directory) || !path.exists() {
                continue;
            }
            let retained = read_events(&path)?
                .into_iter()
                .filter(|event| event.timestamp >= cutoff)
                .map(|event| serde_json::to_string(&event).map_err(|_| DiagnosticError::Operation))
                .collect::<Result<Vec<_>, _>>()?;
            let contents = if retained.is_empty() {
                String::new()
            } else {
                format!("{}\n", retained.join("\n"))
            };
            fs::write(path, contents).map_err(|_| DiagnosticError::Operation)?;
        }
        Ok(())
    }

    pub fn events(&self) -> Result<Vec<DiagnosticEvent>, DiagnosticError> {
        let mut events = Vec::new();
        for path in [self.rotated_path(), self.active_path()] {
            if path.exists() {
                events.extend(read_events(&path)?);
            }
        }
        Ok(events)
    }

    pub fn export(
        &self,
        destination: &Path,
        public_config: Value,
        service_status: Value,
    ) -> Result<(), DiagnosticError> {
        if !destination.is_absolute() {
            return Err(DiagnosticError::InvalidPath);
        }
        let report = serde_json::json!({
            "applicationVersion": env!("CARGO_PKG_VERSION"),
            "toolchain": { "rustTarget": std::env::consts::ARCH },
            "publicConfig": public_config,
            "serviceStatus": service_status,
            "events": self.events()?,
        });
        let bytes = serde_json::to_vec_pretty(&report).map_err(|_| DiagnosticError::Operation)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(DiagnosticError::Operation);
        }
        fs::write(destination, bytes).map_err(|_| DiagnosticError::Operation)
    }
}

fn read_events(path: &Path) -> Result<Vec<DiagnosticEvent>, DiagnosticError> {
    let contents = fs::read_to_string(path).map_err(|_| DiagnosticError::Operation)?;
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(|_| DiagnosticError::Operation))
        .collect()
}

pub fn redact_text(input: &str) -> String {
    let mut output = redact_bearer(input);
    for key in [
        "authorization",
        "apikey",
        "api_key",
        "accesstoken",
        "access_token",
        "token",
        "secret",
        "password",
    ] {
        output = redact_assignments(&output, key);
    }
    output = redact_url_credentials(&output);
    output.chars().take(MAX_MESSAGE_CHARS).collect()
}

fn redact_assignments(input: &str, key: &str) -> String {
    let mut result = input.to_owned();
    let mut offset = 0;
    while let Some(relative) = result[offset..].to_ascii_lowercase().find(key) {
        let key_start = offset + relative;
        let mut value_start = key_start + key.len();
        let bytes = result.as_bytes();
        while value_start < bytes.len() && matches!(bytes[value_start], b' ' | b'"' | b'\'' | b'\\')
        {
            value_start += 1;
        }
        if value_start >= bytes.len() || !matches!(bytes[value_start], b'=' | b':') {
            offset = key_start + key.len();
            continue;
        }
        value_start += 1;
        while value_start < bytes.len() && matches!(bytes[value_start], b' ' | b'"' | b'\'' | b'\\')
        {
            value_start += 1;
        }
        let mut value_end = value_start;
        while value_end < bytes.len()
            && !matches!(
                bytes[value_end],
                b' ' | b'&' | b',' | b'}' | b'"' | b'\'' | b'\\'
            )
        {
            value_end += 1;
        }
        result.replace_range(value_start..value_end, "[REDACTED]");
        offset = value_start + "[REDACTED]".len();
    }
    result
}

fn redact_bearer(input: &str) -> String {
    let mut output = input.to_owned();
    let mut offset = 0;
    while let Some(relative) = output[offset..].to_ascii_lowercase().find("bearer ") {
        let start = offset + relative;
        let value_start = start + 7;
        let value_end = output[value_start..]
            .find(char::is_whitespace)
            .map_or(output.len(), |end| value_start + end);
        output.replace_range(value_start..value_end, "[REDACTED]");
        offset = value_start + "[REDACTED]".len();
    }
    output
}

fn redact_url_credentials(input: &str) -> String {
    let mut output = input.to_owned();
    for scheme in ["https://", "http://", "wss://", "ws://"] {
        let mut search_from = 0;
        while let Some(relative) = output[search_from..].find(scheme) {
            let start = search_from + relative;
            let end = output[start..]
                .find(char::is_whitespace)
                .map_or(output.len(), |end| start + end);
            let raw = output[start..end].trim_end_matches([',', '}', ')']);
            let raw_end = start + raw.len();
            if let Ok(mut url) = tauri::Url::parse(raw) {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                let pairs = url
                    .query_pairs()
                    .map(|(key, value)| (key.into_owned(), value.into_owned()))
                    .collect::<Vec<_>>();
                if !pairs.is_empty() {
                    url.set_query(None);
                    let mut query = url.query_pairs_mut();
                    for (key, value) in pairs {
                        let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
                        let sensitive = ["token", "secret", "password", "apikey", "accesstoken"]
                            .iter()
                            .any(|part| normalized.contains(part));
                        query.append_pair(&key, if sensitive { "[REDACTED]" } else { &value });
                    }
                    drop(query);
                }
                output.replace_range(start..raw_end, url.as_str());
            }
            search_from = start + scheme.len();
        }
    }
    output
}

#[cfg(test)]
mod tests;
