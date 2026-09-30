//! 会话生命周期命令：启动/停止/模式切换、麦克风与视频推流、实时泵装配（纯搬移自 sessions.rs）。

use super::*;

#[cfg(test)]
pub(in crate::commands) fn session_start_cmd(
    state: &AppState,
    allow_barge_in: Option<bool>,
) -> CommandResult<SessionStartResult> {
    session_start_selected_cmd(state, None, None, false, allow_barge_in)
}

#[cfg(test)]
pub(in crate::commands) fn session_start_selected_cmd(
    state: &AppState,
    role_profile_id: Option<&str>,
    voice_route_id: Option<&str>,
    allow_web_search: bool,
    allow_barge_in: Option<bool>,
) -> CommandResult<SessionStartResult> {
    session_start_capture_cmd(
        state,
        role_profile_id,
        voice_route_id,
        allow_web_search,
        None,
        allow_barge_in,
    )
}

pub(in crate::commands) fn session_start_capture_cmd(
    state: &AppState,
    role_profile_id: Option<&str>,
    voice_route_id: Option<&str>,
    allow_web_search: bool,
    capture: Option<crate::services::MeetingCapture<'_>>,
    allow_barge_in: Option<bool>,
) -> CommandResult<SessionStartResult> {
    let mut config = match load_public_config(state) {
        Ok(config) => config,
        Err(error) => return CommandResult::Err { error },
    };
    if let Some(id) = role_profile_id {
        if !config
            .role_profiles
            .iter()
            .any(|role| role.id == id && role.config_version > 0)
        {
            return service_error("SESSION_ROLE_REQUIRED", "请选择有效的会话角色");
        }
        config.active_role_profile_id = Some(id.into());
        for role in &mut config.role_profiles {
            role.active = role.id == id;
        }
    }
    if let Some(id) = voice_route_id {
        if !config
            .speech
            .voice_routes
            .iter()
            .any(|route| route.id == id && route.config_version > 0)
        {
            return service_error("SESSION_ROUTE_REQUIRED", "请选择有效的语音线路");
        }
        config.speech.active_voice_route_id = Some(id.into());
        for route in &mut config.speech.voice_routes {
            route.active = route.id == id;
        }
    }
    let secrets_ready = secrets_backend_ready(state);
    if !allow_web_search {
        for provider in &mut config.models.providers {
            provider.web_capability = None;
        }
    }
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return CommandResult::Ok {
            data: SessionStartResult::Blocked {
                issues: preflight(&config, secrets_ready, false),
            },
        };
    };
    let mut sessions = match state.sessions.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error(
                "SERVICE_BUSY",
                "Service configuration is temporarily unavailable",
            );
        }
    };
    let outcome = if let Some(capture) = capture {
        sessions.start_with_meeting_capture(
            database,
            &config,
            secrets_ready,
            capture,
            allow_barge_in.unwrap_or(false),
        )
    } else {
        sessions.start(
            database,
            &config,
            secrets_ready,
            allow_barge_in.unwrap_or(false),
        )
    };
    match outcome {
        Ok(SessionStartOutcome::Started { session }) => CommandResult::Ok {
            data: SessionStartResult::Started {
                session: session.into(),
            },
        },
        Ok(SessionStartOutcome::Blocked { issues }) => CommandResult::Ok {
            data: SessionStartResult::Blocked { issues },
        },
        Err(error) => session_service_error(error),
    }
}

pub(in crate::commands) fn keep_session_routing(
    state: &AppState,
    change: crate::prerequisites::AudioRoutingChange,
) {
    let _ = crate::prerequisites::persist_audio_routing(&state.paths.data_directory, &change);
    if let Ok(mut slot) = state.audio_routing.lock() {
        *slot = Some(change);
    }
}

pub(in crate::commands) fn rollback_session_routing(
    state: &AppState,
    change: crate::prerequisites::AudioRoutingChange,
) -> Option<&'static str> {
    match crate::prerequisites::restore_communications_mic(&change) {
        Ok(()) => {
            crate::prerequisites::clear_persisted_audio_routing(&state.paths.data_directory);
            None
        }
        Err(code) => {
            keep_session_routing(state, change);
            Some(code)
        }
    }
}

pub(in crate::commands) fn session_stop_cmd(state: &AppState) -> CommandResult<SessionSummary> {
    state.session_control.request_stop();
    stop_operator_monitor(state);
    if let Ok(mut slot) = state.mic_ingest.lock() {
        *slot = None;
    }
    let routing = state
        .audio_routing
        .lock()
        .ok()
        .and_then(|mut slot| slot.take());
    let restore_failed = routing
        .as_ref()
        .is_some_and(|change| crate::prerequisites::restore_communications_mic(change).is_err());
    if restore_failed {
        if let Some(change) = routing
            && let Ok(mut slot) = state.audio_routing.lock()
        {
            *slot = Some(change.clone());
            let _ =
                crate::prerequisites::persist_audio_routing(&state.paths.data_directory, &change);
        }
    } else {
        crate::prerequisites::clear_persisted_audio_routing(&state.paths.data_directory);
    }
    let database = match state.database.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => return session_stop_pending(state),
        Err(TryLockError::Poisoned(_)) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let mut sessions = match state.sessions.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => return session_stop_pending(state),
        Err(TryLockError::Poisoned(_)) => {
            return service_error(
                "SERVICE_BUSY",
                "Service configuration is temporarily unavailable",
            );
        }
    };
    let stopped = match sessions.stop(database) {
        Ok(session) => CommandResult::Ok {
            data: session.into(),
        },
        Err(error) => session_service_error(error),
    };
    if restore_failed {
        return service_error(
            "AUDIO_ROUTING_RESTORE_FAILED",
            "会话已停止，但原通信麦克风恢复失败；请在系统声音设置中检查",
        );
    }
    stopped
}

pub(in crate::commands) fn session_stop_pending(state: &AppState) -> CommandResult<SessionSummary> {
    let Some(id) = state.session_control.session_id() else {
        return session_service_error(SessionServiceError::NotFound);
    };
    CommandResult::Ok {
        data: SessionSummary {
            id,
            status: "stopping".into(),
            role_profile_id: String::new(),
            voice_route_id: String::new(),
            transport_mode: "direct".into(),
            started_at: None,
            finished_at: None,
            updated_at: String::new(),
        },
    }
}

pub(in crate::commands) fn session_set_mode_cmd(
    state: &AppState,
    mode: String,
) -> CommandResult<RuntimeStatus> {
    let Some(mode) = AgentMode::from_name(&mode) else {
        return CommandResult::Err {
            error: PublicError::new("SESSION_MODE_INVALID", "Unsupported session mode", false)
                .with_field("mode"),
        };
    };
    state.session_control.set_mode(mode);
    match state.database.try_lock() {
        Ok(database) => {
            let Some(database) = database.as_ref() else {
                return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
            };
            match state.sessions.try_lock() {
                Ok(mut sessions) => {
                    if let Err(error) = sessions.set_mode(database, mode) {
                        return session_service_error(error);
                    }
                }
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Poisoned(_)) => {
                    return service_error(
                        "SERVICE_BUSY",
                        "Service configuration is temporarily unavailable",
                    );
                }
            }
        }
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Poisoned(_)) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    }
    CommandResult::Ok {
        data: bump_runtime_status(state),
    }
}

#[tauri::command]
pub fn session_audio_ready(state: State<'_, AppState>) -> CommandResult<FoundationStatus> {
    session_audio_ready_cmd(&state)
}

pub(in crate::commands) fn session_audio_ready_cmd(
    state: &AppState,
) -> CommandResult<FoundationStatus> {
    match state.sessions.try_lock() {
        Ok(sessions) => CommandResult::Ok {
            data: FoundationStatus {
                ready: sessions.utterance_ready(),
            },
        },
        // This synchronous IPC command runs on the UI thread. An active model
        // request holds sessions; polling must never wait for that request.
        Err(TryLockError::WouldBlock) => CommandResult::Ok {
            data: FoundationStatus { ready: false },
        },
        Err(TryLockError::Poisoned(_)) => service_error("SERVICE_BUSY", "会话音频状态暂时不可用"),
    }
}

#[tauri::command]
pub fn session_push_mic_pcm(
    state: State<'_, AppState>,
    pcm: String,
    sample_rate: u32,
) -> CommandResult<MicPcmAcceptance> {
    session_push_mic_pcm_cmd(&state, &pcm, sample_rate)
}

pub(in crate::commands) fn session_push_mic_pcm_cmd(
    state: &AppState,
    pcm: &str,
    sample_rate: u32,
) -> CommandResult<MicPcmAcceptance> {
    use base64::Engine as _;
    let decoded = match base64::engine::general_purpose::STANDARD.decode(pcm) {
        Ok(decoded) => decoded,
        Err(_) => {
            return service_error(
                "SESSION_MIC_PCM_INVALID",
                "麦克风音频数据无效，请重新开始对练。",
            );
        }
    };
    let pcm = if sample_rate != crate::audio::pcm::CAPTURE_SAMPLE_RATE {
        crate::audio::pcm::resample_pcm16_mono(
            &decoded,
            sample_rate,
            crate::audio::pcm::CAPTURE_SAMPLE_RATE,
        )
    } else {
        decoded
    };
    match state.sessions.try_lock() {
        Ok(mut sessions) => {
            if sessions.session_id().is_none() {
                return session_service_error(SessionServiceError::NotFound);
            }
            sessions.push_pcm(&pcm);
            CommandResult::Ok {
                data: MicPcmAcceptance { accepted: true },
            }
        }
        // Same UI-thread Rule as session_audio_ready: a model request holding
        // the lock must never stall mic streaming; drop the chunk instead.
        Err(TryLockError::WouldBlock) => CommandResult::Ok {
            data: MicPcmAcceptance { accepted: false },
        },
        Err(TryLockError::Poisoned(_)) => service_error("SERVICE_BUSY", "会话音频状态暂时不可用"),
    }
}

/// DashScope Qwen-Omni 系视频帧上限：base64 ≤ 256KB（官方文档）。
/// 再留少量余量，超出直接拒绝，避免服务端断链后整场会话重连。
pub(in crate::commands) const VIDEO_FRAME_BASE64_LIMIT: usize = 300_000;

#[tauri::command]
pub fn session_push_video_frame(
    state: State<'_, AppState>,
    frame: String,
) -> CommandResult<VideoFrameAcceptance> {
    session_push_video_frame_cmd(&state, &frame)
}

pub(in crate::commands) fn session_push_video_frame_cmd(
    state: &AppState,
    frame: &str,
) -> CommandResult<VideoFrameAcceptance> {
    let frame = frame.trim();
    if frame.is_empty() || frame.len() > VIDEO_FRAME_BASE64_LIMIT {
        return service_error(
            "SESSION_VIDEO_FRAME_INVALID",
            "视频帧数据无效或过大，请降低分辨率后重试。",
        );
    }
    match state.sessions.try_lock() {
        Ok(sessions) => CommandResult::Ok {
            data: VideoFrameAcceptance {
                accepted: sessions.push_video_frame(frame),
            },
        },
        // 与麦克风推流同规则：锁被模型请求占用时丢帧，绝不阻塞 UI 线程。
        Err(TryLockError::WouldBlock) => CommandResult::Ok {
            data: VideoFrameAcceptance { accepted: false },
        },
        Err(TryLockError::Poisoned(_)) => service_error("SERVICE_BUSY", "会话视频状态暂时不可用"),
    }
}

/// 端到端流式路线的实时泵装配（session_start 成功后调用）。
fn attach_realtime_pump(
    state: &AppState,
    sessions: &mut crate::services::SessionService,
    config: &PublicConfig,
    bridge: &std::path::Path,
    endpoint_id: Option<&str>,
    allow_web_search: bool,
    live_sink: Option<Box<dyn Fn(crate::services::PumpLive) + Send + Sync>>,
) -> Result<(), String> {
    use crate::config::VoiceRouteMode;
    use crate::services::{RealtimePumpDeps, active_session_role_scenario, e2e_instructions};
    let active_role_profile = crate::runtime::active_role_profile;
    let Some(route) = active_voice_route(config) else {
        return Ok(()); // 无语音线路：纯文字会话，无需泵
    };
    if route.mode != VoiceRouteMode::E2e {
        return Ok(()); // 级联路线维持既有管线
    }
    let provider_id = route
        .e2e_provider_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or("ROUTE_E2E_PROVIDER_MISSING")?;
    let model_id = route
        .e2e_model_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or("ROUTE_E2E_MODEL_MISSING")?;
    let provider = config
        .models
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or("MODEL_PROVIDER_MISSING")?;
    let credential = read_provider_secret(state, config, Some(&provider_id))
        .map_err(|error| error.code.to_string())?
        .map(|secret| secret.to_string());
    let scenario = active_session_role_scenario(config);
    // 会议助手点名门控只适用于会议桥接输入（会上别人说话不应答，点名才开口）。
    // 本机麦克风是用户与助手直接对话，每句都应答——否则用户说什么都被静默。
    let meeting_bridge = sessions.capture().is_meeting_bridge();
    let auto_respond =
        !(scenario == Some(crate::config::RoleScenario::MeetingAssistant) && meeting_bridge);
    let hold_playback = scenario == Some(crate::config::RoleScenario::Candidate);
    let playback_mode = if meeting_bridge {
        crate::services::RealtimePlaybackMode::Native
    } else {
        crate::services::RealtimePlaybackMode::WebAudio
    };
    let role_name = active_role_profile(config)
        .map(|profile| profile.name.clone())
        .unwrap_or_else(|| "会议助手".to_owned());
    let deps = RealtimePumpDeps {
        endpoint: crate::providers::ProviderEndpoint {
            provider_id,
            base_url: provider.base_url.clone(),
        },
        credential,
        model_id,
        voice: route.voice_id.clone().unwrap_or_default(),
        instructions: e2e_instructions(active_role_profile(config), &[]),
        history: Vec::new(),
        auto_respond,
        enable_search: allow_web_search,
        role_name,
        hold_playback,
        playback_mode,
        playback_exe: Some(bridge.to_path_buf()),
        playback_endpoint_id: endpoint_id.map(str::to_owned),
    };
    sessions
        .attach_realtime_pump(deps, live_sink)
        .map_err(|error| error.code().to_string())
}

#[allow(clippy::too_many_arguments)] // Tauri exposes the backward-compatible IPC fields individually.
pub fn session_start_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    role_profile_id: Option<String>,
    voice_route_id: Option<String>,
    allow_web_search: Option<bool>,
    meeting_pid: Option<u32>,
    output_device_id: Option<String>,
    allow_barge_in: Option<bool>,
) -> CommandResult<SessionStartResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    if meeting_pid.is_some() && !cfg!(windows) {
        // 会议进程采集依赖 Windows 专属 AudioBridge sidecar；非 Windows 返回
        // 稳定错误码，前端据此隐藏会议采集入口。纯语音会话不受影响。
        return service_error("PLATFORM_UNSUPPORTED", "当前平台不支持会议进程采集");
    }
    let bridge = if cfg!(debug_assertions) {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../native/AudioBridge/publish/AudioBridge.exe")
    } else {
        match app.path().resource_dir() {
            Ok(root) => root.join("audio-bridge/AudioBridge.exe"),
            Err(_) => {
                return service_error("SESSION_SIDECAR_MISSING", "无法定位音频组件，请修复安装");
            }
        }
    };
    let enumerator = crate::processes::PowerShellProcessEnumerator;
    let output_device_id = if meeting_pid.is_some() {
        let devices = match crate::prerequisites::enumerate_audio_devices(&bridge) {
            Ok(devices) => devices,
            Err(code) => return service_error(code, "无法检测虚拟声卡"),
        };
        let status = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
        if !status.installed {
            return service_error("VIRTUAL_AUDIO_REQUIRED", "请先安装并自动配置虚拟声卡");
        }
        status.render_endpoint_id
    } else {
        output_device_id
    };
    let routing = if meeting_pid.is_some() {
        match crate::prerequisites::configure_communications_mic(&bridge) {
            Ok(change) => Some(change),
            Err(code) => return service_error(code, "无法自动配置会议麦克风"),
        }
    } else {
        None
    };
    let capture = meeting_pid.map(|pid| crate::services::MeetingCapture {
        exe: &bridge,
        pid,
        enumerator: &enumerator,
    });
    let result = session_start_capture_cmd(
        &state,
        role_profile_id.as_deref(),
        voice_route_id.as_deref(),
        allow_web_search.unwrap_or(false),
        capture,
        allow_barge_in,
    );
    if matches!(
        &result,
        CommandResult::Ok {
            data: SessionStartResult::Started { .. }
        }
    ) {
        // 端到端流式路线：装配实时泵（常驻连接 + 流式播放 + 无锁上行）。
        // 配置必须先于 sessions 锁读取——load_session_config 内部会锁 sessions，
        // 在锁内调用会自死锁。
        let realtime_attach_config =
            if std::env::var("AI_VOICE_REALTIME_MODE").as_deref() != Ok("legacy") {
                load_session_config(&state).ok()
            } else {
                None
            };
        // 流式字幕：泵的文本快照经闭包转发为既有 partial 事件（done=false），
        // 轮次落库后的 done=true 收尾仍由 finalize 包装层统一发出。
        let live_app = app.clone();
        let live_seq: std::sync::Arc<std::sync::atomic::AtomicU64> =
            std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1 << 32));
        let live_seq_sink = std::sync::Arc::clone(&live_seq);
        let live_sink: Box<dyn Fn(crate::services::PumpLive) + Send + Sync> =
            Box::new(move |event| {
                let seq = live_seq_sink.fetch_add(1, Ordering::SeqCst);
                match event {
                    crate::services::PumpLive::User(text) => {
                        let _ = live_app.emit(
                            EVENT_SESSION_TRANSCRIPT,
                            SessionTranscriptEvent {
                                seq,
                                text: clip_event_text(&text),
                                done: false,
                            },
                        );
                    }
                    crate::services::PumpLive::Assistant(text) => {
                        let _ = live_app.emit(
                            EVENT_SESSION_REPLY,
                            SessionReplyEvent {
                                seq,
                                text: clip_event_text(&text),
                                done: false,
                            },
                        );
                    }
                    crate::services::PumpLive::AssistantAudio(pcm) => {
                        let _ = live_app.emit(
                            EVENT_SESSION_AUDIO,
                            SessionAudioEvent {
                                seq,
                                pcm_base64: STANDARD.encode(pcm),
                                sample_rate: 24_000,
                            },
                        );
                    }
                    crate::services::PumpLive::PlaybackControl(control) => {
                        let action = match control {
                            crate::services::PlaybackControl::Clear => "clear",
                        };
                        let _ = live_app.emit(
                            EVENT_SESSION_PLAYBACK_CONTROL,
                            SessionPlaybackControlEvent {
                                seq,
                                action: action.to_owned(),
                            },
                        );
                    }
                    // 播报状态由 runtime_status 读取泵共享状态呈现，无需事件。
                    crate::services::PumpLive::Speaking(_) => {}
                }
            });
        if let Ok(mut sessions) = state.sessions.lock() {
            sessions.configure_playback(
                output_device_id
                    .clone()
                    .filter(|id| !id.trim().is_empty())
                    .map(|endpoint_id| crate::audio::playback::BridgePlayback {
                        executable: bridge.clone(),
                        endpoint_id,
                    }),
            );
            // 装配失败降级旧每轮路径，不阻塞会话启动。
            if let Some(config) = realtime_attach_config
                && let Err(error) = attach_realtime_pump(
                    &state,
                    &mut sessions,
                    &config,
                    &bridge,
                    output_device_id.as_deref(),
                    allow_web_search.unwrap_or(false),
                    Some(live_sink),
                )
            {
                eprintln!("realtime pump attach degraded to legacy: {error}");
            }
            if let Ok(mut slot) = state.mic_ingest.lock() {
                *slot = Some(sessions.mic_ingest_handle());
            }
        }
        if let Some(change) = routing {
            keep_session_routing(&state, change);
        }
        emit_runtime(&app, &state);
        return result;
    }
    if let Some(change) = routing
        && let Some(restore_code) = rollback_session_routing(&state, change)
    {
        return match result {
            CommandResult::Err { error } => CommandResult::Err {
                error: PublicError::new(
                    error.code,
                    format!(
                        "{}；同时未能恢复会议麦克风，已保留恢复信息，请检查系统声音设置或稍后停止会话以重试。",
                        error.message
                    ),
                    error.retryable,
                ),
            },
            other => {
                let _ = other;
                service_error(
                    restore_code,
                    "会话未能开始，且未能恢复会议麦克风。已保留恢复信息，请检查系统声音设置。",
                )
            }
        };
    }
    result
}

#[tauri::command]
pub fn session_stop<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CommandResult<SessionSummary> {
    state.session_control.request_stop();
    let _guard = match service_guard_try(&state) {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            let result = session_stop_pending(&state);
            if matches!(result, CommandResult::Ok { .. }) {
                emit_runtime(&app, &state);
            }
            return result;
        }
        Err(error) => return error,
    };
    let result = session_stop_cmd(&state);
    if matches!(result, CommandResult::Ok { .. }) {
        emit_runtime(&app, &state);
    }
    result
}

#[tauri::command]
pub fn session_set_mode<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    mode: String,
) -> CommandResult<RuntimeStatus> {
    let operator_speaking = mode == "operator_speaking";
    if !operator_speaking {
        stop_operator_monitor(&state);
    }
    let mut result = session_set_mode_cmd(&state, mode);
    if operator_speaking
        && matches!(result, CommandResult::Ok { .. })
        && let Err(code) = start_operator_monitor(&app, &state)
    {
        let _ = session_set_mode_cmd(&state, "ai_active".into());
        result = service_error(code, "物理麦克风未能进入会议线路");
    }
    if matches!(result, CommandResult::Ok { .. }) {
        let status = match &result {
            CommandResult::Ok { data } => data.clone(),
            CommandResult::Err { .. } => runtime_status_from_state(&state),
        };
        let _ = app.emit(EVENT_RUNTIME_STATUS, &status);
    }
    result
}

fn stop_operator_monitor(state: &AppState) {
    if let Ok(mut slot) = state.operator_monitor.lock()
        && let Some(mut child) = slot.take()
    {
        crate::audio::monitor::stop(&mut child);
    }
}

fn start_operator_monitor<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
) -> Result<(), &'static str> {
    stop_operator_monitor(state);
    let routing = state
        .audio_routing
        .lock()
        .ok()
        .and_then(|slot| slot.clone())
        .ok_or("OPERATOR_MONITOR_ROUTE_MISSING")?;
    if routing.previous_id.is_empty() || routing.previous_id == routing.cable_id {
        return Err("OPERATOR_PHYSICAL_MIC_MISSING");
    }
    let bridge = audio_bridge_path(app)?;
    let devices = crate::prerequisites::enumerate_audio_devices(&bridge)?;
    let preparation = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
    let output_id = preparation
        .render_endpoint_id
        .ok_or("VIRTUAL_AUDIO_RENDER_MISSING")?;
    let child = crate::audio::monitor::spawn(&bridge, &routing.previous_id, &output_id)?;
    let mut slot = state.operator_monitor.lock().map_err(|_| "SERVICE_BUSY")?;
    *slot = Some(child);
    Ok(())
}

blocking_command!(with_events session_start, session_start_blocking(role_profile_id: Option<String>, voice_route_id: Option<String>, allow_web_search: Option<bool>, meeting_pid: Option<u32>, output_device_id: Option<String>, allow_barge_in: Option<bool>) -> SessionStartResult);
