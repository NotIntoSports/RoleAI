//! 单轮编排命令：收尾三阶段、agent 指令与运行时状态（纯搬移自 sessions.rs）。

use super::*;
use crate::providers::CascadeError;
use crate::services::sessions::finalize::{
    BeginFinalize, TextPersist, finalize_network, run_finalize_playback,
};

#[cfg(test)]
pub(in crate::commands) fn session_agent_command_cmd(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    input: AgentCommandInput,
) -> CommandResult<AgentCommandResult> {
    session_agent_command_cmd_inner(state, probes, credentials, input, &TurnStreamHooks::none())
}

pub(in crate::commands) fn session_agent_command_cmd_inner(
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
pub(in crate::commands) fn session_finalize_utterance_cmd(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    text: Option<&str>,
) -> CommandResult<SessionTurnView> {
    session_finalize_utterance_cmd_inner(state, probes, credentials, text, &TurnStreamHooks::none())
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

pub(in crate::commands) fn session_finalize_utterance_cmd_inner(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    text: Option<&str>,
    hooks: &TurnStreamHooks<'_>,
) -> CommandResult<SessionTurnView> {
    let config = match load_session_config(state) {
        Ok(config) => config,
        Err(error) => return CommandResult::Err { error },
    };
    // 数据库只借 Arc：收尾的网络与播放阶段不持有外层互斥
    // （Database 内部自带连接互斥），记录/资料/诊断命令不再被收尾阻塞。
    let database_slot = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database_slot.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let database = Arc::clone(database);
    drop(database_slot);
    let mut sessions = match state.sessions.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error(
                "SERVICE_BUSY",
                "Service configuration is temporarily unavailable",
            );
        }
    };
    // 阶段一（持锁）：读取快照、生成本轮 id、登记收尾代次。
    // 会议助手点名门控在泵（realtime_pump）与 finalize（mention 判定）内完成；
    // 「让助手回答」手动兜底已随按钮/热键移除，仅转写发言等点名后自然作答。
    let plan = match sessions.begin_finalize(&database, &config, text) {
        Ok(BeginFinalize::Plan(plan)) => plan,
        Ok(BeginFinalize::Idle) => {
            drop(sessions);
            return session_service_error(SessionServiceError::StateInvalid);
        }
        Err(error) => {
            drop(sessions);
            return session_service_error(error);
        }
    };
    drop(sessions);
    // 阶段二（不持锁）：ASR/LLM/TTS 网络调用；停止与只读命令正常响应。
    let outcome = finalize_network(&plan, &database, probes, credentials, hooks);
    let mut sessions = match state.sessions.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error(
                "SERVICE_BUSY",
                "Service configuration is temporarily unavailable",
            );
        }
    };
    // 阶段三（持锁）：校验代次（未被停止/更替）后落库，安排播放。
    let persist = match sessions.complete_finalize_text(*plan, outcome, &database, credentials) {
        Ok(persist) => persist,
        Err(error) => {
            drop(sessions);
            return session_service_error(error);
        }
    };
    let persist = match persist {
        TextPersist::Dropped => {
            drop(sessions);
            return session_service_error(SessionServiceError::Cascade(CascadeError::Cancelled));
        }
        TextPersist::Idle => {
            drop(sessions);
            return session_service_error(SessionServiceError::StateInvalid);
        }
        TextPersist::Turn(persist) => persist,
    };
    drop(sessions);
    // 播放（不持锁）：扬声器输出期间所有命令正常响应。
    let (playback_status, playback_error) = match &persist.playback_job {
        None => (persist.playback_status, None),
        Some(job) => match run_finalize_playback(job) {
            Ok(status) => (status, None),
            Err(code) => ("failed", Some(code)),
        },
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
    let finalized =
        sessions.complete_finalize_playback(*persist, playback_status, playback_error, &database);
    match finalized {
        Ok(Some(_)) => last_session_turn_view(&sessions, &database),
        Ok(None) => session_service_error(SessionServiceError::StateInvalid),
        Err(error) => session_service_error(error),
    }
}

pub(in crate::commands) fn runtime_get_status_cmd(
    state: &AppState,
) -> CommandResult<RuntimeStatus> {
    CommandResult::Ok {
        data: runtime_status_from_state(state),
    }
}

pub fn session_finalize_utterance_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    text: String,
) -> CommandResult<SessionTurnView> {
    session_finalize_utterance_blocking_inner(app, state, text)
}

fn session_finalize_utterance_blocking_inner<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    text: String,
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
    // 供应商/密钥读取完成后立即释放 service_lock：收尾的网络与播放阶段
    // 不再阻塞配置读写、会话开始/导出/删除等同样持该锁的命令。
    // 本轮的配置/端点/密钥已在上方固化为快照，阶段二期间改配置不影响本轮。
    drop(_guard);
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
    let mut result = session_finalize_utterance_cmd_inner(
        &state,
        &probes,
        credentials,
        (!trimmed.is_empty()).then_some(trimmed),
        &hooks,
    );
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

blocking_command!(with_events session_finalize_utterance, session_finalize_utterance_blocking(text: String) -> SessionTurnView);
blocking_command!(with_events session_agent_command, session_agent_command_blocking(input: AgentCommandInput) -> AgentCommandResult);
