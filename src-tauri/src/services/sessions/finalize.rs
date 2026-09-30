//! finalize 三阶段拆分（C12 锁拆分）：
//! 阶段一 `begin_finalize`（持锁）：快照与登记，产出 `FinalizePlan`；
//! 阶段二 `finalize_network` / `run_finalize_playback`（不持任何 AppState 锁）：
//! ASR/LLM/TTS 网络调用与扬声器播放；
//! 阶段三 `complete_finalize_text` / `complete_finalize_playback`（持锁）：
//! 校验收尾代次后落库，代次更替则按“已取消”落库或丢弃。
//! 命令层在三段之间释放 sessions 锁（database 以 `Arc<Database>` 借出，不持外层锁）；
//! `finalize_utterance*` 便捷包装保持原签名，供既有调用方与单测使用。
use super::*;

/// 阶段一结果：无可落库内容（对齐旧实现的 `Ok(None)`）或携带快照进入阶段二。
/// `FinalizePlan` 体积较大（含配置与历史快照），装箱避免 `Idle` 哨兵分支放大栈占用。
pub(crate) enum BeginFinalize {
    Idle,
    Plan(Box<FinalizePlan>),
}

/// 收尾快照：阶段二/三只依赖这些 owned 数据，不再借用 SessionService。
pub(crate) struct FinalizePlan {
    pub session_id: String,
    /// 本轮 id（阶段一生成，落库与 turn_meta 共用）。
    pub turn_id: String,
    pub turn_index: i64,
    /// begin_finalize 入口递增后的代次；阶段三比对用。
    pub generation: u64,
    /// 阶段一 mode 快照：阶段二 `can_answer` 语义与原实现一致（不再中途变向）。
    pub mode: AgentMode,
    pub confirmation_epoch: u64,
    /// `with_default_voice` 后的配置快照：阶段二期间改配置不影响本轮。
    pub config: PublicConfig,
    pub history: Vec<HistoryTurn>,
    pub history_after: Vec<HistoryTurn>,
    pub existing_summary: String,
    pub summary_upto: i64,
    pub user_text: Option<String>,
    pub pcm: Vec<u8>,
    pub role_scenario: Option<RoleScenario>,
    pub e2e_route: bool,
    pub meeting_bridge: bool,
    pub meeting_role_name: Option<String>,
    pub force_meeting_assistant: bool,
    /// 阶段一 tap 丢帧累计快照（原实现在落库前读取；累计值，仅诊断字段）。
    pub tap_dropped: u64,
    /// 泵读侧共享状态：阶段二轮询泵轮次与终局失败，不持 sessions 锁。
    pub realtime_shared: Option<std::sync::Arc<crate::services::realtime_pump::PumpShared>>,
    /// 落库后是否需要向泵同步上下文（原 `self.realtime_pump.is_some()`）。
    pub pump_running: bool,
    pub control: std::sync::Arc<SessionControl>,
}

/// 阶段二网络结果。
pub(crate) enum NetworkOutcome {
    /// 无可落库内容（泵轮次缺失且无终局失败），对齐旧 `Ok(None)`。
    Idle,
    Turn {
        turn: CascadeTurn,
        pump_turn_meta: serde_json::Value,
        pump_playback_status: Option<&'static str>,
        pump_response_failed: bool,
    },
    /// 级联/会议 ASR 失败：阶段三走 `recover_from_cascade_error`。
    Cascade(CascadeError),
    /// 实时链路失败：阶段三走 `recover_from_realtime_error`。
    Realtime(RealtimeError),
    /// 端到端泵的终局失败（原分支直接 `Err(Realtime::Remote)` + 回 Listening）。
    RealtimePumpFailure(String),
}

/// 阶段二文本落库结果。
/// `TurnPersist` 体积较大，装箱避免 `Dropped`/`Idle` 哨兵分支放大栈占用。
pub(crate) enum TextPersist {
    /// 会话已被更替（stop→start）：旧轮结果整体丢弃。
    Dropped,
    /// 无可落库内容：对齐旧 `Ok(None)`（命令层映射 STATE_INVALID）。
    Idle,
    /// 文本已落库；剩 turn_meta 与收尾在阶段三。
    Turn(Box<TurnPersist>),
}

/// 阶段三输入：turn_meta 前置字段 + 播放安排。
pub(crate) struct TurnPersist {
    pub session_id: String,
    /// 所属收尾的代次：阶段三释放守卫时按代次比对（只清自己的）。
    pub generation: u64,
    pub turn: CascadeTurn,
    /// 完整 meta（含 pump 字段），仅缺 `playbackStatus`。
    pub meta_prelude: serde_json::Value,
    /// 泵/文本路线的预置播放状态（播放作业存在时被阶段二播放结果覆盖）。
    pub playback_status: &'static str,
    pub playback_job: Option<PlaybackJob>,
}

/// 阶段二播放作业：BridgePlayback 是纯配置结构（Clone），控制旗标走原子量。
pub(crate) struct PlaybackJob {
    pub pcm: Vec<u8>,
    pub output: crate::audio::playback::BridgePlayback,
    pub control: std::sync::Arc<SessionControl>,
}

impl<S: PlaybackSink> SessionService<S> {
    pub fn finalize_utterance(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
        text: Option<&str>,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        self.finalize_utterance_with_hooks(
            database,
            config,
            probes,
            credentials,
            text,
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
        self.run_finalize_split(database, config, probes, credentials, text, false, hooks)
    }

    pub fn finalize_utterance_forced(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        self.finalize_utterance_forced_with_hooks(
            database,
            config,
            probes,
            credentials,
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
        self.run_finalize_split(database, config, probes, credentials, None, true, hooks)
    }

    /// 便捷包装：三阶段连续执行（调用方本就持有 `&mut self`，语义与拆分前一致）。
    // 例外：too_many_arguments（账本登记）——保持旧 finalize_utterance* 签名，便于既有调用方。
    #[allow(clippy::too_many_arguments)]
    fn run_finalize_split(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        probes: &SessionProbes<'_>,
        credentials: CascadeCredentials<'_>,
        text: Option<&str>,
        force_meeting_assistant: bool,
        hooks: &TurnStreamHooks<'_>,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        match self.begin_finalize(database, config, text, force_meeting_assistant)? {
            BeginFinalize::Idle => Ok(None),
            BeginFinalize::Plan(plan) => {
                let outcome = finalize_network(&plan, database, probes, credentials, hooks);
                match self.complete_finalize_text(*plan, outcome, database, credentials)? {
                    TextPersist::Idle | TextPersist::Dropped => Ok(None),
                    TextPersist::Turn(persist) => {
                        let (status, error) = run_persist_playback(&persist);
                        self.complete_finalize_playback(*persist, status, error, database)
                    }
                }
            }
        }
    }

    /// 阶段一（持锁）：快照与登记。命令层随后释放锁进入阶段二。
    pub(crate) fn begin_finalize(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        text: Option<&str>,
        force_meeting_assistant: bool,
    ) -> Result<BeginFinalize, SessionServiceError> {
        if force_meeting_assistant
            && self
                .config_snapshot
                .as_ref()
                .and_then(active_session_role_scenario)
                != Some(RoleScenario::MeetingAssistant)
        {
            return Err(SessionServiceError::StateInvalid);
        }
        // 并发收尾：阶段二/播放期间另一个 finalize 到来，对齐旧 `Ok(None)`。
        if self.finalizing_generation.is_some() {
            return Ok(BeginFinalize::Idle);
        }
        self.finalize_generation = self.finalize_generation.wrapping_add(1);
        let generation = self.finalize_generation;
        // 登记本代次为在飞收尾；begin 全程持锁，早退分支不存在并发方。
        self.finalizing_generation = Some(generation);
        match self.begin_finalize_inner(database, config, text, force_meeting_assistant, generation)
        {
            Ok(Some(plan)) => Ok(BeginFinalize::Plan(Box::new(plan))),
            Ok(None) => {
                self.finalizing_generation = None;
                Ok(BeginFinalize::Idle)
            }
            Err(error) => {
                self.finalizing_generation = None;
                Err(error)
            }
        }
    }

    fn begin_finalize_inner(
        &mut self,
        database: &Database,
        config: &PublicConfig,
        text: Option<&str>,
        force_meeting_assistant: bool,
        generation: u64,
    ) -> Result<Option<FinalizePlan>, SessionServiceError> {
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
        // 会议桥 ASR 移到阶段二（不持锁网络）；这里只快照判定所需标志。
        Ok(Some(FinalizePlan {
            session_id,
            turn_id: uuid::Uuid::new_v4().to_string(),
            turn_index: self.turn_index,
            generation,
            mode: self.control.mode(),
            confirmation_epoch,
            history: history.clone(),
            history_after: history_after.to_vec(),
            existing_summary,
            summary_upto: upto,
            user_text: user_text.map(str::to_owned),
            pcm,
            role_scenario,
            e2e_route,
            meeting_bridge: self.capture.is_meeting_bridge(),
            meeting_role_name: active_role_profile(&config).map(|role| role.name.clone()),
            force_meeting_assistant,
            tap_dropped: self.capture.tap_dropped(),
            realtime_shared: self.realtime_shared.clone(),
            pump_running: self.realtime_pump.is_some(),
            control: Arc::clone(&self.control),
            config,
        }))
    }

    /// 阶段三前半（持锁）：代次校验、回声过滤、文本落库、播放安排。
    /// 阶段三前半（持锁）：文本落库。放弃与失败路径（含 `?` 传播）在此复位
    /// 收尾守卫，只清自己代次的——更替后新收尾若已在飞不受影响（T03 §2）；
    /// `Turn` 成功路径的守卫保持到 `complete_finalize_playback`（播放窗口
    /// 仍需挡住并发收尾），由其按代次释放。
    pub(crate) fn complete_finalize_text(
        &mut self,
        plan: FinalizePlan,
        outcome: NetworkOutcome,
        database: &Database,
        credentials: CascadeCredentials<'_>,
    ) -> Result<TextPersist, SessionServiceError> {
        let generation = plan.generation;
        let result = self.complete_finalize_text_inner(plan, outcome, database, credentials);
        if !matches!(result, Ok(TextPersist::Turn(_)))
            && self.finalizing_generation == Some(generation)
        {
            self.finalizing_generation = None;
        }
        result
    }

    fn complete_finalize_text_inner(
        &mut self,
        plan: FinalizePlan,
        outcome: NetworkOutcome,
        database: &Database,
        credentials: CascadeCredentials<'_>,
    ) -> Result<TextPersist, SessionServiceError> {
        // 会话已被更替（stop→start）：旧轮结果整体丢弃，不写任何数据。
        if self.session_id.as_deref() != Some(plan.session_id.as_str()) {
            return Ok(TextPersist::Dropped);
        }
        let (turn, pump_turn_meta, pump_playback_status, pump_response_failed) = match outcome {
            NetworkOutcome::Idle => {
                return Ok(TextPersist::Idle);
            }
            NetworkOutcome::Cascade(error) => {
                let result = self.recover_from_cascade_error(database, &plan.session_id, error);
                return result.map(|_| TextPersist::Idle);
            }
            NetworkOutcome::Realtime(error) => {
                let result = self.recover_from_realtime_error(database, &plan.session_id, error);
                return result.map(|_| TextPersist::Idle);
            }
            NetworkOutcome::RealtimePumpFailure(reason) => {
                self.last_error_code = Some(reason.clone());
                self.return_to_listening(database, &plan.session_id)?;
                return Err(SessionServiceError::Realtime(RealtimeError::Remote(reason)));
            }
            NetworkOutcome::Turn {
                turn,
                pump_turn_meta,
                pump_playback_status,
                pump_response_failed,
            } => (
                turn,
                pump_turn_meta,
                pump_playback_status,
                pump_response_failed,
            ),
        };

        // 文本级回声过滤（最后一道防线）：AI 播报声被麦克风回收、被 ASR
        // 转写成「用户发言」时整轮丢弃——绝不落库/进上下文，否则
        // 「自己回答自己」的循环被固化为正式对话历史。音频闸门（上行门控/
        // 回声抑制窗）是第一道防线；这里兜底级联、旧 e2e 与文本注入所有路线。
        // 比对只看此前轮次的播报（history 取自本轮写入前的数据库）。
        let recent_assistant: Vec<String> = plan
            .history
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
            self.return_to_listening(database, &plan.session_id)?;
            return Ok(TextPersist::Idle);
        }

        let store = SessionStore::new(database);
        store.insert_turn(NewTurn {
            id: &plan.turn_id,
            session_id: &plan.session_id,
            turn_index: plan.turn_index,
            user_text: &turn.user_text,
            assistant_text: &turn.assistant_text,
            materials_used: turn.materials_used,
        })?;
        if !turn.citations.is_empty() {
            let citations = turn
                .citations
                .iter()
                .map(|citation| NewCitation {
                    turn_id: &plan.turn_id,
                    material_id: &citation.material_id,
                    chunk_id: &citation.chunk_id,
                    snippet: &citation.snippet,
                })
                .collect::<Vec<_>>();
            store.insert_citations(&citations)?;
        }
        store.append_event(
            &plan.session_id,
            "transcript",
            &serde_json::json!({ "text": truncate(&turn.user_text) }).to_string(),
        )?;
        store.append_event(
            &plan.session_id,
            "reply",
            &serde_json::json!({ "text": truncate(&turn.assistant_text) }).to_string(),
        )?;
        self.turn_index += 1;
        self.revision += 1;
        self.unused_materials = !turn.materials_used;
        self.last_error_code = None;
        if pump_response_failed {
            // 端到端服务端始终没有开始响应（看门狗放弃）：错误码经 runtime
            // status 通道透出，前端映射为「助手这次没有响应」提示。
            self.last_error_code = Some("REALTIME_NO_RESPONSE".into());
        }
        // 收尾期间被停止或更替（代次失配）：本轮按“已取消”收尾——不进
        // Speaking、不播放、不派摘要；turn_meta 由阶段三标注 cancelled。
        let superseded =
            self.finalize_generation != plan.generation || self.control.stop_requested();
        if !superseded && plan.pump_running {
            let mut updated_history: Vec<(String, String)> = plan
                .history
                .iter()
                .map(|entry| (entry.user_text.clone(), entry.assistant_text.clone()))
                .collect();
            updated_history.push((turn.user_text.clone(), turn.assistant_text.clone()));
            self.push_realtime_history(updated_history);
        }

        // 阶段 4：未摘要轮次超过阈值时，后台把较早轮次压缩为滚动摘要。
        // 压缩失败不影响本轮回答；落库要等下一次 finalize 开头的轮询。
        let newest_index = plan.history.len() as i64; // 含刚落的这一轮
        if !superseded
            && should_compress(newest_index, plan.summary_upto)
            && self.summary_job.is_none()
        {
            let compress_from = plan.summary_upto + 1;
            let compress_to = newest_index - KEEP_RECENT_TURNS as i64;
            // endpoint/model 解析沿用既有 llm_endpoint 助手；e2e 路由无 llm 端点，
            // 解析失败则静默跳过（e2e 上下文由 instructions 自带）。
            if compress_to >= compress_from
                && let Ok((llm_endpoint, llm_model_id)) = llm_endpoint(&plan.config)
            {
                let old_summary = plan.existing_summary.clone();
                // 复用阶段一取出的 history：与 list_turns 同序，
                // 且 compress_to < newest_index，所需轮次必然都在其中，不重复查库。
                let turns: Vec<String> = plan
                    .history
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

        let candidate_confirmation_required =
            active_session_role_scenario(&plan.config) == Some(RoleScenario::Candidate);
        let mut meta = serde_json::json!({
            "turnId": plan.turn_id,
            "triggerSource": if plan.force_meeting_assistant { "hotkey" } else if plan.user_text.is_some() { "manual" } else { "voice" },
            "userConfirmed": !candidate_confirmation_required,
        });
        if let Some(object) = pump_turn_meta.as_object() {
            for (key, value) in object {
                meta[key.as_str()] = value.clone();
            }
        }
        let mut playback_status = if candidate_confirmation_required {
            "pending_confirmation"
        } else if let Some(status) = pump_playback_status {
            status
        } else if turn.tts_pcm.is_empty() {
            "text_only"
        } else {
            "not_played"
        };
        let mut playback_job = None;
        if candidate_confirmation_required {
            if plan.confirmation_epoch != self.control.confirmation_epoch.load(Ordering::SeqCst) {
                playback_status = "cancelled";
                self.pending_confirmation_epoch = None;
            } else {
                self.pending_confirmation_epoch = Some(plan.confirmation_epoch);
            }
            // Candidate mode is advisory: generated speech must never reach the
            // meeting until the user explicitly confirms the current answer.
        } else if superseded {
            playback_status = "cancelled";
        } else if self.control.take_stop_tts() || self.control.is_cancelled() {
            self.sink.cancel();
            playback_status = "cancelled";
        } else if !turn.tts_pcm.is_empty() {
            if let Some(output) = self.playback.clone() {
                let seconds = turn.tts_pcm.len() as f64 / (24_000.0 * 2.0) + 0.7;
                self.capture
                    .suppress_echo_for(std::time::Duration::from_secs_f64(seconds));
                // 每轮播报从干净旗标开始：上一轮 played 尾窗（700ms）内的真实人声
                // 会残留旗标，不显式消费会让本轮播报一开场即被取消、整段回答被吞。
                self.control.take_barge_in();
                playback_job = Some(PlaybackJob {
                    pcm: turn.tts_pcm.clone(),
                    output,
                    control: Arc::clone(&self.control),
                });
            } else if !self.text_only {
                self.sink.play_pcm(&turn.tts_pcm, 24_000);
                playback_status = "played";
            }
        }
        Ok(TextPersist::Turn(Box::new(TurnPersist {
            session_id: plan.session_id,
            generation: plan.generation,
            turn,
            meta_prelude: meta,
            playback_status,
            playback_job,
        })))
    }

    /// 阶段三后半（持锁）：turn_meta 落库与收尾。无论成功失败（含 `?` 传播），
    /// 在此按代次释放收尾守卫——收尾飞行到此结束。
    pub(crate) fn complete_finalize_playback(
        &mut self,
        persist: TurnPersist,
        playback_status: &'static str,
        playback_error: Option<&'static str>,
        database: &Database,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        let generation = persist.generation;
        let result = self.complete_finalize_playback_inner(
            persist,
            playback_status,
            playback_error,
            database,
        );
        if self.finalizing_generation == Some(generation) {
            self.finalizing_generation = None;
        }
        result
    }

    fn complete_finalize_playback_inner(
        &mut self,
        persist: TurnPersist,
        playback_status: &'static str,
        playback_error: Option<&'static str>,
        database: &Database,
    ) -> Result<Option<CascadeTurn>, SessionServiceError> {
        if playback_status == "played" {
            // 播报尾窗回声抑制：阶段二播放成功后的 700ms 余量。
            self.capture
                .suppress_echo_for(std::time::Duration::from_millis(700));
        }
        if let Some(code) = playback_error {
            self.last_error_code = Some(code.to_owned());
        }
        let mut meta = persist.meta_prelude;
        meta["playbackStatus"] = serde_json::json!(playback_status);
        let store = SessionStore::new(database);
        store.append_event(&persist.session_id, "turn_meta", &meta.to_string())?;
        if self.control.stop_requested() {
            self.finish_stop(database)?;
            return Ok(Some(persist.turn));
        }
        if self.control.is_cancelled() {
            self.return_to_listening(database, &persist.session_id)?;
            return Ok(Some(persist.turn));
        }
        self.runtime.transition(SessionPhase::Listening)?;
        persist_phase(&store, &persist.session_id, SessionPhase::Listening)?;
        Ok(Some(persist.turn))
    }
}

/// 便捷包装用的播放执行：返回 (状态, 失败码)。
fn run_persist_playback(persist: &TurnPersist) -> (&'static str, Option<&'static str>) {
    match &persist.playback_job {
        None => (persist.playback_status, None),
        Some(job) => match run_finalize_playback(job) {
            Ok(status) => (status, None),
            Err(code) => ("failed", Some(code)),
        },
    }
}

/// 阶段二播放（不持锁）：取消回调读原子旗标，随时可被停止/打断。
pub(crate) fn run_finalize_playback(job: &PlaybackJob) -> Result<&'static str, &'static str> {
    let result = job.output.play(&job.pcm, 24_000, || {
        job.control.is_cancelled() || job.control.barge_in_requested()
    });
    match result {
        Ok(()) => Ok("played"),
        Err("PLAYBACK_CANCELLED") if job.control.take_barge_in() => Ok("interrupted"),
        Err(code) => Err(code),
    }
}

/// 阶段二网络（不持锁）：会议 ASR、端到端泵轮次、级联/实时调用。
/// 只读 `FinalizePlan` 快照与调用方传入的 probes/credentials/database（`Arc` 解引用）。
pub(crate) fn finalize_network(
    plan: &FinalizePlan,
    database: &Database,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    hooks: &TurnStreamHooks<'_>,
) -> NetworkOutcome {
    let config = &plan.config;
    let user_text = plan.user_text.as_deref();
    let transcribed_meeting_text = if user_text.is_none()
        && plan.meeting_bridge
        && plan.role_scenario == Some(RoleScenario::MeetingAssistant)
        && !plan.e2e_route
    {
        match transcribe_meeting_pcm(probes.asr, config, credentials, &plan.pcm) {
            Ok(text) => Some(text),
            Err(error) => return NetworkOutcome::Cascade(error),
        }
    } else {
        None
    };
    let effective_user_text = user_text.or(transcribed_meeting_text.as_deref());
    let meeting_role_name = plan.meeting_role_name.as_deref();
    let transcript_only = !plan.force_meeting_assistant
        && transcribed_meeting_text.as_deref().is_some_and(|text| {
            !meeting_assistant_was_mentioned(text, meeting_role_name.unwrap_or("会议助手"))
        });
    // mode 快照：`run_cascade_turn` 只读 `can_answer`（mode 比较），
    // 语义与原实现一致（mode 在 finalize 入口自 control 同步）。
    let mut runtime_snapshot = SessionRuntime::new();
    runtime_snapshot.set_mode(plan.mode);
    let deps = CascadeTurnDeps {
        asr: probes.asr,
        llm: probes.llm,
        tts: probes.tts,
        embed: probes.embed,
        database,
        runtime: &runtime_snapshot,
        sleep: &std::thread::sleep,
    };
    let request = CascadeTurnRequest {
        config,
        credentials,
        pcm: effective_user_text.is_none().then_some(plan.pcm.as_slice()),
        sample_rate: ASR_SAMPLE_RATE,
        user_text: effective_user_text,
        history: &plan.history_after,
        context_summary: Some(plan.existing_summary.as_str()),
    };
    let mut pump_playback_status: Option<&'static str> = None;
    let mut pump_turn_meta = serde_json::Value::Null;
    // 看门狗放弃的轮次：落库后把 REALTIME_NO_RESPONSE 透给 runtime status。
    let mut pump_response_failed = false;
    let turn = if transcript_only {
        CascadeTurn {
            user_text: transcribed_meeting_text.unwrap_or_default(),
            assistant_text: String::new(),
            tts_pcm: Vec::new(),
            citations: Vec::new(),
            materials_used: false,
            error_code: None,
            timeline: crate::services::realtime_pump::TurnTimeline::default(),
        }
    } else if plan.e2e_route && plan.realtime_shared.is_some() && user_text.is_none() {
        // 端到端流式路线：轮次已由泵完成（含播放/打断），这里只取结果落库。
        // 手动文本（composer 输入）不经过泵：等泵轮次只会把输入静默丢弃，
        // 与 say 相同走独立的 text_turn 连接（见下方 run_e2e_turn 分支）。
        // 先取轮后查失败：历史遗留的终局失败（如 401 后槽未消费）不得误吞
        // 已排队的完整轮次；仅当本轮确实无内容可落库时才上报失败。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let mut completed = plan
            .realtime_shared
            .as_ref()
            .and_then(|shared| shared.take_completed());
        while completed.is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
            completed = plan
                .realtime_shared
                .as_ref()
                .and_then(|shared| shared.take_completed());
        }
        let Some(completed) = completed else {
            if let Some(reason) = plan.realtime_shared.as_ref().and_then(|shared| {
                shared
                    .failed
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
            }) {
                return NetworkOutcome::RealtimePumpFailure(reason);
            }
            // 就绪信号竞态兜底：无可落库轮次，不打扰前端。
            return NetworkOutcome::Idle;
        };
        pump_turn_meta = serde_json::json!({
            "latencyMode": "realtime",
            "latencyMsFirstAudio": completed.first_audio_ms,
            "ingressDropped": plan.tap_dropped,
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
        if completed.forced {
            // 热键/按钮强制回答的轮次：覆盖默认的 voice 触发来源。
            pump_turn_meta["triggerSource"] = serde_json::json!("hotkey");
        }
        if completed.response_failed {
            // 回答始终未开始（看门狗放弃）：turn_meta 留痕并上报错误码。
            pump_turn_meta["responseFailed"] = serde_json::json!(true);
            pump_response_failed = true;
        }
        pump_playback_status = if completed.interrupted {
            Some("interrupted")
        } else if completed.transcript_only {
            Some("text_only")
        } else if completed.held {
            None // 候选闸门：由 candidate 分支标注 pending_confirmation
        } else if completed.response_failed
            || completed.playback_write_failed
            || !completed.playback_alive
        {
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
            timeline: crate::services::realtime_pump::TurnTimeline::default(),
        }
    } else if plan.e2e_route {
        match run_e2e_turn(
            probes.realtime,
            &deps,
            &request,
            &plan.pcm,
            plan.control.cancel_flag(),
            hooks,
        ) {
            Ok(mut turn) => {
                // e2e 非泵路径：时间线随轮次写入 turn_meta（latencyMode 标注 realtime）。
                pump_turn_meta = serde_json::json!({
                    "latencyMode": "realtime",
                    "timeline": turn.timeline,
                });
                if let Some(first_audio) = turn.timeline.response_done_ms {
                    // e2e 音频随轮次一次到达：轮次完成即首包可听时刻。
                    pump_turn_meta["latencyMsFirstAudio"] = serde_json::json!(first_audio);
                }
                turn.timeline = crate::services::realtime_pump::TurnTimeline::default();
                turn
            }
            Err(error) => return NetworkOutcome::Realtime(error),
        }
    } else {
        match run_cascade_turn(&deps, request, plan.control.cancel_flag(), hooks) {
            Ok(mut turn) => {
                // 级联路径：ASR/RAG/LLM/TTS 分阶段时间线写入 turn_meta。
                pump_turn_meta = serde_json::json!({
                    "latencyMode": "cascade",
                    "timeline": turn.timeline,
                });
                if let Some(first_audio) = turn.timeline.tts_done_ms {
                    // 级联的「首响」= TTS 合成完成（播放紧随其后）。
                    pump_turn_meta["latencyMsFirstAudio"] = serde_json::json!(first_audio);
                }
                turn.timeline = crate::services::realtime_pump::TurnTimeline::default();
                turn
            }
            Err(error) => return NetworkOutcome::Cascade(error),
        }
    };
    NetworkOutcome::Turn {
        turn,
        pump_turn_meta,
        pump_playback_status,
        pump_response_failed,
    }
}
