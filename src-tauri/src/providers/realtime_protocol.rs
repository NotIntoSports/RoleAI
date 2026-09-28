//! realtime_protocol 子模块：Realtime WS 协议纯逻辑——
//! 方言常量与判定、URL 构造、请求事件构造、服务端事件解析、输入转写装配。
//! 纯搬移自 providers/openai_realtime.rs，不含行为变更；
//! WebSocket 读写、握手、重连等 IO 留在原文件。
use std::collections::{HashMap, HashSet};

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Url;
use serde_json::{Value, json};

use super::RealtimeError;

pub const ALIYUN_REALTIME_PATH: &str = "/api-ws/v1/realtime";
pub const MAX_TEXT_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimeDialectName {
    Openai,
    Aliyun,
    Bigmodel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealtimeDialect {
    pub name: RealtimeDialectName,
    pub audio_format: &'static str,
    pub output_audio_format: &'static str,
    pub input_rate: u32,
    pub default_voice: Option<&'static str>,
}

const OPENAI_DIALECT: RealtimeDialect = RealtimeDialect {
    name: RealtimeDialectName::Openai,
    audio_format: "pcm16",
    output_audio_format: "pcm16",
    input_rate: 24_000,
    default_voice: Some("alloy"),
};

const ALIYUN_DIALECT: RealtimeDialect = RealtimeDialect {
    name: RealtimeDialectName::Aliyun,
    audio_format: "pcm",
    output_audio_format: "pcm",
    input_rate: 16_000,
    default_voice: None,
};

// GLM 的 "pcm16" 输入表示 16kHz（与 OpenAI 的 24kHz 语义不同）；VAD 也按
// 16kHz/单声道/16bit 解码，采样率不符会变调变速导致转写失败。输出只支持
// "pcm"（固定 24kHz），音色用 GLM 自己的默认值。
const BIGMODEL_DIALECT: RealtimeDialect = RealtimeDialect {
    name: RealtimeDialectName::Bigmodel,
    audio_format: "pcm16",
    output_audio_format: "pcm",
    input_rate: 16_000,
    default_voice: Some("tongtong"),
};

pub struct InputTranscriptAssembler {
    texts: HashMap<String, String>,
    finalized: HashSet<String>,
}

impl InputTranscriptAssembler {
    pub fn new() -> Self {
        Self {
            texts: HashMap::new(),
            finalized: HashSet::new(),
        }
    }

    pub fn update(&mut self, kind: &str, payload: &Value) -> Option<(String, String, bool)> {
        let item_id = payload
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        if item_id.is_empty() || self.finalized.contains(&item_id) {
            return None;
        }
        if kind == "input_transcript_completed" {
            let transcript = payload
                .get("transcript")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
            if transcript.is_empty() {
                return None;
            }
            self.texts.insert(item_id.clone(), transcript.clone());
            self.finalized.insert(item_id.clone());
            return Some((item_id, transcript, true));
        }
        if kind != "input_transcript_delta" {
            return None;
        }
        let stable = payload.get("text").and_then(Value::as_str).unwrap_or("");
        let stash = payload.get("stash").and_then(Value::as_str).unwrap_or("");
        let delta = payload.get("delta").and_then(Value::as_str).unwrap_or("");
        let text = if !stable.is_empty() || !stash.is_empty() {
            format!("{stable}{stash}")
        } else {
            format!(
                "{}{delta}",
                self.texts.get(&item_id).map(String::as_str).unwrap_or("")
            )
        };
        let text = text.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        self.texts.insert(item_id.clone(), text.clone());
        Some((item_id, text, false))
    }
}

pub fn realtime_dialect(base_url: &str) -> RealtimeDialect {
    let Ok(parsed) = Url::parse(base_url.trim()) else {
        return OPENAI_DIALECT;
    };
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    let path = parsed.path().to_ascii_lowercase();
    if host.contains("bigmodel") {
        BIGMODEL_DIALECT
    } else {
        let is_aliyun = path.contains("compatible-mode")
            || path.trim_end_matches('/').ends_with(ALIYUN_REALTIME_PATH)
            || host.contains("dashscope")
            || host.contains("token-plan");
        if is_aliyun {
            ALIYUN_DIALECT
        } else {
            OPENAI_DIALECT
        }
    }
}

pub fn realtime_url(base_url: &str, model: &str) -> Result<Url, RealtimeError> {
    let raw = base_url.trim().trim_end_matches('/');
    let mut url = Url::parse(raw).map_err(|_| RealtimeError::UrlInvalid)?;
    if !matches!(url.scheme(), "http" | "https" | "ws" | "wss")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(RealtimeError::UrlInvalid);
    }
    let dialect = realtime_dialect(raw);
    let scheme = match url.scheme() {
        "https" | "wss" => "wss",
        "http" | "ws" => "ws",
        _ => return Err(RealtimeError::UrlInvalid),
    };
    url.set_scheme(scheme)
        .map_err(|_| RealtimeError::UrlInvalid)?;
    let path = if dialect.name == RealtimeDialectName::Aliyun {
        ALIYUN_REALTIME_PATH.to_owned()
    } else {
        let path = url.path().trim_end_matches('/');
        if path.ends_with("/realtime") {
            path.to_owned()
        } else if path.is_empty() {
            "/realtime".to_owned()
        } else {
            format!("{path}/realtime")
        }
    };
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    url.query_pairs_mut().append_pair("model", model);
    Ok(url)
}

pub fn dialect_input_rate(dialect: &RealtimeDialect) -> u32 {
    dialect.input_rate
}

pub fn session_update_event(voice: &str, instructions: &str, dialect: &RealtimeDialect) -> Value {
    let mut session = json!({
        "modalities": ["text", "audio"],
        "instructions": instructions,
        "input_audio_format": dialect.audio_format,
        "output_audio_format": dialect.output_audio_format,
    });
    if dialect.name == RealtimeDialectName::Aliyun {
        // DashScope WS 为 Manual 模式：本地 SmartTurn 已判定语句结束，
        // 禁用服务端 VAD（保留会与客户端 commit 竞态：自动提交后手动 commit
        // 撞空缓冲被拒 "buffer too small, or have no audio"）。
        session["turn_detection"] = Value::Null;
    } else {
        session["turn_detection"] = json!({
            "type": "server_vad",
            "threshold": 0.5,
            "silence_duration_ms": 800,
            "create_response": true,
        });
    }
    let selected_voice = if voice.is_empty() {
        dialect.default_voice.unwrap_or("")
    } else {
        voice
    };
    if !selected_voice.is_empty() {
        session["voice"] = json!(selected_voice);
    }
    json!({
        "type": "session.update",
        "session": session,
    })
}

/// 本轮合成音色：线路配置优先，其次方言内置默认（GLM tongtong / OpenAI 系默认），
/// 都没有则空（session.update 不带 voice 字段）。
pub(crate) fn selected_voice<'a>(request_voice: &'a str, dialect: &RealtimeDialect) -> &'a str {
    let request_voice = request_voice.trim();
    if !request_voice.is_empty() {
        return request_voice;
    }
    dialect.default_voice.unwrap_or("")
}

pub(super) fn append_audio_event(pcm: &[u8]) -> String {
    json!({
        "type": "input_audio_buffer.append",
        "audio": STANDARD.encode(pcm),
    })
    .to_string()
}

/// append 分块大小：16kHz×16bit×100ms。DashScope 对单帧大小有限制，
/// 一句话整帧发送会被立即断链（真网实测 WriteFailed），按 100ms 块流式发送。
pub(crate) const AUDIO_APPEND_CHUNK_BYTES: usize = 3_200;

pub(super) fn conversation_text_event(text: &str) -> String {
    json!({
        "type": "conversation.item.create",
        "item": {
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        },
    })
    .to_string()
}

pub(super) fn response_create_event(include_audio: bool) -> String {
    let modalities = if include_audio {
        json!(["text", "audio"])
    } else {
        json!(["text"])
    };
    json!({
        "type": "response.create",
        "response": { "modalities": modalities },
    })
    .to_string()
}

/// 透出服务端错误 code 与限长的 message，供 UI/诊断定位拒绝原因；
/// message 中密钥样式片段（sk-…、secret=…、apikey=…）整段剔除。
fn sanitize_remote_code(raw: &str) -> String {
    let redacted = raw
        .trim()
        .split([' ', ';', ',', '，', '；'])
        .filter(|token| {
            let lower = token.to_ascii_lowercase();
            !(lower.contains("sk-")
                && (token.starts_with("sk-")
                    || lower.contains("secret=")
                    || lower.contains("apikey=")
                    || lower.contains("api-key=")))
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut sanitized: String = redacted.chars().take(200).collect();
    if sanitized.is_empty() {
        sanitized = "REALTIME_REMOTE_ERROR".to_owned();
    }
    sanitized
}

pub(crate) enum ServerEvent {
    SessionCreated,
    SessionUpdated,
    /// response.done 携带的 response.status（completed/cancelled，缺省 None）。
    ResponseDone(Option<String>),
    Audio(Vec<u8>),
    OutputTranscript(String),
    OutputText(String),
    InputTranscriptDelta(Value),
    InputTranscriptCompleted(Value),
    SpeechStarted,
    SpeechStopped,
    InputCommitted,
    ResponseCreated,
}

pub(crate) fn parse_server_event(raw: &str) -> Result<Option<ServerEvent>, RealtimeError> {
    if raw.len() > MAX_TEXT_FRAME_BYTES {
        return Err(RealtimeError::ResponseTooLarge);
    }
    let event: Value = serde_json::from_str(raw).map_err(|_| RealtimeError::EventInvalid)?;
    let Some(kind) = event.get("type").and_then(Value::as_str) else {
        return Ok(None);
    };
    match kind {
        "response.audio.delta" => {
            let Some(delta) = event.get("delta").and_then(Value::as_str) else {
                return Ok(None);
            };
            let pcm = STANDARD
                .decode(delta)
                .map_err(|_| RealtimeError::AudioInvalid)?;
            Ok(Some(ServerEvent::Audio(pcm)))
        }
        "response.audio_transcript.delta" => Ok(event
            .get("delta")
            .and_then(Value::as_str)
            .map(|delta| ServerEvent::OutputTranscript(delta.to_owned()))),
        "response.text.delta" => Ok(event
            .get("delta")
            .and_then(Value::as_str)
            .map(|delta| ServerEvent::OutputText(delta.to_owned()))),
        "conversation.item.input_audio_transcription.delta" => {
            Ok(Some(ServerEvent::InputTranscriptDelta(json!({
                "item_id": event.get("item_id").and_then(Value::as_str).unwrap_or(""),
                "delta": event.get("delta").and_then(Value::as_str).unwrap_or(""),
                "text": event.get("text").and_then(Value::as_str).unwrap_or(""),
                "stash": event.get("stash").and_then(Value::as_str).unwrap_or(""),
            }))))
        }
        "conversation.item.input_audio_transcription.completed" => {
            let Some(transcript) = event.get("transcript").and_then(Value::as_str) else {
                return Ok(None);
            };
            Ok(Some(ServerEvent::InputTranscriptCompleted(json!({
                "item_id": event.get("item_id").and_then(Value::as_str).unwrap_or(""),
                "transcript": transcript,
            }))))
        }
        "response.done" => Ok(Some(ServerEvent::ResponseDone(
            event
                .get("response")
                .and_then(|response| response.get("status"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        ))),
        "input_audio_buffer.speech_started" => Ok(Some(ServerEvent::SpeechStarted)),
        "input_audio_buffer.speech_stopped" => Ok(Some(ServerEvent::SpeechStopped)),
        "input_audio_buffer.committed" => Ok(Some(ServerEvent::InputCommitted)),
        "response.created" => Ok(Some(ServerEvent::ResponseCreated)),
        "session.updated" => Ok(Some(ServerEvent::SessionUpdated)),
        "session.created" => Ok(Some(ServerEvent::SessionCreated)),
        "error" => {
            let error = event.get("error");
            // code 缺省时回退；同时透出限长的 message（剔除密钥样式片段）——
            // DashScope 的拒绝原因只在 message 里，缺失时用户只能看到笼统错误。
            let code = error
                .and_then(|value| value.get("code").or_else(|| value.get("msg_code")))
                .and_then(Value::as_str)
                .unwrap_or("REALTIME_REMOTE_ERROR");
            let message = error
                .and_then(|value| value.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            Err(RealtimeError::Remote(sanitize_remote_code(
                if message.is_empty() {
                    code.to_owned()
                } else {
                    format!("{code}: {message}")
                }
                .as_str(),
            )))
        }
        _ => Ok(None),
    }
}
