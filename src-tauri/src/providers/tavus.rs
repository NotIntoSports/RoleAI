//! Tavus CVI HTTP。密钥由调用方传入，本模块不读取配置、不写日志。

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use thiserror::Error;
use ts_rs::TS;

pub const API_ORIGIN: &str = "https://tavusapi.com";
pub const PERSONA_CHAR_LIMIT: usize = 20_000;
pub const CHINESE_INSTRUCTION: &str = "请始终使用中文回答，不要切换到其他语言。";

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct TavusFace {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct TavusConversationStarted {
    pub conversation_id: String,
    pub conversation_url: String,
}

#[derive(Debug, Error)]
pub enum TavusError {
    #[error("Tavus rejected the API key")]
    Unauthorized,
    #[error("Tavus resource was not found")]
    NotFound,
    #[error("Tavus could not be reached")]
    Unreachable,
    #[error("Tavus rejected the request")]
    Rejected(u16),
    #[error("Tavus returned an unexpected response")]
    BadResponse,
}

impl TavusError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => "TAVUS_UNAUTHORIZED",
            Self::NotFound => "TAVUS_NOT_FOUND",
            Self::Unreachable => "TAVUS_UNREACHABLE",
            Self::Rejected(_) => "TAVUS_REJECTED",
            Self::BadResponse => "TAVUS_BAD_RESPONSE",
        }
    }
}

pub trait TavusApi: Send + Sync {
    fn list_faces(&self, api_key: &str) -> Result<Vec<TavusFace>, TavusError>;
    fn create_pal(&self, api_key: &str, body: &Value) -> Result<String, TavusError>;
    fn update_pal(&self, api_key: &str, pal_id: &str, body: &Value) -> Result<(), TavusError>;
    fn create_conversation(
        &self,
        api_key: &str,
        body: &Value,
    ) -> Result<TavusConversationStarted, TavusError>;
    fn end_conversation(&self, api_key: &str, conversation_id: &str) -> Result<(), TavusError>;
}

pub struct TavusHttpClient {
    client: reqwest::blocking::Client,
}

impl TavusHttpClient {
    pub fn new() -> Result<Self, TavusError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(25))
            .build()
            .map_err(|_| TavusError::Unreachable)?;
        Ok(Self { client })
    }
}

impl TavusApi for TavusHttpClient {
    fn list_faces(&self, api_key: &str) -> Result<Vec<TavusFace>, TavusError> {
        let value = exchange(&self.client, api_key, "GET", "/v2/replicas", None)?;
        Ok(parse_faces(&value))
    }

    fn create_pal(&self, api_key: &str, body: &Value) -> Result<String, TavusError> {
        let value = exchange(&self.client, api_key, "POST", "/v2/pals", Some(body))?;
        parse_pal_id(&value).ok_or(TavusError::BadResponse)
    }

    fn update_pal(&self, api_key: &str, pal_id: &str, body: &Value) -> Result<(), TavusError> {
        if !tavus_id_is_valid(pal_id) {
            return Err(TavusError::BadResponse);
        }
        let _ = exchange(
            &self.client,
            api_key,
            "PATCH",
            &format!("/v2/pals/{pal_id}"),
            Some(body),
        )?;
        Ok(())
    }

    fn create_conversation(
        &self,
        api_key: &str,
        body: &Value,
    ) -> Result<TavusConversationStarted, TavusError> {
        let value = exchange(
            &self.client,
            api_key,
            "POST",
            "/v2/conversations",
            Some(body),
        )?;
        parse_conversation(&value).ok_or(TavusError::BadResponse)
    }

    fn end_conversation(&self, api_key: &str, conversation_id: &str) -> Result<(), TavusError> {
        if !tavus_id_is_valid(conversation_id) {
            return Err(TavusError::BadResponse);
        }
        let _ = exchange(
            &self.client,
            api_key,
            "POST",
            &format!("/v2/conversations/{conversation_id}/end"),
            None,
        )?;
        Ok(())
    }
}

pub fn persona_too_long(system_prompt: &str, style_instructions: &str) -> bool {
    system_prompt.chars().count() + style_instructions.chars().count() > PERSONA_CHAR_LIMIT
}

pub fn tavus_id_is_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=64).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn api_key_is_valid(value: &str) -> bool {
    let bytes = value.as_bytes();
    (8..=128).contains(&bytes.len()) && bytes.iter().all(|byte| byte.is_ascii_alphanumeric())
}

pub fn build_system_prompt(system_prompt: &str, style_instructions: &str) -> String {
    let mut prompt = String::from(CHINESE_INSTRUCTION);
    prompt.push_str("\n\n");
    prompt.push_str(system_prompt);
    if !style_instructions.is_empty() {
        prompt.push_str("\n\n风格要求：\n");
        prompt.push_str(style_instructions);
    }
    prompt
}

pub fn build_pal_body(
    name: &str,
    system_prompt: &str,
    style_instructions: &str,
    opening_message: &str,
    face_id: &str,
) -> Value {
    let mut body = json!({
        "pal_name": name,
        "system_prompt": build_system_prompt(system_prompt, style_instructions),
        "pipeline_mode": "full",
        "default_face_id": face_id,
        "languages": ["zh"],
    });
    if !opening_message.is_empty() {
        body["greeting"] = json!(opening_message);
    }
    body
}

pub fn build_conversation_body(pal_id: &str, name: &str, opening_message: &str) -> Value {
    let mut body = json!({
        "pal_id": pal_id,
        "conversation_name": name,
        "properties": { "languages": ["zh"] },
    });
    if !opening_message.is_empty() {
        body["custom_greeting"] = json!(opening_message);
    }
    body
}

pub fn trusted_conversation_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    host == "daily.co" || host.ends_with(".daily.co")
}

fn exchange(
    client: &reqwest::blocking::Client,
    api_key: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, TavusError> {
    let method = method.parse().map_err(|_| TavusError::BadResponse)?;
    let mut request = client
        .request(method, format!("{API_ORIGIN}{path}"))
        .header("x-api-key", api_key);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().map_err(|_| TavusError::Unreachable)?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(TavusError::Unauthorized);
    }
    if status.as_u16() == 404 {
        return Err(TavusError::NotFound);
    }
    if !status.is_success() {
        return Err(TavusError::Rejected(status.as_u16()));
    }
    let bytes = response.bytes().map_err(|_| TavusError::BadResponse)?;
    if bytes.len() > 2_000_000 {
        return Err(TavusError::BadResponse);
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|_| TavusError::BadResponse)
}

fn parse_faces(value: &Value) -> Vec<TavusFace> {
    value
        .get("data")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let id = item
                        .get("replica_id")
                        .or_else(|| item.get("face_id"))
                        .and_then(Value::as_str)
                        .filter(|id| tavus_id_is_valid(id))?;
                    let name = item
                        .get("replica_name")
                        .or_else(|| item.get("face_name"))
                        .or_else(|| item.get("name"))
                        .and_then(Value::as_str)
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or(id);
                    Some(TavusFace {
                        id: id.to_owned(),
                        name: name.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_pal_id(value: &Value) -> Option<String> {
    value
        .get("pal_id")
        .or_else(|| value.get("persona_id"))
        .and_then(Value::as_str)
        .filter(|id| tavus_id_is_valid(id))
        .map(str::to_owned)
}

fn parse_conversation(value: &Value) -> Option<TavusConversationStarted> {
    let conversation_id = value
        .get("conversation_id")
        .and_then(Value::as_str)
        .filter(|id| tavus_id_is_valid(id))?
        .to_owned();
    let conversation_url = value.get("conversation_url").and_then(Value::as_str)?;
    if !trusted_conversation_url(conversation_url) {
        return None;
    }
    Some(TavusConversationStarted {
        conversation_id,
        conversation_url: conversation_url.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pal_body_forces_chinese_and_omits_engines_and_secrets() {
        let body = build_pal_body(
            "面试官",
            "一次只问一个问题",
            "语气平稳",
            "你好",
            "r9d30b0e55ac",
        );
        assert_eq!(body["languages"], json!(["zh"]));
        assert_eq!(body["pipeline_mode"], "full");
        assert_eq!(body["default_face_id"], "r9d30b0e55ac");
        assert_eq!(body["greeting"], "你好");
        let prompt = body["system_prompt"].as_str().unwrap();
        assert!(prompt.starts_with(CHINESE_INSTRUCTION));
        assert!(prompt.contains("一次只问一个问题"));
        assert!(prompt.contains("语气平稳"));
        let text = body.to_string();
        assert!(!text.contains("api_key"));
        assert!(!text.contains("stt_engine"));
        assert!(!text.contains("tts_engine"));
        assert!(!text.contains("multilingual"));
    }

    #[test]
    fn conversation_body_pins_chinese_and_skips_empty_greeting() {
        let body = build_conversation_body("pabc123", "面试官", "");
        assert_eq!(body["properties"]["languages"], json!(["zh"]));
        assert!(body.get("custom_greeting").is_none());
        assert!(!body.to_string().contains("multilingual"));
    }

    #[test]
    fn persona_limit_counts_characters() {
        assert!(!persona_too_long(&"字".repeat(20_000), ""));
        assert!(persona_too_long(&"字".repeat(20_001), ""));
    }

    #[test]
    fn conversation_url_must_be_https_on_daily() {
        assert!(trusted_conversation_url("https://tavus.daily.co/c123"));
        assert!(!trusted_conversation_url("http://tavus.daily.co/c123"));
        assert!(!trusted_conversation_url("https://evil.com/c123"));
        assert!(!trusted_conversation_url("https://daily.co.evil.com/c123"));
    }

    #[test]
    fn parsers_keep_valid_faces_and_reject_untrusted_rooms() {
        let faces = parse_faces(&json!({
            "data": [
                {"replica_id": "r9d30b0e55ac", "replica_name": "Luna"},
                {"replica_id": "../x", "replica_name": "Bad"}
            ]
        }));
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].name, "Luna");
        assert!(
            parse_conversation(&json!({
                "conversation_id": "c123",
                "conversation_url": "https://evil.example/room"
            }))
            .is_none()
        );
        assert!(
            parse_conversation(&json!({
                "conversation_id": "c123",
                "conversation_url": "https://tavus.daily.co/c123"
            }))
            .is_some()
        );
    }
}
