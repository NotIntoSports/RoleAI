use std::{
    collections::{HashMap, HashSet},
    fmt,
    io::{Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Url;
use serde_json::{Value, json};
use tungstenite::{
    ClientRequestBuilder, Error as WsError, HandshakeError, Message, client::IntoClientRequest,
    protocol::WebSocketConfig, stream::MaybeTlsStream,
};

use crate::audio::resample_pcm16_mono;

use super::ProviderEndpoint;

pub const ALIYUN_REALTIME_PATH: &str = "/api-ws/v1/realtime";
pub const CONNECT_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
pub const SESSION_UPDATED_TIMEOUT: Duration = Duration::from_secs(10);
const TURN_TIMEOUT: Duration = Duration::from_secs(90);
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const CANCEL_POLL: Duration = Duration::from_millis(25);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealtimeError {
    UrlInvalid,
    ConnectFailed,
    DnsFailed,
    TcpFailed,
    TlsFailed,
    HandshakeFailed(u16),
    ProtocolFailed,
    ConnectionClosed,
    ReadFailed,
    WriteFailed,
    Timeout,
    SessionUpdateTimeout,
    SessionUpdateUnexpected,
    EventInvalid,
    AudioInvalid,
    Unauthorized,
    ResponseTooLarge,
    Cancelled,
    TextEmpty,
    Remote(String),
}

impl RealtimeError {
    pub fn public_message(&self) -> String {
        match self {
            Self::HandshakeFailed(status) => {
                format!("实时语音握手失败（HTTP {status}），请检查服务地址、套餐额度及模型权限。")
            }
            Self::DnsFailed => "无法解析实时语音服务地址，请检查网络和服务地址。".into(),
            Self::TcpFailed => "无法连接实时语音服务，请检查网络后重试。".into(),
            Self::TlsFailed => "实时语音安全连接失败，请检查系统时间、证书或代理设置。".into(),
            Self::Unauthorized => "实时语音鉴权失败，请检查 API Key、套餐状态和模型权限。".into(),
            Self::Timeout | Self::SessionUpdateTimeout => "实时语音请求超时，请重试。".into(),
            Self::Cancelled => "已取消本次发送。".into(),
            Self::ConnectionClosed => "实时语音服务已断开连接，请重试。".into(),
            Self::ReadFailed => "接收实时语音回复失败，请重试。".into(),
            Self::WriteFailed => "发送至实时语音服务失败，请重试。".into(),
            Self::UrlInvalid => "实时语音服务地址无效，请检查供应商设置。".into(),
            Self::ProtocolFailed | Self::SessionUpdateUnexpected | Self::EventInvalid => {
                "实时语音协议或会话响应异常，请检查服务兼容性。".into()
            }
            Self::ResponseTooLarge => "实时语音回复超过大小限制。".into(),
            Self::TextEmpty => "实时语音服务未返回文字回复，请重试。".into(),
            Self::AudioInvalid => "实时语音服务返回了无效音频。".into(),
            Self::Remote(_) => "实时语音服务拒绝了本次请求，请检查模型配置或稍后重试。".into(),
            Self::ConnectFailed => "实时语音连接失败，请检查网络和供应商设置。".into(),
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::ConnectFailed
                | Self::DnsFailed
                | Self::TcpFailed
                | Self::ConnectionClosed
                | Self::ReadFailed
                | Self::WriteFailed
                | Self::Timeout
                | Self::SessionUpdateTimeout
                | Self::TextEmpty
                | Self::HandshakeFailed(429 | 500..=599)
        )
    }

    pub fn code(&self) -> &str {
        match self {
            Self::UrlInvalid => "REALTIME_URL_INVALID",
            Self::ConnectFailed => "REALTIME_CONNECT_FAILED",
            Self::DnsFailed => "REALTIME_DNS_FAILED",
            Self::TcpFailed => "REALTIME_TCP_FAILED",
            Self::TlsFailed => "REALTIME_TLS_FAILED",
            Self::HandshakeFailed(_) => "REALTIME_HANDSHAKE_FAILED",
            Self::ProtocolFailed => "REALTIME_PROTOCOL_FAILED",
            Self::ConnectionClosed => "REALTIME_CONNECTION_CLOSED",
            Self::ReadFailed => "REALTIME_READ_FAILED",
            Self::WriteFailed => "REALTIME_WRITE_FAILED",
            Self::Timeout => "REALTIME_TIMEOUT",
            Self::SessionUpdateTimeout => "REALTIME_SESSION_UPDATE_TIMEOUT",
            Self::SessionUpdateUnexpected => "REALTIME_SESSION_UPDATE_UNEXPECTED",
            Self::EventInvalid => "REALTIME_EVENT_INVALID",
            Self::AudioInvalid => "REALTIME_AUDIO_INVALID",
            Self::Unauthorized => "REALTIME_UNAUTHORIZED",
            Self::ResponseTooLarge => "REALTIME_RESPONSE_TOO_LARGE",
            Self::Cancelled => "SESSION_CANCELLED",
            Self::TextEmpty => "REALTIME_TEXT_EMPTY",
            Self::Remote(code) => code,
        }
    }
}

impl fmt::Display for RealtimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for RealtimeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeTurn {
    pub user_text: String,
    pub assistant_text: String,
    pub tts_pcm: Vec<u8>,
}

pub struct RealtimeTextRequest<'a> {
    pub endpoint: &'a ProviderEndpoint,
    pub credential: Option<&'a str>,
    pub model_id: &'a str,
    pub instructions: &'a str,
    pub prompt: &'a str,
    pub include_audio: bool,
}

pub struct RealtimeAudioRequest<'a> {
    pub endpoint: &'a ProviderEndpoint,
    pub credential: Option<&'a str>,
    pub model_id: &'a str,
    pub pcm16le: &'a [u8],
    pub sample_rate: u32,
    pub instructions: &'a str,
}

pub trait RealtimeModel: Send + Sync {
    fn transcribe_turn(
        &self,
        request: RealtimeAudioRequest<'_>,
        cancel: &AtomicBool,
    ) -> Result<RealtimeTurn, RealtimeError>;

    fn text_turn(
        &self,
        request: RealtimeTextRequest<'_>,
        cancel: &AtomicBool,
    ) -> Result<RealtimeTurn, RealtimeError> {
        let _ = (request, cancel);
        Err(RealtimeError::TextEmpty)
    }
}

pub trait RealtimeTransport {
    fn recv_text(&mut self, timeout: Duration) -> Result<String, RealtimeError>;
}

pub struct OpenAiCompatibleRealtime;

impl OpenAiCompatibleRealtime {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenAiCompatibleRealtime {
    fn default() -> Self {
        Self::new()
    }
}

impl RealtimeModel for OpenAiCompatibleRealtime {
    fn transcribe_turn(
        &self,
        request: RealtimeAudioRequest<'_>,
        cancel: &AtomicBool,
    ) -> Result<RealtimeTurn, RealtimeError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(RealtimeError::Cancelled);
        }
        let url = realtime_url(&request.endpoint.base_url, request.model_id)?;
        let dialect = realtime_dialect(&request.endpoint.base_url);
        let pcm16le = resample_pcm16_mono(
            request.pcm16le,
            request.sample_rate,
            dialect_input_rate(&dialect),
        );
        with_realtime_socket(&url, request.credential, cancel, |socket| {
            let voice = dialect.default_voice.unwrap_or("");
            let update = session_update_event(voice, request.instructions, &dialect);
            send_text(socket, &update.to_string())?;
            wait_session_updated(socket, SESSION_UPDATED_TIMEOUT)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(RealtimeError::Cancelled);
            }
            send_text(socket, &append_audio_event(&pcm16le))?;
            send_text(socket, r#"{"type":"input_audio_buffer.commit"}"#)?;
            collect_turn(socket, cancel)
        })
    }

    fn text_turn(
        &self,
        request: RealtimeTextRequest<'_>,
        cancel: &AtomicBool,
    ) -> Result<RealtimeTurn, RealtimeError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(RealtimeError::Cancelled);
        }
        let url = realtime_url(&request.endpoint.base_url, request.model_id)?;
        let dialect = realtime_dialect(&request.endpoint.base_url);
        with_realtime_socket(&url, request.credential, cancel, |socket| {
            let modalities = if request.include_audio {
                json!(["text", "audio"])
            } else {
                json!(["text"])
            };
            let mut session = json!({
                "modalities": modalities,
                "instructions": request.instructions,
                "input_audio_format": dialect.audio_format,
                "output_audio_format": dialect.audio_format,
            });
            let voice = dialect.default_voice.unwrap_or("");
            if !voice.is_empty() {
                session["voice"] = json!(voice);
            }
            send_text(
                socket,
                &json!({ "type": "session.update", "session": session }).to_string(),
            )?;
            wait_session_updated(socket, SESSION_UPDATED_TIMEOUT)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(RealtimeError::Cancelled);
            }
            send_text(socket, &conversation_text_event(request.prompt))?;
            send_text(socket, &response_create_event(request.include_audio))?;
            collect_turn(socket, cancel)
        })
    }
}

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
        "turn_detection": {
            "type": "server_vad",
            "threshold": 0.5,
            "silence_duration_ms": 800,
            "create_response": true,
        },
    });
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

pub fn wait_session_updated<T: RealtimeTransport + ?Sized>(
    socket: &mut T,
    timeout: Duration,
) -> Result<(), RealtimeError> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RealtimeError::SessionUpdateTimeout);
        }
        let raw = match socket.recv_text(remaining) {
            Ok(text) => text,
            Err(RealtimeError::Timeout | RealtimeError::SessionUpdateTimeout) => {
                return Err(RealtimeError::SessionUpdateTimeout);
            }
            Err(error) => return Err(error),
        };
        match parse_server_event(&raw)? {
            Some(ServerEvent::SessionCreated) | None => {}
            Some(ServerEvent::SessionUpdated) => return Ok(()),
            Some(_) => return Err(RealtimeError::SessionUpdateUnexpected),
        }
    }
}

/// 线路测试用的最小实时会话探测：只完成握手（session.update → session.updated），
/// 不发送音频、不产生模型调用。目录比对无法发现账号级的模型权限/套餐问题，
/// 只有真实握手能把它们提前到测试阶段暴露。
pub fn probe_realtime_session(
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
    model_id: &str,
) -> Result<(), RealtimeError> {
    let url = realtime_url(&endpoint.base_url, model_id)?;
    let dialect = realtime_dialect(&endpoint.base_url);
    let cancel = AtomicBool::new(false);
    with_realtime_socket_budget(&url, credential, &cancel, PROBE_TIMEOUT, |socket| {
        let update = session_update_event(dialect.default_voice.unwrap_or(""), "自检", &dialect);
        send_text(socket, &update.to_string())?;
        wait_session_updated(socket, PROBE_TIMEOUT)
    })
}

fn append_audio_event(pcm: &[u8]) -> String {
    json!({
        "type": "input_audio_buffer.append",
        "audio": STANDARD.encode(pcm),
    })
    .to_string()
}

fn conversation_text_event(text: &str) -> String {
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

fn response_create_event(include_audio: bool) -> String {
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

fn sanitize_remote_code(code: &str) -> String {
    let trimmed = code.trim();
    let mut sanitized = trimmed.chars().take(80).collect::<String>();
    if sanitized.is_empty() {
        sanitized = "REALTIME_REMOTE_ERROR".to_owned();
    }
    sanitized
}

enum ServerEvent {
    SessionCreated,
    SessionUpdated,
    ResponseDone,
    Audio(Vec<u8>),
    OutputTranscript(String),
    OutputText(String),
    InputTranscriptDelta(Value),
    InputTranscriptCompleted(Value),
}

fn parse_server_event(raw: &str) -> Result<Option<ServerEvent>, RealtimeError> {
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
        "response.done" => Ok(Some(ServerEvent::ResponseDone)),
        "session.updated" => Ok(Some(ServerEvent::SessionUpdated)),
        "session.created" => Ok(Some(ServerEvent::SessionCreated)),
        "error" => {
            let error = event.get("error");
            let code = error
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
                .unwrap_or("REALTIME_REMOTE_ERROR");
            Err(RealtimeError::Remote(sanitize_remote_code(code)))
        }
        _ => Ok(None),
    }
}

struct TungsteniteSocket {
    socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
}

impl RealtimeTransport for TungsteniteSocket {
    fn recv_text(&mut self, timeout: Duration) -> Result<String, RealtimeError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RealtimeError::Timeout);
            }
            set_socket_timeouts(&mut self.socket, remaining)?;
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    if text.len() > MAX_TEXT_FRAME_BYTES {
                        return Err(RealtimeError::ResponseTooLarge);
                    }
                    return Ok(text.to_string());
                }
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Binary(_)) => continue,
                Ok(Message::Close(_)) => return Err(RealtimeError::ConnectionClosed),
                Err(WsError::Io(error))
                    if error.kind() == std::io::ErrorKind::TimedOut
                        || error.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    return Err(RealtimeError::Timeout);
                }
                Err(error) => return Err(socket_error(error, RealtimeError::ReadFailed)),
            }
        }
    }
}

// Only the OS resolver/connect worker is detached. It never owns credentials or
// sends a request. A late stream is dropped when its receiver has been cancelled.
fn connect_tcp(url: &Url, cancel: &AtomicBool) -> Result<TcpStream, RealtimeError> {
    let host = url.host_str().ok_or(RealtimeError::UrlInvalid)?.to_owned();
    let port = url
        .port_or_known_default()
        .ok_or(RealtimeError::UrlInvalid)?;
    let deadline = Instant::now() + CONNECT_OPEN_TIMEOUT;
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = (|| {
            let addresses = (host.as_str(), port)
                .to_socket_addrs()
                .map_err(|_| RealtimeError::DnsFailed)?;
            let mut last = RealtimeError::DnsFailed;
            for address in addresses {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(RealtimeError::Timeout);
                }
                match TcpStream::connect_timeout(&address, remaining) {
                    Ok(stream) => return Ok(stream),
                    Err(error) => {
                        last = if error.kind() == std::io::ErrorKind::TimedOut {
                            RealtimeError::Timeout
                        } else {
                            RealtimeError::TcpFailed
                        }
                    }
                }
            }
            Err(last)
        })();
        let _ = tx.send(result);
    });
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(RealtimeError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RealtimeError::Timeout);
        }
        match rx.recv_timeout(remaining.min(CANCEL_POLL)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err(RealtimeError::TcpFailed),
        }
    }
}

fn with_realtime_socket<T>(
    url: &Url,
    credential: Option<&str>,
    cancel: &AtomicBool,
    action: impl FnOnce(&mut TungsteniteSocket) -> Result<T, RealtimeError>,
) -> Result<T, RealtimeError> {
    with_realtime_socket_budget(url, credential, cancel, TURN_TIMEOUT, action)
}

fn with_realtime_socket_budget<T>(
    url: &Url,
    credential: Option<&str>,
    cancel: &AtomicBool,
    turn_timeout: Duration,
    action: impl FnOnce(&mut TungsteniteSocket) -> Result<T, RealtimeError>,
) -> Result<T, RealtimeError> {
    let stream = connect_tcp(url, cancel)?;
    let _ = stream.set_nodelay(true);
    set_tcp_timeouts(&stream, CONNECT_OPEN_TIMEOUT)?;
    let mut builder = ClientRequestBuilder::new(
        url.as_str()
            .parse()
            .map_err(|_| RealtimeError::UrlInvalid)?,
    )
    .with_header("OpenAI-Beta", "realtime=v1");
    if let Some(credential) = credential.filter(|value| !value.is_empty()) {
        builder = builder.with_header("Authorization", format!("Bearer {credential}"));
    }
    let request = builder
        .into_client_request()
        .map_err(|_| RealtimeError::UrlInvalid)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_TEXT_FRAME_BYTES))
        .max_frame_size(Some(MAX_TEXT_FRAME_BYTES));
    let interrupt = stream.try_clone().map_err(|_| RealtimeError::TcpFailed)?;
    let finished = AtomicBool::new(false);
    let expired = AtomicBool::new(false);
    let start = Instant::now();
    let budget_ms = AtomicU64::new(CONNECT_OPEN_TIMEOUT.as_millis() as u64);
    // Shutdown interrupts blocking TLS/read/write as well as endless control
    // frames. The scoped monitor is joined before returning; no orphan request.
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = interrupt.shutdown(Shutdown::Both);
                    break;
                }
                if start.elapsed().as_millis() >= u128::from(budget_ms.load(Ordering::Acquire)) {
                    expired.store(true, Ordering::Release);
                    let _ = interrupt.shutdown(Shutdown::Both);
                    break;
                }
                std::thread::sleep(CANCEL_POLL);
            }
        });
        struct FinishOnDrop<'a>(&'a AtomicBool);
        impl Drop for FinishOnDrop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let finish = FinishOnDrop(&finished);
        let result = (|| {
            let (socket, _) = complete_handshake(tungstenite::client_tls_with_config(
                request,
                stream,
                Some(config),
                None,
            ))?;
            budget_ms.store(
                (start.elapsed() + turn_timeout).as_millis() as u64,
                Ordering::Release,
            );
            action(&mut TungsteniteSocket { socket })
        })();
        drop(finish);
        if cancel.load(Ordering::Relaxed) {
            Err(RealtimeError::Cancelled)
        } else if expired.load(Ordering::Acquire) {
            Err(RealtimeError::Timeout)
        } else {
            result
        }
    })
}

fn socket_error(error: WsError, fallback: RealtimeError) -> RealtimeError {
    match error {
        WsError::Http(response) => match response.status().as_u16() {
            401 | 403 => RealtimeError::Unauthorized,
            status => RealtimeError::HandshakeFailed(status),
        },
        WsError::Tls(_) => RealtimeError::TlsFailed,
        WsError::ConnectionClosed | WsError::AlreadyClosed => RealtimeError::ConnectionClosed,
        WsError::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            RealtimeError::Timeout
        }
        // rustls certificate/record failures are surfaced as io::InvalidData.
        WsError::Io(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            RealtimeError::TlsFailed
        }
        WsError::Protocol(_) => RealtimeError::ProtocolFailed,
        WsError::Capacity(_) => RealtimeError::ResponseTooLarge,
        _ => fallback,
    }
}

fn complete_handshake<S: Read + Write>(
    result: Result<
        (
            tungstenite::WebSocket<S>,
            tungstenite::handshake::client::Response,
        ),
        HandshakeError<tungstenite::ClientHandshake<S>>,
    >,
) -> Result<
    (
        tungstenite::WebSocket<S>,
        tungstenite::handshake::client::Response,
    ),
    RealtimeError,
> {
    match result {
        Ok(ready) => Ok(ready),
        Err(HandshakeError::Failure(error)) => {
            Err(socket_error(error, RealtimeError::ConnectFailed))
        }
        Err(HandshakeError::Interrupted(mut mid)) => loop {
            match mid.handshake() {
                Ok(ready) => return Ok(ready),
                Err(HandshakeError::Interrupted(next)) => mid = next,
                Err(HandshakeError::Failure(error)) => {
                    return Err(socket_error(error, RealtimeError::ConnectFailed));
                }
            }
        },
    }
}

fn send_text(socket: &mut TungsteniteSocket, payload: &str) -> Result<(), RealtimeError> {
    socket
        .socket
        .send(Message::Text(payload.into()))
        .map_err(|error| socket_error(error, RealtimeError::WriteFailed))?;
    socket
        .socket
        .flush()
        .map_err(|error| socket_error(error, RealtimeError::WriteFailed))
}

fn collect_turn(
    socket: &mut TungsteniteSocket,
    cancel: &AtomicBool,
) -> Result<RealtimeTurn, RealtimeError> {
    let mut assembler = InputTranscriptAssembler::new();
    let mut user_text = String::new();
    let mut assistant_text = String::new();
    let mut tts_pcm = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(RealtimeError::Cancelled);
        }
        let raw = socket.recv_text(SESSION_UPDATED_TIMEOUT)?;
        match parse_server_event(&raw)? {
            Some(ServerEvent::InputTranscriptDelta(payload)) => {
                if let Some((_, text, _)) = assembler.update("input_transcript_delta", &payload) {
                    user_text = text;
                }
            }
            Some(ServerEvent::InputTranscriptCompleted(payload)) => {
                if let Some((_, text, _)) = assembler.update("input_transcript_completed", &payload)
                {
                    user_text = text;
                }
            }
            Some(ServerEvent::OutputTranscript(delta) | ServerEvent::OutputText(delta)) => {
                assistant_text.push_str(&delta);
            }
            Some(ServerEvent::Audio(pcm)) => tts_pcm.extend_from_slice(&pcm),
            Some(ServerEvent::ResponseDone) => {
                let assistant_text = assistant_text.trim().to_owned();
                if assistant_text.is_empty() {
                    return Err(RealtimeError::TextEmpty);
                }
                return Ok(RealtimeTurn {
                    user_text: user_text.trim().to_owned(),
                    assistant_text,
                    tts_pcm,
                });
            }
            Some(ServerEvent::SessionCreated | ServerEvent::SessionUpdated) | None => {}
        }
    }
}

fn set_socket_timeouts(
    socket: &mut tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> Result<(), RealtimeError> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => set_tcp_timeouts(stream, timeout),
        MaybeTlsStream::Rustls(stream) => set_tcp_timeouts(stream.get_mut(), timeout),
        _ => Ok(()),
    }
}

fn set_tcp_timeouts(stream: &TcpStream, timeout: Duration) -> Result<(), RealtimeError> {
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|_| RealtimeError::ConnectFailed)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|_| RealtimeError::ConnectFailed)
}

#[cfg(test)]
mod connection_tests {
    use super::*;

    #[test]
    fn text_turn_sends_typed_content_and_receives_text_and_audio_twice() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = ProviderEndpoint {
            provider_id: "local-test".into(),
            base_url: format!(
                "http://{}/compatible-mode/v1",
                listener.local_addr().unwrap()
            ),
        };
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut ws = tungstenite::accept(stream).unwrap();
                let update: Value =
                    serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(update["type"], "session.update");
                assert_eq!(update["session"]["input_audio_format"], "pcm");
                ws.send(Message::Text(r#"{"type":"session.created"}"#.into()))
                    .unwrap();
                ws.send(Message::Text(r#"{"type":"session.updated"}"#.into()))
                    .unwrap();
                let item: Value =
                    serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(item["type"], "conversation.item.create");
                assert_eq!(item["item"]["content"][0]["type"], "input_text");
                assert_eq!(item["item"]["content"][0]["text"], "你好啊");
                let create: Value =
                    serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                assert_eq!(create["type"], "response.create");
                for event in [
                    json!({"type":"response.audio_transcript.delta","delta":"你好"}),
                    json!({"type":"response.audio.delta","delta":STANDARD.encode([1_u8, 2])}),
                    json!({"type":"response.done"}),
                ] {
                    ws.send(Message::Text(event.to_string().into())).unwrap();
                }
            }
        });
        let cancel = AtomicBool::new(false);
        for _ in 0..2 {
            let turn = OpenAiCompatibleRealtime::new()
                .text_turn(
                    RealtimeTextRequest {
                        endpoint: &endpoint,
                        credential: None,
                        model_id: "qwen-audio-3.0-realtime-plus",
                        instructions: "简短回答",
                        prompt: "你好啊",
                        include_audio: true,
                    },
                    &cancel,
                )
                .unwrap();
            assert_eq!(turn.assistant_text, "你好");
            assert_eq!(turn.tts_pcm, [1, 2]);
        }
        server.join().unwrap();
    }

    #[test]
    fn cancel_interrupts_waiting_for_a_server_message() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("ws://{}", listener.local_addr().unwrap())).unwrap();
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut ws = tungstenite::accept(listener.accept().unwrap().0).unwrap();
                std::thread::sleep(Duration::from_millis(50));
                cancel.store(true, Ordering::SeqCst);
                let _ = ws.read();
            });
            let start = Instant::now();
            let result = with_realtime_socket(&url, None, &cancel, |socket| {
                socket.recv_text(Duration::from_secs(10))
            });
            assert_eq!(result, Err(RealtimeError::Cancelled));
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "cancellation took {:?}",
                start.elapsed()
            );
        });
    }

    #[test]
    fn cancel_interrupts_incomplete_http_handshake() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("ws://{}", listener.local_addr().unwrap())).unwrap();
        let cancel = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let (mut stream, _) = listener.accept().unwrap();
                std::thread::sleep(Duration::from_millis(50));
                cancel.store(true, Ordering::SeqCst);
                let mut buffer = [0; 1024];
                while matches!(stream.read(&mut buffer), Ok(n) if n > 0) {}
            });
            let start = Instant::now();
            let result = with_realtime_socket(&url, None, &cancel, |_| Ok(()));
            assert_eq!(result, Err(RealtimeError::Cancelled));
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "cancellation took {:?}",
                start.elapsed()
            );
        });
    }

    #[test]
    fn continuous_events_cannot_extend_total_turn_budget() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("ws://{}", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let mut ws = tungstenite::accept(listener.accept().unwrap().0).unwrap();
            for _ in 0..100 {
                if ws
                    .send(Message::Text(r#"{"type":"irrelevant"}"#.into()))
                    .is_err()
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let cancel = AtomicBool::new(false);
        let start = Instant::now();
        let result =
            with_realtime_socket_budget(&url, None, &cancel, Duration::from_millis(75), |socket| {
                collect_turn(socket, &cancel)
            });
        let elapsed = start.elapsed();
        server.join().unwrap();
        assert_eq!(result, Err(RealtimeError::Timeout));
        assert!(elapsed < Duration::from_millis(500));
    }

    #[test]
    fn handshake_diagnostic_retains_only_status_and_safe_message() {
        let response = tungstenite::http::Response::builder()
            .status(429)
            .body(Some(b"Bearer secret-value".to_vec()))
            .unwrap();
        let error = socket_error(
            WsError::Http(Box::new(response)),
            RealtimeError::ConnectFailed,
        );
        assert_eq!(error, RealtimeError::HandshakeFailed(429));
        assert!(error.retryable());
        assert!(error.public_message().contains("429"));
        assert!(!error.public_message().contains("secret-value"));
    }

    #[test]
    fn control_frames_do_not_extend_receive_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut ws = tungstenite::accept(listener.accept().unwrap().0).unwrap();
            for _ in 0..40 {
                if ws.send(Message::Ping(vec![1].into())).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let (socket, _) = tungstenite::connect(format!("ws://{address}")).unwrap();
        let mut socket = TungsteniteSocket { socket };
        let start = Instant::now();
        let result = socket.recv_text(Duration::from_millis(50));
        let elapsed = start.elapsed();
        drop(socket);
        server.join().unwrap();
        assert!(matches!(result, Err(RealtimeError::Timeout)));
        assert!(
            elapsed < Duration::from_millis(250),
            "control frames extended the deadline"
        );
    }

    #[test]
    fn server_close_is_reported_as_closed() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut ws = tungstenite::accept(listener.accept().unwrap().0).unwrap();
            ws.close(None).unwrap();
        });
        let (socket, _) = tungstenite::connect(format!("ws://{address}")).unwrap();
        let result = TungsteniteSocket { socket }.recv_text(Duration::from_secs(1));
        server.join().unwrap();
        assert_eq!(result.unwrap_err().code(), "REALTIME_CONNECTION_CLOSED");
    }

    #[test]
    fn failed_handshake_preserves_auth_status_without_response_body() {
        for status in [401, 403] {
            let response = tungstenite::http::Response::builder()
                .status(status)
                .body(Some(b"secret response body".to_vec()))
                .unwrap();
            let result = complete_handshake::<TcpStream>(Err(HandshakeError::Failure(
                WsError::Http(Box::new(response)),
            )));
            assert!(matches!(result, Err(RealtimeError::Unauthorized)));
        }
    }

    #[test]
    #[ignore = "Uses the saved Windows credential and consumes provider quota; set REALTIME_SMOKE_CONFIG explicitly"]
    #[cfg(windows)]
    fn live_saved_realtime_text_smoke() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;
        let path = std::env::var("REALTIME_SMOKE_CONFIG").expect("explicit config path required");
        let config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let route_id = std::env::var("REALTIME_SMOKE_ROUTE")
            .ok()
            .or_else(|| {
                config["speech"]["activeVoiceRouteId"]
                    .as_str()
                    .map(str::to_owned)
            })
            .expect("select a route in the app or set REALTIME_SMOKE_ROUTE");
        let route = config["speech"]["voiceRoutes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"].as_str() == Some(route_id.as_str()))
            .expect("selected route must exist");
        let provider = config["models"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == route["e2eProviderId"])
            .unwrap();
        let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
        let credential = secrets
            .read(provider["credential"]["reference"].as_str().unwrap())
            .unwrap()
            .expect("saved credential must exist");
        let endpoint = ProviderEndpoint {
            provider_id: provider["id"].as_str().unwrap().to_owned(),
            base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
        };
        let cancel = AtomicBool::new(false);
        for prompt in ["你好啊", "请再打一次招呼"] {
            let result = OpenAiCompatibleRealtime::new().text_turn(
                RealtimeTextRequest {
                    endpoint: &endpoint,
                    credential: Some(credential.as_str()),
                    model_id: route["e2eModelId"].as_str().unwrap(),
                    instructions: "请用中文简短回答。",
                    prompt,
                    include_audio: true,
                },
                &cancel,
            );
            match result {
                Ok(turn) => {
                    assert!(!turn.assistant_text.is_empty());
                    assert!(!turn.tts_pcm.is_empty());
                    eprintln!(
                        "realtime smoke: reply_chars={}, audio_bytes={}",
                        turn.assistant_text.chars().count(),
                        turn.tts_pcm.len()
                    );
                }
                Err(error) => panic!("realtime smoke failed: {}", error.code()),
            }
        }
    }
}
