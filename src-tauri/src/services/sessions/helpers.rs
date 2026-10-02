//! 会话落库与文本/端点辅助：persist、端点解析、命令文本生成、回声/终态判定（纯搬移自 mod.rs）。

use super::*;

/// 距上次摘要累计的未压缩轮次是否已超过触发阈值。
pub(super) fn should_compress(newest_index: i64, upto: i64) -> bool {
    newest_index - upto > COMPRESS_THRESHOLD + KEEP_RECENT_TURNS as i64
}

pub(super) struct CommandGenerate<'a> {
    pub(super) probes: &'a SessionProbes<'a>,
    pub(super) config: &'a PublicConfig,
    pub(super) credentials: CascadeCredentials<'a>,
    pub(super) history: &'a [HistoryTurn],
    pub(super) prompt: &'a str,
    pub(super) e2e_route: bool,
    pub(super) include_audio: bool,
    pub(super) hooks: &'a TurnStreamHooks<'a>,
}

pub(super) fn generate_command_text(
    request: CommandGenerate<'_>,
    cancel: &AtomicBool,
) -> Result<(String, Vec<u8>), SessionServiceError> {
    if request.e2e_route {
        let (endpoint, model_id) = e2e_endpoint(request.config)?;
        let instructions = e2e_instructions(
            active_role_profile(request.config),
            &[],
            active_session_role_scenario(request.config).as_ref(),
        );
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

pub(super) fn speak_command_text(
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

pub(super) fn e2e_endpoint(
    config: &PublicConfig,
) -> Result<(ProviderEndpoint, String), SessionServiceError> {
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

pub(super) fn llm_endpoint(
    config: &PublicConfig,
) -> Result<(ProviderEndpoint, String), SessionServiceError> {
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

pub(super) fn command_messages(
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

pub(super) fn command_prompt(history: &[HistoryTurn], prompt: &str) -> String {
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

pub(super) fn persist_snapshot(
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

pub(super) fn persist_phase(
    store: &SessionStore<'_>,
    session_id: &str,
    phase: SessionPhase,
) -> Result<(), SessionServiceError> {
    store.set_status(session_id, phase.as_str())?;
    store.append_event(session_id, "status", &status_payload(phase))?;
    Ok(())
}

pub(super) fn status_payload(phase: SessionPhase) -> String {
    serde_json::json!({ "status": phase.as_str() }).to_string()
}

pub(super) fn mode_name(mode: AgentMode) -> &'static str {
    mode.as_str()
}

pub(super) fn mode_u8(mode: AgentMode) -> u8 {
    match mode {
        AgentMode::AiActive => 0,
        AgentMode::OperatorSpeaking => 1,
        AgentMode::Paused => 2,
        AgentMode::Muted => 3,
    }
}

pub(super) fn mode_from_u8(value: u8) -> AgentMode {
    match value {
        1 => AgentMode::OperatorSpeaking,
        2 => AgentMode::Paused,
        3 => AgentMode::Muted,
        _ => AgentMode::AiActive,
    }
}

pub(super) fn is_fatal_cascade(error: &CascadeError) -> bool {
    matches!(
        error,
        CascadeError::Unauthorized(_) | CascadeError::EndpointInvalid(_)
    )
}

pub(super) fn is_fatal_realtime(error: &RealtimeError) -> bool {
    matches!(
        error,
        RealtimeError::Unauthorized | RealtimeError::UrlInvalid
    )
}

pub(super) fn run_e2e_turn(
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
    // 分阶段打点（相对本函数入口）：e2e 非泵路径的转写/补全/音频随单次请求
    // 一次性返回，无法拆出 ASR/LLM/TTS 各自的完成点，只记检索、首 token 与
    // 轮次完成三个锚点（见时间线字段注释）。
    let started = std::time::Instant::now();
    let retrieval_done_ms = started.elapsed().as_millis() as u64;
    let mut timeline = crate::services::realtime_pump::TurnTimeline {
        retrieval_done_ms: Some(retrieval_done_ms),
        ..crate::services::realtime_pump::TurnTimeline::default()
    };
    let instructions = e2e_instructions(
        active_role_profile(request.config),
        &citations,
        active_session_role_scenario(request.config).as_ref(),
    );
    // 首个增量快照即首 token：包一层回调记录时间点后转发原钩子。
    let first_token = std::cell::OnceCell::<Option<u64>>::new();
    let user_snapshot = hooks.assistant_text;
    let on_snapshot = |text: &str| {
        if first_token.get().is_none() {
            let _ = first_token.set(Some(started.elapsed().as_millis() as u64));
        }
        if let Some(snapshot) = user_snapshot {
            snapshot(text);
        }
    };
    let snapshot_ref: &dyn Fn(&str) = &on_snapshot;
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
                    assistant_text: Some(snapshot_ref),
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
                hooks: TurnStreamHooks {
                    user_text: hooks.user_text,
                    assistant_text: Some(snapshot_ref),
                },
            },
            cancel,
        )
    }?;
    if cancel.load(Ordering::SeqCst) {
        return Err(RealtimeError::Cancelled);
    }
    // 轮次完成：e2e 非泵路径的补全结束、TTS 音频到达与首包同点收束。
    let done_ms = started.elapsed().as_millis() as u64;
    timeline.llm_first_token_ms = first_token.get().copied().flatten();
    timeline.llm_done_ms = Some(done_ms);
    timeline.tts_done_ms = Some(done_ms);
    timeline.response_done_ms = Some(done_ms);
    let user_text = known_text.map(ToOwned::to_owned).unwrap_or(turn.user_text);
    Ok(CascadeTurn {
        user_text,
        assistant_text: turn.assistant_text,
        tts_pcm: turn.tts_pcm,
        materials_used: !citations.is_empty(),
        citations,
        error_code: None,
        timeline: Box::new(timeline),
    })
}

/// 端到端线路的会话指令：角色提示词、风格说明、点名规则的会议场景固定段、
/// 资料引用。会议点名门控由泵与 finalize 的 mention 判定在协议层保证，
/// 这里的固定段只影响被点名后的回应行为（知道自己的名字、不抢答不闲聊）。
pub(crate) fn e2e_instructions(
    role: Option<&crate::config::RoleProfileConfig>,
    citations: &[crate::runtime::TurnCitation],
    scenario: Option<&RoleScenario>,
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
    if scenario == Some(&RoleScenario::MeetingAssistant) {
        let name = role
            .map(|role| role.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or("会议助手");
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!(
            "【点名规则】你的名字是「{name}」。会议中只有参会者喊到你的名字时才开口回应；\
             未被点名时保持安静，不要主动插话或总结。"
        ));
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

pub(super) fn is_terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "interrupted")
}

pub(super) fn is_terminal_phase(phase: SessionPhase) -> bool {
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

pub(super) fn transcribe_meeting_pcm(
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

/// 点名判定：文本与角色名先统一规范化（去空白与中英文标点、英文转小写、
/// 全角折半角），「会议 助手，你好」「AI 助手?」这类口语转写也算点名，
/// 而「你听得见我说话吗」不算。规范化复用回声判定同款实现，两侧行为一致。
pub(crate) fn meeting_assistant_was_mentioned(text: &str, role_name: &str) -> bool {
    let normalized = crate::services::echo_guard::normalize(text);
    let name = crate::services::echo_guard::normalize(role_name);
    (!name.is_empty() && normalized.contains(&name))
        || normalized.contains("会议助手")
        || normalized.contains("ai助手")
}

pub(super) fn with_default_voice(config: &PublicConfig) -> PublicConfig {
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

pub(super) fn truncate(text: &str) -> String {
    text.chars().take(160).collect()
}
