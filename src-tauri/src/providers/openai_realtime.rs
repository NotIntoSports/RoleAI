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
pub(crate) fn connect_tcp(url: &Url, cancel: &AtomicBool) -> Result<TcpStream, RealtimeError> {
    let host = url.host_str().ok_or(RealtimeError::UrlInvalid)?.to_owned();
    let port = url
        .port_or_known_default()
        .ok_or(RealtimeError::UrlInvalid)?;
    let deadline = Instant::now() + CONNECT_OPEN_TIMEOUT;
    // getaddrinfo 常把 IPv6 排在首位，而本机 IPv6 路由可能不可达——串行逐个尝试
    // 会让首个黑洞地址耗尽整个连接预算（对称影响任何解析顺序不利的域名）。
    // 并行竞速全部解析地址（RFC 8305 Happy Eyeballs 思路），任一成功即用。
    let addresses: Vec<std::net::SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| RealtimeError::DnsFailed)?
        .take(8)
        .collect();
    race_tcp_connect(&addresses, cancel, deadline, |address, remaining| {
        TcpStream::connect_timeout(&address, remaining)
    })
}

/// 对全部解析地址并行发起 TCP 连接，最先成功者胜出；取消与预算语义不变。
/// 全部快速失败返回最后一次错误，预算耗尽或被取消返回相应错误。
fn race_tcp_connect(
    addresses: &[std::net::SocketAddr],
    cancel: &AtomicBool,
    deadline: Instant,
    connect: impl Fn(std::net::SocketAddr, Duration) -> std::io::Result<TcpStream>
    + Send
    + Sync
    + Clone
    + 'static,
) -> Result<TcpStream, RealtimeError> {
    if addresses.is_empty() {
        return Err(RealtimeError::DnsFailed);
    }
    let (tx, rx) = mpsc::channel();
    for address in addresses {
        let address = *address;
        let sender = tx.clone();
        let connect = connect.clone();
        std::thread::spawn(move || {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let result = if remaining.is_zero() {
                Err(RealtimeError::Timeout)
            } else {
                connect(address, remaining).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::TimedOut {
                        RealtimeError::Timeout
                    } else {
                        RealtimeError::TcpFailed
                    }
                })
            };
            let _ = sender.send(result);
        });
    }
    drop(tx);
    let mut last = RealtimeError::DnsFailed;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(RealtimeError::Cancelled);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RealtimeError::Timeout);
        }
        match rx.recv_timeout(remaining.min(CANCEL_POLL)) {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(error)) => last = error,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(last),
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

pub(crate) fn complete_handshake<S: Read + Write>(
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

pub(crate) fn send_text(
    socket: &mut TungsteniteSocket,
    payload: &str,
) -> Result<(), RealtimeError> {
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

fn set_socket_read_timeout(
    socket: &mut tungstenite::WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Duration,
) -> Result<(), RealtimeError> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => stream.get_mut(),
        _ => return Ok(()),
    };
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|_| RealtimeError::ConnectFailed)
}

pub(crate) fn set_tcp_timeouts(stream: &TcpStream, timeout: Duration) -> Result<(), RealtimeError> {
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
    // 事件解析与 base64 音频编解码已拆至 realtime_protocol；测试目标仍要用。
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde_json::Value;

    #[test]
    fn recv_poll_does_not_shrink_write_timeout() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let _ = ws.read();
        });
        let stream = TcpStream::connect(address).unwrap();
        set_tcp_timeouts(&stream, CONNECT_OPEN_TIMEOUT).unwrap();
        let (socket, _) =
            tungstenite::client(format!("ws://{address}/"), MaybeTlsStream::Plain(stream)).unwrap();
        let mut socket = TungsteniteSocket { socket };

        assert_eq!(
            socket.recv_text(Duration::from_millis(20)),
            Err(RealtimeError::Timeout)
        );
        let MaybeTlsStream::Plain(stream) = socket.socket.get_ref() else {
            panic!("expected plain stream");
        };
        assert_eq!(stream.write_timeout().unwrap(), Some(CONNECT_OPEN_TIMEOUT));
        let _ = socket.socket.close(None);
        let _ = socket.socket.flush();
        drop(socket);
        server.join().unwrap();
    }

    #[test]
    fn race_tcp_connect_wins_via_later_address_when_first_attempt_stalls() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let good = listener.local_addr().unwrap();
        let stalled = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        // 地址顺序模拟 getaddrinfo 的 IPv6 在前：首个地址的连接尝试挂住不返回。
        let addresses = vec![stalled, good];
        let connect = move |address: std::net::SocketAddr, budget: Duration| {
            if address == stalled {
                std::thread::sleep(budget.min(Duration::from_millis(800)));
                Err(std::io::Error::from(std::io::ErrorKind::TimedOut))
            } else {
                TcpStream::connect_timeout(&address, budget)
            }
        };
        let cancel = AtomicBool::new(false);
        let start = Instant::now();
        let stream = race_tcp_connect(
            &addresses,
            &cancel,
            Instant::now() + Duration::from_secs(10),
            connect,
        )
        .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(stream.peer_addr().unwrap(), good);
        assert!(
            elapsed < Duration::from_secs(2),
            "race should not wait for the stalled first attempt, took {elapsed:?}"
        );
    }

    #[test]
    fn race_tcp_connect_returns_timeout_when_budget_expires_without_success() {
        let addresses = vec![std::net::SocketAddr::from(([127, 0, 0, 1], 1))];
        let connect = |_address: std::net::SocketAddr, budget: Duration| {
            std::thread::sleep(budget.min(Duration::from_millis(300)));
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut))
        };
        let cancel = AtomicBool::new(false);
        let result = race_tcp_connect(
            &addresses,
            &cancel,
            Instant::now() + Duration::from_millis(500),
            connect,
        );
        assert!(matches!(result, Err(RealtimeError::Timeout)));
    }

    #[test]
    fn race_tcp_connect_reports_tcp_failed_when_all_attempts_fail_fast() {
        let first = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let second = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = first.local_addr().unwrap();
        let taken2 = second.local_addr().unwrap();
        drop(first);
        drop(second);
        let addresses = vec![taken, taken2];
        let cancel = AtomicBool::new(false);
        let result = race_tcp_connect(
            &addresses,
            &cancel,
            Instant::now() + Duration::from_secs(5),
            |address, budget| TcpStream::connect_timeout(&address, budget),
        );
        assert!(matches!(result, Err(RealtimeError::TcpFailed)));
    }

    #[test]
    fn race_tcp_connect_honours_cancellation() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let cancel = AtomicBool::new(true);
        let start = Instant::now();
        let result = race_tcp_connect(
            &[address],
            &cancel,
            Instant::now() + Duration::from_secs(5),
            |address, budget| TcpStream::connect_timeout(&address, budget),
        );
        assert!(matches!(result, Err(RealtimeError::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

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
                        voice: "",
                        hooks: super::super::TurnStreamHooks::none(),
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
                collect_turn(socket, &cancel, &super::super::TurnStreamHooks::none())
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

    /// 诊断探测：对 DashScope realtime 握手并原样打印服务端前若干帧（定位 COMMON_ERROR 等远端错误细节）。
    /// 运行：REALTIME_SMOKE_CONFIG=<config> REALTIME_SMOKE_ROUTE=<route id>
    /// cargo test --lib live_dashscope_realtime_raw_probe -- --ignored --nocapture
    #[test]
    #[ignore = "Uses the saved Windows credential; set REALTIME_SMOKE_CONFIG and REALTIME_SMOKE_ROUTE explicitly"]
    #[cfg(windows)]
    fn live_dashscope_realtime_raw_probe() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;

        let path = std::env::var("REALTIME_SMOKE_CONFIG").expect("explicit config path required");
        let config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let route_id = std::env::var("REALTIME_SMOKE_ROUTE").expect("route id required");
        let route = config["speech"]["voiceRoutes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"].as_str() == Some(route_id.as_str()))
            .expect("selected route must exist")
            .clone();
        let provider = config["models"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == route["e2eProviderId"])
            .unwrap()
            .clone();
        let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
        let credential = secrets
            .read(provider["credential"]["reference"].as_str().unwrap())
            .unwrap()
            .expect("saved credential must exist");
        let endpoint = ProviderEndpoint {
            provider_id: provider["id"].as_str().unwrap().to_owned(),
            base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
        };
        let model = route["e2eModelId"].as_str().unwrap_or("");
        let url = realtime_url(&endpoint.base_url, model).unwrap();
        eprintln!("raw probe: url={url}");
        let dialect = realtime_dialect(&endpoint.base_url);
        eprintln!(
            "raw probe: dialect={:?} input_rate={}",
            dialect.name, dialect.input_rate
        );
        let voice_override = std::env::var("REALTIME_SMOKE_VOICE")
            .unwrap_or_else(|_| route["voiceId"].as_str().unwrap_or("").to_owned());
        let update = session_update_event(
            &voice_override,
            "你是会议助手，请用中文简短回答。",
            &dialect,
        );
        let cancel = AtomicBool::new(false);
        let result = with_realtime_socket(&url, Some(credential.as_str()), &cancel, |socket| {
            send_text(socket, &update.to_string())?;
            for step in 0..6 {
                match socket.recv_text(Duration::from_secs(8)) {
                    Ok(text) => eprintln!("raw probe <- {text}"),
                    Err(error) => {
                        eprintln!("raw probe recv error: {error:?}");
                        return Err(error);
                    }
                }
                if step == 1 {
                    send_text(socket, &conversation_text_event("你好，请打个招呼。"))?;
                    send_text(socket, &response_create_event(true))?;
                }
            }
            Ok(())
        });
        eprintln!("raw probe result: {result:?}");
    }

    /// 音频轮诊断：复刻 transcribe_turn 的完整流程（session.update → append → commit），
    /// 原样打印服务端每一帧，定位音频路径快速失败的远端原因。
    /// 运行：REALTIME_SMOKE_CONFIG=<config> REALTIME_SMOKE_ROUTE=<route id>
    /// [REALTIME_SMOKE_WAV=<16k mono wav，默认内置 1s 静音>]
    #[test]
    #[ignore = "Uses the saved Windows credential and consumes provider quota; set REALTIME_SMOKE_CONFIG and REALTIME_SMOKE_ROUTE explicitly"]
    #[cfg(windows)]
    fn live_dashscope_realtime_audio_probe() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;

        let path = std::env::var("REALTIME_SMOKE_CONFIG").expect("explicit config path required");
        let config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let route_id = std::env::var("REALTIME_SMOKE_ROUTE").expect("route id required");
        let route = config["speech"]["voiceRoutes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"].as_str() == Some(route_id.as_str()))
            .expect("selected route must exist")
            .clone();
        let provider = config["models"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == route["e2eProviderId"])
            .unwrap()
            .clone();
        let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
        let credential = secrets
            .read(provider["credential"]["reference"].as_str().unwrap())
            .unwrap()
            .expect("saved credential must exist");
        let endpoint = ProviderEndpoint {
            provider_id: provider["id"].as_str().unwrap().to_owned(),
            base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
        };
        let url = realtime_url(
            &endpoint.base_url,
            route["e2eModelId"].as_str().unwrap_or(""),
        )
        .unwrap();
        let dialect = realtime_dialect(&endpoint.base_url);

        // 读取 wav 的 data 块作为 16-bit mono PCM；raw PCM 文件直接使用；未提供则用 1 秒静音。
        let wav_path = std::env::var("REALTIME_SMOKE_WAV").ok();
        let pcm: Vec<u8> = wav_path.as_deref().map_or_else(
            || vec![0_u8; dialect.input_rate as usize * 2],
            |wav_path| {
                let bytes = std::fs::read(wav_path).unwrap();
                if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" {
                    let mut offset = 12usize;
                    while offset + 8 <= bytes.len() {
                        let id = &bytes[offset..offset + 4];
                        let size =
                            u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap())
                                as usize;
                        if id == b"data" {
                            let end = (offset + 8 + size).min(bytes.len());
                            return bytes[offset + 8..end].to_vec();
                        }
                        offset += 8 + size + (size & 1);
                    }
                    panic!("no data chunk in {wav_path}");
                }
                bytes
            },
        );
        eprintln!(
            "audio probe: pcm_bytes={} rate={}",
            pcm.len(),
            dialect.input_rate
        );

        let voice_override = std::env::var("REALTIME_SMOKE_VOICE")
            .unwrap_or_else(|_| route["voiceId"].as_str().unwrap_or("").to_owned());
        // 默认复刻应用的 session.update 载荷（角色 instructions 来自会话配置）；
        // 设 REALTIME_PROBE_PLAIN=1 时用短句，便于二分定位。
        let instructions = if std::env::var("REALTIME_PROBE_PLAIN").is_ok() {
            "你是会议助手，请用中文简短回答。".to_owned()
        } else {
            std::env::var("REALTIME_PROBE_INSTRUCTIONS").unwrap_or_else(|_| {
                config["roleProfiles"]
                    .as_array()
                    .and_then(|profiles| {
                        profiles
                            .iter()
                            .find(|p| p["active"].as_bool() == Some(true))
                            .or_else(|| profiles.first())
                    })
                    .map(|p| {
                        let mut out = p["systemPrompt"].as_str().unwrap_or("").to_owned();
                        if let Some(style) = p["styleInstructions"].as_str()
                            && !style.is_empty()
                        {
                            out.push_str("\n\n");
                            out.push_str(style);
                        }
                        out
                    })
                    .unwrap_or_default()
            })
        };
        eprintln!(
            "audio probe: instructions_len={}",
            instructions.chars().count()
        );
        let update = session_update_event(&voice_override, &instructions, &dialect);
        let cancel = AtomicBool::new(false);
        let result = with_realtime_socket(&url, Some(credential.as_str()), &cancel, |socket| {
            send_text(socket, &update.to_string())?;
            // 复刻 transcribe_turn 新流程：等 session.updated → 分块 append →
            // 等 VAD 自动 committed → response.create。
            wait_session_updated(socket, SESSION_UPDATED_TIMEOUT)?;
            eprintln!("audio probe: session updated confirmed");
            let chunk = dialect.input_rate as usize * 2 / 10;
            for piece in pcm.chunks(chunk) {
                send_text(socket, &append_audio_event(piece))?;
            }
            send_text(socket, r#"{"type":"input_audio_buffer.commit"}"#)?;
            send_text(socket, &response_create_event(true))?;
            for _ in 0..200 {
                match socket.recv_text(Duration::from_secs(10)) {
                    Ok(text) => {
                        eprintln!("audio probe <- {}", &text[..text.len().min(500)]);
                        if text.contains("\"response.done\"") || text.contains("\"type\":\"error\"")
                        {
                            return Ok(());
                        }
                    }
                    Err(error) => {
                        eprintln!("audio probe recv error: {error:?}");
                        return Err(error);
                    }
                }
            }
            Ok(())
        });
        eprintln!("audio probe result: {result:?}");
    }

    /// 强制回答诊断（仅转写追答链路）：复刻泵的真实时序——append 全部语音 →
    /// commit → 等转写 completed → conversation.item.create(原文) + response.create，
    /// 逐帧打印服务端返回，定位「强制回答后服务端沉默/断链」发生在哪一步。
    /// 运行：REALTIME_SMOKE_CONFIG=<config> REALTIME_SMOKE_ROUTE=<route id>
    /// REALTIME_SMOKE_WAV=<16k mono wav>
    #[test]
    #[ignore = "Uses the saved Windows credential and consumes provider quota; set REALTIME_SMOKE_* explicitly"]
    #[cfg(windows)]
    fn live_dashscope_forced_respond_after_commit_probe() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;

        let path = std::env::var("REALTIME_SMOKE_CONFIG").expect("explicit config path required");
        let config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let route_id = std::env::var("REALTIME_SMOKE_ROUTE").expect("route id required");
        let route = config["speech"]["voiceRoutes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"].as_str() == Some(route_id.as_str()))
            .expect("selected route must exist")
            .clone();
        let provider = config["models"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == route["e2eProviderId"])
            .unwrap()
            .clone();
        let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
        let credential = secrets
            .read(provider["credential"]["reference"].as_str().unwrap())
            .unwrap()
            .expect("saved credential must exist");
        let endpoint = ProviderEndpoint {
            provider_id: provider["id"].as_str().unwrap().to_owned(),
            base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
        };
        let url = realtime_url(
            &endpoint.base_url,
            route["e2eModelId"].as_str().unwrap_or(""),
        )
        .unwrap();
        let dialect = realtime_dialect(&endpoint.base_url);
        let wav_path = std::env::var("REALTIME_SMOKE_WAV").expect("speech wav required");
        let bytes = std::fs::read(&wav_path).unwrap();
        let pcm = {
            assert!(bytes.len() >= 12 && &bytes[0..4] == b"RIFF", "expect wav");
            let mut offset = 12usize;
            loop {
                assert!(offset + 8 <= bytes.len(), "no data chunk");
                let id = &bytes[offset..offset + 4];
                let size =
                    u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
                if id == b"data" {
                    break bytes[offset + 8..(offset + 8 + size).min(bytes.len())].to_vec();
                }
                offset += 8 + size + (size & 1);
            }
        };
        let item_text =
            std::env::var("REALTIME_PROBE_ITEM").unwrap_or_else(|_| "你听得见我说话吗".to_owned());
        let voice = route["voiceId"].as_str().unwrap_or("").to_owned();
        let update = session_update_event(
            &voice,
            "你是会议助手。普通讨论只听不答；被点名或被要求回答时用中文简短回答。",
            &dialect,
        );
        let cancel = AtomicBool::new(false);
        let result = with_realtime_socket(&url, Some(credential.as_str()), &cancel, |socket| {
            send_text(socket, &update.to_string())?;
            wait_session_updated(socket, SESSION_UPDATED_TIMEOUT)?;
            eprintln!("forced probe: session updated");
            let chunk = dialect.input_rate as usize / 5; // 200ms 一块，贴近真实上行
            for piece in pcm.chunks(chunk) {
                send_text(socket, &append_audio_event(piece))?;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            // 700ms 静音尾窗（与泵测试/真实分段器一致）再提交。
            for _ in 0..7 {
                send_text(socket, &append_audio_event(&[0u8; 3200]))?;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            send_text(socket, r#"{"type":"input_audio_buffer.commit"}"#)?;
            eprintln!("forced probe: committed, waiting for transcription");
            // 等转写 completed（同泵流程：转写定稿后泵才发 ForceRespond）。
            // delta 与 completed 之间服务端常有数秒空窗，Timeout 必须继续轮询；
            // 服务端转写流本身偶发停滞（实测卡在半句），超时后仍继续观测 respond 段。
            let mut transcript = String::new();
            let mut got_completed = false;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !got_completed {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    eprintln!(
                        "forced probe: transcription incomplete after 30s, responding anyway"
                    );
                    break;
                }
                let text = match socket.recv_text(remaining.min(std::time::Duration::from_secs(5)))
                {
                    Ok(text) => text,
                    Err(RealtimeError::Timeout) => continue,
                    Err(error) => return Err(error),
                };
                eprintln!("forced probe <- {}", text.get(..300).unwrap_or(&text));
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                match value["type"].as_str().unwrap_or("") {
                    "conversation.item.input_audio_transcription.completed" => {
                        transcript = value["transcript"].as_str().unwrap_or("").to_owned();
                        got_completed = true;
                    }
                    "error" => panic!("forced probe: server error before respond: {text}"),
                    _ => {}
                }
            }
            eprintln!("forced probe: transcript=\"{transcript}\" -> item.create + response.create");
            eprintln!("forced probe: transcript=\"{transcript}\" -> item.create + response.create");
            send_text(socket, &conversation_text_event(&item_text))?;
            send_text(socket, &response_create_event(true))?;
            // 观测服务端对强制回答的反应：response.done / error / 沉默断链。
            let respond_deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                let remaining =
                    respond_deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    eprintln!("forced probe: 60s with no response.done — server stalled");
                    return Ok(());
                }
                match socket.recv_text(remaining.min(std::time::Duration::from_secs(5))) {
                    Ok(text) => {
                        eprintln!("forced probe <- {}", text.get(..300).unwrap_or(&text));
                        if text.contains("\"response.done\"") {
                            eprintln!("forced probe: response.done received");
                            return Ok(());
                        }
                        if text.contains("\"type\":\"error\"") {
                            eprintln!("forced probe: server error event");
                            return Ok(());
                        }
                    }
                    Err(error) => {
                        eprintln!("forced probe: recv error after respond: {error:?}");
                        return Err(error);
                    }
                }
            }
        });
        eprintln!("forced probe result: {result:?}");
    }

    /// 应用同路径音频轮诊断：直接调 transcribe_turn（session.update→append→commit→collect_turn），
    /// 打印最终 Err 的真实错误码，与手拼探针的帧级观测互为对照。
    /// 运行：REALTIME_SMOKE_CONFIG=<config> REALTIME_SMOKE_ROUTE=<route id>
    /// [REALTIME_SMOKE_WAV=<16k mono wav>]
    #[test]
    #[ignore = "Uses the saved Windows credential and consumes provider quota; set REALTIME_SMOKE_CONFIG and REALTIME_SMOKE_ROUTE explicitly"]
    #[cfg(windows)]
    fn live_transcribe_turn_smoke() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;

        let path = std::env::var("REALTIME_SMOKE_CONFIG").expect("explicit config path required");
        let config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let route_id = std::env::var("REALTIME_SMOKE_ROUTE").expect("route id required");
        let route = config["speech"]["voiceRoutes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"].as_str() == Some(route_id.as_str()))
            .expect("selected route must exist")
            .clone();
        let provider = config["models"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == route["e2eProviderId"])
            .unwrap()
            .clone();
        let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
        let credential = secrets
            .read(provider["credential"]["reference"].as_str().unwrap())
            .unwrap()
            .expect("saved credential must exist");
        let endpoint = ProviderEndpoint {
            provider_id: provider["id"].as_str().unwrap().to_owned(),
            base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
        };

        let wav_path = std::env::var("REALTIME_SMOKE_WAV")
            .unwrap_or_else(|_| "C:/Users/28839/AppData/Local/Temp/clone-sample.wav".into());
        let bytes = std::fs::read(&wav_path).unwrap();
        let pcm = if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" {
            let mut offset = 12usize;
            loop {
                if offset + 8 > bytes.len() {
                    panic!("no data chunk in {wav_path}");
                }
                let id = &bytes[offset..offset + 4];
                let size =
                    u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
                if id == b"data" {
                    let end = (offset + 8 + size).min(bytes.len());
                    break bytes[offset + 8..end].to_vec();
                }
                offset += 8 + size + (size & 1);
            }
        } else {
            // 原始 16k mono PCM（如 roleai-utterance-latest.pcm 诊断转储）。
            bytes
        };
        eprintln!("transcribe smoke: pcm_bytes={}", pcm.len());

        // 与真实会话一致：优先用线路配置的音色，未设环境变量覆盖时取 route.voiceId。
        let voice_override = std::env::var("REALTIME_SMOKE_VOICE")
            .unwrap_or_else(|_| route["voiceId"].as_str().unwrap_or("").to_owned());
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let result = OpenAiCompatibleRealtime::new().transcribe_turn(
            RealtimeAudioRequest {
                endpoint: &endpoint,
                credential: Some(credential.as_str()),
                model_id: route["e2eModelId"].as_str().unwrap_or(""),
                pcm16le: &pcm,
                sample_rate: 16_000,
                instructions: "你是会议助手，请用中文简短回答。",
                voice: &voice_override,
                hooks: super::super::TurnStreamHooks::none(),
            },
            &cancel,
        );
        eprintln!(
            "transcribe smoke: elapsed={:?} result={result:?}",
            started.elapsed()
        );
        let turn = result.expect("transcribe_turn should succeed");
        eprintln!(
            "transcribe smoke: user={} assistant={} pcm={}",
            turn.user_text,
            turn.assistant_text,
            turn.tts_pcm.len()
        );
    }

    #[test]
    #[allow(unused_assignments)]
    fn transcribe_turn_streams_audio_in_chunks_and_collects_reply() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = ProviderEndpoint {
            provider_id: "local-test".into(),
            base_url: format!(
                "http://{}/api-ws/v1/realtime",
                listener.local_addr().unwrap()
            ),
        };
        // 3 秒 16kHz PCM：单帧 ~96KB，分块后应远大于 1 帧。
        let pcm = vec![0x10_u8; 16_000 * 2 * 3];
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            // session.update
            let update: Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(update["type"], "session.update");
            assert_eq!(
                update["session"]["voice"], "qwen-omni-vc-test",
                "route-configured voice must reach session.update"
            );
            // Aliyun Manual 模式：禁用服务端 VAD（本地 SmartTurn 已判定语句结束），
            // 由客户端 commit + response.create 驱动，避免 VAD 自动提交的竞态。
            assert!(update["session"]["turn_detection"].is_null());
            ws.send(Message::Text(r#"{"type":"session.created"}"#.into()))
                .unwrap();
            ws.send(Message::Text(r#"{"type":"session.updated"}"#.into()))
                .unwrap();
            // 收 append 帧与 commit；Manual 模式下 commit 后必须紧跟 response.create。
            let mut chunk_sizes = Vec::new();
            let mut commit_at: Option<usize> = None;
            let mut create_at: Option<usize> = Some(0);
            let mut frame_no = 0usize;
            loop {
                frame_no += 1;
                let frame: Value =
                    serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
                match frame["type"].as_str().unwrap() {
                    "input_audio_buffer.append" => {
                        let decoded = STANDARD.decode(frame["audio"].as_str().unwrap()).unwrap();
                        chunk_sizes.push(decoded.len());
                    }
                    "input_audio_buffer.commit" => commit_at = Some(frame_no),
                    "response.create" => {
                        create_at = Some(frame_no);
                        break;
                    }
                    other => panic!("unexpected frame {other}"),
                }
            }
            let commit = commit_at.expect("manual commit is required in manual mode");
            let create = create_at.expect("response.create must follow commit");
            assert!(create > commit, "response.create must be sent after commit");
            assert!(
                chunk_sizes.len() > 10,
                "expected streamed chunks, got {}",
                chunk_sizes.len()
            );
            for size in &chunk_sizes[..chunk_sizes.len() - 1] {
                assert_eq!(
                    *size, AUDIO_APPEND_CHUNK_BYTES,
                    "all but last chunk are fixed size"
                );
            }
            ws.send(Message::Text(
                r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"item_1","transcript":"你好"}"#
                    .into(),
            ))
            .unwrap();
            ws.send(Message::Text(
                format!(
                    r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                    STANDARD.encode([0x01_u8, 0x02])
                )
                .into(),
            ))
            .unwrap();
            ws.send(Message::Text(
                r#"{"type":"response.audio_transcript.delta","delta":"你好呀"}"#.into(),
            ))
            .unwrap();
            ws.send(Message::Text(r#"{"type":"response.done"}"#.into()))
                .unwrap();
            chunk_sizes.len()
        });

        let cancel = AtomicBool::new(false);
        let turn = OpenAiCompatibleRealtime::new()
            .transcribe_turn(
                RealtimeAudioRequest {
                    endpoint: &endpoint,
                    credential: None,
                    model_id: "qwen3.8-omni-flash-realtime",
                    pcm16le: &pcm,
                    sample_rate: 16_000,
                    instructions: "请简短回答。",
                    voice: "qwen-omni-vc-test",
                    hooks: super::super::TurnStreamHooks::none(),
                },
                &cancel,
            )
            .unwrap();
        let chunk_count = server.join().unwrap();
        assert_eq!(turn.user_text, "你好");
        assert_eq!(turn.assistant_text, "你好呀");
        assert_eq!(turn.tts_pcm, vec![0x01_u8, 0x02]);
        assert!(
            chunk_count <= 32,
            "3s audio should be ~30 chunks, got {chunk_count}"
        );
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
                    voice: route["voiceId"].as_str().unwrap_or(""),
                    hooks: super::super::TurnStreamHooks::none(),
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
