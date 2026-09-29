//! agent_commands 子模块：智能体指令（say/confirm/retry/correct/report）
//! 与接管模式切换（set_mode）。
//! 纯搬移自 services/sessions.rs，不含行为变更。
use super::*;

impl<S: PlaybackSink> SessionService<S> {
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
}
