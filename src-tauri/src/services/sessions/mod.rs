//! 会话服务核心：`SessionService` 持有 service_lock/database/sessions 三把全局锁，
//! 编排会话生命周期（lifecycle）、实时链路（realtime）、级联单轮（cascade_turn）与
//! 点名/追答命令（agent_commands）。锁的获取顺序与持锁范围见各函数文档。

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
mod agent_commands;
mod cascade_turn;
pub(crate) mod finalize;
mod lifecycle;

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
    /// 强制回答时没有可回答的仅转写发言。
    NothingToAnswer,
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
            Self::NothingToAnswer => "NOTHING_TO_ANSWER",
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

// 子模块：纯搬移拆分（C22）。
mod control;
mod helpers;

pub use self::control::SessionControl;
use self::helpers::*;
pub(crate) use self::helpers::{
    active_session_role_scenario, e2e_instructions, meeting_assistant_was_mentioned,
};

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
    // 收尾代次：begin_finalize/finish_stop/reset_runtime/fail_session 各 +1。
    // 三阶段收尾的阶段三用它判定“收尾期间会话被停止/更替”，决定落库为取消或丢弃。
    finalize_generation: u64,
    // 收尾单飞行守卫：Some(代次) 表示该代次的收尾在飞行中（阶段二/播放）。
    // 记录代次使 complete 阶段只清自己的守卫——更替后新收尾若已在飞，
    // 旧收尾的放弃分支不得误清（D 线审查 T03 §2）；落库失败路径也必须复位，
    // 否则该会话后续收尾全部 Idle→STATE_INVALID。
    finalizing_generation: Option<u64>,
    // 会议会话失败时的系统默认麦克风恢复凭证（会议期间接管了全部默认采集角色）。
    pending_fail_routing: Option<crate::prerequisites::AudioRoutingChange>,
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
            finalize_generation: 0,
            finalizing_generation: None,
            pending_fail_routing: None,
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

    pub fn configure_playback(&mut self, playback: Option<crate::audio::playback::BridgePlayback>) {
        self.text_only = playback.is_none();
        self.playback = playback;
    }

    #[cfg(test)]
    fn runtime_can_answer(&self) -> bool {
        self.runtime.can_answer()
    }

    fn has_active_session(&self) -> bool {
        self.session_id.is_some() && !is_terminal_phase(self.runtime.phase())
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
        // 在途收尾（阶段二）的阶段三将看到代次变化，按“已取消”落库或丢弃。
        self.finalize_generation = self.finalize_generation.wrapping_add(1);
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
}

/// `context_summary_turn_index` 的哨兵值：从未写入过摘要。
const NO_CONTEXT_SUMMARY: i64 = -1;
/// 未摘要轮次超过此数触发后台压缩。
const COMPRESS_THRESHOLD: i64 = 12;
/// 压缩时始终保留的最近原始轮次。
const KEEP_RECENT_TURNS: usize = 4;

#[cfg(test)]
mod tests;
