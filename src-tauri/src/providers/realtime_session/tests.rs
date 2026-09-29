//! realtime_session 单元测试（纯搬移自 realtime_session.rs 的 tests 模块）。

use super::*;
use std::net::{TcpListener, TcpStream};
use tungstenite::{Message, WebSocket};

type MockSocket = WebSocket<TcpStream>;

/// 接受一条连接：读 session.update → 回 created/updated；返回 update 载荷与 socket。
fn accept_session(listener: &TcpListener) -> (Value, MockSocket) {
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut ws = tungstenite::accept(stream).unwrap();
    let update: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(update["type"], "session.update");
    ws.send(Message::Text(r#"{"type":"session.created"}"#.into()))
        .unwrap();
    ws.send(Message::Text(r#"{"type":"session.updated"}"#.into()))
        .unwrap();
    (update, ws)
}

fn read_frame(ws: &mut MockSocket) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "mock server timed out waiting for frame"
        );
        ws.get_mut().set_read_timeout(Some(remaining)).unwrap();
        match ws.read().unwrap() {
            Message::Text(text) => {
                let frame: Value = serde_json::from_str(&text).unwrap();
                // 回放逐条等回执：桩收到 item.create 即回 created，
                // 客户端 send_item_create 不必空等 5s 超时。
                if frame["type"] == "conversation.item.create" {
                    ws.send(Message::Text(
                        r#"{"type":"conversation.item.created"}"#.into(),
                    ))
                    .unwrap();
                }
                return frame;
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            Message::Binary(_) => continue,
            Message::Close(_) => panic!("unexpected close"),
        }
    }
}

fn spawn_actor(port: u16, auto_respond: bool, history: Vec<(String, String)>) -> RealtimeSession {
    RealtimeSession::start(RealtimeSessionConfig {
        endpoint: ProviderEndpoint {
            provider_id: "test".into(),
            base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
        },
        credential: None,
        model_id: "qwen3.8-omni-flash-realtime".into(),
        voice: String::new(),
        instructions: "测试指令".into(),
        history,
        auto_respond,
        enable_search: false,
    })
    .unwrap()
}

/// 联网搜索：enable_search=true 时 Aliyan 方言 session.update 携带顶层
/// 开关与来源选项；OpenAI 方言（无此字段）即使开启也不发送。
#[test]
fn enable_search_injected_only_for_aliyun_dialect() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = RealtimeSession::start(RealtimeSessionConfig {
        endpoint: ProviderEndpoint {
            provider_id: "test".into(),
            base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
        },
        credential: None,
        model_id: "qwen3.8-omni-flash-realtime".into(),
        voice: String::new(),
        instructions: "测试指令".into(),
        history: vec![],
        auto_respond: true,
        enable_search: true,
    })
    .unwrap();
    let (update, _ws) = accept_session(&listener);
    assert_eq!(update["session"]["enable_search"], true);
    assert_eq!(update["session"]["search_options"]["enable_source"], true);
    drop(actor);
}

#[test]
fn enable_search_omitted_for_openai_dialect() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = RealtimeSession::start(RealtimeSessionConfig {
        endpoint: ProviderEndpoint {
            provider_id: "test".into(),
            // 非阿里云 base_url → OpenAI 方言。
            base_url: format!("http://127.0.0.1:{port}/v1/realtime"),
        },
        credential: None,
        model_id: "gpt-realtime".into(),
        voice: String::new(),
        instructions: "测试指令".into(),
        history: vec![],
        auto_respond: true,
        enable_search: true,
    })
    .unwrap();
    let (update, _ws) = accept_session(&listener);
    assert!(update["session"].get("enable_search").is_none());
    drop(actor);
}

/// Tier C 画像直接注入（不走环境变量开关，避免并行测试互扰）。
fn spawn_manual_actor(port: u16, auto_respond: bool) -> RealtimeSession {
    let mut profile = RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
        "http://127.0.0.1/api-ws/v1/realtime",
    ));
    profile.turn_detection = TurnDetection::Manual;
    RealtimeSession::start_with_profile(
        profile,
        RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: "test".into(),
                base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
            },
            credential: None,
            model_id: "qwen3.8-omni-flash-realtime".into(),
            voice: String::new(),
            instructions: "测试指令".into(),
            history: vec![],
            auto_respond,
            enable_search: false,
        },
    )
    .unwrap()
}

/// 等 Connected；Aliyun 画像回放只发 user item 且逐条等回执，桩侧
/// 回执由这里泵出（读到 item.create 即回 created），避免客户端空等超时。
fn wait_connected(actor: &RealtimeSession, ws: &mut MockSocket) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "Connected event not observed"
        );
        // 短超时读：桩可能在等客户端下一帧，这里不能长阻塞，
        // 否则 deadline 全被 read 耗光、Connected 轮询饿死。
        let _ = ws
            .get_mut()
            .set_read_timeout(Some(Duration::from_millis(100)));
        if let Ok(message) = ws.read()
            && let Message::Text(text) = message
            && text.contains("conversation.item.create")
        {
            ws.send(Message::Text(
                r#"{"type":"conversation.item.created"}"#.into(),
            ))
            .unwrap();
        }
        if matches!(
            actor.recv_event(Duration::from_millis(100)),
            Some(ActorEvent::Connected)
        ) {
            return;
        }
    }
}

fn assert_no_frame(ws: &mut MockSocket, window: Duration) {
    let deadline = std::time::Instant::now() + window;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        ws.get_mut().set_read_timeout(Some(remaining)).unwrap();
        match ws.read() {
            Ok(Message::Text(text)) => panic!("unexpected frame: {text}"),
            Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
            Ok(Message::Binary(_)) => continue,
            Ok(Message::Close(_)) => panic!("unexpected close"),
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("read error: {error}"),
        }
    }
}

#[test]
fn connects_with_profile_session_update_replays_history_and_streams_audio() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, true, vec![("你好".into(), "你好呀".into())]);
    let (update, mut ws) = accept_session(&listener);

    // 画像：Qwen-Omni Realtime 使用官方 server_vad；服务端判完句并触发响应。
    assert_eq!(update["session"]["turn_detection"]["type"], "server_vad");
    assert_eq!(update["session"]["turn_detection"]["threshold"], 0.5);
    assert_eq!(
        update["session"]["turn_detection"]["prefix_padding_ms"],
        300
    );
    assert_eq!(
        update["session"]["turn_detection"]["silence_duration_ms"],
        500
    );
    assert_eq!(update["session"]["turn_detection"]["create_response"], true);
    assert_eq!(update["session"]["input_audio_format"], "pcm");
    // 上下文回放：只回 user 轮——Qwen-Omni 的 item.create 拒收 assistant
    // item（服务端不回执直接断连），Aliyun 画像跳过 assistant。
    let item1 = read_frame(&mut ws);
    assert_eq!(item1["type"], "conversation.item.create");
    assert_eq!(item1["item"]["role"], "user");
    assert_eq!(item1["item"]["content"][0]["text"], "你好");
    assert_eq!(item1["item"]["content"][0]["type"], "input_text");
    wait_connected(&actor, &mut ws);

    // 采集环 48kHz：300ms = 28800B 48k → 降采样到方言 16k = 9600B → 3 个 3200B 帧。
    actor.send(ActorCommand::AppendAudio(vec![0x10; 28_800]));
    let mut decoded_total = 0usize;
    for _ in 0..3 {
        let frame = read_frame(&mut ws);
        assert_eq!(frame["type"], "input_audio_buffer.append");
        let decoded = STANDARD.decode(frame["audio"].as_str().unwrap()).unwrap();
        assert_eq!(decoded.len(), AUDIO_APPEND_CHUNK_BYTES);
        decoded_total += decoded.len();
    }
    assert_eq!(
        decoded_total, 9600,
        "48k input must downsample to dialect 16k rate"
    );
}

/// Qwen-Omni Realtime 的 server_vad 需要跟随点名门控：会议桥接不自动应答。
#[test]
fn qwen_omni_server_vad_create_response_follows_auto_respond() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, false, vec![]);
    let (update, _ws) = accept_session(&listener);
    assert_eq!(update["session"]["turn_detection"]["type"], "server_vad");
    assert_eq!(
        update["session"]["turn_detection"]["create_response"],
        false
    );
    drop(actor);
}

/// 服务端 VAD 负责提交和触发响应：本地 CommitTurn 必须是 no-op。
#[test]
fn qwen_omni_server_vad_ignores_local_commit_turn() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, true, vec![]);
    let (_, mut ws) = accept_session(&listener);
    wait_connected(&actor, &mut ws);

    actor.send(ActorCommand::AppendAudio(vec![0x30; 9600]));
    actor.send(ActorCommand::CommitTurn);
    actor.send(ActorCommand::RespondText("哨兵".into()));
    let append = read_frame(&mut ws);
    assert_eq!(append["type"], "input_audio_buffer.append");
    let sentinel = read_frame(&mut ws);
    assert_eq!(sentinel["type"], "conversation.item.create");
    assert_eq!(sentinel["item"]["content"][0]["text"], "哨兵");
    drop(actor);
}

/// 未核实 server_vad 能力的 DashScope 模型继续走 Manual 兜底。
#[test]
fn legacy_dashscope_model_defaults_to_manual_commit_and_create() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = RealtimeSession::start(RealtimeSessionConfig {
        endpoint: ProviderEndpoint {
            provider_id: "test".into(),
            base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
        },
        credential: None,
        model_id: "qwen-audio-3.0-realtime-plus".into(),
        voice: String::new(),
        instructions: "测试指令".into(),
        history: vec![],
        auto_respond: true,
        enable_search: false,
    })
    .unwrap();
    let (update, mut ws) = accept_session(&listener);
    assert_eq!(update["session"]["turn_detection"], Value::Null);
    wait_connected(&actor, &mut ws);
    actor.send(ActorCommand::CommitTurn);
    assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.commit");
    assert_eq!(read_frame(&mut ws)["type"], "response.create");
    drop(actor);
}

/// 视频帧：Aliyun 方言满足“音频先行”后原样透传 input_image_buffer.append；
/// OpenAI 方言无此能力，静默丢弃且不断链。
#[test]
fn append_image_forwarded_for_aliyun_and_dropped_for_openai() {
    // Aliyun 方言：帧透传。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, true, vec![]);
    let (_, mut ws) = accept_session(&listener);
    wait_connected(&actor, &mut ws);
    let jpeg = STANDARD.encode([0xAA, 0xBB, 0xCC]);
    actor.send(ActorCommand::AppendAudio(vec![0x30; 9600]));
    assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.append");
    actor.send(ActorCommand::AppendImage(jpeg.clone()));
    let frame = read_frame(&mut ws);
    assert_eq!(frame["type"], "input_image_buffer.append");
    assert_eq!(frame["image"], Value::String(jpeg));
    drop(actor);

    // OpenAI 方言：无视频能力，帧被静默丢弃（300ms 窗口内无任何帧到达）。
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = RealtimeSession::start(RealtimeSessionConfig {
        endpoint: ProviderEndpoint {
            provider_id: "test".into(),
            base_url: format!("http://127.0.0.1:{port}/v1/realtime"),
        },
        credential: None,
        model_id: "gpt-realtime".into(),
        voice: String::new(),
        instructions: "测试指令".into(),
        history: vec![],
        auto_respond: true,
        enable_search: false,
    })
    .unwrap();
    let (_, mut ws) = accept_session(&listener);
    wait_connected(&actor, &mut ws);
    actor.send(ActorCommand::AppendImage("aGk=".into()));
    ws.get_mut()
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    match ws.read() {
        // 同步 tungstenite 的读超时表现为 Io(WouldBlock/TimedOut)：无帧到达。
        Err(tungstenite::Error::Io(io_error))
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) => {}
        other => panic!("openai dialect must drop video frames, got {other:?}"),
    }
    drop(actor);
}

/// 桌面共享可在说话前开启：新连接后的图片先缓存；重连后门控重置，
/// 仍等下一块音频先行，并且只保留最新一帧。
#[test]
fn image_waits_for_audio_across_reconnect_and_sends_latest_frame() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap();
    let port = port.port();
    let actor = spawn_actor(port, true, vec![]);
    let (_, mut ws1) = accept_session(&listener);
    wait_connected(&actor, &mut ws1);

    actor.send(ActorCommand::AppendImage("first".into()));
    actor.send(ActorCommand::AppendImage("latest".into()));
    assert_no_frame(&mut ws1, Duration::from_millis(300));

    ws1.send(Message::Close(None)).unwrap();
    ws1.get_mut().shutdown(std::net::Shutdown::Both).ok();
    let (_, mut ws2) = accept_session(&listener);
    wait_connected(&actor, &mut ws2);

    actor.send(ActorCommand::AppendImage("after-reconnect".into()));
    assert_no_frame(&mut ws2, Duration::from_millis(300));
    actor.send(ActorCommand::AppendAudio(vec![0x30; 9600]));
    assert_eq!(read_frame(&mut ws2)["type"], "input_audio_buffer.append");
    let image = read_frame(&mut ws2);
    assert_eq!(image["type"], "input_image_buffer.append");
    assert_eq!(image["image"], "after-reconnect");
    drop(actor);
}

#[test]
fn server_events_are_forwarded_to_orchestrator() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, true, vec![]);
    let (_, mut ws) = accept_session(&listener);
    wait_connected(&actor, &mut ws);

    for event in [
        r#"{"type":"input_audio_buffer.speech_started"}"#.to_string(),
        r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"i1","text":"十一"}"#.to_string(),
        r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"十一点嘛"}"#.to_string(),
        format!(
            r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
            STANDARD.encode([7u8, 8, 9])
        ),
        r#"{"type":"response.audio_transcript.delta","delta":"十一点了"}"#.to_string(),
        r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
    ] {
        ws.send(Message::Text(event.into())).unwrap();
    }

    let mut seen: Vec<ActorEvent> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while seen.len() < 6 && std::time::Instant::now() < deadline {
        if let Some(event) = actor.recv_event(Duration::from_millis(200)) {
            seen.push(event);
        }
    }
    assert_eq!(seen.len(), 6, "expected all six events, got {seen:?}");
    assert!(matches!(seen.remove(0), ActorEvent::SpeechStarted));
    assert!(matches!(
        seen.remove(0),
        ActorEvent::UserTranscriptDelta { ref text, .. } if text == "十一"
    ));
    assert!(matches!(
        seen.remove(0),
        ActorEvent::UserTranscriptCompleted { ref text, .. } if text == "十一点嘛"
    ));
    assert!(matches!(
        seen.remove(0),
        ActorEvent::AssistantAudioDelta(ref pcm) if pcm == &[7u8, 8, 9]
    ));
    assert!(matches!(
        seen.remove(0),
        ActorEvent::AssistantTextDelta(ref text) if text == "十一点了"
    ));
    assert!(matches!(
        seen.remove(0),
        ActorEvent::ResponseDone { cancelled: false }
    ));
}

#[test]
fn cancel_sends_response_cancel_and_done_reports_cancelled() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, true, vec![]);
    let (_, mut ws) = accept_session(&listener);
    wait_connected(&actor, &mut ws);

    actor.send(ActorCommand::CancelResponse);
    assert_eq!(read_frame(&mut ws)["type"], "response.cancel");

    ws.send(Message::Text(
        r#"{"type":"response.done","response":{"status":"cancelled"}}"#.into(),
    ))
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        assert!(std::time::Instant::now() < deadline, "no ResponseDone");
        match actor.recv_event(Duration::from_millis(200)) {
            Some(ActorEvent::ResponseDone { cancelled }) => {
                assert!(cancelled);
                return;
            }
            Some(_) | None => continue,
        }
    }
}

#[test]
fn gated_input_drops_audio_until_reenabled() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, true, vec![]);
    let (_, mut ws) = accept_session(&listener);
    wait_connected(&actor, &mut ws);

    actor.send(ActorCommand::GateInput(true));
    actor.send(ActorCommand::AppendAudio(vec![0x20; 3200]));
    // 门控期间的音频必须被丢弃：下一个到达服务端的帧是文本轮次（哨兵）。
    actor.send(ActorCommand::GateInput(false));
    actor.send(ActorCommand::RespondText("接管一句".into()));
    let sentinel = read_frame(&mut ws);
    assert_eq!(sentinel["type"], "conversation.item.create");
    assert_eq!(sentinel["item"]["content"][0]["text"], "接管一句");
    assert_eq!(read_frame(&mut ws)["type"], "response.create");
}

#[test]
fn manual_profile_disables_server_vad_and_commit_creates_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_manual_actor(port, true);
    let (update, mut ws) = accept_session(&listener);
    assert!(update["session"]["turn_detection"].is_null());
    wait_connected(&actor, &mut ws);

    // Tier C：客户端 commit + response.create（auto_respond=true）。
    // 9600B@48k → 降采样 3200B@16k，恰一个 100ms append 帧。
    actor.send(ActorCommand::AppendAudio(vec![0x30; 9600]));
    actor.send(ActorCommand::CommitTurn);
    let append = read_frame(&mut ws);
    assert_eq!(append["type"], "input_audio_buffer.append");
    assert_eq!(
        STANDARD
            .decode(append["audio"].as_str().unwrap())
            .unwrap()
            .len(),
        3200
    );
    assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.commit");
    assert_eq!(read_frame(&mut ws)["type"], "response.create");
}

#[test]
fn manual_profile_without_auto_respond_commits_without_create() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_manual_actor(port, false);
    let (_, mut ws) = accept_session(&listener);
    wait_connected(&actor, &mut ws);

    actor.send(ActorCommand::CommitTurn);
    assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.commit");
    // 无 response.create：编排层按点名门控自行触发。
}

#[test]
fn reconnects_after_server_close_and_replays_context_again() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, true, vec![("上轮".into(), "上答".into())]);
    let (_, mut ws1) = accept_session(&listener);
    wait_connected(&actor, &mut ws1);
    ws1.send(Message::Close(None)).unwrap();
    ws1.get_mut().shutdown(std::net::Shutdown::Both).ok();

    // 第二次连接：上下文再次回放（服务端会话无历史）；Aliyun 画像只回 user 轮。
    let (update2, mut ws2) = accept_session(&listener);
    assert_eq!(update2["session"]["turn_detection"]["type"], "server_vad");
    let replayed = read_frame(&mut ws2);
    assert_eq!(replayed["type"], "conversation.item.create");
    assert_eq!(replayed["item"]["content"][0]["text"], "上轮");
    wait_connected(&actor, &mut ws2);
}

/// DashScope 流式首响 live 冒烟（Tier A 全链路）：常驻连接 + 持续上行真实语音 +
/// server_vad 自动提交 + 首个 response.audio.delta 时间戳。
/// 运行：REALTIME_SMOKE_CONFIG=<config> REALTIME_SMOKE_ROUTE=<route id>
/// REALTIME_SMOKE_WAV=<16k mono wav（必须是人声，静音不会触发服务端 VAD）>
/// cargo test --lib live_streaming_first_audio_latency -- --ignored --nocapture
#[test]
#[ignore = "Uses the saved Windows credential and consumes provider quota; set REALTIME_SMOKE_CONFIG and REALTIME_SMOKE_ROUTE explicitly"]
#[cfg(windows)]
fn live_streaming_first_audio_latency() {
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
    let voice = route["voiceId"].as_str().unwrap_or("").to_owned();

    // 读取 wav data 块作为 16k mono PCM（必须是人声样本）。
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
    let pcm = match std::env::var("REALTIME_SMOKE_AUDIO_MS")
        .ok()
        .and_then(|ms| ms.parse::<usize>().ok())
    {
        Some(ms) => {
            let capped = &pcm[..pcm.len().min(ms * 32)]; // 16k×2B/ms
            println!("live streaming: pcm capped to {ms}ms ({}B)", capped.len());
            capped.to_vec()
        }
        None => pcm,
    };
    // ActorCommand::AppendAudio 的契约是采集环 48kHz；live 样本文件是 16kHz。
    // 先补齐到采集率，让 Actor 内部再降回 DashScope 16kHz，复刻真实链路。
    let pcm = crate::audio::pcm::resample_pcm16_mono(
        &pcm,
        16_000,
        crate::audio::pcm::CAPTURE_SAMPLE_RATE,
    );
    println!("live streaming: pcm_bytes={}", pcm.len());

    let actor = RealtimeSession::start(RealtimeSessionConfig {
        endpoint: ProviderEndpoint {
            provider_id: provider["id"].as_str().unwrap().to_owned(),
            base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
        },
        credential: Some(credential.as_str().to_owned()),
        model_id: route["e2eModelId"].as_str().unwrap_or("").to_owned(),
        voice,
        instructions: "你是会议助手，请用中文简短回答。".into(),
        history: vec![],
        auto_respond: true,
        enable_search: false,
    })
    .unwrap();

    let t_start = std::time::Instant::now();
    let mut connected_at: Option<Duration> = None;
    let mut first_delta_at: Option<Duration> = None;
    let mut speech_stopped_at: Option<Duration> = None;
    let mut audio_sent = 0usize;
    let mut user_text = String::new();
    let mut assistant_text = String::new();
    let mut audio_bytes = 0usize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    // 模拟持续上行：真实说话约每 100ms 一帧；整句送完后停止上行。
    let mut last_push = std::time::Instant::now();
    while std::time::Instant::now() < deadline {
        // 每帧 100ms 语音按墙钟节奏上行（复刻真实麦克风节奏）。
        // 语音按墙钟节奏上行；语音送完后继续持续送静音，让服务端 VAD 判定
        // 句尾并生成响应（真实麦克风持续在线，静音帧一直存在）。
        if last_push.elapsed() >= std::time::Duration::from_millis(100) {
            let chunk = if audio_sent < pcm.len() {
                let end = (audio_sent + 3200).min(pcm.len());
                pcm[audio_sent..end].to_vec()
            } else {
                vec![0u8; 3200]
            };
            audio_sent += chunk.len();
            actor.send(ActorCommand::AppendAudio(chunk));
            last_push = std::time::Instant::now();
        }
        match actor.recv_event(std::time::Duration::from_millis(50)) {
            Some(ActorEvent::Connected) => {
                connected_at = Some(t_start.elapsed());
                println!("live streaming: connected at {:?}", connected_at.unwrap());
            }
            Some(ActorEvent::SpeechStopped) => {
                speech_stopped_at = Some(t_start.elapsed()); // 最近一次说完
            }
            Some(ActorEvent::UserTranscriptDelta { text, .. }) => user_text = text,
            Some(ActorEvent::UserTranscriptCompleted { text, .. }) => user_text = text,
            Some(ActorEvent::AssistantAudioDelta(pcm)) => {
                if first_delta_at.is_none() {
                    first_delta_at = Some(t_start.elapsed());
                    println!(
                        "live streaming: FIRST AUDIO DELTA at {:?} (speech_stopped at {speech_stopped_at:?})",
                        first_delta_at.unwrap()
                    );
                }
                audio_bytes += pcm.len();
            }
            Some(ActorEvent::AssistantTextDelta(delta)) => assistant_text.push_str(&delta),
            Some(ActorEvent::ResponseDone { cancelled }) => {
                println!(
                    "live streaming: done at {:?} cancelled={cancelled} user=\\\"{user_text}\\\" assistant=\\\"{assistant_text}\\\" audio_bytes={audio_bytes}",
                    t_start.elapsed()
                );
                // 多段语音样本会先触发一次 barge-in cancelled；继续等最终
                // completed，避免把“被打断的一轮”误报成验收成功。
                if cancelled {
                    continue;
                }
                if let (Some(connected), Some(first)) = (connected_at, first_delta_at) {
                    println!(
                        "live streaming: connect={connected:?} first_delta_since_start={first:?}"
                    );
                    if let Some(stopped) = speech_stopped_at {
                        println!(
                            "live streaming: first_audio_after_speech_stopped={:?}",
                            first.saturating_sub(stopped)
                        );
                    }
                }
                return;
            }
            Some(ActorEvent::Reconnecting(reason)) => {
                println!("live streaming: reconnecting ({reason})");
            }
            Some(ActorEvent::Failed(reason)) => panic!("session failed: {reason}"),
            _ => {}
        }
    }
    panic!(
        "live streaming smoke timed out; user=\\\"{user_text}\\\" assistant=\\\"{assistant_text}\\\""
    );
}

/// 凭证识别冒烟：打印三个已存凭证的前缀特征（脱敏），判断对应供应商。
/// 运行：cargo test --lib live_identify_provider_credentials -- --ignored --nocapture
#[test]
#[ignore = "Reads saved Windows credentials; prints only masked prefixes"]
#[cfg(windows)]
fn live_identify_provider_credentials() {
    use crate::secrets::{SecretService, WindowsSecretStore};
    use std::sync::Arc;
    let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
    for id in [
        "3ebf81df-78de-46d3-adb3-2f050db54d6f",
        "c3925e07-1a33-4869-8f66-c93c3ec864f9",
        "38a37317-725f-4db1-97c3-111aa1e5d168",
    ] {
        let reference = format!("providers/{id}/api-key");
        let masked = match secrets.read(&reference) {
            Ok(Some(secret)) => {
                let value = secret.as_str();
                let prefix: String = value.chars().take(8).collect();
                format!(
                    "len={} prefix={prefix}… has_dot={}",
                    value.chars().count(),
                    value.contains('.')
                )
            }
            Ok(None) => "missing".to_owned(),
            Err(_) => "read-error".to_owned(),
        };
        println!("credential {id}: {masked}");
    }
}

/// GLM 模型清单探测：用已存 key 拉取可用模型列表（定位 realtime 模型名）。
/// 运行：cargo test --lib live_glm_list_models -- --ignored --nocapture
#[test]
#[ignore = "Reads saved Windows credentials and calls the provider HTTP API"]
#[cfg(windows)]
fn live_glm_list_models() {
    use crate::secrets::{SecretService, WindowsSecretStore};
    use std::sync::Arc;
    let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
    for id in [
        "c3925e07-1a33-4869-8f66-c93c3ec864f9",
        "38a37317-725f-4db1-97c3-111aa1e5d168",
    ] {
        let Ok(Some(secret)) = secrets.read(&format!("providers/{id}/api-key")) else {
            println!("{id}: key missing");
            continue;
        };
        let client = reqwest::blocking::Client::new();
        let response = client
            .get("https://open.bigmodel.cn/api/paas/v4/models")
            .bearer_auth(secret.as_str())
            .timeout(std::time::Duration::from_secs(15))
            .send();
        match response {
            Ok(response) => {
                let status = response.status();
                let body = response.text().unwrap_or_default();
                let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let models: Vec<String> = parsed["data"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item["id"].as_str().map(str::to_owned))
                            .filter(|id| {
                                id.contains("realtime")
                                    || id.contains("voice")
                                    || id.contains("audio")
                                    || id.contains("omni")
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                println!("{id}: HTTP {status} realtime_like={}", models.join(", "));
            }
            Err(error) => println!("{id}: request failed: {error}"),
        }
    }
}

/// 重连退避：未建会话指数递增且永不突破 RECONNECT_CAP；建过会话重置基准。
#[test]
fn reconnect_backoff_doubles_but_never_exceeds_cap() {
    let mut backoff = RECONNECT_BASE;
    let mut steps = Vec::new();
    for _ in 0..16 {
        steps.push(backoff);
        backoff = next_reconnect_backoff(backoff, false);
    }
    assert_eq!(steps[0], Duration::from_millis(500));
    assert_eq!(steps[5], Duration::from_secs(16));
    assert!(
        steps.iter().all(|step| *step <= RECONNECT_CAP),
        "退避不得突破上限: {steps:?}"
    );
    // 触顶后保持上限（32s→30s 钳制），不会无限翻倍。
    assert_eq!(*steps.last().unwrap(), RECONNECT_CAP);
}

#[test]
fn reconnect_backoff_resets_after_established_session() {
    let capped = next_reconnect_backoff(RECONNECT_CAP, false);
    assert_eq!(next_reconnect_backoff(capped, true), RECONNECT_BASE);
}

/// session.update 被服务端 error 事件拒绝：实现没有逐字段回退——
/// 错误原因转发给编排层（Reconnecting 事件），整条连接拆掉重连并按
/// 原画像重放 session.update（连接仍可用）。
#[test]
fn session_update_error_rejects_connection_and_forwards_reason_to_orchestrator() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let actor = spawn_actor(port, true, vec![]);

    // 首连：读走 session.update 后直接回 error 拒绝（不给 created/updated）。
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut ws = tungstenite::accept(stream).unwrap();
    let update: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(update["type"], "session.update");
    ws.send(Message::Text(
        r#"{"type":"error","error":{"code":"SESSION_FIELD_REJECTED","message":"turn_detection not supported"}}"#
            .into(),
    ))
    .unwrap();

    // 错误原因必须转发给编排层，并携带服务端返回的 code。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        assert!(
            std::time::Instant::now() < deadline,
            "Reconnecting reason not forwarded"
        );
        match actor.recv_event(Duration::from_millis(100)) {
            Some(ActorEvent::Reconnecting(reason)) => {
                assert!(reason.contains("SESSION_FIELD_REJECTED"), "reason={reason}");
                break;
            }
            Some(_) | None => continue,
        }
    }

    // 拒绝后连接仍可用：重连成功，画像原样重放（无逐字段回退）。
    let (update2, mut ws2) = accept_session(&listener);
    assert_eq!(update2["session"]["turn_detection"]["type"], "server_vad");
    wait_connected(&actor, &mut ws2);
}
