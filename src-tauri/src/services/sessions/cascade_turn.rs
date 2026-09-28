//! cascade_turn 子模块：级联（ASR→LLM→TTS）单轮编排：
//! PCM 推入、语音成句 finalize 与强制成句的共享实现。
//! 纯搬移自 services/sessions.rs，不含行为变更。
use super::*;

impl<S: PlaybackSink> SessionService<S> {
    pub fn push_pcm(&mut self, pcm: &[u8]) {
        self.capture.push_pcm(pcm);
    }

    /// 诊断：tap 满丢帧累计（实时上行背压观测）。
    pub fn capture_tap_dropped(&self) -> u64 {
        self.capture.tap_dropped()
    }

    /// 无锁热路径句柄（AppState 持有，IPC 推流不再等 sessions 锁）。
    pub fn mic_ingest_handle(&self) -> crate::audio::capture::MicIngestHandle {
        self.capture.mic_ingest_handle()
    }

    pub fn finalize_utterance(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
        text: Option<&str>,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        self.finalize_utterance_inner(
            database,
            config,
            probes,
            credentials,
            text,
            false,
            &TurnStreamHooks::none(),
        )
    }

    pub fn finalize_utterance_with_hooks(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
        text: Option<&str>,
        hooks: &TurnStreamHooks<'_>,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        self.finalize_utterance_inner(database, config, probes, credentials, text, false, hooks)
    }

    pub fn finalize_utterance_forced(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        self.finalize_utterance_inner(
            database,
            config,
            probes,
            credentials,
            None,
            true,
            &TurnStreamHooks::none(),
        )
    }

    pub fn finalize_utterance_forced_with_hooks(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
        hooks: &TurnStreamHooks<'_>,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        self.finalize_utterance_inner(database, config, probes, credentials, None, true, hooks)
    }

    #[allow(clippy::too_many_arguments)]
    fn finalize_utterance_inner(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
        text: Option<&str>,
        force_meeting_assistant: bool,
        hooks: &TurnStreamHooks<'_>,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        if force_meeting_assistant
            && self
                .config_snapshot
                .as_ref()
                .and_then(active_session_role_scenario)
                != Some(RoleScenario::MeetingAssistant)
        {
            return Err(SessionServiceError::StateInvalid);
        }
        self.poll_sidecar(database)?;
        // 阶段 4：轮询已完成的摘要压缩 job。成功则 trim 后落库；
        // 失败静默丢弃——上下文回退为原始截断历史，下次满足条件会重试。
        if let Some(session_id) = self.session_id.clone()
            && let Some(rx) = self.summary_job.take()
        {
            match rx.try_recv() {
                Ok(Ok((summary, upto))) => {
                    SessionStore::new(database).set_context_summary(
                        &session_id,
                        summary.trim(),
                        upto,
                    )?;
                }
                // job 未完成：继续挂起，留给下一次 finalize。
                Err(std::sync::mpsc::TryRecvError::Empty) => self.summary_job = Some(rx),
                // 失败或发送端已断（含压缩线程异常退出）：按无结果处理。
                Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
            }
        }
        self.runtime.set_mode(self.control.mode());
        if self.control.take_stop_tts() {
            self.sink.cancel();
        }
        if !self.runtime.can_answer() {
            return Ok(None);
        }
        if self.control.stop_requested() {
            self.finish_stop(database)?;
            return Ok(None);
        }
        self.control.clear_cancel();
        // A new submitted question invalidates an older suggestion even if the
        // next ASR/model request fails before creating a new persisted turn.
        self.supersede_pending_confirmation(database);
        let confirmation_epoch = self.control.confirmation_epoch.load(Ordering::SeqCst);
        let session_id = self
            .session_id
            .clone()
            .ok_or(SessionServiceError::NotFound)?;
        self.runtime.transition(SessionPhase::Thinking)?;
        let store = SessionStore::new(database);
        persist_phase(&store, &session_id, SessionPhase::Thinking)?;

        let config = with_default_voice(self.config_snapshot.as_ref().unwrap_or(config));
        let history = store
            .list_turns(&session_id)?
            .into_iter()
            .map(|turn| HistoryTurn {
                user_text: turn.user_text,
                assistant_text: turn.assistant_text,
            })
            .collect::<Vec<_>>();
        // 阶段 4：读取既有滚动摘要；哨兵 -1 表示从未写入，此时全程保留原始历史。
        let (existing_summary, upto) = store
            .context_summary(&session_id)?
            .unwrap_or((String::new(), NO_CONTEXT_SUMMARY));
        let history_after = &history[(upto + 1).clamp(0, history.len() as i64) as usize..];
        let user_text = text.map(str::trim).filter(|value| !value.is_empty());
        let pcm = if user_text.is_none() {
            self.capture
                .take_utterance_for_asr()
                .unwrap_or_else(|| self.capture.pcm_for_asr())
        } else {
            Vec::new()
        };
        let role_scenario = active_session_role_scenario(&config);
        let e2e_route =
            active_voice_route(&config).is_some_and(|route| route.mode == VoiceRouteMode::E2e);
        // “未点名只转写”只应抑制会议音频会话（对方讨论不触发作答）；
        // 本机麦克风是与角色直接对话，每句话都应得到回复。
        let transcribed_meeting_text = if user_text.is_none()
            && self.capture.is_meeting_bridge()
            && role_scenario == Some(RoleScenario::MeetingAssistant)
            && !e2e_route
        {
            match transcribe_meeting_pcm(probes.asr, &config, credentials, &pcm) {
                Ok(text) => Some(text),
                Err(error) => {
                    return self.recover_from_cascade_error(database, &session_id, error);
                }
            }
        } else {
            None
        };
        let effective_user_text = user_text.or(transcribed_meeting_text.as_deref());
        let meeting_role_name = active_role_profile(&config).map(|role| role.name.as_str());
        let transcript_only = !force_meeting_assistant
            && transcribed_meeting_text.as_deref().is_some_and(|text| {
                !meeting_assistant_was_mentioned(text, meeting_role_name.unwrap_or("会议助手"))
            });
        let deps = CascadeTurnDeps {
            asr: probes.asr,
            llm: probes.llm,
            tts: probes.tts,
            embed: probes.embed,
            database,
            runtime: &self.runtime,
            sleep: &std::thread::sleep,
        };
        let request = CascadeTurnRequest {
            config: &config,
            credentials,
            pcm: effective_user_text.is_none().then_some(pcm.as_slice()),
            sample_rate: ASR_SAMPLE_RATE,
            user_text: effective_user_text,
            history: history_after,
            context_summary: Some(existing_summary.as_str()),
        };
        let mut pump_playback_status: Option<&'static str> = None;
        let mut pump_turn_meta = serde_json::Value::Null;
        let turn = if transcript_only {
            CascadeTurn {
                user_text: transcribed_meeting_text.unwrap_or_default(),
                assistant_text: String::new(),
                tts_pcm: Vec::new(),
                citations: Vec::new(),
                materials_used: false,
                error_code: None,
            }
        } else if e2e_route && self.realtime_shared.is_some() && user_text.is_none() {
            // 端到端流式路线：轮次已由泵完成（含播放/打断），这里只取结果落库。
            // 手动文本（composer 输入）不经过泵：等泵轮次只会把输入静默丢弃，
            // 与 say 相同走独立的 text_turn 连接（见下方 run_e2e_turn 分支）。
            // 先取轮后查失败：历史遗留的终局失败（如 401 后槽未消费）不得误吞
            // 已排队的完整轮次；仅当本轮确实无内容可落库时才上报失败。
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
            let mut completed = self.take_realtime_turn();
            while completed.is_none() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(25));
                completed = self.take_realtime_turn();
            }
            let Some(completed) = completed else {
                if let Some(reason) = self.take_realtime_failure() {
                    self.last_error_code = Some(reason.clone());
                    self.return_to_listening(database, &session_id)?;
                    return Err(SessionServiceError::Realtime(RealtimeError::Remote(reason)));
                }
                // 就绪信号竞态兜底：无可落库轮次，不打扰前端。
                return Ok(None);
            };
            pump_turn_meta = serde_json::json!({
                "latencyMsFirstAudio": completed.first_audio_ms,
                "ingressDropped": self.capture.tap_dropped(),
                "audioBytes": completed.audio_bytes,
                "playbackWriteFailed": completed.playback_write_failed,
                "playbackAlive": completed.playback_alive,
                "audioDeltaCount": completed.audio_delta_count,
                "audioDeltaMaxGapMs": completed.audio_delta_max_gap_ms,
                "audioDeltaGapsOver150Ms": completed.audio_delta_gaps_over_150_ms,
                "audioDeltaGapsOver500Ms": completed.audio_delta_gaps_over_500_ms,
                "aecResidualCorrelation": completed.aec_residual_correlation,
                "aecResidualMax": completed.aec_residual_max,
                "aecResidualOverThreshold": completed.aec_residual_over_threshold,
                "playbackMode": if completed.playback_mode
                    == crate::services::realtime_pump::RealtimePlaybackMode::WebAudio
                {
                    "web_audio"
                } else {
                    "native"
                },
                "playbackGeneration": completed.playback_generation,
                "playbackRestarts": completed.playback_restarts,
                "playbackRestartedDuringTurn": completed.playback_restarted_during_turn,
                "playbackLastEvent": completed.playback_last_event,
                "echoDropped": completed.echo_dropped,
                "echoDroppedTotal": completed.echo_dropped_total,
                "timeline": completed.timeline,
                "finalizeLagMs": completed.completed_at.elapsed().as_millis() as u64,
            });
            pump_playback_status = if completed.interrupted {
                Some("interrupted")
            } else if completed.transcript_only {
                Some("text_only")
            } else if completed.held {
                None // 候选闸门：由 candidate 分支标注 pending_confirmation
            } else if completed.playback_write_failed || !completed.playback_alive {
                Some("failed")
            } else if completed.audio_bytes == 0 {
                Some("text_only")
            } else {
                Some("played")
            };
            CascadeTurn {
                user_text: completed.user_text,
                assistant_text: completed.assistant_text,
                tts_pcm: Vec::new(),
                materials_used: false,
                citations: Vec::new(),
                error_code: None,
            }
        } else if e2e_route {
            match run_e2e_turn(
                probes.realtime,
                &deps,
                &request,
                &pcm,
                self.control.cancel_flag(),
                hooks,
            ) {
                Ok(turn) => turn,
                Err(error) => {
                    return self.recover_from_realtime_error(database, &session_id, error);
                }
            }
        } else {
            match run_cascade_turn(&deps, request, self.control.cancel_flag(), hooks) {
                Ok(turn) => turn,
                Err(error) => {
                    return self.recover_from_cascade_error(database, &session_id, error);
                }
            }
        };

        // 文本级回声过滤（最后一道防线）：AI 播报声被麦克风回收、被 ASR
        // 转写成「用户发言」时整轮丢弃——绝不落库/进上下文，否则
        // 「自己回答自己」的循环被固化为正式对话历史。音频闸门（上行门控/
        // 回声抑制窗）是第一道防线；这里兜底级联、旧 e2e 与文本注入所有路线。
        // 比对只看此前轮次的播报（history 取自本轮写入前的数据库）。
        let recent_assistant: Vec<String> = history
            .iter()
            .rev()
            .map(|turn| turn.assistant_text.as_str())
            .filter(|text| !text.is_empty())
            .take(2)
            .map(str::to_owned)
            .collect();
        if !turn.user_text.trim().is_empty()
            && crate::services::echo_guard::is_echo(&turn.user_text, &recent_assistant)
        {
            self.return_to_listening(database, &session_id)?;
            return Ok(None);
        }

        let turn_id = uuid::Uuid::new_v4().to_string();
        store.insert_turn(NewTurn {
            id: &turn_id,
            session_id: &session_id,
            turn_index: self.turn_index,
            user_text: &turn.user_text,
            assistant_text: &turn.assistant_text,
            materials_used: turn.materials_used,
        })?;
        if !turn.citations.is_empty() {
            let citations = turn
                .citations
                .iter()
                .map(|citation| NewCitation {
                    turn_id: &turn_id,
                    material_id: &citation.material_id,
                    chunk_id: &citation.chunk_id,
                    snippet: &citation.snippet,
                })
                .collect::<Vec<_>>();
            store.insert_citations(&citations)?;
        }
        store.append_event(
            &session_id,
            "transcript",
            &serde_json::json!({ "text": truncate(&turn.user_text) }).to_string(),
        )?;
        store.append_event(
            &session_id,
            "reply",
            &serde_json::json!({ "text": truncate(&turn.assistant_text) }).to_string(),
        )?;
        self.turn_index += 1;
        self.revision += 1;
        self.unused_materials = !turn.materials_used;
        self.last_error_code = None;
        if self.realtime_pump.is_some() {
            let mut updated_history: Vec<(String, String)> = history
                .iter()
                .map(|entry| (entry.user_text.clone(), entry.assistant_text.clone()))
                .collect();
            updated_history.push((turn.user_text.clone(), turn.assistant_text.clone()));
            self.push_realtime_history(updated_history);
        }

        // 阶段 4：未摘要轮次超过阈值时，后台把较早轮次压缩为滚动摘要。
        // 压缩失败不影响本轮回答；落库要等下一次 finalize 开头的轮询。
        let newest_index = history.len() as i64; // 含刚落的这一轮
        if should_compress(newest_index, upto) && self.summary_job.is_none() {
            let compress_from = upto + 1;
            let compress_to = newest_index - KEEP_RECENT_TURNS as i64;
            // endpoint/model 解析沿用既有 llm_endpoint 助手；e2e 路由无 llm 端点，
            // 解析失败则静默跳过（e2e 上下文由 instructions 自带）。
            if compress_to >= compress_from
                && let Ok((llm_endpoint, llm_model_id)) = llm_endpoint(&config)
            {
                let old_summary = existing_summary.clone();
                // 复用本 finalize 开头取出的 history：与 list_turns 同序，
                // 且 compress_to < newest_index，所需轮次必然都在其中，不重复查库。
                let turns: Vec<String> = history
                    .iter()
                    .skip(compress_from as usize)
                    .take((compress_to - compress_from + 1) as usize)
                    .map(|turn| format!("用户：{}\n助手：{}", turn.user_text, turn.assistant_text))
                    .collect();
                let credential = credentials
                    .llm
                    .map(|s| zeroize::Zeroizing::new(s.to_string()));
                let (tx, rx) = std::sync::mpsc::channel();
                self.summary_job = Some(rx);
                std::thread::spawn(move || {
                    let client = match OpenAiCompatibleCascade::new() {
                        Ok(client) => client,
                        Err(_) => {
                            let _ = tx.send(Err("client".into()));
                            return;
                        }
                    };
                    let prompt = format!(
                        "请把以下对话压缩为不超过 800 字的中文摘要，保留关键事实、决定与未决问题，直接输出摘要正文：\n\n既有摘要：{}\n\n对话：\n{}",
                        old_summary,
                        turns.join("\n\n")
                    );
                    let messages = vec![ChatMessage {
                        role: "user".into(),
                        content: prompt,
                    }];
                    let result = client
                        .complete(
                            &llm_endpoint,
                            credential.as_deref().map(String::as_str),
                            &llm_model_id,
                            &messages,
                        )
                        .map(|text| {
                            let trimmed: String = text.chars().take(2000).collect();
                            (trimmed, compress_to)
                        })
                        .map_err(|error| error.code().to_string());
                    let _ = tx.send(result);
                });
            }
        }

        self.runtime.transition(SessionPhase::Speaking)?;
        persist_phase(&store, &session_id, SessionPhase::Speaking)?;
        let candidate_confirmation_required = self
            .config_snapshot
            .as_ref()
            .and_then(active_session_role_scenario)
            == Some(RoleScenario::Candidate);
        let mut playback_status = if candidate_confirmation_required {
            "pending_confirmation"
        } else if let Some(status) = pump_playback_status {
            status
        } else if turn.tts_pcm.is_empty() {
            "text_only"
        } else {
            "not_played"
        };
        if candidate_confirmation_required {
            if confirmation_epoch != self.control.confirmation_epoch.load(Ordering::SeqCst) {
                playback_status = "cancelled";
                self.pending_confirmation_epoch = None;
            } else {
                self.pending_confirmation_epoch = Some(confirmation_epoch);
            }
            // Candidate mode is advisory: generated speech must never reach the
            // meeting until the user explicitly confirms the current answer.
        } else if self.control.take_stop_tts() || self.control.is_cancelled() {
            self.sink.cancel();
            playback_status = "cancelled";
        } else if !turn.tts_pcm.is_empty() {
            if let Some(output) = &self.playback {
                let seconds = turn.tts_pcm.len() as f64 / (24_000.0 * 2.0) + 0.7;
                self.capture
                    .suppress_echo_for(std::time::Duration::from_secs_f64(seconds));
                // 每轮播报从干净旗标开始：上一轮 played 尾窗（700ms）内的真实人声
                // 会残留旗标，不显式消费会让本轮播报一开场即被取消、整段回答被吞。
                self.control.take_barge_in();
                let result = output.play(&turn.tts_pcm, 24_000, || {
                    self.control.is_cancelled() || self.control.barge_in_requested()
                });
                match result {
                    Ok(()) => {
                        self.capture
                            .suppress_echo_for(std::time::Duration::from_millis(700));
                        playback_status = "played";
                    }
                    Err("PLAYBACK_CANCELLED") if self.control.take_barge_in() => {
                        playback_status = "interrupted";
                    }
                    Err(code) => {
                        self.last_error_code = Some(code.into());
                        playback_status = "failed";
                    }
                }
            } else if !self.text_only {
                self.sink.play_pcm(&turn.tts_pcm, 24_000);
                playback_status = "played";
            }
        }
        let mut meta = serde_json::json!({
            "turnId": turn_id,
            "triggerSource": if force_meeting_assistant { "hotkey" } else if user_text.is_some() { "manual" } else { "voice" },
            "userConfirmed": !candidate_confirmation_required,
            "playbackStatus": playback_status,
        });
        if let Some(object) = pump_turn_meta.as_object() {
            for (key, value) in object {
                meta[key.as_str()] = value.clone();
            }
        }
        store.append_event(&session_id, "turn_meta", &meta.to_string())?;
        if self.control.stop_requested() {
            self.finish_stop(database)?;
            return Ok(Some(turn));
        }
        if self.control.is_cancelled() {
            self.return_to_listening(database, &session_id)?;
            return Ok(Some(turn));
        }
        self.runtime.transition(SessionPhase::Listening)?;
        persist_phase(&store, &session_id, SessionPhase::Listening)?;
        Ok(Some(turn))
    }
}
