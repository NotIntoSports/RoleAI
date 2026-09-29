//! sessions 域命令：会话生命周期、单轮编排、麦克风/视频推流、运行时状态。
//! 纯搬移自 commands.rs，不含行为变更。
use super::*;

pub(super) const EVENT_RUNTIME_STATUS: &str = "runtime:status:v1";
pub(super) const EVENT_AUDIO_LEVEL: &str = "audio:level:v1";
pub(super) const EVENT_SESSION_TRANSCRIPT: &str = "session:transcript:v1";
pub(super) const EVENT_SESSION_REPLY: &str = "session:reply:v1";
pub(super) const EVENT_SESSION_AUDIO: &str = "session:audio:v1";
pub(super) const EVENT_SESSION_PLAYBACK_CONTROL: &str = "session:playback-control:v1";

fn session_service_error<T: ts_rs::TS>(error: SessionServiceError) -> CommandResult<T> {
    if let SessionServiceError::Realtime(ref realtime) = error {
        return CommandResult::Err {
            error: PublicError::new(
                realtime.code(),
                realtime.public_message(),
                realtime.retryable(),
            ),
        };
    }
    let code = error.code();
    let message = match code {
        "SESSION_ALREADY_ACTIVE" => "A session is already active",
        "SESSION_NOT_FOUND" => "Session not found",
        "SESSION_STATE_INVALID" => "Session state is invalid",
        "SESSION_TRANSPORT_INVALID" => "Unsupported session transport",
        "NOTHING_TO_ANSWER" => "No speech is awaiting an answer",
        _ => "Session operation failed",
    };
    let mut public = PublicError::new(code, message, false);
    if matches!(code, "SESSION_NOT_FOUND") {
        public = public.with_field("sessionId");
    }
    if matches!(code, "SESSION_TRANSPORT_INVALID") {
        public = public.with_field("transportMode");
    }
    CommandResult::Err { error: public }
}

/// 引用摘要的展示截断（与流式事件文本上限无关）。
pub(super) fn clip_snippet(text: &str) -> String {
    text.chars().take(160).collect()
}

/// 会话转写/回复事件的文本上限。流式上屏需要完整前缀（远超旧 160 字符摘要），
/// 最终全文仍以落库后的会话详情为准。
pub(super) const EVENT_TEXT_LIMIT: usize = 4000;

pub(super) fn clip_event_text(text: &str) -> String {
    text.chars().take(EVENT_TEXT_LIMIT).collect()
}

pub(super) fn load_public_config(state: &AppState) -> Result<PublicConfig, PublicError> {
    match state.config.load() {
        Ok(config) => Ok(public_view(&config)),
        Err(error) => Err(PublicError::new(
            error.code(),
            "Configuration is unavailable",
            false,
        )),
    }
}

pub(super) fn load_session_config(state: &AppState) -> Result<PublicConfig, PublicError> {
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| PublicError::new("SERVICE_BUSY", "会话暂时忙碌", true))?;
    if let Some(config) = sessions.config_snapshot() {
        return Ok(config.clone());
    }
    drop(sessions);
    load_public_config(state)
}

pub(super) fn secrets_backend_ready(state: &AppState) -> bool {
    state.secrets.status("system/startup-probe").is_ok()
}

pub(super) fn runtime_status_from_state(state: &AppState) -> RuntimeStatus {
    let seq = state.event_seq.load(Ordering::SeqCst);
    match state.sessions.try_lock() {
        Ok(sessions) => RuntimeStatus {
            // 端到端流式路线：泵播报期间 runtime 停在 Listening，用泵状态覆盖显示。
            phase: if sessions.realtime_speaking() {
                crate::runtime::SessionPhase::Speaking.as_str().to_owned()
            } else {
                sessions.phase().as_str().to_owned()
            },
            mode: sessions.mode().as_str().to_owned(),
            seq,
            unused_materials: sessions.unused_materials(),
            last_error_code: sessions.last_error_code().map(str::to_owned),
            revision: sessions.revision(),
            realtime_status: sessions.realtime_link_status().to_owned(),
        },
        Err(TryLockError::WouldBlock) => RuntimeStatus {
            phase: "thinking".into(),
            mode: state.session_control.mode().as_str().to_owned(),
            seq,
            unused_materials: false,
            last_error_code: None,
            revision: 0,
            realtime_status: "unknown".into(),
        },
        Err(TryLockError::Poisoned(_)) => RuntimeStatus {
            phase: "idle".into(),
            mode: "ai_active".into(),
            seq,
            unused_materials: false,
            last_error_code: Some("SERVICE_BUSY".into()),
            revision: 0,
            realtime_status: "unknown".into(),
        },
    }
}

pub(super) fn bump_event_seq(state: &AppState) -> u64 {
    state.event_seq.fetch_add(1, Ordering::SeqCst);
    state.event_seq.load(Ordering::SeqCst)
}

pub(super) fn bump_runtime_status(state: &AppState) -> RuntimeStatus {
    bump_event_seq(state);
    runtime_status_from_state(state)
}

fn emit_runtime<R: tauri::Runtime>(app: &AppHandle<R>, state: &AppState) {
    let status = bump_runtime_status(state);
    let _ = app.emit(EVENT_RUNTIME_STATUS, &status);
    if let Ok(sessions) = state.sessions.try_lock() {
        let _ = app.emit(
            EVENT_AUDIO_LEVEL,
            AudioLevelEvent {
                peak: sessions.capture().last_peak(),
                seq: status.seq,
            },
        );
    }
}

#[cfg(test)]
pub(super) fn runtime_status_event(
    seq: u64,
    phase: &str,
    mode: &str,
    unused_materials: bool,
    last_error_code: Option<&str>,
    revision: u64,
) -> serde_json::Value {
    serde_json::to_value(RuntimeStatus {
        phase: phase.to_owned(),
        mode: mode.to_owned(),
        seq,
        unused_materials,
        last_error_code: last_error_code.map(str::to_owned),
        revision,
        realtime_status: "idle".to_owned(),
    })
    .expect("runtime status")
}

#[cfg(test)]
pub(super) fn transcript_event(seq: u64, text: &str) -> serde_json::Value {
    serde_json::to_value(SessionTranscriptEvent {
        seq,
        text: clip_event_text(text),
        done: true,
    })
    .expect("transcript event")
}

#[cfg(test)]
pub(super) fn reply_event(seq: u64, text: &str) -> serde_json::Value {
    serde_json::to_value(SessionReplyEvent {
        seq,
        text: clip_event_text(text),
        done: true,
    })
    .expect("reply event")
}

#[cfg(test)]
pub(super) fn audio_level_event(seq: u64, peak: f64) -> serde_json::Value {
    serde_json::to_value(AudioLevelEvent { peak, seq }).expect("audio level")
}

fn emit_transcript_and_reply<R: tauri::Runtime>(
    app: &AppHandle<R>,
    state: &AppState,
    user_text: &str,
    assistant_text: &str,
) {
    let _ = app.emit(
        EVENT_SESSION_TRANSCRIPT,
        SessionTranscriptEvent {
            seq: bump_event_seq(state),
            text: clip_event_text(user_text),
            done: true,
        },
    );
    let _ = app.emit(
        EVENT_SESSION_REPLY,
        SessionReplyEvent {
            seq: bump_event_seq(state),
            text: clip_event_text(assistant_text),
            done: true,
        },
    );
}
fn read_embedding_secret(
    state: &AppState,
    config: &PublicConfig,
    embedding: Option<&EmbeddingConfig>,
) -> Result<Option<zeroize::Zeroizing<String>>, PublicError> {
    let Some(embedding) = embedding else {
        return Ok(None);
    };
    let Some(slot) = crate::services::embedding_credential_slot(&config.models, embedding) else {
        return Ok(None);
    };
    state.secrets.read(&slot.reference).map_err(|_| {
        PublicError::new(
            "SECRET_BACKEND_UNAVAILABLE",
            "Secret backend is unavailable",
            false,
        )
    })
}

pub(super) fn session_detail(
    store: &SessionStore<'_>,
    id: &str,
) -> Result<SessionDetail, SessionServiceError> {
    let session = store.get(id)?.ok_or(SessionServiceError::NotFound)?;
    let mut turns = Vec::new();
    let events = store.list_events(id)?;
    for turn in store.list_turns(id)? {
        let web = events
            .iter()
            .rev()
            .filter(|event| event.kind == "web_sources")
            .filter_map(|event| serde_json::from_str::<serde_json::Value>(&event.payload).ok())
            .find(|event| event["turnId"] == turn.id);
        let meta = events
            .iter()
            .rev()
            .filter(|event| event.kind == "turn_meta")
            .filter_map(|event| serde_json::from_str::<serde_json::Value>(&event.payload).ok())
            .find(|event| event["turnId"] == turn.id);
        let citations = store
            .list_citations(&turn.id)?
            .into_iter()
            .map(|citation| SessionCitationView {
                material_id: citation.material_id,
                chunk_id: citation.chunk_id,
                snippet: clip_snippet(&citation.snippet),
            })
            .collect();
        turns.push(SessionTurnView {
            trigger_source: meta
                .as_ref()
                .and_then(|value| value["triggerSource"].as_str())
                .map(str::to_owned),
            user_confirmed: meta
                .as_ref()
                .and_then(|value| value["userConfirmed"].as_bool()),
            playback_status: meta
                .as_ref()
                .and_then(|value| value["playbackStatus"].as_str())
                .map(str::to_owned),
            web_sources: web
                .as_ref()
                .and_then(|event| serde_json::from_value(event["sources"].clone()).ok()),
            web_degraded: web.as_ref().and_then(|event| event["degraded"].as_bool()),
            id: turn.id,
            turn_index: turn.turn_index,
            user_text: turn.user_text,
            assistant_text: turn.assistant_text,
            materials_used: turn.materials_used,
            citations,
            created_at: turn.created_at,
        });
    }
    Ok(SessionDetail {
        session: session.into(),
        turns,
    })
}

#[cfg(test)]
pub(super) fn session_start_cmd(
    state: &AppState,
    allow_barge_in: Option<bool>,
) -> CommandResult<SessionStartResult> {
    session_start_selected_cmd(state, None, None, false, allow_barge_in)
}

#[cfg(test)]
pub(super) fn session_start_selected_cmd(
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

pub(super) fn session_start_capture_cmd(
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

pub(super) fn keep_session_routing(
    state: &AppState,
    change: crate::prerequisites::AudioRoutingChange,
) {
    let _ = crate::prerequisites::persist_audio_routing(&state.paths.data_directory, &change);
    if let Ok(mut slot) = state.audio_routing.lock() {
        *slot = Some(change);
    }
}

pub(super) fn rollback_session_routing(
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

pub(super) fn session_stop_cmd(state: &AppState) -> CommandResult<SessionSummary> {
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

pub(super) fn session_stop_pending(state: &AppState) -> CommandResult<SessionSummary> {
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

pub(super) fn session_set_mode_cmd(state: &AppState, mode: String) -> CommandResult<RuntimeStatus> {
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

pub(super) fn session_list_cmd(state: &AppState) -> CommandResult<Vec<SessionSummary>> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    match SessionStore::new(database).list() {
        Ok(rows) => CommandResult::Ok {
            data: rows.into_iter().map(SessionSummary::from).collect(),
        },
        Err(error) => service_error(error.code(), "Session list failed"),
    }
}

pub(super) fn session_get_cmd(
    state: &AppState,
    session_id: String,
) -> CommandResult<SessionDetail> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    match session_detail(&SessionStore::new(database), &session_id) {
        Ok(detail) => CommandResult::Ok { data: detail },
        Err(error) => session_service_error(error),
    }
}

pub(super) fn session_delete_cmd(
    state: &AppState,
    session_id: String,
) -> CommandResult<FoundationStatus> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
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
    if sessions.session_id() == Some(session_id.as_str()) {
        sessions.reset();
    }
    match SessionStore::new(database).delete_session(&session_id) {
        Ok(()) => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        Err(error) => service_error(error.code(), "Session delete failed"),
    }
}

pub(super) fn session_export_cmd(
    state: &AppState,
    session_id: String,
    format: String,
) -> CommandResult<SessionExportResult> {
    let Some(format) = SessionExportFormat::from_name(&format) else {
        return CommandResult::Err {
            error: PublicError::new(
                SessionExportError::FormatInvalid.code(),
                "Unsupported export format",
                false,
            )
            .with_field("format"),
        };
    };
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let export_root = state.paths.data_directory.join("exports");
    match export_session(
        &SessionStore::new(database),
        &session_id,
        format,
        &export_root,
    ) {
        Ok(path) => CommandResult::Ok {
            data: SessionExportResult {
                path: path.to_string_lossy().into_owned(),
            },
        },
        Err(error) => {
            let mut public = PublicError::new(error.code(), "Session export failed", false);
            if error == SessionExportError::NotFound {
                public = public.with_field("sessionId");
            }
            CommandResult::Err { error: public }
        }
    }
}

#[cfg(test)]
pub(super) fn session_agent_command_cmd(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    input: AgentCommandInput,
) -> CommandResult<AgentCommandResult> {
    session_agent_command_cmd_inner(state, probes, credentials, input, &TurnStreamHooks::none())
}

pub(super) fn session_agent_command_cmd_inner(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    input: AgentCommandInput,
    hooks: &TurnStreamHooks<'_>,
) -> CommandResult<AgentCommandResult> {
    let payload = serde_json::json!({
        "v": 1,
        "id": input.id,
        "action": input.action,
        "text": input.text.unwrap_or_default(),
        "answer": input.answer.unwrap_or_default(),
        "expectedRevision": input.expected_revision,
        "mode": input.mode.unwrap_or_default(),
    });
    let command = match parse_agent_command(&payload) {
        Ok(command) => command,
        Err(error) => {
            return CommandResult::Ok {
                data: AgentCommandResult {
                    command_id: input.id,
                    action: input.action,
                    ok: false,
                    result: serde_json::json!({}),
                    error: error.code().to_owned(),
                },
            };
        }
    };
    let config = match load_session_config(state) {
        Ok(config) => config,
        Err(error) => return CommandResult::Err { error },
    };
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
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
    match sessions.execute_command_with_hooks(
        database,
        &config,
        probes,
        credentials,
        command,
        hooks,
    ) {
        Ok(outcome) => CommandResult::Ok {
            data: AgentCommandResult {
                command_id: outcome.command_id,
                action: outcome.action.as_str().to_owned(),
                ok: outcome.ok,
                result: serde_json::Value::Object(outcome.result),
                error: outcome.error,
            },
        },
        Err(error) => session_service_error(error),
    }
}

#[cfg(test)]
pub(super) fn session_finalize_utterance_cmd(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    text: Option<&str>,
) -> CommandResult<SessionTurnView> {
    session_finalize_utterance_cmd_inner(
        state,
        probes,
        credentials,
        text,
        false,
        &TurnStreamHooks::none(),
    )
}

/// 取当前会话最后一轮的视图（finalize 与强制回答共用返回结构）。
fn last_session_turn_view(
    sessions: &crate::services::SessionService,
    database: &crate::database::Database,
) -> CommandResult<SessionTurnView> {
    let Some(session_id) = sessions.session_id() else {
        return session_service_error(SessionServiceError::NotFound);
    };
    match session_detail(&SessionStore::new(database), session_id) {
        Ok(detail) => match detail.turns.into_iter().last() {
            Some(turn) => CommandResult::Ok { data: turn },
            None => session_service_error(SessionServiceError::StateInvalid),
        },
        Err(error) => session_service_error(error),
    }
}

pub(super) fn session_finalize_utterance_cmd_inner(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    text: Option<&str>,
    force_meeting_assistant: bool,
    hooks: &TurnStreamHooks<'_>,
) -> CommandResult<SessionTurnView> {
    let config = match load_session_config(state) {
        Ok(config) => config,
        Err(error) => return CommandResult::Err { error },
    };
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
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
    let finalized = if force_meeting_assistant {
        // 端到端路线 + 会议桥接 + 泵在跑：级联 forced 分支取不到轮次（应答
        // 由泵的点名门控决定），改为让泵对最近一条仅转写发言发起回答。回答
        // 异步生成、由前端就绪轮询落库，这里立即返回最后一轮视图，不持
        // sessions 锁等待——阻塞等待会占住锁并拖死就绪轮询。
        let via_pump = active_voice_route(&config)
            .is_some_and(|route| route.mode == crate::config::VoiceRouteMode::E2e)
            && sessions.realtime_pump_running()
            && sessions.capture().is_meeting_bridge();
        if via_pump {
            if !sessions.realtime_has_transcript_only() {
                return session_service_error(SessionServiceError::NothingToAnswer);
            }
            sessions.trigger_realtime_assistant();
            return last_session_turn_view(&sessions, database);
        }
        sessions.finalize_utterance_forced_with_hooks(database, &config, probes, credentials, hooks)
    } else {
        sessions.finalize_utterance_with_hooks(database, &config, probes, credentials, text, hooks)
    };
    match finalized {
        Ok(Some(_)) => last_session_turn_view(&sessions, database),
        Ok(None) => session_service_error(SessionServiceError::StateInvalid),
        Err(error) => session_service_error(error),
    }
}

pub(super) fn runtime_get_status_cmd(state: &AppState) -> CommandResult<RuntimeStatus> {
    CommandResult::Ok {
        data: runtime_status_from_state(state),
    }
}

#[tauri::command]
pub fn session_audio_ready(state: State<'_, AppState>) -> CommandResult<FoundationStatus> {
    session_audio_ready_cmd(&state)
}

pub(super) fn session_audio_ready_cmd(state: &AppState) -> CommandResult<FoundationStatus> {
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

pub(super) fn session_push_mic_pcm_cmd(
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
pub(super) const VIDEO_FRAME_BASE64_LIMIT: usize = 300_000;

#[tauri::command]
pub fn session_push_video_frame(
    state: State<'_, AppState>,
    frame: String,
) -> CommandResult<VideoFrameAcceptance> {
    session_push_video_frame_cmd(&state, &frame)
}

pub(super) fn session_push_video_frame_cmd(
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

pub fn session_export_blocking(
    state: State<'_, AppState>,
    session_id: String,
    format: String,
) -> CommandResult<SessionExportResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    session_export_cmd(&state, session_id, format)
}

pub fn session_list_blocking(state: State<'_, AppState>) -> CommandResult<Vec<SessionSummary>> {
    session_list_cmd(&state)
}

pub fn session_get_blocking(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<SessionDetail> {
    session_get_cmd(&state, session_id)
}

pub fn session_delete_blocking(
    state: State<'_, AppState>,
    session_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    session_delete_cmd(&state, session_id)
}

pub fn session_finalize_utterance_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    text: String,
) -> CommandResult<SessionTurnView> {
    session_finalize_utterance_blocking_inner(app, state, text, false)
}

pub fn session_trigger_assistant_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CommandResult<SessionTurnView> {
    session_finalize_utterance_blocking_inner(app, state, String::new(), true)
}

fn session_finalize_utterance_blocking_inner<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    text: String,
    force_meeting_assistant: bool,
) -> CommandResult<SessionTurnView> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let cascade = match OpenAiCompatibleCascade::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "Cascade client is unavailable"),
    };
    let embed = match OpenAiCompatibleEmbeddingProbe::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "Embedding client is unavailable"),
    };
    let config = match load_session_config(&state) {
        Ok(config) => config,
        Err(error) => return CommandResult::Err { error },
    };
    let route = active_voice_route(&config);
    let cascade = cascade.with_web_capability(
        config
            .models
            .providers
            .iter()
            .find(|p| Some(p.id.as_str()) == route.and_then(|r| r.llm_provider_id.as_deref()))
            .and_then(|p| p.web_capability)
            .unwrap_or_default(),
    );
    let embedding = active_embedding(&config);
    let asr_secret = match read_provider_secret(
        &state,
        &config,
        route.and_then(|item| item.asr_provider_id.as_deref()),
    ) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let llm_secret = match read_provider_secret(
        &state,
        &config,
        route.and_then(|item| item.llm_provider_id.as_deref()),
    ) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let tts_secret = match read_provider_secret(
        &state,
        &config,
        route.and_then(|item| item.tts_provider_id.as_deref()),
    ) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let embed_secret = match read_embedding_secret(&state, &config, embedding) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let e2e_secret = match read_provider_secret(
        &state,
        &config,
        route.and_then(|item| item.e2e_provider_id.as_deref()),
    ) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let realtime = OpenAiCompatibleRealtime::new();
    let probes = SessionProbes {
        asr: &cascade,
        llm: &cascade,
        tts: &cascade,
        embed: &embed,
        realtime: &realtime,
    };
    let credentials = CascadeCredentials {
        asr: asr_secret.as_deref().map(String::as_str),
        llm: llm_secret.as_deref().map(String::as_str),
        tts: tts_secret.as_deref().map(String::as_str),
        embed: embed_secret.as_deref().map(String::as_str),
        e2e: e2e_secret.as_deref().map(String::as_str),
    };
    // 流式上屏：文本快照即时发事件（done=false），轮次落库后由
    // emit_transcript_and_reply 补发 done=true 的收尾事件。闭包与 hooks
    // 必须存活于本函数作用域，故在此内联构造而非收敛成辅助函数。
    let emit_transcript = |text: &str| {
        let _ = app.emit(
            EVENT_SESSION_TRANSCRIPT,
            SessionTranscriptEvent {
                seq: bump_event_seq(&state),
                text: clip_event_text(text),
                done: false,
            },
        );
    };
    let emit_reply = |text: &str| {
        let _ = app.emit(
            EVENT_SESSION_REPLY,
            SessionReplyEvent {
                seq: bump_event_seq(&state),
                text: clip_event_text(text),
                done: false,
            },
        );
    };
    let hooks = TurnStreamHooks {
        user_text: Some(&emit_transcript),
        assistant_text: Some(&emit_reply),
    };
    let trimmed = text.trim();
    let mut result = if force_meeting_assistant {
        session_finalize_utterance_cmd_inner(&state, &probes, credentials, None, true, &hooks)
    } else {
        session_finalize_utterance_cmd_inner(
            &state,
            &probes,
            credentials,
            (!trimmed.is_empty()).then_some(trimmed),
            false,
            &hooks,
        )
    };
    if let CommandResult::Ok { data } = &mut result {
        let web = cascade.web_result();
        data.web_sources = Some(web.sources.clone());
        data.web_degraded = Some(web.degraded);
        if let Ok(database) = state.database.lock()
            && let Some(database) = database.as_ref()
            && let Ok(sessions) = state.sessions.lock()
            && let Some(id) = sessions.session_id()
        {
            let payload = serde_json::json!({ "turnId": data.id, "sources": web.sources, "degraded": web.degraded });
            if SessionStore::new(database)
                .append_event(id, "web_sources", &payload.to_string())
                .is_err()
            {
                return service_error(
                    "DATABASE_OPERATION_FAILED",
                    "回答已生成，但联网来源保存失败",
                );
            }
        }
        emit_transcript_and_reply(&app, &state, &data.user_text, &data.assistant_text);
    }
    emit_runtime(&app, &state);
    result
}

pub fn session_agent_command_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    input: AgentCommandInput,
) -> CommandResult<AgentCommandResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let cascade = match OpenAiCompatibleCascade::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "Cascade client is unavailable"),
    };
    let embed = match OpenAiCompatibleEmbeddingProbe::new() {
        Ok(client) => client,
        Err(error) => return service_error(error.code(), "Embedding client is unavailable"),
    };
    let config = match load_session_config(&state) {
        Ok(config) => config,
        Err(error) => return CommandResult::Err { error },
    };
    let route = active_voice_route(&config);
    let cascade = cascade.with_web_capability(
        config
            .models
            .providers
            .iter()
            .find(|p| Some(p.id.as_str()) == route.and_then(|r| r.llm_provider_id.as_deref()))
            .and_then(|p| p.web_capability)
            .unwrap_or_default(),
    );
    let embedding = active_embedding(&config);
    let asr_secret = match read_provider_secret(
        &state,
        &config,
        route.and_then(|item| item.asr_provider_id.as_deref()),
    ) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let llm_secret = match read_provider_secret(
        &state,
        &config,
        route.and_then(|item| item.llm_provider_id.as_deref()),
    ) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let tts_secret = match read_provider_secret(
        &state,
        &config,
        route.and_then(|item| item.tts_provider_id.as_deref()),
    ) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let embed_secret = match read_embedding_secret(&state, &config, embedding) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let e2e_secret = match read_provider_secret(
        &state,
        &config,
        route.and_then(|item| item.e2e_provider_id.as_deref()),
    ) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let realtime = OpenAiCompatibleRealtime::new();
    let probes = SessionProbes {
        asr: &cascade,
        llm: &cascade,
        tts: &cascade,
        embed: &embed,
        realtime: &realtime,
    };
    let credentials = CascadeCredentials {
        asr: asr_secret.as_deref().map(String::as_str),
        llm: llm_secret.as_deref().map(String::as_str),
        tts: tts_secret.as_deref().map(String::as_str),
        embed: embed_secret.as_deref().map(String::as_str),
        e2e: e2e_secret.as_deref().map(String::as_str),
    };
    // 流式上屏只用于 retry：重试会重新生成问题文本，边生成边进回复气泡。
    // 纪要（report）生成的是结构化 JSON、say/correct 的文本本就已知，
    // 都不该流式残留，故其余动作不挂回调。
    let emit_reply = |text: &str| {
        let _ = app.emit(
            EVENT_SESSION_REPLY,
            SessionReplyEvent {
                seq: bump_event_seq(&state),
                text: clip_event_text(text),
                done: false,
            },
        );
    };
    let hooks = if input.action == "retry" {
        TurnStreamHooks {
            user_text: None,
            assistant_text: Some(&emit_reply),
        }
    } else {
        TurnStreamHooks::none()
    };
    let result = session_agent_command_cmd_inner(&state, &probes, credentials, input, &hooks);
    if let CommandResult::Ok { data } = &result
        && data.ok
    {
        if let Some(text) = data.result.get("text").and_then(|value| value.as_str()) {
            emit_transcript_and_reply(&app, &state, "", text);
        } else if let Some(question) = data.result.get("question").and_then(|value| value.as_str())
        {
            emit_transcript_and_reply(&app, &state, "", question);
        } else if let Some(answer) = data.result.get("answer").and_then(|value| value.as_str()) {
            emit_transcript_and_reply(&app, &state, "", answer);
        }
    }
    emit_runtime(&app, &state);
    result
}

#[tauri::command]
pub fn runtime_get_status(state: State<'_, AppState>) -> CommandResult<RuntimeStatus> {
    runtime_get_status_cmd(&state)
}
blocking_command!(with_events session_start, session_start_blocking(role_profile_id: Option<String>, voice_route_id: Option<String>, allow_web_search: Option<bool>, meeting_pid: Option<u32>, output_device_id: Option<String>, allow_barge_in: Option<bool>) -> SessionStartResult);
blocking_command!(session_export, session_export_blocking(session_id: String, format: String) -> SessionExportResult);
blocking_command!(session_list, session_list_blocking() -> Vec<SessionSummary>);
blocking_command!(session_get, session_get_blocking(session_id: String) -> SessionDetail);
blocking_command!(session_delete, session_delete_blocking(session_id: String) -> FoundationStatus);
blocking_command!(with_events session_finalize_utterance, session_finalize_utterance_blocking(text: String) -> SessionTurnView);
blocking_command!(with_events session_trigger_assistant, session_trigger_assistant_blocking() -> SessionTurnView);
blocking_command!(with_events session_agent_command, session_agent_command_blocking(input: AgentCommandInput) -> AgentCommandResult);
