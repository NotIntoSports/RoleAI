//! 会话建立后的上行流程：session.update、历史回放、音频/图片上行、
//! 轮次提交与服务端事件处理（纯搬移自 realtime_session.rs）。

use super::*;

/// session.update（画像驱动）→ 等 session.updated → 上下文回放 → Connected。
pub(super) fn session_setup(
    state: &SharedState,
    config: &RealtimeSessionConfig,
    socket: &mut TungsteniteSocket,
    reconnect: bool,
) -> Result<(), RealtimeError> {
    // 每次重连都是新连接：转写装配状态不复用。
    *state.assembler.lock().unwrap_or_else(|p| p.into_inner()) = InputTranscriptAssembler::new();
    let voice = selected_voice(&config.voice, &state.profile.dialect);
    let mut session = json!({
        "modalities": ["text", "audio"],
        "instructions": config.instructions,
        "input_audio_format": state.profile.dialect.audio_format,
        "output_audio_format": state.profile.dialect.output_audio_format,
    });
    session["turn_detection"] = match state.profile.turn_detection {
        TurnDetection::ServerVad => json!({
            "type": "server_vad",
            "threshold": 0.5,
            "prefix_padding_ms": 300,
            "silence_duration_ms": 500,
            // 会议助手点名门控：服务端只提交转写，应答由编排层决定。
            "create_response": config.auto_respond,
        }),
        TurnDetection::SemanticVad => json!({
            "type": "semantic_vad",
            "create_response": config.auto_respond,
        }),
        // 此前 Aliyun Manual 竞态的根源是"服务端自动提交 × 客户端 commit 并存"；
        // Manual 画像下提交权完全归客户端，不再竞态。
        TurnDetection::Manual => Value::Null,
    };
    if !voice.is_empty() {
        session["voice"] = json!(voice);
    }
    // DashScope 联网：session.update 顶层开关（Qwen3.8/Qwen3.5-Omni-Realtime
    // 支持，模型自主判断何时搜索）。OpenAI 方言无此字段，发送会被服务端拒绝。
    if config.enable_search && state.profile.dialect.name == RealtimeDialectName::Aliyun {
        session["enable_search"] = json!(true);
        session["search_options"] = json!({ "enable_source": true });
    }
    send_text(
        socket,
        &json!({"type": "session.update", "session": session}).to_string(),
    )?;
    let deadline = Instant::now() + SESSION_UPDATED_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RealtimeError::SessionUpdateTimeout);
        }
        let raw = socket.recv_text(remaining)?;
        match parse_server_event(&raw)? {
            Some(ServerEvent::SessionUpdated) => break,
            Some(ServerEvent::SessionCreated) | None => {}
            Some(_) => {
                eprintln!(
                    "[realtime] unexpected during session.update: {}",
                    &raw.chars().take(220).collect::<String>()
                );
                return Err(RealtimeError::SessionUpdateUnexpected);
            }
        }
    }
    replay_history(state, config, socket)?;
    // 重连重置了服务端会话：积压的提交/应答命令属于旧连接的上下文，重放会
    // 对同一句话二次 commit+response.create（重复播报）。只保留音频追加——
    // 那是断连期间用户正在说的话，落进缓冲随下一次 commit 提交。
    // 首次连接没有旧上下文：握手期间排队的命令照常处理。
    while reconnect && let Some(command) = state.try_command() {
        if matches!(
            command,
            ActorCommand::CommitTurn
                | ActorCommand::RespondText(_)
                | ActorCommand::CancelResponse
                | ActorCommand::Truncate { .. }
                | ActorCommand::ClearInputBuffer
        ) {
            eprintln!("[realtime] dropped stale command after reconnect");
        }
    }
    let _ = state.events.send(ActorEvent::Connected);
    Ok(())
}

/// 上下文回放：旧→新逐轮 conversation.item.create；画像不支持 item_replay 时
/// 上下文由 instructions 承载（调用方拼摘要），此处跳过。
pub(super) fn replay_history(
    state: &SharedState,
    config: &RealtimeSessionConfig,
    socket: &mut TungsteniteSocket,
) -> Result<(), RealtimeError> {
    if !state.profile.item_replay {
        return Ok(());
    }
    for (user, assistant) in config.history_window() {
        if !user.is_empty() {
            eprintln!(
                "[realtime] replay user item: {}…",
                &user.chars().take(16).collect::<String>()
            );
            send_item_create(
                socket,
                &json!({
                    "type": "conversation.item.create",
                    "item": {
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": user }],
                    },
                })
                .to_string(),
                "user",
            )?;
        }
        if !assistant.is_empty() {
            if !state.profile.replay_assistant {
                // Qwen-Omni：assistant item 服务端不回执直接断连，跳过（上下文
                // 仍有 user 问题序列 + instructions）。
                eprintln!(
                    "[realtime] skip assistant item (profile rejects): {}…",
                    &assistant.chars().take(16).collect::<String>()
                );
                continue;
            }
            eprintln!(
                "[realtime] replay assistant item: {}…",
                &assistant.chars().take(16).collect::<String>()
            );
            send_item_create(
                socket,
                &json!({
                    "type": "conversation.item.create",
                    "item": {
                        "type": "message",
                        "role": "assistant",
                        // assistant 消息的 content type 是 "text"（"output_text"
                        // 只出现在响应事件 payload）。qwen 对 "output_text"
                        // 直接 400「content field is required」，重连即死循环。
                        "content": [{ "type": "text", "text": assistant }],
                    },
                })
                .to_string(),
                "assistant",
            )?;
        }
    }
    Ok(())
}

/// 回放单条 item 并等服务端回执：conversation.item.created 视为通过；
/// error 事件带上是哪类 item 被拒；无回执画像（不回 created）超时后放行。
fn send_item_create(
    socket: &mut TungsteniteSocket,
    payload: &str,
    kind: &str,
) -> Result<(), RealtimeError> {
    send_text(socket, payload)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            eprintln!("[realtime] replay {kind} item: no ack within 5s, continue");
            return Ok(());
        }
        let raw = socket.recv_text(remaining)?;
        match parse_server_event(&raw)? {
            Some(ServerEvent::ItemCreated) => return Ok(()),
            Some(ServerEvent::SessionCreated) | Some(ServerEvent::SessionUpdated) | None => {}
            Some(_) => {
                eprintln!(
                    "[realtime] replay {kind} item rejected <- {}",
                    &raw.chars().take(200).collect::<String>()
                );
                return Err(RealtimeError::Remote("REALTIME_ITEM_REJECTED".to_owned()));
            }
        }
    }
}

/// 按方言 100ms 帧切片 append（DashScope 单帧限制安全值，其余方言等时长换算）。
/// 采集环是 48kHz，方言输入率不同（DashScope 16k / OpenAI 24k）时必须先降采样：
/// 服务端按 session.update 声明的格式解码，原样发送会得到变调音频、转写失效。
pub(super) fn append_audio(
    profile: &RealtimeCapabilityProfile,
    socket: &mut TungsteniteSocket,
    pcm: &[u8],
) -> Result<(), RealtimeError> {
    let rate = dialect_input_rate(&profile.dialect);
    let pcm =
        crate::audio::pcm::resample_pcm16_mono(pcm, crate::audio::pcm::CAPTURE_SAMPLE_RATE, rate);
    let chunk_bytes = (rate as usize * 2 / 10).max(AUDIO_APPEND_CHUNK_BYTES / 100);
    for chunk in pcm.chunks(chunk_bytes.max(1)) {
        send_text(
            socket,
            &json!({
                "type": "input_audio_buffer.append",
                "audio": STANDARD.encode(chunk),
            })
            .to_string(),
        )?;
    }
    Ok(())
}

/// 上行一帧 base64 JPEG。调用方必须保证当前连接已发送过音频 append。
pub(super) fn append_image(
    socket: &mut TungsteniteSocket,
    jpeg_b64: &str,
) -> Result<(), RealtimeError> {
    send_text(
        socket,
        &json!({
            "type": "input_image_buffer.append",
            "image": jpeg_b64,
        })
        .to_string(),
    )
}

/// Manual（Tier C）画像：commit 后由客户端触发应答。服务端 VAD 画像下客户端
/// 永不 commit（避免撞空缓冲竞态），此命令为 no-op。
pub(super) fn commit_turn(
    state: &SharedState,
    socket: &mut TungsteniteSocket,
    auto_respond: bool,
) -> Result<(), RealtimeError> {
    if state.profile.turn_detection != TurnDetection::Manual {
        return Ok(());
    }
    send_text(socket, r#"{"type":"input_audio_buffer.commit"}"#)?;
    if auto_respond {
        send_text(socket, &response_create_event(true))?;
    }
    Ok(())
}

pub(super) fn respond_text(
    state: &SharedState,
    socket: &mut TungsteniteSocket,
    text: &str,
) -> Result<(), RealtimeError> {
    if state.profile.item_replay {
        send_text(
            socket,
            &json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": text }],
                },
            })
            .to_string(),
        )?;
    }
    send_text(socket, &response_create_event(true))?;
    Ok(())
}

/// 打断：画像支持 response.cancel 直接取消；否则连接复位丢弃在途响应
/// （监督线程按"建过会话"快速重连）。
pub(super) fn cancel_response(
    profile: &RealtimeCapabilityProfile,
    socket: &mut TungsteniteSocket,
) -> Result<(), RealtimeError> {
    if profile.response_cancel {
        send_text(socket, r#"{"type":"response.cancel"}"#)
    } else {
        Err(RealtimeError::ConnectionClosed)
    }
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

/// 服务端事件 → 编排层事件；返回 Err 表示连接必须重建。
pub(super) fn handle_server_event(
    state: &SharedState,
    raw: &str,
    response_opened_at: &mut Option<Instant>,
) -> Result<(), RealtimeError> {
    match parse_server_event(raw)? {
        Some(ServerEvent::Audio(pcm)) => {
            let _ = state.events.send(ActorEvent::AssistantAudioDelta(pcm));
        }
        Some(ServerEvent::OutputTranscript(delta) | ServerEvent::OutputText(delta)) => {
            let _ = state.events.send(ActorEvent::AssistantTextDelta(delta));
        }
        Some(ServerEvent::InputTranscriptDelta(payload)) => {
            let item_id = payload["item_id"].as_str().unwrap_or("").to_owned();
            if let Some((_, text, _)) = assembler_update(state, "input_transcript_delta", &payload)
            {
                let _ = state
                    .events
                    .send(ActorEvent::UserTranscriptDelta { item_id, text });
            }
        }
        Some(ServerEvent::InputTranscriptCompleted(payload)) => {
            let item_id = payload["item_id"].as_str().unwrap_or("").to_owned();
            if let Some((_, text, _)) =
                assembler_update(state, "input_transcript_completed", &payload)
            {
                let _ = state
                    .events
                    .send(ActorEvent::UserTranscriptCompleted { item_id, text });
            }
        }
        Some(ServerEvent::SpeechStarted) => {
            if state.profile.speech_events {
                let _ = state.events.send(ActorEvent::SpeechStarted);
            }
        }
        Some(ServerEvent::SpeechStopped) => {
            if state.profile.speech_events {
                let _ = state.events.send(ActorEvent::SpeechStopped);
            }
        }
        Some(ServerEvent::ResponseDone(status)) => {
            *response_opened_at = None;
            let _ = state.events.send(ActorEvent::ResponseDone {
                cancelled: status.as_deref() == Some("cancelled"),
            });
        }
        Some(ServerEvent::ResponseCreated) => {
            *response_opened_at = Some(Instant::now());
            let _ = state.events.send(ActorEvent::ResponseStarted);
        }
        Some(
            ServerEvent::SessionCreated
            | ServerEvent::SessionUpdated
            | ServerEvent::InputCommitted
            | ServerEvent::ItemCreated,
        )
        | None => {}
    }
    Ok(())
}

fn assembler_update(
    state: &SharedState,
    kind: &str,
    payload: &Value,
) -> Option<(String, String, bool)> {
    let mut assembler = state.assembler.lock().unwrap_or_else(|p| p.into_inner());
    assembler.update(kind, payload)
}
