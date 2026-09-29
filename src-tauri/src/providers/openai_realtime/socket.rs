//! WebSocket 连接建立、竞速与超时预算、握手与帧写出（纯搬移自 openai_realtime.rs）。

use super::*;

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
pub(super) fn race_tcp_connect(
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

pub(super) fn with_realtime_socket<T>(
    url: &Url,
    credential: Option<&str>,
    cancel: &AtomicBool,
    action: impl FnOnce(&mut TungsteniteSocket) -> Result<T, RealtimeError>,
) -> Result<T, RealtimeError> {
    with_realtime_socket_budget(url, credential, cancel, TURN_TIMEOUT, action)
}

pub(super) fn with_realtime_socket_budget<T>(
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

pub(super) fn socket_error(error: WsError, fallback: RealtimeError) -> RealtimeError {
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

pub(super) fn set_socket_read_timeout(
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
