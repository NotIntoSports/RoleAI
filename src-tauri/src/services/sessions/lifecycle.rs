//! lifecycle 子模块：会话生命周期编排
//! start / stop / fail / reset / poll_sidecar。
//! 纯搬移自 services/sessions/mod.rs，不含行为变更。
use super::*;

impl<S: PlaybackSink> SessionService<S> {
    pub fn reset(&mut self) {
        self.reset_runtime();
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

    pub(super) fn start_inner(
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

    pub(super) fn fail_session(&mut self, database: &Database) -> Result<(), SessionServiceError> {
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
