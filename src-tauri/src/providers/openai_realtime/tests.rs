//! openai_realtime 连接层单元测试（纯搬移自 openai_realtime.rs 的 connection_tests 模块）。

use super::*;
// 纯搬移后 race_tcp_connect 为 socket 子模块私有项，测试经显式路径引入。
use super::socket::race_tcp_connect;
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
            let item: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
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
                    let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap())
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
                    if text.contains("\"response.done\"") || text.contains("\"type\":\"error\"") {
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
                eprintln!("forced probe: transcription incomplete after 30s, responding anyway");
                break;
            }
            let text = match socket.recv_text(remaining.min(std::time::Duration::from_secs(5))) {
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
            let remaining = respond_deadline.saturating_duration_since(std::time::Instant::now());
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
        let update: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
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
            let frame: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
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
        let result = complete_handshake::<TcpStream>(Err(HandshakeError::Failure(WsError::Http(
            Box::new(response),
        ))));
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
