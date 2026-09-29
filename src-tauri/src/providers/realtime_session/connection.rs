//! 监督线程与单条连接的建立/退出分类（纯搬移自 realtime_session.rs）。

use super::*;

pub(super) struct SharedState {
    pub(super) profile: RealtimeCapabilityProfile,
    pub(super) config: Mutex<RealtimeSessionConfig>,
    pub(super) commands: Mutex<mpsc::Receiver<ActorCommand>>,
    pub(super) events: mpsc::Sender<ActorEvent>,
    pub(super) shutdown: Arc<AtomicBool>,
    pub(super) gated: AtomicBool,
    pub(super) assembler: Mutex<InputTranscriptAssembler>,
    /// 最新待发图片；跨重连保留，但发送前必须等当前连接音频先行。
    pub(super) pending_image: Mutex<Option<String>>,
}

impl SharedState {
    fn config_snapshot(&self) -> RealtimeSessionConfig {
        self.config
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(super) fn try_command(&self) -> Option<ActorCommand> {
        self.commands
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_recv()
            .ok()
    }
}

enum ConnectionOutcome {
    Shutdown,
    /// 参数：是否成功建立过会话（用于退避复位）+ 原因。
    Reconnect {
        had_session: bool,
        reason: String,
    },
    Terminal(String),
}

pub(super) fn supervise(state: Arc<SharedState>) {
    let mut backoff = RECONNECT_BASE;
    let mut connections: u64 = 0;
    loop {
        if state.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let config = state.config_snapshot();
        connections += 1;
        eprintln!("[realtime] connection #{connections} opening");
        match run_connection(&state, &config) {
            ConnectionOutcome::Shutdown => {
                eprintln!("[realtime] shutdown after {connections} connection(s)");
                return;
            }
            ConnectionOutcome::Terminal(reason) => {
                eprintln!("[realtime] TERMINAL: {reason}");
                let _ = state.events.send(ActorEvent::Failed(reason));
                return;
            }
            ConnectionOutcome::Reconnect {
                had_session,
                reason,
            } => {
                eprintln!(
                    "[realtime] reconnecting (had_session={had_session}, backoff={}ms): {reason}",
                    backoff.min(RECONNECT_CAP).as_millis()
                );
                let _ = state.events.send(ActorEvent::Reconnecting(reason));
                backoff = next_reconnect_backoff(backoff, had_session);
            }
        }
        // 指数退避等待；Shutdown 立即返回。连续未建会话的失败退避递增，
        // 建过会话（网络闪断）则快速重试。
        let wake = Instant::now() + backoff.min(RECONNECT_CAP);
        while Instant::now() < wake {
            if state.shutdown.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// 连接主体：握手 → session.update → 上下文回放 → 事件/命令轮询循环。
fn run_connection(state: &SharedState, config: &RealtimeSessionConfig) -> ConnectionOutcome {
    let url = match realtime_url(&config.endpoint.base_url, &config.model_id) {
        Ok(url) => url,
        Err(error) => return ConnectionOutcome::Terminal(error.code().to_owned()),
    };
    let mut socket = match connect_socket(&url, config, state) {
        Ok(socket) => socket,
        Err(error) => return classify_connect_error(error),
    };

    // 读用短超时轮询，命令在无服务端帧的间隙写出；单线程持有 socket。
    let mut idle_at = Instant::now() + state.profile.idle_reconnect_after;
    let mut response_opened_at: Option<Instant> = None;
    // Qwen Realtime 图片输入要求当前连接至少 append 过一次音频。桌面共享常在
    // 说话前开启，重连后若图片先到会触发远端错误并形成重连循环；这里只保留
    // 最新帧，等首块音频成功上行后按“音频 → 图片”顺序发送。
    let mut audio_appended = false;
    loop {
        if state.shutdown.load(Ordering::SeqCst) {
            return ConnectionOutcome::Shutdown;
        }
        match socket.recv_text(RECV_POLL) {
            Ok(raw) => {
                idle_at = Instant::now() + state.profile.idle_reconnect_after;
                if let Err(error) = handle_server_event(state, &raw, &mut response_opened_at) {
                    eprintln!(
                        "[realtime] server rejected: {} <- {}",
                        error.code(),
                        &raw.chars().take(220).collect::<String>()
                    );
                    return ConnectionOutcome::Reconnect {
                        had_session: true,
                        reason: error.code().to_owned(),
                    };
                }
            }
            Err(RealtimeError::Timeout) => {}
            Err(error) => {
                return ConnectionOutcome::Reconnect {
                    had_session: true,
                    reason: error.code().to_owned(),
                };
            }
        }
        while let Some(command) = state.try_command() {
            match command {
                ActorCommand::Shutdown => return ConnectionOutcome::Shutdown,
                ActorCommand::AppendAudio(pcm) => {
                    if state.gated.load(Ordering::SeqCst) {
                        continue;
                    }
                    if let Err(error) = append_audio(&state.profile, &mut socket, &pcm) {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                    if !pcm.is_empty() {
                        audio_appended = true;
                    }
                    if audio_appended {
                        let image = state
                            .pending_image
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .clone();
                        if let Some(image) = image {
                            if let Err(error) = append_image(&mut socket, &image) {
                                return ConnectionOutcome::Reconnect {
                                    had_session: true,
                                    reason: error.code().to_owned(),
                                };
                            }
                            *state
                                .pending_image
                                .lock()
                                .unwrap_or_else(|p| p.into_inner()) = None;
                        }
                    }
                }
                ActorCommand::AppendImage(jpeg_b64) => {
                    if !state.profile.video_input {
                        continue;
                    }
                    if audio_appended {
                        if let Err(error) = append_image(&mut socket, &jpeg_b64) {
                            return ConnectionOutcome::Reconnect {
                                had_session: true,
                                reason: error.code().to_owned(),
                            };
                        }
                        *state
                            .pending_image
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = None;
                    } else {
                        *state
                            .pending_image
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = Some(jpeg_b64);
                    }
                }
                ActorCommand::CommitTurn => {
                    if let Err(error) = commit_turn(state, &mut socket, config.auto_respond) {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::RespondText(text) => {
                    if let Err(error) = respond_text(state, &mut socket, &text) {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::CancelResponse => {
                    if let Err(error) = cancel_response(&state.profile, &mut socket) {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::Truncate { audio_end_ms } => {
                    if state.profile.truncate
                        && let Err(error) = send_text(
                            &mut socket,
                            &json!({
                                "type": "input_audio_buffer.truncate",
                                "audio_end_ms": audio_end_ms,
                            })
                            .to_string(),
                        )
                    {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::ClearInputBuffer => {
                    if state.profile.clear_input
                        && let Err(error) =
                            send_text(&mut socket, r#"{"type":"input_audio_buffer.clear"}"#)
                    {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::GateInput(enabled) => {
                    state.gated.store(enabled, Ordering::SeqCst);
                }
                ActorCommand::SetHistory(history) => {
                    state
                        .config
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .history = history;
                }
            }
        }
        if Instant::now() > idle_at {
            return ConnectionOutcome::Reconnect {
                had_session: true,
                reason: "REALTIME_IDLE_RECONNECT".to_owned(),
            };
        }
        if let Some(opened_at) = response_opened_at
            && opened_at.elapsed() > RESPONSE_TIMEOUT
        {
            return ConnectionOutcome::Reconnect {
                had_session: true,
                reason: "REALTIME_RESPONSE_TIMEOUT".to_owned(),
            };
        }
    }
}

fn classify_connect_error(error: RealtimeError) -> ConnectionOutcome {
    match error {
        // 鉴权失败重连无意义，直接终局。
        RealtimeError::Unauthorized => ConnectionOutcome::Terminal("REALTIME_UNAUTHORIZED".into()),
        other => ConnectionOutcome::Reconnect {
            had_session: false,
            reason: other.code().to_owned(),
        },
    }
}

fn connect_socket(
    url: &reqwest::Url,
    config: &RealtimeSessionConfig,
    state: &SharedState,
) -> Result<TungsteniteSocket, RealtimeError> {
    let stream = connect_tcp(url, &state.shutdown)?;
    let _ = stream.set_nodelay(true);
    // 握手与 session.update 等待都要有上限，否则服务端卡住时重连会无限挂起。
    set_tcp_timeouts(&stream, CONNECT_OPEN_TIMEOUT)?;
    let mut builder = tungstenite::ClientRequestBuilder::new(
        url.as_str()
            .parse()
            .map_err(|_| RealtimeError::UrlInvalid)?,
    )
    .with_header("OpenAI-Beta", "realtime=v1");
    if let Some(credential) = config
        .credential
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        builder = builder.with_header("Authorization", format!("Bearer {credential}"));
    }
    let request = builder
        .into_client_request()
        .map_err(|_| RealtimeError::UrlInvalid)?;
    let ws_config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_TEXT_FRAME_BYTES))
        .max_frame_size(Some(MAX_TEXT_FRAME_BYTES));
    let (socket, _) = complete_handshake(tungstenite::client_tls_with_config(
        request,
        stream,
        Some(ws_config),
        None,
    ))?;
    let mut socket = TungsteniteSocket { socket };
    session_setup(state, config, &mut socket)?;
    Ok(socket)
}
