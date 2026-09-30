//! sessions 域命令：会话生命周期、单轮编排、麦克风/视频推流、运行时状态。
//! 纯搬移自 commands.rs，不含行为变更。
use super::*;

pub(super) const EVENT_RUNTIME_STATUS: &str = "runtime:status:v1";
pub(super) const EVENT_AUDIO_LEVEL: &str = "audio:level:v1";
pub(super) const EVENT_SESSION_TRANSCRIPT: &str = "session:transcript:v1";
pub(super) const EVENT_SESSION_REPLY: &str = "session:reply:v1";
pub(super) const EVENT_SESSION_AUDIO: &str = "session:audio:v1";
pub(super) const EVENT_SESSION_PLAYBACK_CONTROL: &str = "session:playback-control:v1";

pub(in crate::commands) fn session_service_error<T: ts_rs::TS>(
    error: SessionServiceError,
) -> CommandResult<T> {
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

// 子模块：纯搬移拆分（C23）。glob 再导出沿用 config.rs 的既有做法：
// pub 项（含 tauri 宏的隐藏伴随项 __cmd__*/__tauri_command_name_*）与
// pub(in crate::commands) 项（原 pub(super)，可见性不变）一起上浮到 commands。
mod lifecycle;
mod records;
mod turns;

pub use self::lifecycle::*;
pub use self::records::*;
pub use self::turns::*;
