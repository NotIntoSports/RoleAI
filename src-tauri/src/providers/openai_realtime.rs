use std::{
    fmt,
    io::{Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use reqwest::Url;
use serde_json::json;
use tungstenite::{
    ClientRequestBuilder, Error as WsError, HandshakeError, Message, client::IntoClientRequest,
    protocol::WebSocketConfig, stream::MaybeTlsStream,
};

use crate::audio::resample_pcm16_mono;

use super::ProviderEndpoint;

pub use super::realtime_protocol::*;

pub const CONNECT_OPEN_TIMEOUT: Duration = Duration::from_secs(10);
pub const SESSION_UPDATED_TIMEOUT: Duration = Duration::from_secs(10);
const TURN_TIMEOUT: Duration = Duration::from_secs(90);
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
const CANCEL_POLL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq)]
/// 实时会话链路的稳定错误码，前端据此提示，不携带敏感细节。
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
    /// 线路配置的音色（克隆音色 ID 或官方音色名）。空串时回退方言默认；
    /// DashScope 无方言默认可用音色，缺失会被服务端以 Voice not supported 拒绝。
    pub voice: &'a str,
    /// 应答文本流式快照回调；手动输入场景用户文本已知，通常只挂 assistant。
    pub hooks: super::TurnStreamHooks<'a>,
}

pub struct RealtimeAudioRequest<'a> {
    pub endpoint: &'a ProviderEndpoint,
    pub credential: Option<&'a str>,
    pub model_id: &'a str,
    pub pcm16le: &'a [u8],
    pub sample_rate: u32,
    pub instructions: &'a str,
    pub voice: &'a str,
    /// 用户转写与应答文本的流式快照回调。
    pub hooks: super::TurnStreamHooks<'a>,
}

/// 实时全双工模型会话能力：推流、提交、收轮，由 openai_realtime 实现。
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
            let voice = selected_voice(request.voice, &dialect);
            let update = session_update_event(voice, request.instructions, &dialect);
            send_text(socket, &update.to_string())?;
            wait_session_updated(socket, SESSION_UPDATED_TIMEOUT)?;
            if cancel.load(Ordering::Relaxed) {
                return Err(RealtimeError::Cancelled);
            }
            // 整段 utterance 单帧发送会被服务端断开（DashScope 单帧上限远小于
            // 一句话的 PCM，实测 WriteFailed）；按 ~100ms 分块流式 append。
            // Aliyun Manual 模式：commit 提交缓冲后必须显式 response.create 触发应答；
            // 其余方言（server_vad 自动提交并触发应答）只 append + commit。
            for chunk in pcm16le.chunks(AUDIO_APPEND_CHUNK_BYTES) {
                send_text(socket, &append_audio_event(chunk))?;
            }
            send_text(socket, r#"{"type":"input_audio_buffer.commit"}"#)?;
            if dialect.name == RealtimeDialectName::Aliyun {
                send_text(socket, &response_create_event(true))?;
            }
            collect_turn(socket, cancel, &request.hooks)
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
            let voice = selected_voice(request.voice, &dialect);
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
            collect_turn(socket, cancel, &request.hooks)
        })
    }
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

pub(crate) struct TungsteniteSocket {
    pub(crate) socket: tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
}

impl RealtimeTransport for TungsteniteSocket {
    fn recv_text(&mut self, timeout: Duration) -> Result<String, RealtimeError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RealtimeError::Timeout);
            }
            // 只改读超时：实时泵以 20ms 轮询读，若写超时也被压到 20ms，
            // 随后的音频/图片 append 在网络稍慢时就会误报 REALTIME_TIMEOUT 并重连。
            set_socket_read_timeout(&mut self.socket, remaining)?;
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

fn collect_turn(
    socket: &mut TungsteniteSocket,
    cancel: &AtomicBool,
    hooks: &super::TurnStreamHooks<'_>,
) -> Result<RealtimeTurn, RealtimeError> {
    let mut assembler = InputTranscriptAssembler::new();
    let mut user_text = String::new();
    let mut assistant_text = String::new();
    let mut tts_pcm = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(RealtimeError::Cancelled);
        }
        let raw = socket.recv_text(COLLECT_IDLE_TIMEOUT)?;
        match parse_server_event(&raw)? {
            Some(ServerEvent::InputTranscriptDelta(payload)) => {
                if let Some((_, text, _)) = assembler.update("input_transcript_delta", &payload) {
                    user_text = text;
                    hooks.notify_user(&user_text);
                }
            }
            Some(ServerEvent::InputTranscriptCompleted(payload)) => {
                if let Some((_, text, _)) = assembler.update("input_transcript_completed", &payload)
                {
                    user_text = text;
                    hooks.notify_user(&user_text);
                }
            }
            Some(ServerEvent::OutputTranscript(delta) | ServerEvent::OutputText(delta)) => {
                assistant_text.push_str(&delta);
                hooks.notify_assistant(&assistant_text);
            }
            Some(ServerEvent::Audio(pcm)) => tts_pcm.extend_from_slice(&pcm),
            Some(ServerEvent::ResponseDone(_)) => {
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
            Some(
                ServerEvent::SessionCreated
                | ServerEvent::SessionUpdated
                | ServerEvent::SpeechStarted
                | ServerEvent::SpeechStopped
                | ServerEvent::InputCommitted
                | ServerEvent::ItemCreated
                | ServerEvent::ResponseCreated,
            )
            | None => {}
        }
    }
}

/// collect_turn 单帧间隔上限：长语音转写/首 token 生成间隙可能超过 10 秒，
/// 误杀在途成功轮次；总时长仍由 with_realtime_socket 的轮预算（90 秒）兜底。
const COLLECT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

// 子模块：纯搬移拆分（C24）。pub/pub(crate) 项经再导出保持原路径；
// 原私有、父区域或测试仍使用的项升 pub(super)，父模块私有 use 引入。
mod socket;

pub(crate) use self::socket::{complete_handshake, connect_tcp, send_text, set_tcp_timeouts};
use self::socket::{
    set_socket_read_timeout, socket_error, with_realtime_socket, with_realtime_socket_budget,
};

#[cfg(test)]
mod tests;
