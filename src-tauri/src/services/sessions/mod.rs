use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};

use crate::{
    audio::{ASR_SAMPLE_RATE, AudioCapture, AudioError, NoopSink, PlaybackSink, SidecarPoll},
    config::{PublicConfig, RoleScenario, VoiceRouteMode},
    database::{Database, DatabaseError},
    providers::{
        CascadeError, CascadeStage, ChatMessage, ChatModel, EmbeddingProbe,
        OpenAiCompatibleCascade, ProviderEndpoint, RealtimeAudioRequest, RealtimeError,
        RealtimeModel, RealtimeTextRequest, SpeechToText, TextToSpeech, TurnStreamHooks,
    },
    runtime::{
        AgentCommand, AgentCommandAction, AgentCommandError, AgentCommandOutcome, AgentMode,
        CascadeCredentials, CascadeTurn, CascadeTurnDeps, CascadeTurnRequest, HistoryTurn,
        PreflightIssue, RuntimeError, SessionPhase, SessionRuntime, active_role_profile,
        active_voice_route, assert_expected_revision, build_snapshot, cascade::retrieve,
        execute_agent_command, preflight, run_cascade_turn,
    },
    sessions::{NewCitation, NewSession, NewSnapshot, NewTurn, SessionRecord, SessionStore},
};

mod realtime;
pub use self::realtime::*;
mod cascade_turn;
pub use self::cascade_turn::*;

pub struct MeetingCapture<'a> {
    pub exe: &'a std::path::Path,
    pub pid: u32,
    pub enumerator: &'a dyn crate::processes::ProcessEnumerator,
}

#[derive(Debug)]
pub enum SessionServiceError {
    AlreadyActive,
    NotFound,
    StateInvalid,
    TransportInvalid,
    SidecarFailed,
    Cascade(CascadeError),
    Realtime(RealtimeError),
    Audio(AudioError),
    Database(DatabaseError),
    /// 播放流句柄启动失败（携带 sidecar 错误码）。
    Playback(&'static str),
}

impl SessionServiceError {
    pub fn code(&self) -> &str {
        match self {
            Self::AlreadyActive => "SESSION_ALREADY_ACTIVE",
            Self::NotFound => "SESSION_NOT_FOUND",
            Self::StateInvalid => "SESSION_STATE_INVALID",
            Self::TransportInvalid => "SESSION_TRANSPORT_INVALID",
            Self::SidecarFailed => "SESSION_SIDECAR_FAILED",
            Self::Cascade(error) => error.code(),
            Self::Realtime(error) => error.code(),
            Self::Audio(error) => error.code(),
            Self::Database(error) => error.code(),
            Self::Playback(code) => code,
        }
    }
}

impl From<CascadeError> for SessionServiceError {
    fn from(error: CascadeError) -> Self {
        Self::Cascade(error)
    }
}

impl From<RealtimeError> for SessionServiceError {
    fn from(error: RealtimeError) -> Self {
        Self::Realtime(error)
    }
}

impl From<AudioError> for SessionServiceError {
    fn from(error: AudioError) -> Self {
        if error == AudioError::SidecarFailed {
            Self::SidecarFailed
        } else {
            Self::Audio(error)
        }
    }
}

impl From<DatabaseError> for SessionServiceError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<RuntimeError> for SessionServiceError {
    fn from(_: RuntimeError) -> Self {
        Self::StateInvalid
    }
}

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SessionStartOutcome {
    Started { session: SessionRecord },
    Blocked { issues: Vec<PreflightIssue> },
}

pub struct SessionProbes<'a> {
    pub asr: &'a dyn SpeechToText,
    pub llm: &'a dyn ChatModel,
    pub tts: &'a dyn TextToSpeech,
    pub embed: &'a dyn EmbeddingProbe,
    pub realtime: &'a dyn RealtimeModel,
}

/// Flags stop / takeover can set without waiting for `SessionService`.
pub struct SessionControl {
    cancel: AtomicBool,
    stop_requested: AtomicBool,
    stop_tts: AtomicBool,
    mode: AtomicU8,
    session_id: Mutex<Option<String>>,
    confirmation_epoch: AtomicU64,
    barge_in_source: Mutex<Option<Arc<AtomicBool>>>,
}

impl SessionControl {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cancel: AtomicBool::new(false),
            stop_requested: AtomicBool::new(false),
            stop_tts: AtomicBool::new(false),
            mode: AtomicU8::new(mode_u8(AgentMode::AiActive)),
            session_id: Mutex::new(None),
            confirmation_epoch: AtomicU64::new(0),
            barge_in_source: Mutex::new(None),
        })
    }

    pub fn request_cancel(&self) {
        self.confirmation_epoch.fetch_add(1, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn request_stop(&self) {
        self.confirmation_epoch.fetch_add(1, Ordering::SeqCst);
        self.stop_requested.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
        self.stop_tts.store(true, Ordering::SeqCst);
    }

    pub fn set_mode(&self, mode: AgentMode) {
        self.mode.store(mode_u8(mode), Ordering::SeqCst);
        if mode == AgentMode::AiActive {
            self.stop_tts.store(false, Ordering::SeqCst);
        } else {
            self.confirmation_epoch.fetch_add(1, Ordering::SeqCst);
            self.stop_tts.store(true, Ordering::SeqCst);
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::SeqCst)
    }

    pub fn mode(&self) -> AgentMode {
        mode_from_u8(self.mode.load(Ordering::SeqCst))
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn take_stop_tts(&self) -> bool {
        self.stop_tts.swap(false, Ordering::SeqCst)
    }

    pub fn set_barge_in_source(&self, flag: Arc<AtomicBool>) {
        *self
            .barge_in_source
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(flag);
    }
    pub fn barge_in_requested(&self) -> bool {
        self.barge_in_source
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }
    pub fn take_barge_in(&self) -> bool {
        self.barge_in_source
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .is_some_and(|flag| flag.swap(false, Ordering::SeqCst))
    }

    fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }

    fn clear_cancel(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    fn set_session_id(&self, session_id: Option<String>) {
        if let Ok(mut guard) = self.session_id.lock() {
            *guard = session_id;
        }
    }

    fn reset(&self) {
        self.cancel.store(false, Ordering::SeqCst);
        self.stop_requested.store(false, Ordering::SeqCst);
        self.stop_tts.store(false, Ordering::SeqCst);
        self.mode
            .store(mode_u8(AgentMode::AiActive), Ordering::SeqCst);
        // 丢弃 barge 旗标来源：停止/重建会话后，旧 capture 的旗标不得继续作用于控制端。
        *self
            .barge_in_source
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        self.set_session_id(None);
    }
}

pub struct SessionService<S: PlaybackSink = NoopSink> {
    runtime: SessionRuntime,
    session_id: Option<String>,
    capture: AudioCapture,
    sink: S,
    control: Arc<SessionControl>,
    turn_index: i64,
    revision: u64,
    unused_materials: bool,
    last_error_code: Option<String>,
    config_snapshot: Option<PublicConfig>,
    playback: Option<crate::audio::playback::BridgePlayback>,
    text_only: bool,
    pending_confirmation_epoch: Option<u64>,
    // 阶段 4：后台摘要压缩 job 的结果通道；Some 表示有一个压缩任务在途。
    summary_job: Option<std::sync::mpsc::Receiver<Result<(String, i64), String>>>,
    // 实时会话泵（端到端流式路线）：读侧共享状态 + 泵本体（Drop 即关停）。
    realtime_shared: Option<std::sync::Arc<crate::services::realtime_pump::PumpShared>>,
    realtime_pump: Option<crate::services::realtime_pump::RealtimePump>,
}

impl SessionService<NoopSink> {
    pub fn new() -> Self {
        Self::with_sink(NoopSink)
    }
}

impl Default for SessionService<NoopSink> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: PlaybackSink> SessionService<S> {
    pub fn with_sink(sink: S) -> Self {
        Self {
            runtime: SessionRuntime::new(),
            session_id: None,
            capture: AudioCapture::from_injected(),
            sink,
            control: SessionControl::new(),
            turn_index: 0,
            revision: 0,
            unused_materials: false,
            last_error_code: None,
            config_snapshot: None,
            playback: None,
            text_only: false,
            pending_confirmation_epoch: None,
            summary_job: None,
            realtime_shared: None,
            realtime_pump: None,
        }
    }

    pub fn control(&self) -> Arc<SessionControl> {
        Arc::clone(&self.control)
    }

    pub fn config_snapshot(&self) -> Option<&PublicConfig> {
        self.has_active_session()
            .then_some(self.config_snapshot.as_ref())
            .flatten()
    }

    pub fn phase(&self) -> SessionPhase {
        self.runtime.phase()
    }

    pub fn mode(&self) -> AgentMode {
        self.control.mode()
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn capture(&self) -> &AudioCapture {
        &self.capture
    }

    pub fn capture_mut(&mut self) -> &mut AudioCapture {
        &mut self.capture
    }

    pub fn utterance_ready(&self) -> bool {
        if !self.has_active_session() || self.runtime.phase() != SessionPhase::Listening {
            return false;
        }
        // 端到端流式路线：就绪信号是泵完成轮；其余路线沿用本地 VAD 分段。
        self.realtime_shared
            .as_ref()
            .map(|shared| shared.completed_count() > 0)
            .unwrap_or_else(|| self.capture.utterance_ready())
    }

    /// 端到端流式路线播报中（UI 阶段显示 Speaking）。
    pub fn realtime_speaking(&self) -> bool {
        self.realtime_shared
            .as_ref()
            .is_some_and(|shared| shared.speaking.load(Ordering::SeqCst))
    }

    /// 实时语音 WS 链路状态：idle（无实时路线）/ connected / reconnecting /
    /// failed（终局，如鉴权失败）。读取不消费 failed——finalize 仍会取走。
    pub fn realtime_link_status(&self) -> &'static str {
        let Some(shared) = self.realtime_shared.as_ref() else {
            return "idle";
        };
        if shared
            .failed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some()
        {
            return "failed";
        }
        if shared.reconnecting.load(Ordering::SeqCst) {
            return "reconnecting";
        }
        "connected"
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn unused_materials(&self) -> bool {
        self.unused_materials
    }

    pub fn last_error_code(&self) -> Option<&str> {
        self.last_error_code.as_deref()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn reset(&mut self) {
        self.reset_runtime();
    }

    pub fn configure_playback(&mut self, playback: Option<crate::audio::playback::BridgePlayback>) {
        self.text_only = playback.is_none();
        self.playback = playback;
    }


    #[cfg(test)]
    fn runtime_can_answer(&self) -> bool {
        self.runtime.can_answer()
    }

    pub fn start(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        secrets_ready: bool,
        allow_barge_in: bool,
    ) -> Result<SessionStartOutcome, SessionServiceError> {
        // 会议桥强制关闭的判定在 start_inner 内完成；这里只透传会话级开关。
        self.start_inner(database, config, secrets_ready, None, allow_barge_in)
    }

    pub fn start_with_meeting_capture(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        secrets_ready: bool,
        capture: MeetingCapture<'_>,
        allow_barge_in: bool,
    ) -> Result<SessionStartOutcome, SessionServiceError> {
        self.start_inner(
            database,
            config,
            secrets_ready,
            Some(capture),
            allow_barge_in,
        )
    }

    fn start_inner(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        secrets_ready: bool,
        capture: Option<MeetingCapture<'_>>,
        allow_barge_in: bool,
    ) -> Result<SessionStartOutcome, SessionServiceError> {
        let issues = preflight(config, secrets_ready, true);
        if !issues.is_empty() {
            return Ok(SessionStartOutcome::Blocked { issues });
        }

        let store = SessionStore::new(database);
        if self.has_active_session()
            || store
                .list()?
                .iter()
                .any(|session| !is_terminal_status(&session.status))
        {
            return Err(SessionServiceError::AlreadyActive);
        }

        self.reset_runtime();
        self.unused_materials = false;
        self.last_error_code = None;
        // Keep the process locally owned until every startup step succeeds. Any
        // subsequent database/transport error drops it and terminates the sidecar.
        let pending_capture = capture
            .map(|capture| AudioCapture::spawn_bridge(capture.exe, capture.pid, capture.enumerator))
            .transpose()?;
        let session_id = uuid::Uuid::new_v4().to_string();
        self.control.set_session_id(Some(session_id.clone()));
        let role_profile_id = active_role_profile(config)
            .map(|profile| profile.id.as_str())
            .unwrap_or_default();
        let voice_route_id = active_voice_route(config)
            .map(|route| route.id.as_str())
            .unwrap_or_default();
        store.insert_session(NewSession {
            id: &session_id,
            status: "preparing",
            role_profile_id,
            voice_route_id,
            transport_mode: "direct",
        })?;
        persist_snapshot(&store, &session_id, config)?;
        self.session_id = Some(session_id.clone());
        self.config_snapshot = Some(config.clone());
        self.runtime.transition(SessionPhase::Preparing)?;
        persist_phase(&store, &session_id, SessionPhase::Preparing)?;
        self.runtime.transition(SessionPhase::Listening)?;
        persist_phase(&store, &session_id, SessionPhase::Listening)?;
        let session = store
            .get(&session_id)?
            .ok_or(SessionServiceError::NotFound)?;
        if let Some(capture) = pending_capture {
            self.capture = capture;
        }
        if allow_barge_in && !self.capture.is_meeting_bridge() {
            self.capture.set_barge_in_enabled(true);
            self.control
                .set_barge_in_source(self.capture.barge_in_flag());
        } else {
            self.capture.set_barge_in_enabled(false);
        }
        Ok(SessionStartOutcome::Started { session })
    }

    pub fn stop(&mut self, database: &Database) -> Result<SessionRecord, SessionServiceError> {
        self.control.request_stop();
        self.sink.cancel();
        self.supersede_pending_confirmation(database);
        self.finish_stop(database)
    }

    pub fn set_mode(
        &mut self,
        database: &Database,
        mode: AgentMode,
    ) -> Result<AgentMode, SessionServiceError> {
        self.control.set_mode(mode);
        self.runtime.set_mode(mode);
        if self.control.take_stop_tts() || self.runtime.take_stop_tts() {
            self.sink.cancel();
        }
        if mode != AgentMode::AiActive {
            self.supersede_pending_confirmation(database);
            self.discard_realtime_held();
        }
        if let Some(session_id) = &self.session_id {
            SessionStore::new(database).append_event(
                session_id,
                "takeover",
                &serde_json::json!({ "mode": mode_name(mode) }).to_string(),
            )?;
        }
        Ok(self.control.mode())
    }


    pub fn execute_command(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
        command: AgentCommand,
    ) -> Result<AgentCommandOutcome, SessionServiceError> {
        self.execute_command_with_hooks(
            database,
            config,
            probes,
            credentials,
            command,
            &TurnStreamHooks::none(),
        )
    }

    pub fn execute_command_with_hooks(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
        command: AgentCommand,
        hooks: &TurnStreamHooks<'_>,
    ) -> Result<AgentCommandOutcome, SessionServiceError> {
        self.poll_sidecar(database)?;
        self.runtime.set_mode(self.control.mode());
        if self.control.take_stop_tts() {
            self.sink.cancel();
        }
        let session_id = self
            .session_id
            .clone()
            .ok_or(SessionServiceError::NotFound)?;
        if command.action == AgentCommandAction::SetMode {
            let mode = command.mode.ok_or(SessionServiceError::StateInvalid)?;
            self.set_mode(database, mode)?;
            return Ok(AgentCommandOutcome::ok(
                command.command_id,
                command.action,
                [("mode", serde_json::json!(mode.as_str()))]
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
            ));
        }
        if matches!(
            command.action,
            AgentCommandAction::Retry
                | AgentCommandAction::Correct
                | AgentCommandAction::ConfirmCandidate
        ) && let Err(error) = assert_expected_revision(command.expected_revision, self.revision)
        {
            return Ok(AgentCommandOutcome::fail(
                command.command_id,
                command.action,
                error.code(),
            ));
        }
        let store = SessionStore::new(database);
        let turns = store.list_turns(&session_id)?;
        let candidate_session = self
            .config_snapshot
            .as_ref()
            .and_then(active_session_role_scenario)
            == Some(RoleScenario::Candidate);
        if candidate_session && command.action == AgentCommandAction::Say {
            return Ok(AgentCommandOutcome::fail(
                command.command_id,
                command.action,
                AgentCommandError::Invalid.code(),
            ));
        }
        if matches!(
            command.action,
            AgentCommandAction::Retry
                | AgentCommandAction::Correct
                | AgentCommandAction::ConfirmCandidate
        ) && turns.is_empty()
        {
            return Ok(AgentCommandOutcome::fail(
                command.command_id,
                command.action,
                AgentCommandError::Invalid.code(),
            ));
        }
        if command.action == AgentCommandAction::ConfirmCandidate {
            let pending_current_turn = store
                .list_events(&session_id)?
                .into_iter()
                .rev()
                .find(|event| event.kind == "turn_meta")
                .and_then(|event| serde_json::from_str::<serde_json::Value>(&event.payload).ok())
                .is_some_and(|payload| {
                    payload["turnId"].as_str() == turns.last().map(|turn| turn.id.as_str())
                        && payload["userConfirmed"].as_bool() == Some(false)
                        && payload["playbackStatus"].as_str() == Some("pending_confirmation")
                });
            if !candidate_session
                || !pending_current_turn
                || self.pending_confirmation_epoch
                    != Some(self.control.confirmation_epoch.load(Ordering::SeqCst))
            {
                return Ok(AgentCommandOutcome::fail(
                    command.command_id,
                    command.action,
                    AgentCommandError::Invalid.code(),
                ));
            }
        }
        let config = with_default_voice(self.config_snapshot.as_ref().unwrap_or(config));
        let e2e_route = active_voice_route(&config)
            .is_some_and(|route| route.mode == crate::config::VoiceRouteMode::E2e);
        let history = turns
            .iter()
            .map(|turn| HistoryTurn {
                user_text: turn.user_text.clone(),
                assistant_text: turn.assistant_text.clone(),
            })
            .collect::<Vec<_>>();
        let last_turn_id = turns.last().map(|turn| turn.id.clone());
        // 端到端流式候选确认：未改稿直接放行扣留音频（零合成延迟）；改稿丢弃扣留后走旧合成路径。
        if command.action == AgentCommandAction::ConfirmCandidate && self.realtime_pump.is_some() {
            let last_assistant = turns
                .last()
                .map(|turn| turn.assistant_text.trim().to_owned())
                .unwrap_or_default();
            if command.text.trim() == last_assistant {
                self.flush_realtime_held();
                if let Some(turn_id) = last_turn_id.as_deref() {
                    store.update_assistant_text(turn_id, &command.text)?;
                    store.append_event(
                        &session_id,
                        "reply",
                        &serde_json::json!({ "text": truncate(&command.text) }).to_string(),
                    )?;
                    store.append_event(
                        &session_id,
                        "turn_meta",
                        &serde_json::json!({
                            "turnId": turn_id,
                            "triggerSource": "user_confirmation",
                            "userConfirmed": true,
                            "playbackStatus": "played",
                        })
                        .to_string(),
                    )?;
                }
                self.revision += 1;
                self.last_error_code = None;
                return Ok(AgentCommandOutcome::ok(
                    command.command_id,
                    command.action,
                    serde_json::Map::new(),
                ));
            }
            self.discard_realtime_held();
        }
        let cached_pcm = std::cell::RefCell::new(Option::<Vec<u8>>::None);
        let played_audio = std::cell::Cell::new(false);
        let last_error = std::cell::RefCell::new(Option::<SessionServiceError>::None);
        let include_audio = command.action != AgentCommandAction::Report;
        let cancel = self.control.cancel_flag();
        let generate = |prompt: &str| match generate_command_text(
            CommandGenerate {
                probes,
                config: &config,
                credentials,
                history: &history,
                prompt,
                e2e_route,
                include_audio,
                hooks,
            },
            cancel,
        ) {
            Ok((text, pcm)) => {
                if !pcm.is_empty() {
                    *cached_pcm.borrow_mut() = Some(pcm);
                }
                Ok(text)
            }
            Err(error) => {
                *last_error.borrow_mut() = Some(error);
                Err(AgentCommandError::Invalid)
            }
        };
        let speak = |text: &str| match speak_command_text(
            probes,
            &config,
            credentials,
            text,
            e2e_route,
            cached_pcm.borrow_mut().take(),
            cancel,
        ) {
            Ok(pcm) => {
                if self.control.take_stop_tts() || self.control.is_cancelled() {
                    self.sink.cancel();
                } else if !pcm.is_empty() {
                    if let Some(output) = &self.playback {
                        output
                            .play(&pcm, 24_000, || self.control.is_cancelled())
                            .map_err(|_| AgentCommandError::Invalid)?;
                        played_audio.set(true);
                    } else if !self.text_only {
                        self.sink.play_pcm(&pcm, 24_000);
                        played_audio.set(true);
                    }
                }
                Ok(())
            }
            Err(error) => {
                *last_error.borrow_mut() = Some(error);
                Err(AgentCommandError::Invalid)
            }
        };
        match execute_agent_command(&command, generate, speak) {
            Ok(result) => {
                if command.action == AgentCommandAction::ConfirmCandidate {
                    if let Some(turn_id) = last_turn_id.as_deref() {
                        store.update_assistant_text(turn_id, &command.text)?;
                        store.append_event(
                            &session_id,
                            "reply",
                            &serde_json::json!({ "text": truncate(&command.text) }).to_string(),
                        )?;
                        store.append_event(
                            &session_id,
                            "turn_meta",
                            &serde_json::json!({
                                "turnId": turn_id,
                                "triggerSource": "user_confirmation",
                                "userConfirmed": true,
                                "playbackStatus": if played_audio.get() { "played" } else { "text_only" },
                            })
                            .to_string(),
                        )?;
                    }
                    self.revision += 1;
                }
                if matches!(
                    command.action,
                    AgentCommandAction::Retry | AgentCommandAction::Correct
                ) {
                    let replacement = if command.action == AgentCommandAction::Retry {
                        result
                            .get("question")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                    } else {
                        command.answer.as_str()
                    };
                    if let Some(turn_id) = last_turn_id.as_deref() {
                        store.update_assistant_text(turn_id, replacement)?;
                        store.append_event(
                            &session_id,
                            "reply",
                            &serde_json::json!({ "text": truncate(replacement) }).to_string(),
                        )?;
                    }
                    self.revision += 1;
                }
                self.last_error_code = None;
                Ok(AgentCommandOutcome::ok(
                    command.command_id,
                    command.action,
                    result,
                ))
            }
            Err(error) => {
                if let Some(service_error) = last_error.into_inner() {
                    self.last_error_code = Some(service_error.code().to_owned());
                    return Err(service_error);
                }
                Ok(AgentCommandOutcome::fail(
                    command.command_id,
                    command.action,
                    error.code(),
                ))
            }
        }
    }

    pub fn poll_sidecar(
        &mut self,
        database: &Database,
    ) -> Result<SidecarPoll, SessionServiceError> {
        match self.capture.poll_sidecar()? {
            SidecarPoll::Alive => Ok(SidecarPoll::Alive),
            SidecarPoll::Exited => match self.capture.restart_once() {
                Ok(()) => Ok(SidecarPoll::Alive),
                Err(error) => {
                    self.last_error_code = Some(error.code().to_owned());
                    self.fail_session(database)?;
                    Err(error.into())
                }
            },
        }
    }

    fn has_active_session(&self) -> bool {
        self.session_id.is_some() && !is_terminal_phase(self.runtime.phase())
    }

    fn reset_runtime(&mut self) {
        self.runtime = SessionRuntime::new();
        self.session_id = None;
        self.config_snapshot = None;
        self.playback = None;
        self.text_only = false;
        self.pending_confirmation_epoch = None;
        self.turn_index = 0;
        self.revision = 0;
        self.unused_materials = false;
        self.last_error_code = None;
        // 换会话后旧 job 的结果不得写进新会话，直接丢弃。
        self.summary_job = None;
        self.control.reset();
        self.capture = AudioCapture::from_injected();
    }

    fn supersede_pending_confirmation(&mut self, database: &Database) {
        self.pending_confirmation_epoch = None;
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        let store = SessionStore::new(database);
        let Ok(events) = store.list_events(&session_id) else {
            return;
        };
        let Some(payload) = events
            .iter()
            .rev()
            .find(|event| event.kind == "turn_meta")
            .and_then(|event| serde_json::from_str::<serde_json::Value>(&event.payload).ok())
        else {
            return;
        };
        if payload["playbackStatus"].as_str() != Some("pending_confirmation") {
            return;
        }
        let mut next = payload;
        next["playbackStatus"] = serde_json::json!("superseded");
        let _ = store.append_event(&session_id, "turn_meta", &next.to_string());
    }

    fn recover_from_cascade_error(
        &mut self,
        database: &Database,
        session_id: &str,
        error: CascadeError,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        self.last_error_code = Some(error.code().to_owned());
        if self.control.take_stop_tts() {
            self.sink.cancel();
        }
        if self.control.stop_requested() {
            self.finish_stop(database)?;
            return Err(error.into());
        }
        if is_fatal_cascade(&error) {
            self.fail_session(database)?;
            return Err(error.into());
        }
        self.return_to_listening(database, session_id)?;
        Err(error.into())
    }

    fn recover_from_realtime_error(
        &mut self,
        database: &Database,
        session_id: &str,
        error: RealtimeError,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        self.last_error_code = Some(error.code().to_owned());
        if self.control.take_stop_tts() {
            self.sink.cancel();
        }
        if self.control.stop_requested() {
            self.finish_stop(database)?;
            return Err(error.into());
        }
        if is_fatal_realtime(&error) {
            self.fail_session(database)?;
            return Err(error.into());
        }
        self.return_to_listening(database, session_id)?;
        Err(error.into())
    }

    fn return_to_listening(
        &mut self,
        database: &Database,
        session_id: &str,
    ) -> Result<(), SessionServiceError> {
        if self.runtime.phase() == SessionPhase::Listening {
            return Ok(());
        }
        self.runtime.transition(SessionPhase::Listening)?;
        persist_phase(
            &SessionStore::new(database),
            session_id,
            SessionPhase::Listening,
        )?;
        Ok(())
    }

    fn finish_stop(&mut self, database: &Database) -> Result<SessionRecord, SessionServiceError> {
        // 会话终态：立即关停实时泵、摘除 tap（常驻连接与上行随会话终止）。
        self.detach_realtime_pump();
        let session_id = self
            .session_id
            .clone()
            .ok_or(SessionServiceError::NotFound)?;
        let store = SessionStore::new(database);
        let row = store
            .get(&session_id)?
            .ok_or(SessionServiceError::NotFound)?;
        if is_terminal_phase(self.runtime.phase()) || is_terminal_status(&row.status) {
            return Ok(row);
        }
        self.runtime.transition(SessionPhase::Stopping)?;
        persist_phase(&store, &session_id, SessionPhase::Stopping)?;
        self.runtime.transition(SessionPhase::Completed)?;
        store.finish(&session_id, "completed")?;
        store.append_event(
            &session_id,
            "status",
            &status_payload(SessionPhase::Completed),
        )?;
        store.get(&session_id)?.ok_or(SessionServiceError::NotFound)
    }

    fn fail_session(&mut self, database: &Database) -> Result<(), SessionServiceError> {
        self.detach_realtime_pump();
        let Some(session_id) = self.session_id.clone() else {
            return Ok(());
        };
        if self.runtime.phase() != SessionPhase::Failed {
            self.runtime.transition(SessionPhase::Failed)?;
        }
        let store = SessionStore::new(database);
        store.finish(&session_id, "failed")?;
        store.append_event(&session_id, "status", &status_payload(SessionPhase::Failed))?;
        Ok(())
    }
}

/// `context_summary_turn_index` 的哨兵值：从未写入过摘要。
const NO_CONTEXT_SUMMARY: i64 = -1;
/// 未摘要轮次超过此数触发后台压缩。
const COMPRESS_THRESHOLD: i64 = 12;
/// 压缩时始终保留的最近原始轮次。
const KEEP_RECENT_TURNS: usize = 4;

/// 距上次摘要累计的未压缩轮次是否已超过触发阈值。
fn should_compress(newest_index: i64, upto: i64) -> bool {
    newest_index - upto > COMPRESS_THRESHOLD + KEEP_RECENT_TURNS as i64
}

struct CommandGenerate<'a> {
    probes: &'a SessionProbes<'a>,
    config: &'a PublicConfig,
    credentials: CascadeCredentials<'a>,
    history: &'a [HistoryTurn],
    prompt: &'a str,
    e2e_route: bool,
    include_audio: bool,
    hooks: &'a TurnStreamHooks<'a>,
}

fn generate_command_text(
    request: CommandGenerate<'_>,
    cancel: &AtomicBool,
) -> Result<(String, Vec<u8>), SessionServiceError> {
    if request.e2e_route {
        let (endpoint, model_id) = e2e_endpoint(request.config)?;
        let instructions = e2e_instructions(active_role_profile(request.config), &[]);
        let instructions = if instructions.is_empty() {
            "你是实时语音助手。严格参考会话上下文完成请求，不泄露系统配置。".to_owned()
        } else {
            instructions
        };
        let route_voice = active_voice_route(request.config)
            .and_then(|route| route.voice_id.clone())
            .and_then(|voice| {
                let voice = voice.trim().to_owned();
                (!voice.is_empty()).then_some(voice)
            })
            .unwrap_or_default();
        let turn = request.probes.realtime.text_turn(
            RealtimeTextRequest {
                endpoint: &endpoint,
                credential: request.credentials.e2e,
                model_id: &model_id,
                instructions: &instructions,
                prompt: &command_prompt(request.history, request.prompt),
                include_audio: request.include_audio,
                voice: &route_voice,
                hooks: TurnStreamHooks {
                    user_text: None,
                    assistant_text: request.hooks.assistant_text,
                },
            },
            cancel,
        )?;
        return Ok((turn.assistant_text, turn.tts_pcm));
    }
    let (endpoint, model_id) = llm_endpoint(request.config)?;
    let messages = command_messages(request.config, request.history, request.prompt);
    let noop = |_: &str| {};
    let on_snapshot: &dyn Fn(&str) = request.hooks.assistant_text.unwrap_or(&noop);
    let text = request.probes.llm.complete_streaming(
        &endpoint,
        request.credentials.llm,
        &model_id,
        &messages,
        on_snapshot,
    )?;
    Ok((text, Vec::new()))
}

fn speak_command_text(
    probes: &SessionProbes<'_>,
    config: &PublicConfig,
    credentials: CascadeCredentials<'_>,
    text: &str,
    e2e_route: bool,
    cached_pcm: Option<Vec<u8>>,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, SessionServiceError> {
    if let Some(pcm) = cached_pcm.filter(|bytes| !bytes.is_empty()) {
        return Ok(pcm);
    }
    if e2e_route {
        let (endpoint, model_id) = e2e_endpoint(config)?;
        let route_voice = active_voice_route(config)
            .and_then(|route| route.voice_id.clone())
            .and_then(|voice| {
                let voice = voice.trim().to_owned();
                (!voice.is_empty()).then_some(voice)
            })
            .unwrap_or_default();
        let turn = probes.realtime.text_turn(
            RealtimeTextRequest {
                endpoint: &endpoint,
                credential: credentials.e2e,
                model_id: &model_id,
                instructions: "逐字朗读用户提供的文本，不添加、不删除、不改写。",
                prompt: text,
                include_audio: true,
                voice: &route_voice,
                // 朗读文本已知且由收尾事件整体上屏，这里无需流式回调。
                hooks: TurnStreamHooks::none(),
            },
            cancel,
        )?;
        return Ok(turn.tts_pcm);
    }
    let route =
        active_voice_route(config).ok_or(CascadeError::EndpointInvalid(CascadeStage::Tts))?;
    let endpoint = config
        .models
        .providers
        .iter()
        .find(|provider| Some(provider.id.as_str()) == route.tts_provider_id.as_deref())
        .map(|provider| ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        })
        .filter(|endpoint| !endpoint.base_url.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Tts))?;
    let model_id = route
        .tts_model_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Tts))?;
    let voice_id = route
        .voice_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or("alloy");
    match probes
        .tts
        .synthesize(&endpoint, credentials.tts, model_id, voice_id, text)
    {
        Ok(pcm) => Ok(pcm),
        Err(_) => Ok(Vec::new()),
    }
}

fn e2e_endpoint(config: &PublicConfig) -> Result<(ProviderEndpoint, String), SessionServiceError> {
    let route = active_voice_route(config).ok_or(RealtimeError::UrlInvalid)?;
    let provider_id = route
        .e2e_provider_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(RealtimeError::UrlInvalid)?;
    let model_id = route
        .e2e_model_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(RealtimeError::UrlInvalid)?
        .to_owned();
    let endpoint = config
        .models
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        })
        .filter(|endpoint| !endpoint.base_url.is_empty())
        .ok_or(RealtimeError::UrlInvalid)?;
    Ok((endpoint, model_id))
}

fn llm_endpoint(config: &PublicConfig) -> Result<(ProviderEndpoint, String), SessionServiceError> {
    let route =
        active_voice_route(config).ok_or(CascadeError::EndpointInvalid(CascadeStage::Llm))?;
    let provider_id = route
        .llm_provider_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Llm))?;
    let model_id = route
        .llm_model_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Llm))?
        .to_owned();
    let endpoint = config
        .models
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        })
        .filter(|endpoint| !endpoint.base_url.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Llm))?;
    Ok((endpoint, model_id))
}

fn command_messages(
    config: &PublicConfig,
    history: &[HistoryTurn],
    prompt: &str,
) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    if let Some(role) = active_role_profile(config) {
        let mut system = role.system_prompt.clone();
        if !role.style_instructions.is_empty() {
            if !system.is_empty() {
                system.push_str("\n\n");
            }
            system.push_str(&role.style_instructions);
        }
        if !system.is_empty() {
            messages.push(ChatMessage {
                role: "system".into(),
                content: system,
            });
        }
    }
    for turn in history
        .iter()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        messages.push(ChatMessage {
            role: "user".into(),
            content: turn.user_text.clone(),
        });
        messages.push(ChatMessage {
            role: "assistant".into(),
            content: turn.assistant_text.clone(),
        });
    }
    messages.push(ChatMessage {
        role: "user".into(),
        content: prompt.to_owned(),
    });
    messages
}

fn command_prompt(history: &[HistoryTurn], prompt: &str) -> String {
    let mut context = String::new();
    for turn in history
        .iter()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        context.push_str("用户：");
        context.push_str(&turn.user_text);
        context.push('\n');
        context.push_str("助手：");
        context.push_str(&turn.assistant_text);
        context.push('\n');
    }
    format!("会话上下文：{context}\n任务：{prompt}")
}

fn persist_snapshot(
    store: &SessionStore<'_>,
    session_id: &str,
    config: &PublicConfig,
) -> Result<(), SessionServiceError> {
    let mut snapshot = build_snapshot(config);
    snapshot.transport_mode = "direct".to_owned();
    let provider_ids =
        serde_json::to_string(&snapshot.provider_ids).unwrap_or_else(|_| "[]".into());
    let model_ids = serde_json::to_string(&snapshot.model_ids).unwrap_or_else(|_| "[]".into());
    store.insert_snapshot(NewSnapshot {
        id: &uuid::Uuid::new_v4().to_string(),
        session_id,
        app_version: &snapshot.app_version,
        config_revision: &snapshot.config_revision,
        provider_ids: &provider_ids,
        model_ids: &model_ids,
        voice_route_id: &snapshot.voice_route_id,
        transport_mode: &snapshot.transport_mode,
        role_hash: &snapshot.role_hash,
        knowledge_fingerprint: &snapshot.knowledge_fingerprint,
    })?;
    Ok(())
}

fn persist_phase(
    store: &SessionStore<'_>,
    session_id: &str,
    phase: SessionPhase,
) -> Result<(), SessionServiceError> {
    store.set_status(session_id, phase.as_str())?;
    store.append_event(session_id, "status", &status_payload(phase))?;
    Ok(())
}

fn status_payload(phase: SessionPhase) -> String {
    serde_json::json!({ "status": phase.as_str() }).to_string()
}

fn mode_name(mode: AgentMode) -> &'static str {
    mode.as_str()
}

fn mode_u8(mode: AgentMode) -> u8 {
    match mode {
        AgentMode::AiActive => 0,
        AgentMode::OperatorSpeaking => 1,
        AgentMode::Paused => 2,
        AgentMode::Muted => 3,
    }
}

fn mode_from_u8(value: u8) -> AgentMode {
    match value {
        1 => AgentMode::OperatorSpeaking,
        2 => AgentMode::Paused,
        3 => AgentMode::Muted,
        _ => AgentMode::AiActive,
    }
}

fn is_fatal_cascade(error: &CascadeError) -> bool {
    matches!(
        error,
        CascadeError::Unauthorized(_) | CascadeError::EndpointInvalid(_)
    )
}

fn is_fatal_realtime(error: &RealtimeError) -> bool {
    matches!(
        error,
        RealtimeError::Unauthorized | RealtimeError::UrlInvalid
    )
}

fn run_e2e_turn(
    realtime: &dyn RealtimeModel,
    deps: &CascadeTurnDeps<'_>,
    request: &CascadeTurnRequest<'_>,
    pcm: &[u8],
    cancel: &AtomicBool,
    hooks: &TurnStreamHooks<'_>,
) -> Result<CascadeTurn, RealtimeError> {
    if cancel.load(Ordering::SeqCst) {
        return Err(RealtimeError::Cancelled);
    }
    let route = active_voice_route(request.config).ok_or(RealtimeError::UrlInvalid)?;
    let provider_id = route
        .e2e_provider_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(RealtimeError::UrlInvalid)?;
    let model_id = route
        .e2e_model_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(RealtimeError::UrlInvalid)?;
    let endpoint = request
        .config
        .models
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .map(|provider| ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        })
        .filter(|endpoint| !endpoint.base_url.is_empty())
        .ok_or(RealtimeError::UrlInvalid)?;
    let known_text = request
        .user_text
        .map(str::trim)
        .filter(|text| !text.is_empty());
    // 线路配置的音色（克隆音色 ID 或官方音色名）：DashScope realtime 没有可用的
    // 方言级默认音色，缺失会被服务端以 Voice not supported 拒绝。
    let route_voice = route
        .voice_id
        .as_deref()
        .filter(|voice| !voice.trim().is_empty())
        .unwrap_or("");
    let citations = known_text
        .map(|text| retrieve(deps, request, text))
        .unwrap_or_default();
    let instructions = e2e_instructions(active_role_profile(request.config), &citations);
    let turn = if let Some(prompt) = known_text {
        realtime.text_turn(
            RealtimeTextRequest {
                endpoint: &endpoint,
                credential: request.credentials.e2e,
                model_id,
                instructions: &instructions,
                prompt,
                include_audio: true,
                voice: route_voice,
                // 手动输入时用户文本已知，无需流式转写回调。
                hooks: TurnStreamHooks {
                    user_text: None,
                    assistant_text: hooks.assistant_text,
                },
            },
            cancel,
        )
    } else {
        realtime.transcribe_turn(
            RealtimeAudioRequest {
                endpoint: &endpoint,
                credential: request.credentials.e2e,
                model_id,
                pcm16le: pcm,
                sample_rate: request.sample_rate,
                instructions: &instructions,
                voice: route_voice,
                hooks: *hooks,
            },
            cancel,
        )
    }?;
    if cancel.load(Ordering::SeqCst) {
        return Err(RealtimeError::Cancelled);
    }
    let user_text = known_text.map(ToOwned::to_owned).unwrap_or(turn.user_text);
    Ok(CascadeTurn {
        user_text,
        assistant_text: turn.assistant_text,
        tts_pcm: turn.tts_pcm,
        materials_used: !citations.is_empty(),
        citations,
        error_code: None,
    })
}

pub(crate) fn e2e_instructions(
    role: Option<&crate::config::RoleProfileConfig>,
    citations: &[crate::runtime::TurnCitation],
) -> String {
    let mut out = String::new();
    if let Some(role) = role {
        out.push_str(&role.system_prompt);
        if !role.style_instructions.is_empty() {
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&role.style_instructions);
        }
    }
    if !citations.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("可用资料：\n");
        for citation in citations {
            out.push_str("- ");
            out.push_str(&citation.snippet);
            out.push('\n');
        }
    }
    out
}

fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "interrupted")
}

fn is_terminal_phase(phase: SessionPhase) -> bool {
    matches!(phase, SessionPhase::Completed | SessionPhase::Failed)
}

pub(crate) fn active_session_role_scenario(config: &PublicConfig) -> Option<RoleScenario> {
    let active_id = config.active_role_profile_id.as_deref()?;
    let profile = config
        .role_profiles
        .iter()
        .find(|profile| profile.id == active_id)?;
    profile
        .scenario
        .clone()
        .or_else(|| RoleScenario::from_preset_id(&profile.id))
}

fn transcribe_meeting_pcm(
    asr: &dyn SpeechToText,
    config: &PublicConfig,
    credentials: CascadeCredentials<'_>,
    pcm: &[u8],
) -> Result<String, CascadeError> {
    let route =
        active_voice_route(config).ok_or(CascadeError::EndpointInvalid(CascadeStage::Asr))?;
    let provider_id = route
        .asr_provider_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Asr))?;
    let provider = config
        .models
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Asr))?;
    let model_id = route
        .asr_model_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Asr))?;
    let endpoint = ProviderEndpoint {
        provider_id: provider.id.clone(),
        base_url: provider.base_url.clone(),
    };
    let text = asr.transcribe(&endpoint, credentials.asr, model_id, pcm, ASR_SAMPLE_RATE)?;
    let text = text.trim();
    if text.is_empty() {
        Err(CascadeError::ResponseEmpty(CascadeStage::Asr))
    } else {
        Ok(text.to_owned())
    }
}

pub(crate) fn meeting_assistant_was_mentioned(text: &str, role_name: &str) -> bool {
    let normalized = text.to_ascii_lowercase();
    let name = role_name.trim().to_ascii_lowercase();
    (!name.is_empty() && normalized.contains(&name))
        || normalized.contains("会议助手")
        || normalized.contains("ai助手")
        || normalized.contains("ai 助手")
}

fn with_default_voice(config: &PublicConfig) -> PublicConfig {
    let mut config = config.clone();
    let active_id = config.speech.active_voice_route_id.clone();
    if let Some(route) = config
        .speech
        .voice_routes
        .iter_mut()
        .find(|route| active_id.as_deref() == Some(route.id.as_str()) && route.active)
        && route.voice_id.as_deref().is_none_or(str::is_empty)
    {
        route.voice_id = Some("alloy".into());
    }
    config
}

fn truncate(text: &str) -> String {
    text.chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::{SessionProbes, SessionService, SessionServiceError, SessionStartOutcome};
    use crate::{
        app_state::{AppPaths, AppState},
        audio::{RecordingSink, SidecarPoll},
        config::PublicConfig,
        database::Database,
        providers::{
            CascadeError, ChatMessage, ChatModel, EmbeddingError, EmbeddingProbe, ProviderEndpoint,
            RealtimeAudioRequest, RealtimeError, RealtimeModel, RealtimeTextRequest, RealtimeTurn,
            SpeechToText, TextToSpeech,
        },
        runtime::{
            AgentMode, CascadeCredentials, SessionPhase,
            test_support::{ready_e2e_public_config, ready_public_config},
        },
        secrets::MemorySecretStore,
        sessions::{NewSession, SessionStore},
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    };

    struct ScriptedAsr {
        text: String,
        calls: AtomicU32,
    }

    impl ScriptedAsr {
        fn ok(text: &str) -> Self {
            Self {
                text: text.into(),
                calls: AtomicU32::new(0),
            }
        }
    }

    impl SpeechToText for ScriptedAsr {
        fn transcribe(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[u8],
            _: u32,
        ) -> Result<String, CascadeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.text.clone())
        }
    }

    struct ScriptedLlm {
        reply: String,
        calls: AtomicU32,
        messages: Mutex<Vec<Vec<ChatMessage>>>,
    }

    impl ScriptedLlm {
        fn ok(reply: &str) -> Self {
            Self {
                reply: reply.into(),
                calls: AtomicU32::new(0),
                messages: Mutex::new(Vec::new()),
            }
        }

        fn seen_messages(&self) -> Vec<Vec<ChatMessage>> {
            self.messages.lock().expect("llm messages").clone()
        }
    }

    impl ChatModel for ScriptedLlm {
        fn complete(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            messages: &[ChatMessage],
        ) -> Result<String, CascadeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.messages
                .lock()
                .expect("llm messages")
                .push(messages.to_vec());
            Ok(self.reply.clone())
        }
    }

    struct FailingLlm {
        error: CascadeError,
        calls: AtomicU32,
    }

    impl FailingLlm {
        fn new(error: CascadeError) -> Self {
            Self {
                error,
                calls: AtomicU32::new(0),
            }
        }
    }

    impl ChatModel for FailingLlm {
        fn complete(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[ChatMessage],
        ) -> Result<String, CascadeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(self.error)
        }
    }

    struct GateLlm {
        reply: String,
        entered: Arc<std::sync::atomic::AtomicBool>,
        proceed: Arc<std::sync::atomic::AtomicBool>,
        calls: AtomicU32,
    }

    impl ChatModel for GateLlm {
        fn complete(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[ChatMessage],
        ) -> Result<String, CascadeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.store(true, Ordering::SeqCst);
            while !self.proceed.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(self.reply.clone())
        }
    }

    struct ScriptedTts {
        pcm: Vec<u8>,
        voices: Mutex<Vec<String>>,
        calls: AtomicU32,
    }

    impl ScriptedTts {
        fn ok(pcm: &[u8]) -> Self {
            Self {
                pcm: pcm.to_vec(),
                voices: Mutex::new(Vec::new()),
                calls: AtomicU32::new(0),
            }
        }
    }

    impl TextToSpeech for ScriptedTts {
        fn synthesize(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            voice_id: &str,
            _: &str,
        ) -> Result<Vec<u8>, CascadeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.voices
                .lock()
                .expect("tts voices")
                .push(voice_id.to_owned());
            Ok(self.pcm.clone())
        }
    }

    struct UnusedRealtime;

    impl RealtimeModel for UnusedRealtime {
        fn transcribe_turn(
            &self,
            _: RealtimeAudioRequest<'_>,
            _: &AtomicBool,
        ) -> Result<RealtimeTurn, RealtimeError> {
            panic!("cascaded turn must not call Realtime")
        }
    }

    struct FakeRealtime {
        user_text: String,
        assistant_text: String,
        tts_pcm: Vec<u8>,
        error: Mutex<Option<RealtimeError>>,
        cancel_after: bool,
        calls: AtomicU32,
        text_calls: AtomicU32,
        pcm: Mutex<Vec<u8>>,
        model_id: Mutex<Option<String>>,
        instructions: Mutex<Option<String>>,
        sample_rate: Mutex<Option<u32>>,
    }

    impl FakeRealtime {
        fn ok(user_text: &str, assistant_text: &str, pcm: &[u8]) -> Self {
            Self {
                user_text: user_text.into(),
                assistant_text: assistant_text.into(),
                tts_pcm: pcm.to_vec(),
                error: Mutex::new(None),
                cancel_after: false,
                calls: AtomicU32::new(0),
                text_calls: AtomicU32::new(0),
                pcm: Mutex::new(Vec::new()),
                model_id: Mutex::new(None),
                instructions: Mutex::new(None),
                sample_rate: Mutex::new(None),
            }
        }

        fn fail(error: RealtimeError) -> Self {
            Self {
                user_text: String::new(),
                assistant_text: String::new(),
                tts_pcm: Vec::new(),
                error: Mutex::new(Some(error)),
                cancel_after: false,
                calls: AtomicU32::new(0),
                text_calls: AtomicU32::new(0),
                pcm: Mutex::new(Vec::new()),
                model_id: Mutex::new(None),
                instructions: Mutex::new(None),
                sample_rate: Mutex::new(None),
            }
        }

        fn cancel_after_turn(user_text: &str, assistant_text: &str) -> Self {
            let mut fake = Self::ok(user_text, assistant_text, &[0x09]);
            fake.cancel_after = true;
            fake
        }
    }

    impl RealtimeModel for FakeRealtime {
        fn transcribe_turn(
            &self,
            request: RealtimeAudioRequest<'_>,
            cancel: &AtomicBool,
        ) -> Result<RealtimeTurn, RealtimeError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *self.pcm.lock().expect("pcm") = request.pcm16le.to_vec();
            *self.model_id.lock().expect("model") = Some(request.model_id.to_owned());
            *self.instructions.lock().expect("instructions") =
                Some(request.instructions.to_owned());
            *self.sample_rate.lock().expect("sample_rate") = Some(request.sample_rate);
            if cancel.load(Ordering::SeqCst) {
                return Err(RealtimeError::Cancelled);
            }
            if let Some(error) = self.error.lock().expect("error").take() {
                return Err(error);
            }
            if self.cancel_after {
                cancel.store(true, Ordering::SeqCst);
            }
            // 模拟 WS 转写增量： hooks 挂了回调就会收到完整前缀快照。
            request.hooks.notify_user(&self.user_text);
            request.hooks.notify_assistant(&self.assistant_text);
            Ok(RealtimeTurn {
                user_text: self.user_text.clone(),
                assistant_text: self.assistant_text.clone(),
                tts_pcm: self.tts_pcm.clone(),
            })
        }

        fn text_turn(
            &self,
            request: RealtimeTextRequest<'_>,
            cancel: &AtomicBool,
        ) -> Result<RealtimeTurn, RealtimeError> {
            self.text_calls.fetch_add(1, Ordering::SeqCst);
            *self.model_id.lock().expect("model") = Some(request.model_id.to_owned());
            *self.instructions.lock().expect("instructions") =
                Some(request.instructions.to_owned());
            if cancel.load(Ordering::SeqCst) {
                return Err(RealtimeError::Cancelled);
            }
            if let Some(error) = self.error.lock().expect("error").take() {
                return Err(error);
            }
            request.hooks.notify_assistant(&self.assistant_text);
            Ok(RealtimeTurn {
                user_text: self.user_text.clone(),
                assistant_text: self.assistant_text.clone(),
                tts_pcm: self.tts_pcm.clone(),
            })
        }
    }

    struct UnusedEmbed;

    impl EmbeddingProbe for UnusedEmbed {
        fn embed(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: u32,
            _: &str,
        ) -> Result<Vec<f32>, EmbeddingError> {
            Err(EmbeddingError::RequestFailed)
        }
    }

    fn opened() -> (tempfile::TempDir, Database) {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        (directory, database)
    }

    fn credentials() -> CascadeCredentials<'static> {
        CascadeCredentials {
            asr: Some("asr"),
            llm: Some("llm"),
            tts: Some("tts"),
            embed: Some("emb"),
            e2e: Some("e2e"),
        }
    }

    fn start_ready(service: &mut SessionService, database: &Database) -> String {
        match service
            .start(database, &ready_public_config(), true, false)
            .unwrap()
        {
            SessionStartOutcome::Started { session } => session.id,
            SessionStartOutcome::Blocked { issues } => {
                panic!("expected start, blocked {issues:?}")
            }
        }
    }

    fn start_ready_sink(
        service: &mut SessionService<RecordingSink>,
        database: &Database,
        config: &PublicConfig,
    ) -> String {
        match service.start(database, config, true, false).unwrap() {
            SessionStartOutcome::Started { session } => session.id,
            SessionStartOutcome::Blocked { issues } => {
                panic!("expected start, blocked {issues:?}")
            }
        }
    }

    #[test]
    fn start_returns_blocked_issues_and_does_not_insert_session() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let empty = crate::runtime::test_support::empty_public_config();

        let outcome = service.start(&database, &empty, false, false).unwrap();
        match outcome {
            SessionStartOutcome::Blocked { issues } => {
                assert!(issues.len() >= 2, "{issues:?}");
                assert!(
                    issues
                        .iter()
                        .any(|issue| issue.code == "SESSION_ROUTE_REQUIRED")
                );
            }
            SessionStartOutcome::Started { session, .. } => {
                panic!("started despite preflight {}", session.id)
            }
        }
        assert_eq!(service.phase(), SessionPhase::Idle);
        assert!(SessionStore::new(&database).list().unwrap().is_empty());
    }

    #[test]
    fn start_inserts_session_snapshot_and_reaches_listening() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);

        assert_eq!(service.phase(), SessionPhase::Listening);
        let store = SessionStore::new(&database);
        let row = store.get(&id).unwrap().expect("session");
        assert_eq!(row.status, "listening");
        assert_eq!(row.transport_mode, "direct");
        assert_eq!(row.role_profile_id, "role-1");
        assert_eq!(row.voice_route_id, "route-1");
        assert!(row.started_at.is_some());
        assert!(row.finished_at.is_none());
        let snapshots = store.list_snapshots(&id).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].voice_route_id, "route-1");
        assert_eq!(snapshots[0].transport_mode, "direct");
        assert!(!snapshots[0].role_hash.is_empty());
        assert!(!snapshots[0].provider_ids.contains("sk-"));
    }

    #[test]
    fn start_with_missing_bridge_exe_fails_closed_without_a_session() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        let mut service = SessionService::with_sink(RecordingSink::default());
        let enumerator = crate::processes::InjectedProcessEnumerator::new(vec![
            crate::processes::MeetingProcess {
                pid: 4242,
                name: "zoom.exe".into(),
                title: "Zoom".into(),
            },
        ]);
        let missing = directory.path().join("AudioBridge.exe");
        let error = service
            .start_with_meeting_capture(
                &database,
                &ready_public_config(),
                true,
                super::MeetingCapture {
                    exe: &missing,
                    pid: 4242,
                    enumerator: &enumerator,
                },
                false,
            )
            .expect_err("missing exe must fail");
        assert_eq!(error.code(), "SESSION_SIDECAR_MISSING");
        assert!(SessionStore::new(&database).list().unwrap().is_empty());
        assert_eq!(service.phase(), SessionPhase::Idle);
    }

    #[test]
    fn second_start_while_active_returns_already_active() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        start_ready(&mut service, &database);

        let error = service
            .start(&database, &ready_public_config(), true, false)
            .expect_err("second start");
        assert_eq!(error.code(), "SESSION_ALREADY_ACTIVE");
        assert!(matches!(error, SessionServiceError::AlreadyActive));
        assert_eq!(SessionStore::new(&database).list().unwrap().len(), 1);
        assert_eq!(service.phase(), SessionPhase::Listening);
    }

    #[test]
    fn finalize_text_persists_turn_and_returns_to_listening() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("助手回复");
        let tts = ScriptedTts::ok(&[0x01, 0x02]);
        let embed = UnusedEmbed;
        let probes = SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };

        let turn = service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &probes,
                credentials(),
                Some("你好"),
            )
            .unwrap()
            .expect("turn");

        assert_eq!(turn.user_text, "你好");
        assert_eq!(turn.assistant_text, "助手回复");
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.phase(), SessionPhase::Listening);

        let stored = SessionStore::new(&database).list_turns(&id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].user_text, "你好");
        assert_eq!(stored[0].assistant_text, "助手回复");
        assert!(!stored[0].materials_used);
    }

    #[test]
    fn candidate_suggestion_never_plays_before_user_confirmation() {
        let (_directory, database) = opened();
        let mut config = ready_public_config();
        config.role_profiles[0].id = "personal-candidate".into();
        config.role_profiles[0].scenario = Some(crate::config::RoleScenario::Candidate);
        config.active_role_profile_id = Some("personal-candidate".into());
        let mut service = SessionService::with_sink(RecordingSink::default());
        match service.start(&database, &config, true, false).unwrap() {
            SessionStartOutcome::Started { .. } => {}
            SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
        }
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("建议回答");
        let tts = ScriptedTts::ok(&[1, 2, 3, 4]);
        let embed = UnusedEmbed;
        let probes = SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("面试问题"))
            .unwrap();
        assert!(
            service.sink().recorded().is_empty(),
            "candidate audio played without confirmation"
        );
        let id = service.session_id().unwrap();
        let meta = SessionStore::new(&database)
            .list_events(id)
            .unwrap()
            .into_iter()
            .find(|event| event.kind == "turn_meta")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&meta.payload).unwrap();
        assert_eq!(payload["userConfirmed"], false);
        assert_eq!(payload["playbackStatus"], "pending_confirmation");

        let say = service
            .execute_command(
                &database,
                &config,
                &SessionProbes {
                    asr: &asr,
                    llm: &llm,
                    tts: &tts,
                    embed: &embed,
                    realtime: &UnusedRealtime,
                },
                credentials(),
                parse_cmd(serde_json::json!({
                    "v": 1,
                    "id": "candidate-say-bypass",
                    "action": "say",
                    "text": "绕过确认"
                })),
            )
            .unwrap();
        assert!(!say.ok);
        assert!(service.sink().recorded().is_empty());
    }

    #[test]
    fn candidate_confirmation_plays_only_the_current_edited_answer() {
        let (_directory, database) = opened();
        let mut config = ready_public_config();
        config.role_profiles[0].id = "preset-candidate".into();
        config.active_role_profile_id = Some("preset-candidate".into());
        let mut service = SessionService::with_sink(RecordingSink::default());
        match service.start(&database, &config, true, false).unwrap() {
            SessionStartOutcome::Started { .. } => {}
            SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
        }
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("初始建议");
        let tts = ScriptedTts::ok(&[9, 8, 7]);
        let embed = UnusedEmbed;
        let probes = cascaded_probes(&asr, &llm, &tts, &embed);
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("面试问题"))
            .unwrap();

        let outcome = service
            .execute_command(
                &database,
                &config,
                &probes,
                credentials(),
                parse_cmd(serde_json::json!({
                    "v": 1,
                    "id": "confirm-current",
                    "action": "confirm_candidate",
                    "text": "编辑后的回答",
                    "expectedRevision": service.revision()
                })),
            )
            .expect("confirm candidate answer");

        assert!(outcome.ok);
        assert_eq!(service.sink().recorded(), [9, 8, 7]);
        let id = service.session_id().unwrap();
        let meta = SessionStore::new(&database)
            .list_events(id)
            .unwrap()
            .into_iter()
            .rev()
            .find(|event| event.kind == "turn_meta")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&meta.payload).unwrap();
        assert_eq!(payload["userConfirmed"], true);
        assert_eq!(payload["playbackStatus"], "played");
    }

    #[test]
    fn candidate_confirmation_cannot_be_revived_after_takeover_and_resume() {
        let (_directory, database) = opened();
        let mut config = ready_public_config();
        config.role_profiles[0].id = "preset-candidate".into();
        config.active_role_profile_id = Some("preset-candidate".into());
        let mut service = SessionService::with_sink(RecordingSink::default());
        service.start(&database, &config, true, false).unwrap();
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("old suggestion");
        let tts = ScriptedTts::ok(&[9, 8, 7]);
        let embed = UnusedEmbed;
        let probes = cascaded_probes(&asr, &llm, &tts, &embed);
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
            .unwrap();
        service
            .set_mode(&database, AgentMode::OperatorSpeaking)
            .unwrap();
        service.set_mode(&database, AgentMode::AiActive).unwrap();
        let result = service.execute_command(&database, &config, &probes, credentials(), parse_cmd(serde_json::json!({
            "v": 1, "id": "revive-old", "action": "confirm_candidate", "text": "old answer", "expectedRevision": service.revision()
        }))).unwrap();
        assert!(
            !result.ok,
            "takeover must permanently invalidate the old confirmation"
        );
        assert!(service.sink().recorded().is_empty());
        let meta = latest_turn_meta(&database, service.session_id().unwrap());
        assert_eq!(meta["playbackStatus"], "superseded");
    }

    fn latest_turn_meta(
        database: &crate::database::Database,
        session_id: &str,
    ) -> serde_json::Value {
        SessionStore::new(database)
            .list_events(session_id)
            .unwrap()
            .into_iter()
            .rev()
            .find(|event| event.kind == "turn_meta")
            .map(|event| serde_json::from_str(&event.payload).unwrap())
            .unwrap()
    }

    fn start_candidate(
        service: &mut SessionService<RecordingSink>,
        database: &crate::database::Database,
    ) -> crate::config::PublicConfig {
        let mut config = ready_public_config();
        config.role_profiles[0].id = "preset-candidate".into();
        config.active_role_profile_id = Some("preset-candidate".into());
        service.start(database, &config, true, false).unwrap();
        config
    }

    #[test]
    fn candidate_confirmation_is_superseded_when_a_new_question_fails() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        let config = start_candidate(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("old suggestion");
        let tts = ScriptedTts::ok(&[9, 8, 7]);
        let embed = UnusedEmbed;
        let probes = cascaded_probes(&asr, &llm, &tts, &embed);
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
            .unwrap();
        let fail = FailingLlm::new(crate::providers::CascadeError::RequestFailed(
            crate::providers::CascadeStage::Llm,
        ));
        let failed = SessionProbes {
            asr: &asr,
            llm: &fail,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        assert!(
            service
                .finalize_utterance(&database, &config, &failed, credentials(), Some("next"))
                .is_err()
        );
        let meta = latest_turn_meta(&database, service.session_id().unwrap());
        assert_eq!(meta["playbackStatus"], "superseded");
        let result = service.execute_command(&database, &config, &probes, credentials(), parse_cmd(serde_json::json!({
            "v": 1, "id": "after-fail", "action": "confirm_candidate", "text": "old answer", "expectedRevision": service.revision()
        }))).unwrap();
        assert!(!result.ok);
        assert!(service.sink().recorded().is_empty());
    }

    #[test]
    fn candidate_confirmation_is_superseded_by_pause_mute_and_stop() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        let config = start_candidate(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("old suggestion");
        let tts = ScriptedTts::ok(&[9, 8, 7]);
        let embed = UnusedEmbed;
        let probes = cascaded_probes(&asr, &llm, &tts, &embed);
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
            .unwrap();
        service.set_mode(&database, AgentMode::Paused).unwrap();
        assert_eq!(
            latest_turn_meta(&database, service.session_id().unwrap())["playbackStatus"],
            "superseded"
        );
        let result = service.execute_command(&database, &config, &probes, credentials(), parse_cmd(serde_json::json!({
            "v": 1, "id": "after-pause", "action": "confirm_candidate", "text": "old answer", "expectedRevision": service.revision()
        }))).unwrap();
        assert!(!result.ok);

        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        let config = start_candidate(&mut service, &database);
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
            .unwrap();
        service.set_mode(&database, AgentMode::Muted).unwrap();
        assert_eq!(
            latest_turn_meta(&database, service.session_id().unwrap())["playbackStatus"],
            "superseded"
        );

        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        let config = start_candidate(&mut service, &database);
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
            .unwrap();
        service.stop(&database).unwrap();
        assert_eq!(
            latest_turn_meta(&database, service.session_id().unwrap())["playbackStatus"],
            "superseded"
        );
    }

    #[test]
    fn candidate_late_response_after_stop_does_not_restore_confirmation() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        let config = start_candidate(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let first = ScriptedLlm::ok("old suggestion");
        let tts = ScriptedTts::ok(&[9, 8, 7]);
        let embed = UnusedEmbed;
        service
            .finalize_utterance(
                &database,
                &config,
                &cascaded_probes(&asr, &first, &tts, &embed),
                credentials(),
                Some("question"),
            )
            .unwrap();
        let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let proceed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let gated = GateLlm {
            reply: "late".into(),
            entered: std::sync::Arc::clone(&entered),
            proceed: std::sync::Arc::clone(&proceed),
            calls: AtomicU32::new(0),
        };
        let control = service.control();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !entered.load(std::sync::atomic::Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                control.request_stop();
                proceed.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            let _ = service.finalize_utterance(
                &database,
                &config,
                &SessionProbes {
                    asr: &asr,
                    llm: &gated,
                    tts: &tts,
                    embed: &embed,
                    realtime: &UnusedRealtime,
                },
                credentials(),
                Some("next"),
            );
        });
        let status = latest_turn_meta(&database, service.session_id().unwrap())["playbackStatus"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_ne!(status, "pending_confirmation");
        let result = service.execute_command(&database, &config, &cascaded_probes(&asr, &first, &tts, &embed), credentials(), parse_cmd(serde_json::json!({
            "v": 1, "id": "late-stop", "action": "confirm_candidate", "text": "late", "expectedRevision": service.revision()
        }))).unwrap();
        assert!(!result.ok);
        assert!(service.sink().recorded().is_empty());
    }

    #[test]
    fn meeting_assistant_only_transcribes_ordinary_meeting_discussion() {
        let (_directory, database) = opened();
        let mut config = ready_public_config();
        config.role_profiles[0].id = "personal-meeting-assistant".into();
        config.role_profiles[0].name = "小助理".into();
        config.role_profiles[0].scenario = Some(crate::config::RoleScenario::MeetingAssistant);
        config.active_role_profile_id = Some("personal-meeting-assistant".into());
        let mut service = SessionService::with_sink(RecordingSink::default());
        match service.start(&database, &config, true, false).unwrap() {
            SessionStartOutcome::Started { .. } => {}
            SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
        }
        service.capture_mut().mark_meeting_bridge_for_tests();
        service.push_pcm(&[1, 0, 2, 0, 3, 0]);
        let asr = ScriptedAsr::ok("今天讨论项目进度");
        let llm = ScriptedLlm::ok("不应生成回复");
        let tts = ScriptedTts::ok(&[1, 2]);
        let embed = UnusedEmbed;

        let turn = service
            .finalize_utterance(
                &database,
                &config,
                &cascaded_probes(&asr, &llm, &tts, &embed),
                credentials(),
                None,
            )
            .unwrap()
            .expect("transcript turn");

        assert_eq!(turn.user_text, "今天讨论项目进度");
        assert_eq!(turn.assistant_text, "");
        assert_eq!(asr.calls.load(Ordering::SeqCst), 1);
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
        assert!(service.sink().recorded().is_empty());
    }

    #[test]
    fn meeting_assistant_answers_voice_on_local_microphone() {
        let (_directory, database) = opened();
        let mut config = ready_public_config();
        config.role_profiles[0].id = "personal-meeting-assistant".into();
        config.role_profiles[0].name = "小助理".into();
        config.role_profiles[0].scenario = Some(crate::config::RoleScenario::MeetingAssistant);
        config.active_role_profile_id = Some("personal-meeting-assistant".into());
        let mut service = SessionService::with_sink(RecordingSink::default());
        match service.start(&database, &config, true, false).unwrap() {
            SessionStartOutcome::Started { .. } => {}
            SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
        }
        service.push_pcm(&[1, 0, 2, 0, 3, 0]);
        let asr = ScriptedAsr::ok("反应有点慢");
        let llm = ScriptedLlm::ok("好的，我会加快响应");
        let tts = ScriptedTts::ok(&[3, 4]);
        let embed = UnusedEmbed;

        let turn = service
            .finalize_utterance(
                &database,
                &config,
                &cascaded_probes(&asr, &llm, &tts, &embed),
                credentials(),
                None,
            )
            .unwrap()
            .expect("answer turn");

        assert_eq!(turn.assistant_text, "好的，我会加快响应");
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        assert_eq!(tts.calls.load(Ordering::SeqCst), 1);
        assert_eq!(service.sink().recorded(), [3, 4]);
    }

    #[test]
    fn meeting_assistant_hotkey_forces_one_answer_and_records_trigger() {
        let (_directory, database) = opened();
        let mut config = ready_public_config();
        config.role_profiles[0].scenario = Some(crate::config::RoleScenario::MeetingAssistant);
        let mut service = SessionService::with_sink(RecordingSink::default());
        start_ready_sink(&mut service, &database, &config);
        service.push_pcm(&[1, 0, 2, 0, 3, 0]);
        let asr = ScriptedAsr::ok("今天讨论项目进度");
        let llm = ScriptedLlm::ok("当前进度正常");
        let tts = ScriptedTts::ok(&[7, 8]);
        let embed = UnusedEmbed;

        let turn = service
            .finalize_utterance_forced(
                &database,
                &config,
                &cascaded_probes(&asr, &llm, &tts, &embed),
                credentials(),
            )
            .unwrap()
            .expect("hotkey turn");

        assert_eq!(turn.assistant_text, "当前进度正常");
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        let event = SessionStore::new(&database)
            .list_events(service.session_id().unwrap())
            .unwrap()
            .into_iter()
            .rev()
            .find(|event| event.kind == "turn_meta")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
        assert_eq!(payload["triggerSource"], "hotkey");
    }

    #[test]
    fn meeting_assistant_answers_when_its_configured_name_is_mentioned() {
        let (_directory, database) = opened();
        let mut config = ready_public_config();
        config.role_profiles[0].name = "小助理".into();
        config.role_profiles[0].scenario = Some(crate::config::RoleScenario::MeetingAssistant);
        let mut service = SessionService::with_sink(RecordingSink::default());
        start_ready_sink(&mut service, &database, &config);
        service.push_pcm(&[1, 0, 2, 0, 3, 0]);
        let asr = ScriptedAsr::ok("小助理，请总结刚才的结论");
        let llm = ScriptedLlm::ok("结论是按计划推进");
        let tts = ScriptedTts::ok(&[4, 5, 6]);
        let embed = UnusedEmbed;

        let turn = service
            .finalize_utterance(
                &database,
                &config,
                &cascaded_probes(&asr, &llm, &tts, &embed),
                credentials(),
                None,
            )
            .unwrap()
            .expect("answer turn");

        assert_eq!(turn.assistant_text, "结论是按计划推进");
        assert_eq!(asr.calls.load(Ordering::SeqCst), 1);
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        assert_eq!(service.sink().recorded(), [4, 5, 6]);
    }

    #[test]
    fn finalize_injected_pcm_uses_asr() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        start_ready(&mut service, &database);
        service.push_pcm(&[0x10, 0x00, 0x20, 0x00, 0x30, 0x00]);
        let asr = ScriptedAsr::ok("从音频来");
        let llm = ScriptedLlm::ok("收到");
        let tts = ScriptedTts::ok(&[0x03, 0x04]);
        let embed = UnusedEmbed;
        let probes = SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };

        let turn = service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &probes,
                credentials(),
                None,
            )
            .unwrap()
            .expect("turn");

        assert_eq!(turn.user_text, "从音频来");
        assert_eq!(asr.calls.load(Ordering::SeqCst), 1);
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        assert_eq!(service.phase(), SessionPhase::Listening);
    }

    #[test]
    fn takeover_cancels_sink_and_skips_llm() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        let id = start_ready_sink(&mut service, &database, &ready_public_config());
        service
            .set_mode(&database, AgentMode::OperatorSpeaking)
            .unwrap();

        assert_eq!(service.mode(), AgentMode::OperatorSpeaking);
        assert!(!service.runtime_can_answer());
        assert!(service.sink().cancelled());

        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("should-not-run");
        let tts = ScriptedTts::ok(&[0x09]);
        let embed = UnusedEmbed;
        let probes = SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        let turn = service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &probes,
                credentials(),
                Some("接管后提问"),
            )
            .unwrap();

        assert!(turn.is_none());
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        assert!(
            SessionStore::new(&database)
                .list_turns(&id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(service.phase(), SessionPhase::Listening);
    }

    #[test]
    fn stop_marks_completed_and_sets_finished_at() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);

        let row = service.stop(&database).unwrap();
        assert_eq!(row.status, "completed");
        assert!(row.finished_at.is_some());
        assert_eq!(service.phase(), SessionPhase::Completed);

        let stored = SessionStore::new(&database).get(&id).unwrap().unwrap();
        assert_eq!(stored.status, "completed");
        assert!(stored.finished_at.is_some());
    }

    #[test]
    fn start_after_stop_opens_a_new_session() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let first = start_ready(&mut service, &database);
        service.stop(&database).unwrap();
        let second = start_ready(&mut service, &database);
        assert_ne!(first, second);
        assert_eq!(service.phase(), SessionPhase::Listening);
        assert_eq!(SessionStore::new(&database).list().unwrap().len(), 2);
    }

    #[test]
    fn empty_voice_id_defaults_to_alloy() {
        let (_directory, database) = opened();
        let mut config = ready_public_config();
        config.speech.voice_routes[0].voice_id = Some(String::new());
        let mut service = SessionService::new();
        match service.start(&database, &config, true, false).unwrap() {
            SessionStartOutcome::Started { .. } => {}
            SessionStartOutcome::Blocked { issues } => panic!("blocked {issues:?}"),
        }
        let asr = ScriptedAsr::ok("hi");
        let llm = ScriptedLlm::ok("ok");
        let tts = ScriptedTts::ok(&[0x11, 0x22]);
        let embed = UnusedEmbed;
        let probes = SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };

        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("hi"))
            .unwrap()
            .expect("turn");
        assert_eq!(
            tts.voices.lock().expect("voices").as_slice(),
            ["alloy".to_string()]
        );
    }

    #[test]
    fn finalize_llm_error_returns_to_listening_and_second_finalize_succeeds() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let fail_llm = FailingLlm::new(CascadeError::RequestFailed(
            crate::providers::CascadeStage::Llm,
        ));
        let tts = ScriptedTts::ok(&[0x01]);
        let embed = UnusedEmbed;
        let fail_probes = SessionProbes {
            asr: &asr,
            llm: &fail_llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };

        let error = service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &fail_probes,
                credentials(),
                Some("第一轮"),
            )
            .expect_err("llm fail");
        assert_eq!(error.code(), "LLM_REQUEST_FAILED");
        assert_eq!(service.last_error_code(), Some("LLM_REQUEST_FAILED"));
        assert_eq!(service.phase(), SessionPhase::Listening);
        let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
        assert_eq!(row.status, "listening");
        assert!(
            SessionStore::new(&database)
                .list_turns(&id)
                .unwrap()
                .is_empty()
        );

        let ok_llm = ScriptedLlm::ok("第二轮回复");
        let ok_probes = SessionProbes {
            asr: &asr,
            llm: &ok_llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        let turn = service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &ok_probes,
                credentials(),
                Some("第二轮"),
            )
            .unwrap()
            .expect("second turn");
        assert_eq!(turn.assistant_text, "第二轮回复");
        assert_eq!(service.phase(), SessionPhase::Listening);
        assert_eq!(service.last_error_code(), None);
        assert_eq!(ok_llm.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn finalize_unauthorized_fails_session() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let llm = FailingLlm::new(CascadeError::Unauthorized(
            crate::providers::CascadeStage::Llm,
        ));
        let tts = ScriptedTts::ok(&[0x01]);
        let embed = UnusedEmbed;
        let probes = SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };

        let error = service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &probes,
                credentials(),
                Some("密钥失效"),
            )
            .expect_err("unauthorized");
        assert_eq!(error.code(), "LLM_UNAUTHORIZED");
        assert_eq!(service.last_error_code(), Some("LLM_UNAUTHORIZED"));
        assert_eq!(service.phase(), SessionPhase::Failed);
        let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert!(row.finished_at.is_some());
    }

    #[test]
    fn request_cancel_during_slow_llm_skips_tts() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        start_ready(&mut service, &database);
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let proceed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let asr = ScriptedAsr::ok("ignored");
        let llm = GateLlm {
            reply: "不应播报".into(),
            entered: Arc::clone(&entered),
            proceed: Arc::clone(&proceed),
            calls: AtomicU32::new(0),
        };
        let tts = ScriptedTts::ok(&[0x22]);
        let embed = UnusedEmbed;
        let probes = SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        let control = service.control();
        let waiter = std::thread::spawn(move || {
            while !entered.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            control.request_cancel();
            proceed.store(true, Ordering::SeqCst);
        });

        let error = service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &probes,
                credentials(),
                Some("取消"),
            )
            .expect_err("cancelled");
        waiter.join().expect("cancel thread");
        assert_eq!(error.code(), "SESSION_CANCELLED");
        assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
        assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.phase(), SessionPhase::Listening);
        assert_eq!(service.last_error_code(), Some("SESSION_CANCELLED"));
    }

    #[test]
    fn sidecar_second_crash_fails_session() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);

        service.capture().mark_sidecar_exited();
        assert_eq!(service.poll_sidecar(&database).unwrap(), SidecarPoll::Alive);
        service.capture().mark_sidecar_exited();
        let error = service.poll_sidecar(&database).expect_err("second crash");
        assert_eq!(error.code(), "SESSION_SIDECAR_FAILED");
        assert_eq!(service.phase(), SessionPhase::Failed);
        let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert!(row.finished_at.is_some());
    }

    #[test]
    fn stop_after_sidecar_crash_keeps_failed_status() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);

        service.capture().mark_sidecar_exited();
        assert_eq!(service.poll_sidecar(&database).unwrap(), SidecarPoll::Alive);
        service.capture().mark_sidecar_exited();
        let error = service.poll_sidecar(&database).expect_err("second crash");
        assert_eq!(error.code(), "SESSION_SIDECAR_FAILED");
        assert_eq!(service.phase(), SessionPhase::Failed);

        let row = service.stop(&database).unwrap();
        assert_eq!(row.status, "failed");
        assert!(row.finished_at.is_some());
        assert_eq!(service.phase(), SessionPhase::Failed);

        let stored = SessionStore::new(&database).get(&id).unwrap().unwrap();
        assert_eq!(stored.status, "failed");
        assert!(stored.finished_at.is_some());
    }

    #[test]
    fn app_state_open_marks_leftover_sessions_interrupted_without_resume() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        };
        std::fs::write(&paths.config_path, r#"{"configVersion":1}"#).unwrap();
        std::fs::create_dir_all(&paths.data_directory).unwrap();
        let db_path = paths.data_directory.join("app.sqlite3");
        {
            let database = Database::open(&db_path).unwrap();
            database.migrate().unwrap();
            SessionStore::new(&database)
                .insert_session(NewSession {
                    id: "leftover",
                    status: "listening",
                    role_profile_id: "role-1",
                    voice_route_id: "route-1",
                    transport_mode: "direct",
                })
                .unwrap();
        }

        let state = AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap();
        let guard = state.database.lock().expect("db");
        let database = guard.as_ref().expect("opened");
        let leftover = SessionStore::new(database)
            .get("leftover")
            .unwrap()
            .expect("row");
        assert_eq!(leftover.status, "interrupted");
        assert!(leftover.finished_at.is_some());

        let service = SessionService::new();
        assert_eq!(service.phase(), SessionPhase::Idle);
        assert_eq!(
            service.capture().poll_sidecar().unwrap(),
            SidecarPoll::Alive
        );
        assert_eq!(service.capture().snapshot_48k().len(), 0);
        assert!(service.session_id().is_none());
    }

    fn start_e2e(service: &mut SessionService, database: &Database) -> String {
        match service
            .start(database, &ready_e2e_public_config(), true, false)
            .unwrap()
        {
            SessionStartOutcome::Started { session } => session.id,
            SessionStartOutcome::Blocked { issues } => {
                panic!("expected e2e start, blocked {issues:?}")
            }
        }
    }

    fn e2e_probes<'a>(
        asr: &'a ScriptedAsr,
        llm: &'a ScriptedLlm,
        tts: &'a ScriptedTts,
        embed: &'a UnusedEmbed,
        realtime: &'a FakeRealtime,
    ) -> SessionProbes<'a> {
        SessionProbes {
            asr,
            llm,
            tts,
            embed,
            realtime,
        }
    }

    #[test]
    fn e2e_start_keeps_direct_transport_and_preflight_passes() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_e2e(&mut service, &database);
        assert_eq!(service.phase(), SessionPhase::Listening);
        let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
        assert_eq!(row.transport_mode, "direct");
        assert_eq!(row.status, "listening");
        let snapshots = SessionStore::new(&database).list_snapshots(&id).unwrap();
        assert_eq!(snapshots[0].transport_mode, "direct");
        assert!(snapshots[0].provider_ids.contains("e2e-1"));
        assert!(snapshots[0].model_ids.contains("gpt-realtime"));
    }

    #[test]
    fn e2e_fake_realtime_turn_persists_and_sets_unused_materials() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_e2e(&mut service, &database);
        service.push_pcm(&[0x10, 0x00, 0x20, 0x00]);
        let asr = ScriptedAsr::ok("should-not-run");
        let llm = ScriptedLlm::ok("should-not-run");
        let tts = ScriptedTts::ok(&[0x99]);
        let embed = UnusedEmbed;
        let realtime = FakeRealtime::ok("你好", "实时回复", &[0x01, 0x02]);
        let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

        let turn = service
            .finalize_utterance(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                None,
            )
            .unwrap()
            .expect("turn");

        assert_eq!(turn.user_text, "你好");
        assert_eq!(turn.assistant_text, "实时回复");
        assert_eq!(turn.tts_pcm, [0x01, 0x02]);
        assert!(!turn.materials_used);
        assert_eq!(realtime.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            realtime.model_id.lock().expect("model").as_deref(),
            Some("gpt-realtime")
        );
        assert_eq!(
            realtime.pcm.lock().expect("pcm").as_slice(),
            service.capture().pcm_for_asr().as_slice()
        );
        assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.phase(), SessionPhase::Listening);
        assert!(service.unused_materials());

        let stored = SessionStore::new(&database).list_turns(&id).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].user_text, "你好");
        assert_eq!(stored[0].assistant_text, "实时回复");
        assert!(!stored[0].materials_used);
    }

    #[test]
    fn e2e_cancel_between_realtime_and_persist_returns_to_listening() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_e2e(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("ignored");
        let tts = ScriptedTts::ok(&[0x01]);
        let embed = UnusedEmbed;
        let realtime = FakeRealtime::cancel_after_turn("取消前", "不应落库");
        let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

        let error = service
            .finalize_utterance(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                None,
            )
            .expect_err("cancelled");
        assert_eq!(error.code(), "SESSION_CANCELLED");
        assert_eq!(service.last_error_code(), Some("SESSION_CANCELLED"));
        assert_eq!(service.phase(), SessionPhase::Listening);
        assert!(
            SessionStore::new(&database)
                .list_turns(&id)
                .unwrap()
                .is_empty()
        );
        assert_eq!(realtime.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn e2e_typed_message_uses_text_instead_of_empty_audio() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_e2e(&mut service, &database);
        let asr = ScriptedAsr::ok("unused");
        let llm = ScriptedLlm::ok("unused");
        let tts = ScriptedTts::ok(&[1]);
        let embed = UnusedEmbed;
        let realtime = FakeRealtime::ok("", "你好", &[1, 2]);
        let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);
        let turn = service
            .finalize_utterance(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                Some("你好啊"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            realtime.calls.load(Ordering::SeqCst),
            0,
            "typed input must not submit empty PCM"
        );
        assert_eq!(realtime.text_calls.load(Ordering::SeqCst), 1);
        assert_eq!(turn.user_text, "你好啊");
        assert_eq!(
            SessionStore::new(&database).list_turns(&id).unwrap().len(),
            1
        );
    }

    #[test]
    fn e2e_connection_failure_can_retry_without_creating_a_phantom_turn() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_e2e(&mut service, &database);
        let asr = ScriptedAsr::ok("unused");
        let llm = ScriptedLlm::ok("unused");
        let tts = ScriptedTts::ok(&[1]);
        let embed = UnusedEmbed;
        let realtime = FakeRealtime::ok("", "你好", &[1, 2]);
        *realtime.error.lock().unwrap() = Some(RealtimeError::ConnectionClosed);
        let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);
        let config = ready_e2e_public_config();
        assert!(
            service
                .finalize_utterance(&database, &config, &probes, credentials(), Some("你好啊"))
                .is_err()
        );
        assert_eq!(service.phase(), SessionPhase::Listening);
        assert!(
            SessionStore::new(&database)
                .list_turns(&id)
                .unwrap()
                .is_empty()
        );
        assert!(
            service
                .finalize_utterance(&database, &config, &probes, credentials(), Some("你好啊"))
                .unwrap()
                .is_some()
        );
        assert_eq!(service.last_error_code(), None);
        assert_eq!(
            SessionStore::new(&database).list_turns(&id).unwrap().len(),
            1
        );
    }

    #[test]
    fn e2e_unauthorized_fails_session() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_e2e(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("ignored");
        let tts = ScriptedTts::ok(&[0x01]);
        let embed = UnusedEmbed;
        let realtime = FakeRealtime::fail(RealtimeError::Unauthorized);
        let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

        let error = service
            .finalize_utterance(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                None,
            )
            .expect_err("unauthorized");
        assert_eq!(error.code(), "REALTIME_UNAUTHORIZED");
        assert_eq!(service.last_error_code(), Some("REALTIME_UNAUTHORIZED"));
        assert_eq!(service.phase(), SessionPhase::Failed);
        let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert!(row.finished_at.is_some());
    }

    #[test]
    fn e2e_sends_role_and_materials_before_generate() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        let path = directory.path().join("note.txt");
        std::fs::write(&path, "负责订单服务与 Kafka 链路，完整句子用于检索。").unwrap();
        crate::services::MaterialService::new(&database, directory.path())
            .import_file(&path)
            .unwrap();

        let mut service = SessionService::new();
        match service
            .start(&database, &ready_e2e_public_config(), true, false)
            .unwrap()
        {
            SessionStartOutcome::Started { .. } => {}
            SessionStartOutcome::Blocked { issues } => panic!("blocked {issues:?}"),
        }
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("ignored");
        let tts = ScriptedTts::ok(&[0x01]);
        let embed = UnusedEmbed;
        let realtime = FakeRealtime::ok("ignored", "资料回答", &[0x02]);
        let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

        let turn = service
            .finalize_utterance(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                Some("请介绍你做过的订单服务项目"),
            )
            .unwrap()
            .expect("turn");
        assert!(turn.materials_used);
        assert!(!service.unused_materials());
        assert!(
            turn.citations
                .iter()
                .any(|citation| citation.snippet.contains("订单服务"))
        );
        let instructions = realtime
            .instructions
            .lock()
            .expect("instructions")
            .clone()
            .expect("realtime must receive instructions before generate");
        assert!(instructions.contains("UNIQUE_PROMPT_BODY_DO_NOT_SNAPSHOT"));
        assert!(instructions.contains("UNIQUE_STYLE_DO_NOT_SNAPSHOT"));
        assert!(instructions.contains("订单服务"));
        assert_eq!(realtime.text_calls.load(Ordering::SeqCst), 1);
        assert_eq!(realtime.calls.load(Ordering::SeqCst), 0);
        assert!(realtime.sample_rate.lock().expect("sample_rate").is_none());
    }

    #[test]
    fn e2e_voice_only_does_not_mark_materials_used_after_the_fact() {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        let path = directory.path().join("note.txt");
        std::fs::write(&path, "负责订单服务与 Kafka 链路，完整句子用于检索。").unwrap();
        crate::services::MaterialService::new(&database, directory.path())
            .import_file(&path)
            .unwrap();

        let mut service = SessionService::new();
        match service
            .start(&database, &ready_e2e_public_config(), true, false)
            .unwrap()
        {
            SessionStartOutcome::Started { .. } => {}
            SessionStartOutcome::Blocked { issues } => panic!("blocked {issues:?}"),
        }
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("ignored");
        let tts = ScriptedTts::ok(&[0x01]);
        let embed = UnusedEmbed;
        let realtime = FakeRealtime::ok("订单服务", "资料回答", &[0x02]);
        let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

        let turn = service
            .finalize_utterance(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                None,
            )
            .unwrap()
            .expect("turn");
        assert!(!turn.materials_used);
        assert!(turn.citations.is_empty());
        let instructions = realtime
            .instructions
            .lock()
            .expect("instructions")
            .clone()
            .unwrap();
        assert!(instructions.contains("UNIQUE_PROMPT_BODY_DO_NOT_SNAPSHOT"));
        assert!(!instructions.contains("可用资料"));
    }

    fn cascaded_probes<'a>(
        asr: &'a ScriptedAsr,
        llm: &'a ScriptedLlm,
        tts: &'a ScriptedTts,
        embed: &'a UnusedEmbed,
    ) -> SessionProbes<'a> {
        SessionProbes {
            asr,
            llm,
            tts,
            embed,
            realtime: &UnusedRealtime,
        }
    }

    fn parse_cmd(payload: serde_json::Value) -> crate::runtime::AgentCommand {
        crate::runtime::parse_agent_command(&payload).expect("command")
    }

    #[test]
    fn say_speaks_via_tts_and_result_omits_pcm() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        start_ready_sink(&mut service, &database, &ready_public_config());
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("should-not-run");
        let tts = ScriptedTts::ok(&[0x11, 0x22]);
        let embed = UnusedEmbed;
        let probes = cascaded_probes(&asr, &llm, &tts, &embed);

        let outcome = service
            .execute_command(
                &database,
                &ready_public_config(),
                &probes,
                credentials(),
                parse_cmd(serde_json::json!({
                    "v": 1,
                    "id": "cmd-say",
                    "action": "say",
                    "text": "请开始"
                })),
            )
            .expect("say");

        assert!(outcome.ok);
        assert_eq!(outcome.command_id, "cmd-say");
        assert_eq!(outcome.action.as_str(), "say");
        assert_eq!(outcome.result["text"], "请开始");
        assert_eq!(outcome.error, "");
        assert_eq!(tts.calls.load(Ordering::SeqCst), 1);
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.sink().recorded(), [0x11, 0x22]);
        let json = serde_json::to_string(&outcome.result).unwrap();
        assert!(!json.contains("pcm"));
        assert!(!json.to_ascii_lowercase().contains("sk-"));
    }

    #[test]
    fn retry_replaces_last_assistant_and_revision_mismatch_fails_closed() {
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let first = ScriptedLlm::ok("原回复");
        let tts = ScriptedTts::ok(&[0x01]);
        let embed = UnusedEmbed;
        service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &cascaded_probes(&asr, &first, &tts, &embed),
                credentials(),
                Some("你好"),
            )
            .unwrap()
            .expect("turn");
        assert_eq!(service.revision(), 1);

        let stale = service
            .execute_command(
                &database,
                &ready_public_config(),
                &cascaded_probes(&asr, &first, &tts, &embed),
                credentials(),
                parse_cmd(serde_json::json!({
                    "v": 1,
                    "id": "cmd-stale",
                    "action": "retry",
                    "expectedRevision": 0
                })),
            )
            .expect("stale outcome");
        assert!(!stale.ok);
        assert_eq!(stale.error, "SESSION_CHANGED");
        assert_eq!(
            SessionStore::new(&database).list_turns(&id).unwrap()[0].assistant_text,
            "原回复"
        );

        let retry_llm = ScriptedLlm::ok("新回复");
        let outcome = service
            .execute_command(
                &database,
                &ready_public_config(),
                &cascaded_probes(&asr, &retry_llm, &tts, &embed),
                credentials(),
                parse_cmd(serde_json::json!({
                    "v": 1,
                    "id": "cmd-retry",
                    "action": "retry",
                    "expectedRevision": 1
                })),
            )
            .expect("retry");
        assert!(outcome.ok, "{outcome:?}");
        assert_eq!(outcome.result["question"], "新回复");
        assert_eq!(service.revision(), 2);
        assert_eq!(
            SessionStore::new(&database).list_turns(&id).unwrap()[0].assistant_text,
            "新回复"
        );
    }

    #[test]
    fn correct_replaces_last_assistant_and_speaks_given_text() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        let id = start_ready_sink(&mut service, &database, &ready_public_config());
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("原回复");
        let tts = ScriptedTts::ok(&[0x33]);
        let embed = UnusedEmbed;
        service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &cascaded_probes(&asr, &llm, &tts, &embed),
                credentials(),
                Some("你好"),
            )
            .unwrap();

        let outcome = service
            .execute_command(
                &database,
                &ready_public_config(),
                &cascaded_probes(&asr, &llm, &tts, &embed),
                credentials(),
                parse_cmd(serde_json::json!({
                    "v": 1,
                    "id": "cmd-correct",
                    "action": "correct",
                    "answer": "改成这句",
                    "expectedRevision": 1
                })),
            )
            .expect("correct");
        assert!(outcome.ok);
        assert_eq!(outcome.result["answer"], "改成这句");
        assert_eq!(
            SessionStore::new(&database).list_turns(&id).unwrap()[0].assistant_text,
            "改成这句"
        );
        assert!(service.sink().recorded().ends_with(&[0x33]));
        assert_eq!(service.revision(), 2);
    }

    #[test]
    fn report_returns_summary_without_speaking_or_secrets() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        start_ready_sink(&mut service, &database, &ready_public_config());
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok(
            r#"{"summary":"短纪要","strengths":[],"followUps":[],"limitations":[],"evidence":[]}"#,
        );
        let tts = ScriptedTts::ok(&[0x44]);
        let embed = UnusedEmbed;
        let before = service.sink().recorded().len();
        let outcome = service
            .execute_command(
                &database,
                &ready_public_config(),
                &cascaded_probes(&asr, &llm, &tts, &embed),
                credentials(),
                parse_cmd(serde_json::json!({
                    "v": 1,
                    "id": "cmd-report",
                    "action": "report"
                })),
            )
            .expect("report");
        assert!(outcome.ok);
        assert_eq!(outcome.result["summary"], "短纪要");
        assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.sink().recorded().len(), before);
        let json = serde_json::to_string(&outcome.result).unwrap();
        assert!(!json.contains("pcm"));
        assert!(!json.to_ascii_lowercase().contains("sk-"));
    }

    #[test]
    fn e2e_say_uses_realtime_text_turn() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        start_ready_sink(&mut service, &database, &ready_e2e_public_config());
        let asr = ScriptedAsr::ok("should-not-run");
        let llm = ScriptedLlm::ok("should-not-run");
        let tts = ScriptedTts::ok(&[0x99]);
        let embed = UnusedEmbed;
        let realtime = FakeRealtime::ok("", "请开始", &[0x55, 0x66]);
        let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);
        let outcome = service
            .execute_command(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                parse_cmd(serde_json::json!({
                    "v": 1,
                    "id": "cmd-e2e-say",
                    "action": "say",
                    "text": "请开始"
                })),
            )
            .expect("e2e say");
        assert!(outcome.ok);
        assert_eq!(realtime.text_calls.load(Ordering::SeqCst), 1);
        assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.sink().recorded(), [0x55, 0x66]);
    }

    #[test]
    fn e2e_manual_text_with_pump_uses_dedicated_text_turn() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        start_ready_sink(&mut service, &database, &ready_e2e_public_config());
        // 全双工泵在线（真实泵由 commands 层装配；注入共享状态即可命中泵分支）。
        // 手动文本不得等泵轮次（等 500ms 后静默丢弃），必须走独立 text_turn。
        service.realtime_shared = Some(std::sync::Arc::new(
            crate::services::realtime_pump::PumpShared::new(),
        ));
        let asr = ScriptedAsr::ok("voice-should-not-run");
        let llm = ScriptedLlm::ok("should-not-run");
        let tts = ScriptedTts::ok(&[0x99]);
        let realtime = FakeRealtime::ok("", "文字已收到", &[0x31, 0x32]);
        let probes = e2e_probes(&asr, &llm, &tts, &UnusedEmbed, &realtime);
        let turn = service
            .finalize_utterance_with_hooks(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                Some("用打字问的问题"),
                &crate::providers::TurnStreamHooks::none(),
            )
            .unwrap()
            .expect("manual text turn");
        assert_eq!(turn.user_text, "用打字问的问题");
        assert_eq!(turn.assistant_text, "文字已收到");
        assert_eq!(realtime.text_calls.load(Ordering::SeqCst), 1);
        assert_eq!(realtime.calls.load(Ordering::SeqCst), 0);
        assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
        assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.sink().recorded(), [0x31, 0x32]);
    }

    #[test]
    fn e2e_voice_finalize_forwards_stream_snapshots_to_hooks() {
        let (_directory, database) = opened();
        let mut service = SessionService::with_sink(RecordingSink::default());
        start_ready_sink(&mut service, &database, &ready_e2e_public_config());
        let asr = ScriptedAsr::ok("should-not-run");
        let llm = ScriptedLlm::ok("should-not-run");
        let tts = ScriptedTts::ok(&[0x99]);
        let realtime = FakeRealtime::ok("现在几点", "现在是三点", &[0x77]);
        let probes = e2e_probes(&asr, &llm, &tts, &UnusedEmbed, &realtime);
        let user_snapshots = Mutex::new(Vec::<String>::new());
        let assistant_snapshots = Mutex::new(Vec::<String>::new());
        let hooks = crate::providers::TurnStreamHooks {
            user_text: Some(&|text| user_snapshots.lock().unwrap().push(text.to_owned())),
            assistant_text: Some(&|text| assistant_snapshots.lock().unwrap().push(text.to_owned())),
        };

        let turn = service
            .finalize_utterance_with_hooks(
                &database,
                &ready_e2e_public_config(),
                &probes,
                credentials(),
                None,
                &hooks,
            )
            .unwrap()
            .expect("e2e voice turn");

        assert_eq!(turn.user_text, "现在几点");
        assert_eq!(turn.assistant_text, "现在是三点");
        assert_eq!(user_snapshots.lock().unwrap().as_slice(), ["现在几点"]);
        assert_eq!(
            assistant_snapshots.lock().unwrap().as_slice(),
            ["现在是三点"]
        );
    }

    #[test]
    fn stale_barge_flag_before_playback_is_discarded_not_swallowing_the_answer() {
        // 意义：played 后 700ms 尾窗内的真实人声会残留 barge 旗标；播报开始前必须
        // 显式消费，否则下一轮播报闭包入口即取消、整段回答被吞（interrupted）。
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);
        // 直接驱动 finalize 的播报分支：预置 BridgePlayback（play 首行取消检查，
        // 不会触碰不存在的 sidecar 可执行文件），TTS 产物由 ScriptedTts 提供。
        service.configure_playback(Some(crate::audio::playback::BridgePlayback {
            executable: std::path::PathBuf::from("missing-barge-in-playback.exe"),
            endpoint_id: "test-device".into(),
        }));
        service
            .control()
            .set_barge_in_source(Arc::new(AtomicBool::new(true)));
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("不应被吞掉的回复");
        let tts = ScriptedTts::ok(&[0x01, 0x02]);

        service
            .finalize_utterance(
                &database,
                &ready_public_config(),
                &cascaded_probes(&asr, &llm, &tts, &UnusedEmbed),
                credentials(),
                Some("你好"),
            )
            .unwrap()
            .expect("turn");

        let meta = latest_turn_meta(&database, &id);
        // 残留旗标不得构成取消：play 被真实触达（首行取消检查通过），只因夹具
        // 缺少可执行文件而 failed；绝不是 interrupted。
        assert_eq!(meta["playbackStatus"], "failed");
        assert_eq!(service.last_error_code(), Some("SESSION_SIDECAR_MISSING"));
        // 会话仍存活：阶段与库中状态都回到 listening，且残留旗标已被消费。
        assert_eq!(service.phase(), SessionPhase::Listening);
        let row = SessionStore::new(&database).get(&id).unwrap().expect("row");
        assert_eq!(row.status, "listening");
        assert!(!service.control().barge_in_requested());
    }

    #[test]
    fn session_control_reset_clears_barge_in_source() {
        // 意义：停止/重建会话后，旧 capture 的旗标来源不得继续作用于控制端（stop 泄漏）。
        let control = super::SessionControl::new();
        let flag = Arc::new(AtomicBool::new(true));
        control.set_barge_in_source(flag.clone());
        assert!(control.barge_in_requested());

        control.reset();

        assert!(
            !control.barge_in_requested(),
            "reset must drop the barge-in source"
        );
        // 来源已被置 None：旧 Arc 再置位也不影响控制端。
        flag.store(true, Ordering::SeqCst);
        assert!(!control.barge_in_requested());
    }

    #[test]
    fn barge_in_source_wired_only_when_allowed() {
        // 意义：会话级开关接通 capture 旗标与控制端；未启用时旗标不得构成门控。
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        service
            .start_inner(&database, &ready_public_config(), true, None, true)
            .unwrap();
        service
            .capture()
            .barge_in_flag()
            .store(true, Ordering::SeqCst);
        assert!(service.control().barge_in_requested());
        assert!(service.control().take_barge_in());
        assert!(
            !service.control().take_barge_in(),
            "flag must reset once taken"
        );

        // 未启用（allow=false，含会议桥走的同一条 else 分支）：旗标置位也不生效。
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        service
            .start_inner(&database, &ready_public_config(), true, None, false)
            .unwrap();
        service
            .capture()
            .barge_in_flag()
            .store(true, Ordering::SeqCst);
        assert!(!service.control().barge_in_requested());
    }

    #[test]
    fn stale_summary_job_is_persisted_on_next_finalize() {
        // 意义：后台摘要 job 在下一次 finalize 开头被轮询落库（trim 后写入），
        // 摘要并入同一轮的系统提示，被摘要覆盖的轮次退出原始历史；
        // 失败结果静默丢弃，不落库且回答照常。
        let (_directory, database) = opened();
        let mut service = SessionService::new();
        let id = start_ready(&mut service, &database);
        let asr = ScriptedAsr::ok("ignored");
        let llm = ScriptedLlm::ok("最新回答");
        let tts = ScriptedTts::ok(&[0x01]);
        let embed = UnusedEmbed;
        let probes = cascaded_probes(&asr, &llm, &tts, &embed);
        let config = ready_public_config();

        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("第一问"))
            .unwrap()
            .expect("first turn");

        // 预置一个已完成的摘要 job：摘要文本 + 覆盖到第 0 轮（模拟后台压缩已完成）。
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(("这是滚动摘要文本".to_string(), 0))).unwrap();
        drop(tx);
        service.summary_job = Some(rx);

        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("第二问"))
            .unwrap()
            .expect("second turn");

        // 断言一：context_summary 落库。
        assert_eq!(
            SessionStore::new(&database).context_summary(&id).unwrap(),
            Some(("这是滚动摘要文本".to_string(), 0))
        );
        // 断言二：摘要并入本轮系统提示，被摘要的轮次不再作为原始历史发送。
        let latest = llm.seen_messages().pop().expect("second llm call");
        assert!(
            latest[0].content.contains("此前对话摘要：这是滚动摘要文本"),
            "system prompt: {}",
            latest[0].content
        );
        assert!(latest.iter().any(|message| message.content == "第二问"));
        assert!(!latest.iter().any(|message| message.content == "第一问"));
        assert!(service.summary_job.is_none());

        // 断言三：失败路径（摘要生成失败）不落库，且回答照常。
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err("LLM_RESPONSE_EMPTY".to_string())).unwrap();
        drop(tx);
        service.summary_job = Some(rx);
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("第三问"))
            .unwrap()
            .expect("third turn");
        assert_eq!(
            SessionStore::new(&database).context_summary(&id).unwrap(),
            Some(("这是滚动摘要文本".to_string(), 0))
        );
        assert!(service.summary_job.is_none());
        let latest = llm.seen_messages().pop().expect("third llm call");
        assert!(latest.iter().any(|message| message.content == "第三问"));
        let stored = SessionStore::new(&database).list_turns(&id).unwrap();
        assert_eq!(stored.len(), 3);
        assert_eq!(stored[2].assistant_text, "最新回答");
    }

    #[test]
    fn should_compress_uses_threshold_plus_recent_window() {
        use super::{NO_CONTEXT_SUMMARY, should_compress};
        // 无摘要（哨兵 -1）：未摘要轮次须超过 12 + 4 才触发。
        assert!(!should_compress(15, NO_CONTEXT_SUMMARY));
        assert!(should_compress(16, NO_CONTEXT_SUMMARY));
        // 已有摘要（upto=3）：按距上次摘要的新增轮次计数。
        assert!(!should_compress(19, 3));
        assert!(should_compress(20, 3));
        assert!(!should_compress(3, 3));
    }
}
