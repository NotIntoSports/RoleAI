use std::sync::{
    Arc, TryLockError,
    atomic::{AtomicBool, Ordering},
};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::{
    app_state::AppState,
    config::{
        EmbeddingConfig, ProviderConfig, PublicConfig, RoleProfileConfig,
        VoiceRouteConfig, diagnostic_view, public_view,
    },
    contracts::{
        AgentCommandInput, AgentCommandResult, AudioLevelEvent, CommandResult,
        DiagnosticsExportResult, FoundationStatus, LegacyMigrationStatus, LegacySessionImport,
        LivestreamDraftInput, LivestreamGenerateInput, LivestreamRuntime, MicPcmAcceptance,
        RuntimeStatus, SessionCitationView, SessionDetail, SessionExportResult, SessionReplyEvent,
        SessionStartResult, SessionSummary, SessionTranscriptEvent, SessionTurnView, StartupState,
    },
    error::PublicError,
    providers::{
        ChatMessage, ChatModel, OpenAiCompatibleCascade,
        OpenAiCompatibleEmbeddingProbe, OpenAiCompatibleProbe, OpenAiCompatibleRealtime,
        ProviderEndpoint, StandardRouteProbe, TextToSpeech, VoiceCloneProbe,
    },
    runtime::{
        AgentMode, CascadeCredentials, active_embedding, active_voice_route, parse_agent_command,
        preflight,
    },
    services::{
        EmbeddingConfigSaveInput, EmbeddingService, EmbeddingServiceError, EmbeddingTestResult,
        MaterialIndexResult, MaterialSearchHit, MaterialService,
        MaterialServiceError, MaterialSummary, ModelDiscoveryResult, ProviderSaveInput,
        ProviderService, ProviderServiceError, ProviderTestResult, RoleProfileCopyInput,
        RoleProfileSaveInput, RoleProfileService, RoleProfileServiceError, SessionProbes,
        SessionServiceError, SessionStartOutcome, VoiceReferenceAudioSaveInput,
        VoiceReferenceCloneResult, VoiceReferenceSaveInput, VoiceReferenceService,
        VoiceReferenceServiceError, VoiceReferenceSummary, VoiceReferenceUpdateInput,
        VoiceRouteSaveInput, VoiceRouteService, VoiceRouteServiceError, VoiceRouteTestResult,
    },
    sessions::{SessionExportError, SessionExportFormat, SessionStore, export_session},
};

const OBS_PASSWORD_REF: &str = "obs/websocket-password";

#[tauri::command]
pub fn foundation_get_status() -> CommandResult<FoundationStatus> {
    CommandResult::Ok {
        data: FoundationStatus { ready: true },
    }
}

#[tauri::command]
pub fn livestream_create_draft(
    state: State<'_, AppState>,
    input: LivestreamDraftInput,
) -> CommandResult<LivestreamRuntime> {
    let media_path = match resolve_stage_media(input.media_path.as_deref(), input.media_kind) {
        Ok(path) => path,
        Err(error) => return CommandResult::Err { error },
    };
    let segments = input
        .segments
        .into_iter()
        .map(|segment| {
            crate::livestream::LivestreamSegment::draft(
                segment.title,
                segment.text,
                segment.estimated_seconds,
                segment.sources,
            )
        })
        .collect();
    let script = match crate::livestream::LivestreamScript::draft(
        input.title.clone(),
        segments,
        input.loop_enabled,
    ) {
        Ok(script) => script,
        Err(_) => {
            return CommandResult::Err {
                error: PublicError::new("LIVESTREAM_SCRIPT_INVALID", "直播讲稿无效", false),
            };
        }
    };
    let stage = crate::livestream::LivestreamStageState {
        product_title: input.title,
        current_subtitle: String::new(),
        next_hint: script
            .segments
            .first()
            .map(|segment| format!("下一段：{}", segment.title))
            .unwrap_or_default(),
        state: script.state,
        media_path,
        media_kind: input.media_kind,
        output_state: crate::livestream::LivestreamOutputState::Idle,
        output_error_code: None,
    };
    replace_livestream_runtime(&state, script, stage)
}

#[tauri::command]
pub fn livestream_generate(
    state: State<'_, AppState>,
    input: LivestreamGenerateInput,
) -> CommandResult<LivestreamRuntime> {
    let title = input.title.trim();
    let language = input.language.trim();
    let max_segments = usize::from(input.max_segments);
    if title.is_empty()
        || title.len() > 200
        || language.is_empty()
        || language.len() > 40
        || !(1..=12).contains(&max_segments)
        || input.material_ids.is_empty()
        || input.material_ids.len() > 20
        || input
            .material_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 128)
    {
        return service_error("LIVESTREAM_GENERATE_INVALID", "直播讲稿生成参数无效");
    }
    let database_slot = match state.database.lock() {
        Ok(slot) => slot,
        Err(_) => return service_error("DATABASE_OPERATION_FAILED", "资料库暂时不可用"),
    };
    let Some(database) = database_slot.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "资料库尚未就绪");
    };
    let mut documents = Vec::new();
    let mut source_names = Vec::new();
    let mut total_chars = 0usize;
    for id in &input.material_ids {
        let row = database.with_connection(|connection| {
            connection.query_row(
                "SELECT m.file_name, d.extracted_text
                 FROM materials m JOIN material_documents d ON d.material_id = m.id
                 WHERE m.id = ?1 AND m.retrieval_blocked = 0 AND m.status = 'text_ready'",
                rusqlite::params![id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
        });
        let Ok((name, text)) = row else {
            return service_error("LIVESTREAM_MATERIAL_NOT_READY", "所选产品资料不可用");
        };
        let remaining = 64 * 1024usize - total_chars.min(64 * 1024);
        if remaining == 0 {
            break;
        }
        let excerpt = text.chars().take(remaining).collect::<String>();
        total_chars += excerpt.chars().count();
        source_names.push(name.clone());
        documents.push(format!("资料：{name}\n{excerpt}"));
    }
    drop(database_slot);
    if documents.is_empty() {
        return service_error("LIVESTREAM_MATERIAL_NOT_READY", "所选产品资料不可用");
    }
    let config = match state.config.load() {
        Ok(config) => public_view(&config),
        Err(error) => return service_error(error.code(), "模型配置不可用"),
    };
    let route = match active_voice_route(&config) {
        Some(route) if route.mode == crate::config::VoiceRouteMode::Cascaded => route,
        _ => return service_error("LIVESTREAM_MODEL_REQUIRED", "请选择可用的级联语音线路"),
    };
    let provider_id = match route.llm_provider_id.as_deref() {
        Some(id) => id,
        None => return service_error("LIVESTREAM_MODEL_REQUIRED", "直播模型尚未配置"),
    };
    let model_id = match route.llm_model_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return service_error("LIVESTREAM_MODEL_REQUIRED", "直播模型尚未配置"),
    };
    let provider = match config
        .models
        .providers
        .iter()
        .find(|item| item.id == provider_id)
    {
        Some(provider) => provider,
        None => return service_error("LIVESTREAM_MODEL_REQUIRED", "直播模型供应商不存在"),
    };
    let secret = match read_provider_secret(&state, &config, Some(provider_id)) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let model = match OpenAiCompatibleCascade::new() {
        Ok(model) => model,
        Err(error) => return service_error(error.code(), "无法初始化直播模型"),
    };
    let prompt = format!(
        "产品标题：{title}\n讲解语言：{language}\n最多 {max_segments} 段。\n只依据以下本地资料生成有限讲稿。不得编造价格、库存、优惠或效果。只输出 JSON：{{\"segments\":[{{\"title\":\"\",\"text\":\"\",\"estimatedSeconds\":30,\"sources\":[\"资料文件名\"]}}]}}。\n\n{}",
        documents.join("\n\n---\n\n")
    );
    let response = match model.complete(
        &ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        },
        secret.as_deref().map(|value| value.as_str()),
        model_id,
        &[
            ChatMessage {
                role: "system".into(),
                content: "你是产品直播讲稿编辑器，只能使用提供的资料，并严格输出 JSON。".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: prompt,
            },
        ],
    ) {
        Ok(response) => response,
        Err(error) => return service_error(error.code(), "直播讲稿生成失败"),
    };
    let mut segments = match crate::livestream::parse_generated_segments(&response, max_segments) {
        Ok(segments) => segments,
        Err(_) => {
            return service_error("LIVESTREAM_MODEL_RESPONSE_INVALID", "模型没有返回有效讲稿");
        }
    };
    for segment in &mut segments {
        segment.sources = source_names.clone();
    }
    let media_path = match resolve_stage_media(input.media_path.as_deref(), input.media_kind) {
        Ok(path) => path,
        Err(error) => return CommandResult::Err { error },
    };
    save_livestream_runtime(
        &state,
        title.to_owned(),
        segments,
        input.loop_enabled,
        media_path,
        input.media_kind,
    )
}

#[tauri::command]
pub fn livestream_get(state: State<'_, AppState>) -> CommandResult<LivestreamRuntime> {
    let script = state.livestream.lock().ok().and_then(|slot| slot.clone());
    let stage = state
        .livestream_stage
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    match (script, stage) {
        (Some(script), Some(stage)) => CommandResult::Ok {
            data: LivestreamRuntime { script, stage },
        },
        _ => service_error("LIVESTREAM_NOT_FOUND", "尚未创建直播讲稿"),
    }
}

#[tauri::command]
pub fn livestream_control(
    app: AppHandle,
    state: State<'_, AppState>,
    action: String,
) -> CommandResult<LivestreamRuntime> {
    match prepare_livestream_control(&state, &action) {
        Ok((data, playback)) => {
            if let Some(playback) = playback {
                spawn_livestream_playback(app, playback);
            }
            CommandResult::Ok { data }
        }
        Err(error) => CommandResult::Err { error },
    }
}

#[derive(Clone)]
struct LivestreamPlayback {
    text: String,
    auto_advance: bool,
    cancel: Arc<AtomicBool>,
    voice: Result<crate::livestream::LivestreamVoiceSnapshot, &'static str>,
}

fn livestream_busy() -> PublicError {
    PublicError::new("SERVICE_BUSY", "直播状态暂时不可用", true)
}

fn prepare_livestream_control(
    state: &AppState,
    action: &str,
) -> Result<(LivestreamRuntime, Option<LivestreamPlayback>), PublicError> {
    // Configuration reads happen outside the state transaction. The chosen
    // snapshot and token are captured together before scheduling any worker.
    let voice_candidate = resolve_livestream_voice(state);
    let mut cancel_slot = state.livestream_playback_cancel.lock().map_err(|_| livestream_busy())?;
    let mut script_slot = match state.livestream.lock() {
        Ok(slot) => slot,
        Err(_) => return Err(livestream_busy()),
    };
    let mut stage_slot = match state.livestream_stage.lock() {
        Ok(slot) => slot,
        Err(_) => return Err(livestream_busy()),
    };
    let (Some(script), Some(stage)) = (script_slot.as_mut(), stage_slot.as_mut()) else {
        return Err(PublicError::new("LIVESTREAM_NOT_FOUND", "尚未创建直播讲稿", false));
    };
    let mut next_script = script.clone();
    let result = match action {
        "confirm" => next_script.confirm().map(|_| ()),
        "start" => next_script.start().map(|_| ()),
        "pause" | "takeover" => next_script.pause(),
        "resume" => next_script.resume().map(|_| ()),
        "previous" => next_script.previous().map(|_| ()),
        "next" => next_script.next().map(|_| ()),
        "replay" => next_script.replay().map(|_| ()),
        "complete" => next_script.complete_current(),
        _ => Err(crate::livestream::LivestreamError::Invalid),
    };
    if let Err(error) = result {
        return Err(PublicError::new(
            match error {
                crate::livestream::LivestreamError::ConfirmationRequired => {
                    "LIVESTREAM_CONFIRMATION_REQUIRED"
                }
                crate::livestream::LivestreamError::Finished => "LIVESTREAM_FINISHED",
                crate::livestream::LivestreamError::Invalid => "LIVESTREAM_CONTROL_INVALID",
                crate::livestream::LivestreamError::StateInvalid => "LIVESTREAM_STATE_INVALID",
            },
            "直播控制未执行",
            false,
        ));
    }
    if action != "confirm" {
        cancel_slot.store(true, Ordering::SeqCst);
    }
    *script = next_script;
    update_stage_from_script(stage, script);
    match action {
        "start" | "resume" | "previous" | "next" | "replay" => {
            stage.output_state = crate::livestream::LivestreamOutputState::Synthesizing;
            stage.output_error_code = None;
        }
        "pause" | "takeover" => {
            stage.output_state = crate::livestream::LivestreamOutputState::Cancelled;
            stage.output_error_code = None;
        }
        "complete" => {
            // Manual skip is not proof of playback.
            stage.output_state = crate::livestream::LivestreamOutputState::Cancelled;
            stage.output_error_code = None;
        }
        _ => {}
    }
    write_livestream_stage(state, stage)?;
    let output = LivestreamRuntime {
        script: script.clone(),
        stage: stage.clone(),
    };
    let speech_text = matches!(
        action,
        "start" | "resume" | "previous" | "next" | "replay"
    )
    .then(|| stage.current_subtitle.clone())
    .filter(|text| !text.is_empty());
    let playback = speech_text.map(|text| {
        *cancel_slot = Arc::new(AtomicBool::new(false));
        LivestreamPlayback {
            text,
            auto_advance: true,
            cancel: Arc::clone(&cancel_slot),
            voice: freeze_livestream_voice(state, voice_candidate),
        }
    });
    Ok((output, playback))
}

#[tauri::command]
pub async fn livestream_insert_question(
    app: AppHandle,
    question: String,
) -> CommandResult<LivestreamRuntime> {
    dispatch_blocking(move || livestream_insert_question_blocking(app, question)).await
}

fn livestream_insert_question_blocking(
    app: AppHandle,
    question: String,
) -> CommandResult<LivestreamRuntime> {
    let question = question.trim();
    if question.is_empty() || question.len() > 4096 {
        return service_error("LIVESTREAM_QUESTION_INVALID", "人工问题无效");
    }
    let state = app.state::<AppState>();
    let cancel = match replace_livestream_playback_token(&state) {
        Some(token) => token,
        None => return service_error("SERVICE_BUSY", "直播状态暂时不可用"),
    };
    let context = {
        let mut script_slot = match state.livestream.lock() {
            Ok(slot) => slot,
            Err(_) => return service_error("SERVICE_BUSY", "直播状态暂时不可用"),
        };
        let Some(script) = script_slot.as_mut() else {
            return service_error("LIVESTREAM_NOT_FOUND", "尚未创建直播讲稿");
        };
        if !script.confirmed {
            return service_error("LIVESTREAM_CONFIRMATION_REQUIRED", "请先确认直播讲稿");
        }
        if script.state == crate::livestream::LivestreamState::Playing {
            let _ = script.pause();
        }
        script
            .segments
            .iter()
            .map(|segment| format!("{}：{}", segment.title, segment.text))
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(64 * 1024)
            .collect::<String>()
    };
    let config = match state.config.load() {
        Ok(config) => public_view(&config),
        Err(error) => return service_error(error.code(), "模型配置不可用"),
    };
    let route = match active_voice_route(&config) {
        Some(route) if route.mode == crate::config::VoiceRouteMode::Cascaded => route,
        _ => return service_error("LIVESTREAM_MODEL_REQUIRED", "请选择可用的级联语音线路"),
    };
    let provider_id = match route.llm_provider_id.as_deref() {
        Some(id) => id,
        None => return service_error("LIVESTREAM_MODEL_REQUIRED", "直播模型尚未配置"),
    };
    let model_id = match route.llm_model_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        _ => return service_error("LIVESTREAM_MODEL_REQUIRED", "直播模型尚未配置"),
    };
    let provider = match config
        .models
        .providers
        .iter()
        .find(|item| item.id == provider_id)
    {
        Some(provider) => provider,
        None => return service_error("LIVESTREAM_MODEL_REQUIRED", "直播模型供应商不存在"),
    };
    let secret = match read_provider_secret(&state, &config, Some(provider_id)) {
        Ok(secret) => secret,
        Err(error) => return CommandResult::Err { error },
    };
    let model = match OpenAiCompatibleCascade::new() {
        Ok(model) => model,
        Err(error) => return service_error(error.code(), "无法初始化直播模型"),
    };
    let answer = match model.complete(
        &ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        },
        secret.as_deref().map(|value| value.as_str()),
        model_id,
        &[
            ChatMessage {
                role: "system".into(),
                content: "你是直播讲解员。只依据已确认讲稿回答人工问题；资料不足时明确说不知道，不得编造价格、库存、优惠或效果。只输出要播报的简短答案。".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: format!("已确认讲稿：\n{context}\n\n人工问题：{question}"),
            },
        ],
    ) {
        Ok(answer) if !answer.trim().is_empty() => answer.trim().chars().take(4096).collect::<String>(),
        Ok(_) => return service_error("LIVESTREAM_QUESTION_EMPTY", "模型没有生成可用回答"),
        Err(error) => return service_error(error.code(), "人工问题回答失败"),
    };
    if cancel.load(Ordering::SeqCst) {
        return service_error("PLAYBACK_CANCELLED", "人工问题已取消");
    }
    let output = {
        let script_slot = match state.livestream.lock() {
            Ok(slot) => slot,
            Err(_) => return service_error("SERVICE_BUSY", "直播状态暂时不可用"),
        };
        let mut stage_slot = match state.livestream_stage.lock() {
            Ok(slot) => slot,
            Err(_) => return service_error("SERVICE_BUSY", "直播舞台暂时不可用"),
        };
        let (Some(script), Some(stage)) = (script_slot.as_ref(), stage_slot.as_mut()) else {
            return service_error("LIVESTREAM_NOT_FOUND", "尚未创建直播讲稿");
        };
        stage.state = script.state;
        stage.current_subtitle = answer.clone();
        stage.next_hint = "人工回答结束后可继续讲稿".into();
        stage.output_state = crate::livestream::LivestreamOutputState::Synthesizing;
        stage.output_error_code = None;
        if let Err(error) = write_livestream_stage(&state, stage) {
            return CommandResult::Err { error };
        }
        LivestreamRuntime {
            script: script.clone(),
            stage: stage.clone(),
        }
    };
    let voice = resolve_livestream_voice(&state);
    spawn_livestream_playback(
        app,
        LivestreamPlayback {
            text: answer,
            auto_advance: false,
            cancel,
            voice,
        },
    );
    CommandResult::Ok { data: output }
}

fn spawn_livestream_playback(app: AppHandle, playback: LivestreamPlayback) {
    let LivestreamPlayback {
        text,
        auto_advance,
        cancel,
        voice,
    } = playback;
    let scheduled_voice = voice.clone();
    // Capture the cancellation identity at scheduling time, never after the
    // worker starts: a queued old paragraph must not adopt a newer token. The
    // voice snapshot obeys the same rule: segments already queued keep the
    // voice frozen when they were scheduled, never a later config change.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let result = (|| -> Result<(), &'static str> {
            if cancel.load(Ordering::SeqCst) {
                return Err("PLAYBACK_CANCELLED");
            }
            let voice = voice?;
            let config = state
                .config
                .load()
                .map(|config| public_view(&config))
                .map_err(|_| "LIVESTREAM_VOICE_CONFIG_FAILED")?;
            let secret = read_provider_secret(&state, &config, Some(&voice.provider_id))
                .map_err(|_| "LIVESTREAM_TTS_CREDENTIAL_FAILED")?;
            let tts =
                OpenAiCompatibleCascade::new().map_err(|_| "LIVESTREAM_TTS_INITIALIZE_FAILED")?;
            let pcm = tts
                .synthesize(
                    &ProviderEndpoint {
                        provider_id: voice.provider_id.clone(),
                        base_url: voice.base_url.clone(),
                    },
                    secret.as_deref().map(|value| value.as_str()),
                    &voice.model_id,
                    &voice.voice_id,
                    &text,
                )
                .map_err(|_| "LIVESTREAM_TTS_FAILED")?;
            if pcm.is_empty() {
                return Err("LIVESTREAM_TTS_EMPTY");
            }
            if cancel.load(Ordering::SeqCst) {
                return Err("PLAYBACK_CANCELLED");
            }
            set_livestream_output_state(
                &state,
                &cancel,
                crate::livestream::LivestreamOutputState::Playing,
                None,
            );
            let bridge = audio_bridge_path(&app)?;
            let devices = crate::prerequisites::enumerate_audio_devices(&bridge)?;
            let preparation = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
            let endpoint_id = preparation
                .render_endpoint_id
                .ok_or("VIRTUAL_AUDIO_RENDER_MISSING")?;
            crate::audio::playback::BridgePlayback {
                executable: bridge,
                endpoint_id,
            }
            .play(&pcm, 24_000, || cancel.load(Ordering::SeqCst))
        })();
        match result {
            Ok(()) => {
                if auto_advance {
                    if let Some(next_text) = advance_livestream_after_success(&state, &cancel) {
                        spawn_livestream_playback(
                            app,
                            LivestreamPlayback {
                                text: next_text,
                                auto_advance: true,
                                cancel: Arc::clone(&cancel),
                                voice: scheduled_voice,
                            },
                        );
                    }
                } else {
                    set_livestream_output_state(
                        &state,
                        &cancel,
                        crate::livestream::LivestreamOutputState::Played,
                        None,
                    );
                }
            }
            Err("PLAYBACK_CANCELLED") => set_livestream_output_state(
                &state,
                &cancel,
                crate::livestream::LivestreamOutputState::Cancelled,
                None,
            ),
            Err(code) => set_livestream_output_state(
                &state,
                &cancel,
                crate::livestream::LivestreamOutputState::Failed,
                Some(code),
            ),
        }
    });
}

fn advance_livestream_after_success(state: &AppState, token: &Arc<AtomicBool>) -> Option<String> {
    let current = state.livestream_playback_cancel.lock().ok()?;
    if !Arc::ptr_eq(&current, token) || token.load(Ordering::SeqCst) {
        return None;
    }
    let mut script_slot = state.livestream.lock().ok()?;
    let mut stage_slot = state.livestream_stage.lock().ok()?;
    let script = script_slot.as_mut()?;
    let stage = stage_slot.as_mut()?;
    // Only an actively playing script may auto-advance; a user-paused or
    // finished script must stay where it is until the user resumes it.
    if script.state != crate::livestream::LivestreamState::Playing {
        return None;
    }
    if script.complete_current().is_err() {
        return None;
    }
    let next_text = match script.next() {
        Ok(segment) => Some(segment.text.clone()),
        Err(crate::livestream::LivestreamError::Finished) => None,
        Err(_) => {
            stage.output_state = crate::livestream::LivestreamOutputState::Failed;
            stage.output_error_code = Some("LIVESTREAM_AUTO_ADVANCE_FAILED".into());
            let _ = write_livestream_stage(state, stage);
            return None;
        }
    };
    update_stage_from_script(stage, script);
    stage.output_state = if next_text.is_some() {
        crate::livestream::LivestreamOutputState::Synthesizing
    } else {
        crate::livestream::LivestreamOutputState::Played
    };
    stage.output_error_code = None;
    let _ = write_livestream_stage(state, stage);
    next_text
}

fn set_livestream_output_state(
    state: &AppState,
    token: &Arc<AtomicBool>,
    output_state: crate::livestream::LivestreamOutputState,
    error_code: Option<&str>,
) {
    let is_current = state
        .livestream_playback_cancel
        .lock()
        .map(|current| Arc::ptr_eq(&current, token) && !token.load(Ordering::SeqCst))
        .unwrap_or(false);
    if !is_current {
        return;
    }
    let mut script_slot = match state.livestream.lock() {
        Ok(slot) => slot,
        Err(_) => return,
    };
    let mut stage_slot = match state.livestream_stage.lock() {
        Ok(slot) => slot,
        Err(_) => return,
    };
    let (Some(script), Some(stage)) = (script_slot.as_mut(), stage_slot.as_mut()) else {
        return;
    };
    stage.output_state = output_state;
    stage.output_error_code = error_code.map(str::to_owned);
    if output_state == crate::livestream::LivestreamOutputState::Failed
        && script.state == crate::livestream::LivestreamState::Playing
    {
        let _ = script.pause();
        update_stage_from_script(stage, script);
    }
    let _ = write_livestream_stage(state, stage);
}

#[tauri::command]
pub fn obs_runtime_status(
    state: State<'_, AppState>,
) -> CommandResult<crate::obs::ObsRuntimeStatus> {
    obs_command(&state, "status")
}

#[tauri::command]
pub fn obs_virtual_camera_start(
    state: State<'_, AppState>,
) -> CommandResult<crate::obs::ObsRuntimeStatus> {
    obs_command(&state, "start")
}

#[tauri::command]
pub fn obs_virtual_camera_stop(
    state: State<'_, AppState>,
) -> CommandResult<crate::obs::ObsRuntimeStatus> {
    obs_command(&state, "stop")
}

fn obs_command(state: &AppState, action: &str) -> CommandResult<crate::obs::ObsRuntimeStatus> {
    let password = match state.secrets.read(OBS_PASSWORD_REF) {
        Ok(value) => value,
        Err(_) => return service_error("SECRET_BACKEND_UNAVAILABLE", "OBS 凭据不可用"),
    };
    let stage_file = state
        .paths
        .data_directory
        .join("livestream")
        .join("stage.html");
    let password = password.as_deref().map(|value| value.as_str());
    let status = match action {
        "start" => {
            let (status, previous) = tauri::async_runtime::block_on(async {
                let previous = crate::obs::current_program_scene(password).await.ok();
                let status = crate::obs::start_virtual_camera(password, &stage_file).await;
                (status, previous)
            });
            if status.virtual_camera_active
                && let Some(previous) = previous.filter(|scene| scene != crate::obs::APP_SCENE_NAME)
                && let Ok(mut slot) = state.obs_previous_scene.lock()
            {
                *slot = Some(previous);
            }
            status
        }
        "stop" => {
            let previous = state
                .obs_previous_scene
                .lock()
                .ok()
                .and_then(|slot| slot.clone());
            let mut status =
                tauri::async_runtime::block_on(crate::obs::stop_virtual_camera(password));
            if !status.virtual_camera_active
                && let Some(previous) = previous
            {
                if let Err(code) = tauri::async_runtime::block_on(
                    crate::obs::restore_program_scene(password, &previous),
                ) {
                    status.error_code = Some(code.into());
                } else if let Ok(mut slot) = state.obs_previous_scene.lock() {
                    *slot = None;
                }
            }
            status
        }
        _ => tauri::async_runtime::block_on(crate::obs::ensure_stage(password, &stage_file)),
    };
    CommandResult::Ok { data: status }
}

#[tauri::command]
pub fn obs_password_status(
    state: State<'_, AppState>,
) -> CommandResult<crate::contracts::SecretStatus> {
    match state.secrets.status(OBS_PASSWORD_REF) {
        Ok(data) => CommandResult::Ok { data },
        Err(error) => service_error(error.code(), "OBS 凭据状态不可用"),
    }
}

#[tauri::command]
pub fn obs_password_save(
    state: State<'_, AppState>,
    password: String,
) -> CommandResult<crate::contracts::SecretStatus> {
    if password.len() > 1024 || password.contains(['\r', '\n', '\0']) {
        return service_error("OBS_PASSWORD_INVALID", "OBS 密码无效");
    }
    let result = if password.is_empty() {
        state.secrets.delete(OBS_PASSWORD_REF)
    } else {
        state.secrets.set(OBS_PASSWORD_REF, &password)
    };
    match result {
        Ok(data) => CommandResult::Ok { data },
        Err(error) => service_error(error.code(), "OBS 密码保存失败"),
    }
}

fn resolve_stage_media(
    path: Option<&str>,
    kind: Option<crate::livestream::LivestreamMediaKind>,
) -> Result<Option<String>, PublicError> {
    let Some(path) = path.map(str::trim).filter(|path| !path.is_empty()) else {
        return if kind.is_none() {
            Ok(None)
        } else {
            Err(PublicError::new(
                "LIVESTREAM_MEDIA_INVALID",
                "直播素材无效",
                false,
            ))
        };
    };
    let path = std::path::Path::new(path)
        .canonicalize()
        .map_err(|_| PublicError::new("LIVESTREAM_MEDIA_NOT_FOUND", "找不到直播素材", false))?;
    if !path.is_file() || kind.is_none() {
        return Err(PublicError::new(
            "LIVESTREAM_MEDIA_INVALID",
            "直播素材无效",
            false,
        ));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let valid = match kind {
        Some(crate::livestream::LivestreamMediaKind::Image) => {
            matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif")
        }
        Some(crate::livestream::LivestreamMediaKind::Video) => {
            matches!(extension.as_str(), "mp4" | "webm" | "mov" | "mkv")
        }
        None => false,
    };
    if !valid {
        return Err(PublicError::new(
            "LIVESTREAM_MEDIA_TYPE_UNSUPPORTED",
            "不支持的直播素材格式",
            false,
        ));
    }
    Ok(Some(format!(
        "file:///{}",
        path.to_string_lossy().replace('\\', "/")
    )))
}

fn update_stage_from_script(
    stage: &mut crate::livestream::LivestreamStageState,
    script: &crate::livestream::LivestreamScript,
) {
    stage.state = script.state;
    stage.current_subtitle = script
        .current_index
        .and_then(|index| script.segments.get(index))
        .map(|segment| segment.text.clone())
        .unwrap_or_default();
    stage.next_hint = script
        .current_index
        .and_then(|index| script.segments.get(index + 1))
        .map(|segment| format!("下一段：{}", segment.title))
        .unwrap_or_default();
}

fn write_livestream_stage(
    state: &AppState,
    stage: &crate::livestream::LivestreamStageState,
) -> Result<(), PublicError> {
    let directory = state.paths.data_directory.join("livestream");
    std::fs::create_dir_all(&directory).map_err(|_| {
        PublicError::new("LIVESTREAM_STAGE_WRITE_FAILED", "无法创建直播舞台", false)
    })?;
    std::fs::write(
        directory.join("stage.html"),
        crate::livestream::render_stage_html(stage),
    )
    .map_err(|_| PublicError::new("LIVESTREAM_STAGE_WRITE_FAILED", "无法更新直播舞台", false))
}

fn save_livestream_runtime(
    state: &AppState,
    title: String,
    segments: Vec<crate::livestream::LivestreamSegment>,
    loop_enabled: bool,
    media_path: Option<String>,
    media_kind: Option<crate::livestream::LivestreamMediaKind>,
) -> CommandResult<LivestreamRuntime> {
    let script =
        match crate::livestream::LivestreamScript::draft(title.clone(), segments, loop_enabled) {
            Ok(script) => script,
            Err(_) => return service_error("LIVESTREAM_SCRIPT_INVALID", "直播讲稿无效"),
        };
    let stage = crate::livestream::LivestreamStageState {
        product_title: title,
        current_subtitle: String::new(),
        next_hint: script
            .segments
            .first()
            .map(|segment| format!("下一段：{}", segment.title))
            .unwrap_or_default(),
        state: script.state,
        media_path,
        media_kind,
        output_state: crate::livestream::LivestreamOutputState::Idle,
        output_error_code: None,
    };
    replace_livestream_runtime(state, script, stage)
}

fn replace_livestream_playback_token(state: &AppState) -> Option<Arc<AtomicBool>> {
    let mut slot = state.livestream_playback_cancel.lock().ok()?;
    slot.store(true, Ordering::SeqCst);
    *slot = Arc::new(AtomicBool::new(false));
    Some(Arc::clone(&slot))
}

fn cancel_livestream_playback(state: &AppState) {
    if let Ok(slot) = state.livestream_playback_cancel.lock() {
        slot.store(true, Ordering::SeqCst);
    }
}

fn livestream_voice_from_config(
    config: &PublicConfig,
) -> Result<crate::livestream::LivestreamVoiceSnapshot, &'static str> {
    let route = active_voice_route(config).ok_or("LIVESTREAM_VOICE_ROUTE_MISSING")?;
    if route.mode != crate::config::VoiceRouteMode::Cascaded {
        return Err("LIVESTREAM_VOICE_ROUTE_UNSUPPORTED");
    }
    let provider_id = route
        .tts_provider_id
        .clone()
        .ok_or("LIVESTREAM_TTS_PROVIDER_MISSING")?;
    let model_id = route
        .tts_model_id
        .clone()
        .filter(|id| !id.is_empty())
        .ok_or("LIVESTREAM_TTS_MODEL_MISSING")?;
    let provider = config
        .models
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
        .ok_or("LIVESTREAM_TTS_PROVIDER_MISSING")?;
    Ok(crate::livestream::LivestreamVoiceSnapshot {
        provider_id,
        base_url: provider.base_url.clone(),
        model_id,
        voice_id: route.voice_id.clone().unwrap_or_default(),
    })
}

fn freeze_livestream_voice(
    state: &AppState,
    candidate: Result<crate::livestream::LivestreamVoiceSnapshot, &'static str>,
) -> Result<crate::livestream::LivestreamVoiceSnapshot, &'static str> {
    // The candidate was captured before the state transaction; prefer it when
    // the slot was cleared in between so every queued segment shares one voice.
    if let Ok(mut slot) = state.livestream_voice.lock() {
        if slot.is_none() && let Ok(snapshot) = &candidate {
            *slot = Some(snapshot.clone());
        }
        if let Some(snapshot) = slot.as_ref() {
            return Ok(snapshot.clone());
        }
    }
    candidate
}

fn resolve_livestream_voice(
    state: &AppState,
) -> Result<crate::livestream::LivestreamVoiceSnapshot, &'static str> {
    if let Ok(slot) = state.livestream_voice.lock()
        && let Some(snapshot) = slot.as_ref()
    {
        return Ok(snapshot.clone());
    }
    let config = state
        .config
        .load()
        .map(|config| public_view(&config))
        .map_err(|_| "LIVESTREAM_VOICE_CONFIG_FAILED")?;
    let snapshot = livestream_voice_from_config(&config)?;
    if let Ok(mut slot) = state.livestream_voice.lock() {
        *slot = Some(snapshot.clone());
    }
    Ok(snapshot)
}

fn replace_livestream_runtime(
    state: &AppState,
    script: crate::livestream::LivestreamScript,
    stage: crate::livestream::LivestreamStageState,
) -> CommandResult<LivestreamRuntime> {
    cancel_livestream_playback(state);
    let _ = replace_livestream_playback_token(state);
    if let Ok(mut slot) = state.livestream_voice.lock() {
        *slot = None;
    }
    if let Err(error) = write_livestream_stage(state, &stage) {
        return CommandResult::Err { error };
    }
    let mut script_slot = match state.livestream.lock() {
        Ok(slot) => slot,
        Err(_) => return service_error("SERVICE_BUSY", "直播状态暂时不可用"),
    };
    let mut stage_slot = match state.livestream_stage.lock() {
        Ok(slot) => slot,
        Err(_) => return service_error("SERVICE_BUSY", "直播舞台暂时不可用"),
    };
    *script_slot = Some(script.clone());
    *stage_slot = Some(stage.clone());
    CommandResult::Ok {
        data: LivestreamRuntime { script, stage },
    }
}

pub fn diagnostics_export_blocking(
    state: State<'_, AppState>,
    destination: String,
) -> CommandResult<DiagnosticsExportResult> {
    diagnostics_export_cmd(&state, destination)
}

fn diagnostics_export_cmd(
    state: &AppState,
    destination: String,
) -> CommandResult<DiagnosticsExportResult> {
    // The renderer is untrusted: a compromised page must not be able to place
    // diagnostic files at arbitrary filesystem locations, so writes are pinned
    // to the application data directory.
    if let Err(error) = ensure_diagnostics_destination(state, &destination) {
        return CommandResult::Err { error };
    }
    let public_config = match state.config.load() {
        Ok(config) => match serde_json::to_value(diagnostic_view(&config)) {
            Ok(value) => value,
            Err(_) => {
                return CommandResult::Err {
                    error: PublicError::new(
                        "DIAGNOSTICS_OPERATION_FAILED",
                        "Diagnostic operation failed",
                        false,
                    ),
                };
            }
        },
        Err(error) => {
            return CommandResult::Err {
                error: PublicError::new(
                    error.code(),
                    "Configuration is unavailable for export",
                    false,
                ),
            };
        }
    };
    let database_status = state
        .database
        .lock()
        .ok()
        .and_then(|database| {
            database
                .as_ref()
                .and_then(|database| database.integrity_check().ok())
        })
        .unwrap_or_else(|| "unavailable".to_owned());
    let service_status = serde_json::json!({ "database": database_status });
    match state.diagnostics.export(
        std::path::Path::new(&destination),
        public_config,
        service_status,
    ) {
        Ok(()) => CommandResult::Ok {
            data: DiagnosticsExportResult { exported: true },
        },
        Err(error) => CommandResult::Err {
            error: PublicError::new(error.code(), error.to_string(), false),
        },
    }
}

fn ensure_diagnostics_destination(state: &AppState, destination: &str) -> Result<(), PublicError> {
    const INVALID: &str = "DIAGNOSTICS_DESTINATION_INVALID";
    let destination = std::path::Path::new(destination);
    if !destination.is_absolute() {
        return Err(PublicError::new(INVALID, "诊断报告只能导出到应用数据目录", false));
    }
    std::fs::create_dir_all(&state.paths.data_directory)
        .map_err(|_| PublicError::new("DIAGNOSTICS_OPERATION_FAILED", "Diagnostic operation failed", false))?;
    let data_root = state
        .paths
        .data_directory
        .canonicalize()
        .map_err(|_| PublicError::new("DIAGNOSTICS_OPERATION_FAILED", "Diagnostic operation failed", false))?;
    let Some(parent) = destination.parent() else {
        return Err(PublicError::new(INVALID, "诊断报告只能导出到应用数据目录", false));
    };
    let parent = parent
        .canonicalize()
        .map_err(|_| PublicError::new(INVALID, "诊断报告只能导出到应用数据目录", false))?;
    if !parent.starts_with(&data_root) {
        return Err(PublicError::new(INVALID, "诊断报告只能导出到应用数据目录", false));
    }
    Ok(())
}

#[tauri::command]
pub fn config_get_startup_state(state: State<'_, AppState>) -> CommandResult<StartupState> {
    CommandResult::Ok {
        data: state.startup_state(),
    }
}

fn legacy_migration_status_cmd(state: &AppState) -> CommandResult<LegacyMigrationStatus> {
    let configured = match state.config.load() {
        Ok(config) => crate::migrate::secret_slots_configured(&config),
        Err(_) => false,
    };
    CommandResult::Ok {
        data: crate::migrate::legacy_migration_status(&state.paths.data_directory, configured),
    }
}

pub fn legacy_migration_status_blocking(
    state: State<'_, AppState>,
) -> CommandResult<LegacyMigrationStatus> {
    legacy_migration_status_cmd(&state)
}

fn resolve_legacy_source_path(
    path: &str,
) -> Result<std::path::PathBuf, crate::migrate::MigrateError> {
    let path = std::path::Path::new(path);
    if path.is_relative() {
        return Err(crate::migrate::MigrateError::Operation);
    }
    if std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(crate::migrate::MigrateError::Operation);
    }
    std::fs::canonicalize(path).map_err(|_| crate::migrate::MigrateError::Operation)
}

fn migrate_error<T: ts_rs::TS>(error: crate::migrate::MigrateError) -> CommandResult<T> {
    let message = match error {
        crate::migrate::MigrateError::PayloadInvalid => "旧会话数据无法解析",
        crate::migrate::MigrateError::AlreadyApplied => "目标数据目录已经完成迁移",
        _ => "无法从所选目录导入旧会话",
    };
    service_error(error.code(), message)
}

fn legacy_import_source_cmd(state: &AppState, path: String) -> CommandResult<LegacySessionImport> {
    let source = match resolve_legacy_source_path(&path) {
        Ok(path) => path,
        Err(error) => return migrate_error(error),
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
    crate::migrate::import_legacy_sessions_from_user_path(&source, database)
        .map_or_else(migrate_error, |data| CommandResult::Ok { data })
}

pub fn legacy_import_source_blocking(
    state: State<'_, AppState>,
    path: String,
) -> CommandResult<LegacySessionImport> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    legacy_import_source_cmd(&state, path)
}

/// Thin, read-only projection of the loaded configuration onto its redacted
/// public contract. The real logic lives in [`crate::config::public_view`]; this
/// only maps the load outcome into a [`CommandResult`]. It never writes and never
/// reads a secret value — credentials surface only as `SecretSlot` references.
fn public_config(state: &AppState) -> CommandResult<PublicConfig> {
    match state.config.load() {
        Ok(config) => CommandResult::Ok {
            data: public_view(&config),
        },
        Err(error) => CommandResult::Err {
            error: PublicError::new(error.code(), "Configuration is unavailable", false),
        },
    }
}

pub fn config_get_public_blocking(state: State<'_, AppState>) -> CommandResult<PublicConfig> {
    public_config(&state)
}

fn service_error<T: ts_rs::TS>(code: &str, message: &str) -> CommandResult<T> {
    CommandResult::Err {
        error: PublicError::new(code, message, false),
    }
}

fn provider_service_error<T: ts_rs::TS>(error: ProviderServiceError) -> CommandResult<T> {
    let code = error.code();
    let mut public = PublicError::new(
        code,
        match code {
            "PROVIDER_IN_USE" => "供应商仍被语音线路或 Embedding 配置引用，请先处理关联配置。",
            "SECRET_CLEANUP_FAILED" => "供应商配置已删除，但密钥清理失败，可以重试清理。",
            "CONFIG_WRITE_FAILED" => "无法写入配置，删除未完成。请检查目录写入权限。",
            "CONFIG_READ_FAILED" => "无法读取本地配置，操作未完成。",
            "PROVIDER_NOT_FOUND" => "供应商不存在，请刷新配置列表。",
            _ => "供应商操作未完成，请检查配置后重试。",
        },
        matches!(
            code,
            "PROVIDER_TIMEOUT" | "PROVIDER_REQUEST_FAILED" | "SECRET_CLEANUP_FAILED"
        ),
    );
    if let Some(field) = match code {
        "PROVIDER_ID_INVALID" => Some("id"),
        "CONFIG_URL_INVALID" | "PROVIDER_ENDPOINT_INVALID" => Some("baseUrl"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

fn role_service_error<T: ts_rs::TS>(error: RoleProfileServiceError) -> CommandResult<T> {
    let code = error.code();
    let message = if code == "ROLE_PROFILE_REVIEW_REQUIRED" {
        "Review and save this role profile before activating or copying it"
    } else {
        "Role profile operation failed"
    };
    let mut public = PublicError::new(code, message, false);
    if let Some(field) = match code {
        "ROLE_PROFILE_ID_INVALID"
        | "ROLE_PROFILE_COPY_ID_IN_USE"
        | "ROLE_PROFILE_REVIEW_REQUIRED"
        | "ROLE_PROFILE_NOT_FOUND" => Some("id"),
        "ROLE_PROFILE_FIELDS_INVALID" => Some("name"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

fn embedding_service_error<T: ts_rs::TS>(error: EmbeddingServiceError) -> CommandResult<T> {
    let code = error.code();
    let mut public = PublicError::new(
        code,
        "Embedding configuration operation failed",
        matches!(code, "EMBEDDING_TIMEOUT" | "EMBEDDING_REQUEST_FAILED"),
    );
    if let Some(field) = match code {
        "EMBEDDING_ID_INVALID"
        | "EMBEDDING_NOT_FOUND"
        | "EMBEDDING_NOT_READY"
        | "EMBEDDING_STALE" => Some("id"),
        "EMBEDDING_FIELDS_INVALID" => Some("dimensions"),
        "EMBEDDING_SOURCE_INVALID" | "CONFIG_REFERENCE_MISSING" => Some("providerId"),
        "CONFIG_URL_INVALID" | "EMBEDDING_ENDPOINT_INVALID" => Some("baseUrl"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

fn route_service_error<T: ts_rs::TS>(error: VoiceRouteServiceError) -> CommandResult<T> {
    let code = error.code();
    let probe_message = if let VoiceRouteServiceError::Probe(probe) = &error {
        Some(probe.message.clone())
    } else {
        None
    };
    let mut public = PublicError::new(
        code,
        probe_message.unwrap_or_else(|| "Voice route operation failed".into()),
        matches!(
            code,
            "PROVIDER_TIMEOUT" | "PROVIDER_REQUEST_FAILED" | "VOICE_ROUTE_PROBE_FAILED"
        ),
    );
    if let Some(field) = match code {
        "VOICE_ROUTE_ID_INVALID" => Some("id"),
        "VOICE_ROUTE_FIELDS_INVALID" | "VOICE_ROUTE_MODEL_NOT_FOUND" => Some("route"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

fn provider_probe<T: ts_rs::TS>() -> Result<OpenAiCompatibleProbe, CommandResult<T>> {
    OpenAiCompatibleProbe::new()
        .map_err(|error| service_error(error.code(), "Provider client is unavailable"))
}

fn route_stage_probe<T: ts_rs::TS>() -> Result<StandardRouteProbe, CommandResult<T>> {
    StandardRouteProbe::new()
        .map_err(|error| service_error(error.code(), "Provider client is unavailable"))
}

fn embedding_probe<T: ts_rs::TS>() -> Result<OpenAiCompatibleEmbeddingProbe, CommandResult<T>> {
    OpenAiCompatibleEmbeddingProbe::new()
        .map_err(|error| service_error(error.code(), "Embedding client is unavailable"))
}

fn material_service_error<T: ts_rs::TS>(error: MaterialServiceError) -> CommandResult<T> {
    let code = error.code();
    let message = match code {
        "MATERIAL_TYPE_UNSUPPORTED" => "Unsupported material type",
        "MATERIAL_TOO_LARGE" => "Material is too large",
        "MATERIAL_NOT_UTF8" => "Material is not valid UTF-8",
        "MATERIAL_NO_TEXT_LAYER" => "Material has no extractable text",
        "MATERIAL_PARSE_FAILED" => "Material could not be parsed",
        "MATERIAL_NOT_FOUND" => "Material not found",
        "MATERIAL_PATH_INVALID" => "Material path is invalid",
        "MATERIAL_PARSE_BUDGET" => "Material parse budget was exceeded",
        "EMBEDDING_NOT_READY" => "Embedding configuration is not ready",
        "EMBEDDING_NOT_FOUND" => "Embedding configuration was not found",
        "EMBEDDING_FIELDS_INVALID" => "Embedding configuration is invalid",
        _ => "Material operation failed",
    };
    let mut public = PublicError::new(code, message, false);
    if let Some(field) = match code {
        "MATERIAL_PATH_INVALID" => Some("path"),
        "MATERIAL_NOT_FOUND" => Some("id"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

fn resolve_import_path(path: &str) -> Result<std::path::PathBuf, MaterialServiceError> {
    let path = std::path::Path::new(path);
    if path.is_relative() {
        return Err(MaterialServiceError::PathInvalid);
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| MaterialServiceError::PathInvalid)?;
    if !canonical.is_file() {
        return Err(MaterialServiceError::PathInvalid);
    }
    Ok(canonical)
}

fn with_materials<T: serde::Serialize + ts_rs::TS>(
    state: &AppState,
    work: impl FnOnce(&MaterialService<'_>) -> Result<T, MaterialServiceError>,
) -> CommandResult<T> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    work(&MaterialService::new(database, &state.paths.data_directory))
        .map_or_else(material_service_error, |data| CommandResult::Ok { data })
}

fn material_list_cmd(state: &AppState) -> CommandResult<Vec<MaterialSummary>> {
    with_materials(state, |service| service.list())
}

fn material_import_cmd(state: &AppState, path: String) -> CommandResult<MaterialSummary> {
    let path = match resolve_import_path(&path) {
        Ok(path) => path,
        Err(error) => return material_service_error(error),
    };
    with_materials(state, |service| service.import_file(path))
}

fn material_search_cmd(
    state: &AppState,
    query: String,
    top_k: Option<u32>,
) -> CommandResult<Vec<MaterialSearchHit>> {
    with_materials(state, |service| service.search_text(&query, top_k))
}

fn material_delete_cmd(state: &AppState, id: String) -> CommandResult<FoundationStatus> {
    with_materials(state, |service| {
        service.delete(&id)?;
        Ok(FoundationStatus { ready: true })
    })
}

fn material_index_cmd(state: &AppState) -> CommandResult<MaterialIndexResult> {
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    with_materials(state, |service| {
        service.index_library(&state.config, &state.secrets, &probe)
    })
}

pub fn material_list_blocking(state: State<'_, AppState>) -> CommandResult<Vec<MaterialSummary>> {
    material_list_cmd(&state)
}

pub fn material_import_blocking(
    state: State<'_, AppState>,
    path: String,
) -> CommandResult<MaterialSummary> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    material_import_cmd(&state, path)
}

pub fn material_search_blocking(
    state: State<'_, AppState>,
    query: String,
    top_k: Option<u32>,
) -> CommandResult<Vec<MaterialSearchHit>> {
    material_search_cmd(&state, query, top_k)
}

pub fn material_delete_blocking(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    material_delete_cmd(&state, id)
}

pub fn material_index_blocking(state: State<'_, AppState>) -> CommandResult<MaterialIndexResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    material_index_cmd(&state)
}

const EVENT_RUNTIME_STATUS: &str = "runtime.status.v1";
const EVENT_AUDIO_LEVEL: &str = "audio.level.v1";
const EVENT_SESSION_TRANSCRIPT: &str = "session.transcript.v1";
const EVENT_SESSION_REPLY: &str = "session.reply.v1";

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

fn clip_snippet(text: &str) -> String {
    text.chars().take(160).collect()
}

fn load_public_config(state: &AppState) -> Result<PublicConfig, PublicError> {
    match state.config.load() {
        Ok(config) => Ok(public_view(&config)),
        Err(error) => Err(PublicError::new(
            error.code(),
            "Configuration is unavailable",
            false,
        )),
    }
}

fn load_session_config(state: &AppState) -> Result<PublicConfig, PublicError> {
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

fn secrets_backend_ready(state: &AppState) -> bool {
    state.secrets.status("system/startup-probe").is_ok()
}

fn runtime_status_from_state(state: &AppState) -> RuntimeStatus {
    let seq = state.event_seq.load(Ordering::SeqCst);
    match state.sessions.try_lock() {
        Ok(sessions) => RuntimeStatus {
            phase: sessions.phase().as_str().to_owned(),
            mode: sessions.mode().as_str().to_owned(),
            seq,
            unused_materials: sessions.unused_materials(),
            last_error_code: sessions.last_error_code().map(str::to_owned),
            revision: sessions.revision(),
        },
        Err(TryLockError::WouldBlock) => RuntimeStatus {
            phase: "thinking".into(),
            mode: state.session_control.mode().as_str().to_owned(),
            seq,
            unused_materials: false,
            last_error_code: None,
            revision: 0,
        },
        Err(TryLockError::Poisoned(_)) => RuntimeStatus {
            phase: "idle".into(),
            mode: "ai_active".into(),
            seq,
            unused_materials: false,
            last_error_code: Some("SERVICE_BUSY".into()),
            revision: 0,
        },
    }
}

fn bump_event_seq(state: &AppState) -> u64 {
    state.event_seq.fetch_add(1, Ordering::SeqCst);
    state.event_seq.load(Ordering::SeqCst)
}

fn bump_runtime_status(state: &AppState) -> RuntimeStatus {
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
fn runtime_status_event(
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
    })
    .expect("runtime status")
}

#[cfg(test)]
fn transcript_event(seq: u64, text: &str) -> serde_json::Value {
    serde_json::to_value(SessionTranscriptEvent {
        seq,
        text: clip_snippet(text),
    })
    .expect("transcript event")
}

#[cfg(test)]
fn reply_event(seq: u64, text: &str) -> serde_json::Value {
    serde_json::to_value(SessionReplyEvent {
        seq,
        text: clip_snippet(text),
    })
    .expect("reply event")
}

#[cfg(test)]
fn audio_level_event(seq: u64, peak: f64) -> serde_json::Value {
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
            text: clip_snippet(user_text),
        },
    );
    let _ = app.emit(
        EVENT_SESSION_REPLY,
        SessionReplyEvent {
            seq: bump_event_seq(state),
            text: clip_snippet(assistant_text),
        },
    );
}

fn read_provider_secret(
    state: &AppState,
    config: &PublicConfig,
    provider_id: Option<&str>,
) -> Result<Option<zeroize::Zeroizing<String>>, PublicError> {
    let Some(provider_id) = provider_id.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let Some(provider) = config
        .models
        .providers
        .iter()
        .find(|provider| provider.id == provider_id)
    else {
        return Ok(None);
    };
    let Some(slot) = provider.credential.as_ref().filter(|slot| slot.configured) else {
        return Ok(None);
    };
    let secret = state.secrets.read(&slot.reference).map_err(|_| {
        PublicError::new(
            "SECRET_BACKEND_UNAVAILABLE",
            "Secret backend is unavailable",
            false,
        )
    })?;
    match secret {
        Some(value) if !value.trim().is_empty() => Ok(Some(value)),
        _ => Err(PublicError::new(
            "PROVIDER_CREDENTIAL_MISSING",
            "本机未找到供应商密钥，请在供应商设置中重新保存 API Key。",
            false,
        )),
    }
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

fn session_detail(
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
        });
    }
    Ok(SessionDetail {
        session: session.into(),
        turns,
    })
}

#[cfg(test)]
fn session_start_cmd(state: &AppState) -> CommandResult<SessionStartResult> {
    session_start_selected_cmd(state, None, None, false)
}

#[cfg(test)]
fn session_start_selected_cmd(
    state: &AppState,
    role_profile_id: Option<&str>,
    voice_route_id: Option<&str>,
    allow_web_search: bool,
) -> CommandResult<SessionStartResult> {
    session_start_capture_cmd(
        state,
        role_profile_id,
        voice_route_id,
        allow_web_search,
        None,
    )
}

fn session_start_capture_cmd(
    state: &AppState,
    role_profile_id: Option<&str>,
    voice_route_id: Option<&str>,
    allow_web_search: bool,
    capture: Option<crate::services::MeetingCapture<'_>>,
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
        sessions.start_with_meeting_capture(database, &config, secrets_ready, capture)
    } else {
        sessions.start(database, &config, secrets_ready)
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

fn keep_session_routing(state: &AppState, change: crate::prerequisites::AudioRoutingChange) {
    let _ = crate::prerequisites::persist_audio_routing(&state.paths.data_directory, &change);
    if let Ok(mut slot) = state.audio_routing.lock() {
        *slot = Some(change);
    }
}

fn rollback_session_routing(
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

fn session_stop_cmd(state: &AppState) -> CommandResult<SessionSummary> {
    state.session_control.request_stop();
    stop_operator_monitor(state);
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

fn session_stop_pending(state: &AppState) -> CommandResult<SessionSummary> {
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

fn session_set_mode_cmd(state: &AppState, mode: String) -> CommandResult<RuntimeStatus> {
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

fn session_list_cmd(state: &AppState) -> CommandResult<Vec<SessionSummary>> {
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

fn session_get_cmd(state: &AppState, session_id: String) -> CommandResult<SessionDetail> {
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

fn session_delete_cmd(state: &AppState, session_id: String) -> CommandResult<FoundationStatus> {
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

fn session_export_cmd(
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

fn session_agent_command_cmd(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    input: AgentCommandInput,
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
    match sessions.execute_command(database, &config, probes, credentials, command) {
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

fn session_finalize_utterance_cmd(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    text: Option<&str>,
) -> CommandResult<SessionTurnView> {
    session_finalize_utterance_cmd_inner(state, probes, credentials, text, false)
}

fn session_finalize_utterance_forced_cmd(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
) -> CommandResult<SessionTurnView> {
    session_finalize_utterance_cmd_inner(state, probes, credentials, None, true)
}

fn session_finalize_utterance_cmd_inner(
    state: &AppState,
    probes: &SessionProbes<'_>,
    credentials: CascadeCredentials<'_>,
    text: Option<&str>,
    force_meeting_assistant: bool,
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
        sessions.finalize_utterance_forced(database, &config, probes, credentials)
    } else {
        sessions.finalize_utterance(database, &config, probes, credentials, text)
    };
    match finalized {
        Ok(Some(_)) => {
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
        Ok(None) => session_service_error(SessionServiceError::StateInvalid),
        Err(error) => session_service_error(error),
    }
}

fn runtime_get_status_cmd(state: &AppState) -> CommandResult<RuntimeStatus> {
    CommandResult::Ok {
        data: runtime_status_from_state(state),
    }
}

#[tauri::command]
pub fn session_audio_ready(state: State<'_, AppState>) -> CommandResult<FoundationStatus> {
    session_audio_ready_cmd(&state)
}

fn session_audio_ready_cmd(state: &AppState) -> CommandResult<FoundationStatus> {
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

fn session_push_mic_pcm_cmd(
    state: &AppState,
    pcm: &str,
    sample_rate: u32,
) -> CommandResult<MicPcmAcceptance> {
    use base64::Engine as _;
    let decoded = match base64::engine::general_purpose::STANDARD.decode(pcm) {
        Ok(decoded) => decoded,
        Err(_) => {
            return service_error("SESSION_MIC_PCM_INVALID", "麦克风音频数据无效，请重新开始对练。");
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
        // Same UI-thread rule as session_audio_ready: a model request holding
        // the lock must never stall mic streaming; drop the chunk instead.
        Err(TryLockError::WouldBlock) => CommandResult::Ok {
            data: MicPcmAcceptance { accepted: false },
        },
        Err(TryLockError::Poisoned(_)) => service_error("SERVICE_BUSY", "会话音频状态暂时不可用"),
    }
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
    );
    if matches!(
        &result,
        CommandResult::Ok {
            data: SessionStartResult::Started { .. }
        }
    ) {
        if let Ok(mut sessions) = state.sessions.lock() {
            sessions.configure_playback(output_device_id.filter(|id| !id.trim().is_empty()).map(
                |endpoint_id| crate::audio::playback::BridgePlayback {
                    executable: bridge,
                    endpoint_id,
                },
            ));
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
    let trimmed = text.trim();
    let mut result = if force_meeting_assistant {
        session_finalize_utterance_forced_cmd(&state, &probes, credentials)
    } else {
        session_finalize_utterance_cmd(
            &state,
            &probes,
            credentials,
            (!trimmed.is_empty()).then_some(trimmed),
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
    let result = session_agent_command_cmd(&state, &probes, credentials, input);
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

fn service_guard<T: ts_rs::TS>(
    state: &AppState,
) -> Result<std::sync::MutexGuard<'_, ()>, CommandResult<T>> {
    state.service_lock.lock().map_err(|_| {
        service_error(
            "SERVICE_BUSY",
            "Service configuration is temporarily unavailable",
        )
    })
}

fn service_guard_try<T: ts_rs::TS>(
    state: &AppState,
) -> Result<Option<std::sync::MutexGuard<'_, ()>>, CommandResult<T>> {
    match state.service_lock.try_lock() {
        Ok(guard) => Ok(Some(guard)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Poisoned(_)) => Err(service_error(
            "SERVICE_BUSY",
            "Service configuration is temporarily unavailable",
        )),
    }
}

pub fn model_provider_save_blocking(
    state: State<'_, AppState>,
    input: ProviderSaveInput,
) -> CommandResult<ProviderConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .save(input)
        .map_or_else(provider_service_error, |data| CommandResult::Ok { data })
}

pub fn model_provider_test_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<ProviderTestResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let service = ProviderService::new(&state.config, &state.secrets, &probe);
    let discovered = match service.discover(&provider_id) {
        Ok(discovered) => discovered,
        Err(error) => return provider_service_error(error),
    };
    let config = match state.config.load() {
        Ok(config) => public_view(&config),
        Err(error) => return service_error(error.code(), "模型配置不可用"),
    };
    let Some(provider) = config
        .models
        .providers
        .iter()
        .find(|item| item.id == provider_id)
    else {
        return service_error("PROVIDER_NOT_FOUND", "模型供应商不存在");
    };
    let capability = provider.web_capability.unwrap_or_default();
    let mut data = ProviderTestResult {
        provider_id: provider_id.clone(),
        reachable: true,
        model_count: discovered.models.len(),
        web_status: crate::providers::web_search::WebCapabilityStatus::Disabled,
        web_source_count: 0,
    };
    if capability != crate::providers::web_search::WebCapability::None {
        let Some(model) = discovered.models.first() else {
            data.web_status = crate::providers::web_search::WebCapabilityStatus::ModelUnsupported;
            return CommandResult::Ok { data };
        };
        let secret = match read_provider_secret(&state, &config, Some(&provider_id)) {
            Ok(secret) => secret,
            Err(error) => return CommandResult::Err { error },
        };
        let model_client = match OpenAiCompatibleCascade::new() {
            Ok(client) => client.with_web_capability(capability),
            Err(_) => {
                data.web_status =
                    crate::providers::web_search::WebCapabilityStatus::NetworkUnreachable;
                return CommandResult::Ok { data };
            }
        };
        let result = model_client.complete(
            &ProviderEndpoint {
                provider_id: provider.id.clone(),
                base_url: provider.base_url.clone(),
            },
            secret.as_deref().map(|value| value.as_str()),
            &model.id,
            &[ChatMessage {
                role: "user".into(),
                content: "请使用联网搜索回答当前 UTC 日期，并提供来源。".into(),
            }],
        );
        let web = model_client.web_result();
        data.web_source_count = web.sources.len();
        data.web_status = crate::providers::web_search::probe_status_from_result(
            result.as_ref().map(|_| &web).map_err(|error| *error),
        );
    }
    CommandResult::Ok { data }
}

pub fn model_provider_discover_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<ModelDiscoveryResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .discover(&provider_id)
        .map_or_else(provider_service_error, |data| CommandResult::Ok { data })
}

pub fn model_provider_activate_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<ProviderConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .activate(&provider_id)
        .map_or_else(provider_service_error, |data| CommandResult::Ok { data })
}

pub fn model_provider_delete_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .delete(&provider_id)
        .map_or_else(provider_service_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

pub fn model_provider_dependencies_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<Vec<crate::services::ProviderDependency>> {
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .dependencies(&provider_id)
        .map_or_else(provider_service_error, |data| CommandResult::Ok { data })
}

pub fn speech_route_save_blocking(
    state: State<'_, AppState>,
    input: VoiceRouteSaveInput,
) -> CommandResult<VoiceRouteConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let stage_probe = match route_stage_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    VoiceRouteService::new(&state.config, &state.secrets, &probe, &stage_probe)
        .save(input)
        .map_or_else(route_service_error, |data| CommandResult::Ok { data })
}

pub fn speech_route_test_blocking(
    state: State<'_, AppState>,
    route_id: String,
) -> CommandResult<VoiceRouteTestResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let stage_probe = match route_stage_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    VoiceRouteService::new(&state.config, &state.secrets, &probe, &stage_probe)
        .test(&route_id)
        .map_or_else(route_service_error, |data| CommandResult::Ok { data })
}

pub fn speech_route_activate_blocking(
    state: State<'_, AppState>,
    route_id: String,
) -> CommandResult<VoiceRouteConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let stage_probe = match route_stage_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    VoiceRouteService::new(&state.config, &state.secrets, &probe, &stage_probe)
        .activate(&route_id)
        .map_or_else(route_service_error, |data| CommandResult::Ok { data })
}

pub fn speech_route_delete_blocking(
    state: State<'_, AppState>,
    route_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let stage_probe = match route_stage_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    VoiceRouteService::new(&state.config, &state.secrets, &probe, &stage_probe)
        .delete(&route_id)
        .map_or_else(route_service_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

fn voice_reference_service_error<T: ts_rs::TS>(
    error: VoiceReferenceServiceError,
) -> CommandResult<T> {
    let code = error.code();
    let message = match code {
        "VOICE_REFERENCE_ID_INVALID" => "Voice reference id is invalid",
        "VOICE_REFERENCE_FIELDS_INVALID" => "Voice reference name must not be empty",
        "VOICE_REFERENCE_AUDIO_INVALID" => {
            "Reference audio must be an existing local .wav or .mp3 file"
        }
        "VOICE_REFERENCE_AUDIO_TOO_LARGE" => "Reference audio must be 10 MB or smaller",
        "VOICE_REFERENCE_AUDIO_TOO_SHORT" => "Recorded audio must be at least 3 seconds",
        "VOICE_REFERENCE_NOT_FOUND" => "Voice reference not found",
        "VOICE_REFERENCE_PROVIDER_MISSING" => {
            "Voice reference needs a model provider before cloning"
        }
        "PROVIDER_UNAUTHORIZED" => "Provider rejected the credentials",
        "PROVIDER_TIMEOUT" => "Provider request timed out",
        "PROVIDER_REQUEST_FAILED" => "Provider request failed",
        "PROVIDER_RESPONSE_INVALID" => "Provider response could not be parsed",
        "PROVIDER_RESPONSE_TOO_LARGE" => "Provider response is too large",
        "PROVIDER_ENDPOINT_INVALID" => "Provider endpoint is invalid",
        _ => "Voice reference operation failed",
    };
    service_error(code, message)
}

fn voice_reference_cmd<T: serde::Serialize + ts_rs::TS>(
    state: &AppState,
    work: impl FnOnce(&VoiceReferenceService<'_>) -> Result<T, VoiceReferenceServiceError>,
) -> CommandResult<T> {
    let database = match state.database.lock() {
        Ok(guard) => guard,
        Err(_) => {
            return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
        }
    };
    let Some(database) = database.as_ref() else {
        return service_error("DATABASE_OPERATION_FAILED", "Database is unavailable");
    };
    let gateway = match VoiceCloneProbe::new() {
        Ok(gateway) => gateway,
        Err(error) => {
            return service_error(error.code(), "Provider client is unavailable");
        }
    };
    work(&VoiceReferenceService::new(
        database,
        &state.config,
        &state.secrets,
        &gateway,
    ))
    .map_or_else(voice_reference_service_error, |data| CommandResult::Ok { data })
}

pub fn voice_reference_save_blocking(
    state: State<'_, AppState>,
    input: VoiceReferenceSaveInput,
) -> CommandResult<VoiceReferenceSummary> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    voice_reference_cmd(&state, |service| service.save(input))
}

pub fn voice_reference_save_audio_blocking(
    state: State<'_, AppState>,
    input: VoiceReferenceAudioSaveInput,
) -> CommandResult<VoiceReferenceSummary> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    voice_reference_cmd(&state, |service| service.save_audio(input))
}

pub fn voice_reference_update_blocking(
    state: State<'_, AppState>,
    input: VoiceReferenceUpdateInput,
) -> CommandResult<VoiceReferenceSummary> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    voice_reference_cmd(&state, |service| service.update_metadata(input))
}

pub fn voice_reference_list_blocking(
    state: State<'_, AppState>,
) -> CommandResult<Vec<VoiceReferenceSummary>> {
    voice_reference_cmd(&state, |service| service.list())
}

pub fn voice_reference_delete_blocking(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let result = voice_reference_cmd(&state, |service| service.delete(&id));
    match result {
        CommandResult::Ok { .. } => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        CommandResult::Err { error } => CommandResult::Err { error },
    }
}

pub fn voice_reference_clone_blocking(
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<VoiceReferenceCloneResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    voice_reference_cmd(&state, |service| service.clone_voice(&id))
}

pub fn role_profile_save_blocking(
    state: State<'_, AppState>,
    input: RoleProfileSaveInput,
) -> CommandResult<RoleProfileConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    RoleProfileService::new(&state.config)
        .save(input)
        .map_or_else(role_service_error, |data| CommandResult::Ok { data })
}

pub fn role_profile_copy_blocking(
    state: State<'_, AppState>,
    input: RoleProfileCopyInput,
) -> CommandResult<RoleProfileConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    RoleProfileService::new(&state.config)
        .copy(input)
        .map_or_else(role_service_error, |data| CommandResult::Ok { data })
}

pub fn role_profile_activate_blocking(
    state: State<'_, AppState>,
    role_id: String,
) -> CommandResult<RoleProfileConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    RoleProfileService::new(&state.config)
        .activate(&role_id)
        .map_or_else(role_service_error, |data| CommandResult::Ok { data })
}

pub fn role_profile_delete_blocking(
    state: State<'_, AppState>,
    role_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    RoleProfileService::new(&state.config)
        .delete(&role_id)
        .map_or_else(role_service_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

pub fn embedding_config_save_blocking(
    state: State<'_, AppState>,
    input: EmbeddingConfigSaveInput,
) -> CommandResult<EmbeddingConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    EmbeddingService::new(&state.config, &state.secrets, &probe)
        .save(input)
        .map_or_else(embedding_service_error, |data| CommandResult::Ok { data })
}

pub fn embedding_config_test_blocking(
    state: State<'_, AppState>,
    embedding_id: String,
) -> CommandResult<EmbeddingTestResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    EmbeddingService::new(&state.config, &state.secrets, &probe)
        .test(&embedding_id)
        .map_or_else(embedding_service_error, |data| CommandResult::Ok { data })
}

pub fn embedding_config_activate_blocking(
    state: State<'_, AppState>,
    embedding_id: String,
) -> CommandResult<EmbeddingConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    EmbeddingService::new(&state.config, &state.secrets, &probe)
        .activate(&embedding_id)
        .map_or_else(embedding_service_error, |data| CommandResult::Ok { data })
}

pub fn embedding_config_delete_blocking(
    state: State<'_, AppState>,
    embedding_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    EmbeddingService::new(&state.config, &state.secrets, &probe)
        .delete(&embedding_id)
        .map_or_else(embedding_service_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

pub fn config_restore_last_good_blocking(
    state: State<'_, AppState>,
) -> CommandResult<StartupState> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    CommandResult::Ok {
        data: state.restore_last_good(),
    }
}

pub fn config_restore_defaults_blocking(state: State<'_, AppState>) -> CommandResult<StartupState> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    CommandResult::Ok {
        data: state.restore_defaults(),
    }
}

#[tauri::command]
pub fn open_app_directory(
    state: State<'_, AppState>,
    kind: String,
) -> CommandResult<FoundationStatus> {
    let directory = match kind.as_str() {
        "config" => state
            .paths
            .config_path
            .parent()
            .map(std::path::Path::to_path_buf),
        "data" => Some(state.paths.data_directory.clone()),
        _ => None,
    };
    let Some(directory) = directory else {
        return CommandResult::Err {
            error: PublicError::new(
                "APP_DIRECTORY_INVALID",
                "Unsupported application directory",
                false,
            ),
        };
    };
    match open_directory(&directory) {
        Ok(()) => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        Err(()) => CommandResult::Err {
            error: PublicError::new(
                "APP_DIRECTORY_OPEN_FAILED",
                "Application directory could not be opened",
                false,
            ),
        },
    }
}

#[cfg(windows)]
fn open_directory(path: &std::path::Path) -> Result<(), ()> {
    use std::{os::windows::ffi::OsStrExt, ptr};
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let operation = "open".encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let path = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            path.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize > 32 {
        Ok(())
    } else {
        Err(())
    }
}

#[tauri::command]
pub fn open_web_source(url: String) -> CommandResult<FoundationStatus> {
    let parsed = match reqwest::Url::parse(&url) {
        Ok(url)
            if matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none() =>
        {
            url
        }
        _ => return service_error("WEB_SOURCE_INVALID", "来源地址无效"),
    };
    // ShellExecute opens only a validated web URL; no shell command interpolation.
    match open_directory(std::path::Path::new(parsed.as_str())) {
        Ok(()) => CommandResult::Ok {
            data: FoundationStatus { ready: true },
        },
        Err(()) => service_error("WEB_SOURCE_OPEN_FAILED", "无法打开系统浏览器"),
    }
}

#[cfg(not(windows))]
fn open_directory(_: &std::path::Path) -> Result<(), ()> {
    Err(())
}

// All filesystem/network work and contended locks live on blocking workers.
// The owned handle keeps AppState alive without extending a borrowed State.
async fn dispatch_blocking<T: ts_rs::TS + Send + 'static>(
    work: impl FnOnce() -> CommandResult<T> + Send + 'static,
) -> CommandResult<T> {
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(result) => result,
        Err(_) => service_error("COMMAND_WORKER_FAILED", "Command worker failed"),
    }
}

macro_rules! blocking_command {
    ($name:ident, $worker:ident($($arg:ident: $ty:ty),*) -> $output:ty) => {
        #[tauri::command]
        pub async fn $name<R: tauri::Runtime>(
            app: AppHandle<R>, $($arg: $ty),*
        ) -> CommandResult<$output> {
            dispatch_blocking(move || $worker(app.state(), $($arg),*)).await
        }
    };
    (with_events $name:ident, $worker:ident($($arg:ident: $ty:ty),*) -> $output:ty) => {
        #[tauri::command]
        pub async fn $name<R: tauri::Runtime>(
            app: AppHandle<R>, $($arg: $ty),*
        ) -> CommandResult<$output> {
            dispatch_blocking(move || $worker(app.clone(), app.state(), $($arg),*)).await
        }
    };
}

blocking_command!(diagnostics_export, diagnostics_export_blocking(destination: String) -> DiagnosticsExportResult);
blocking_command!(legacy_migration_status, legacy_migration_status_blocking() -> LegacyMigrationStatus);
blocking_command!(legacy_import_source, legacy_import_source_blocking(path: String) -> LegacySessionImport);
blocking_command!(config_get_public, config_get_public_blocking() -> PublicConfig);
blocking_command!(material_list, material_list_blocking() -> Vec<MaterialSummary>);
blocking_command!(material_import, material_import_blocking(path: String) -> MaterialSummary);
blocking_command!(material_search, material_search_blocking(query: String, top_k: Option<u32>) -> Vec<MaterialSearchHit>);
blocking_command!(material_delete, material_delete_blocking(id: String) -> FoundationStatus);
blocking_command!(material_index, material_index_blocking() -> MaterialIndexResult);
blocking_command!(with_events session_start, session_start_blocking(role_profile_id: Option<String>, voice_route_id: Option<String>, allow_web_search: Option<bool>, meeting_pid: Option<u32>, output_device_id: Option<String>) -> SessionStartResult);

pub fn meeting_process_list_blocking(
    _state: State<'_, AppState>,
) -> CommandResult<Vec<crate::processes::MeetingProcess>> {
    match crate::processes::list_meeting_processes(&crate::processes::PowerShellProcessEnumerator) {
        Ok(data) => CommandResult::Ok { data },
        Err(_) => service_error(
            "MEETING_PROCESS_ENUM_FAILED",
            "无法读取会议进程，请确认会议软件已打开",
        ),
    }
}
blocking_command!(meeting_process_list, meeting_process_list_blocking() -> Vec<crate::processes::MeetingProcess>);

pub fn audio_output_list_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    _state: State<'_, AppState>,
) -> CommandResult<Vec<crate::audio::playback::AudioOutputDevice>> {
    let bridge = if cfg!(debug_assertions) {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../native/AudioBridge/publish/AudioBridge.exe")
    } else {
        match app.path().resource_dir() {
            Ok(root) => root.join("audio-bridge/AudioBridge.exe"),
            Err(_) => return service_error("SESSION_SIDECAR_MISSING", "无法定位音频组件"),
        }
    };
    match crate::audio::playback::list_outputs(&bridge) {
        Ok(data) => CommandResult::Ok { data },
        Err(code) => service_error(
            code,
            "无法读取音频输出设备，请确认 AudioBridge 已安装、音频设备已连接",
        ),
    }
}
blocking_command!(with_events audio_output_list, audio_output_list_blocking() -> Vec<crate::audio::playback::AudioOutputDevice>);

fn audio_bridge_path<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<std::path::PathBuf, &'static str> {
    if cfg!(debug_assertions) {
        Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../native/AudioBridge/publish/AudioBridge.exe"))
    } else {
        app.path()
            .resource_dir()
            .map(|root| root.join("audio-bridge/AudioBridge.exe"))
            .map_err(|_| "SESSION_SIDECAR_MISSING")
    }
}

fn prerequisite_script_path<R: tauri::Runtime>(
    app: &AppHandle<R>,
    name: &str,
) -> Result<std::path::PathBuf, &'static str> {
    if !matches!(name, "fetch-prerequisites.ps1" | "install-prerequisite.ps1") {
        return Err("PREREQUISITE_RESOURCE_MISSING");
    }
    if cfg!(debug_assertions) {
        Ok(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts")
            .join(name))
    } else {
        app.path()
            .resource_dir()
            .map(|root| root.join("prerequisite-scripts").join(name))
            .map_err(|_| "PREREQUISITE_RESOURCE_MISSING")
    }
}

static AUDIO_PREPARATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn audio_preparation_failure<T: ts_rs::TS>(code: &'static str) -> CommandResult<T> {
    service_error(code, crate::prerequisites::preparation_error_message(code))
}

fn persist_preparation_diagnostic(
    state: &AppState,
    diagnostic: &crate::prerequisites::PreparationDiagnostic,
) -> bool {
    let directory = state.paths.data_directory.join("prerequisites");
    if std::fs::create_dir_all(&directory).is_err() {
        return false;
    }
    let record = directory.join("preparation-result.json");
    serde_json::to_vec(diagnostic)
        .ok()
        .and_then(|bytes| std::fs::write(record, bytes).ok())
        .is_some()
}

fn attach_preparation_diagnostic(
    state: &AppState,
    phase: String,
    exit_code: Option<i32>,
    result: CommandResult<crate::prerequisites::VirtualAudioPreparation>,
) -> CommandResult<crate::prerequisites::VirtualAudioPreparation> {
    let code = match &result {
        CommandResult::Err { error } => Some(error.code.clone()),
        CommandResult::Ok { data } => data
            .diagnostic
            .as_ref()
            .and_then(|item| item.error_code.clone()),
    };
    let retry_allowed = !matches!(
        code.as_deref(),
        Some("PREREQUISITE_TIMEOUT" | "PREREQUISITE_INSTALL_BUSY")
    );
    let diagnostic = crate::prerequisites::PreparationDiagnostic {
        phase: phase.clone(),
        retry_allowed,
        error_code: code.clone(),
        exit_code,
    };
    let persist_ok = persist_preparation_diagnostic(state, &diagnostic);
    match result {
        CommandResult::Ok { mut data } => {
            data.diagnostic = Some(diagnostic);
            if !persist_ok {
                data.detail = format!("{}。诊断记录未能写入。", data.detail);
            }
            CommandResult::Ok { data }
        }
        CommandResult::Err { mut error } => {
            error.message = format!(
                "{}（阶段：{}）",
                error.message,
                crate::prerequisites::preparation_phase_label(&phase)
            );
            if !persist_ok {
                error.message.push_str("。诊断记录未能写入。");
            }
            CommandResult::Err { error }
        }
    }
}

fn audio_preparation_state(
    state: &str,
    detail: &str,
) -> crate::prerequisites::VirtualAudioPreparation {
    let mut result = crate::prerequisites::VirtualAudioPreparation::from_devices(&[]);
    result.state = state.into();
    result.detail = detail.into();
    result
}

pub fn virtual_audio_status_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CommandResult<crate::prerequisites::VirtualAudioPreparation> {
    let Ok(_guard) = AUDIO_PREPARATION_LOCK.try_lock() else {
        return CommandResult::Ok {
            data: audio_preparation_state("installing", "安装任务正在运行，请等待完成。"),
        };
    };
    let bridge = match audio_bridge_path(&app) {
        Ok(path) => path,
        Err(code) => return service_error(code, "缺少 AudioBridge 音频组件"),
    };
    match crate::prerequisites::enumerate_audio_devices(&bridge) {
        Ok(devices) => {
            let mut status = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
            if !status.installed
                && let Ok(script) = prerequisite_script_path(&app, "install-prerequisite.ps1")
            {
                let directory = state
                    .paths
                    .data_directory
                    .join("prerequisites")
                    .to_string_lossy()
                    .into_owned();
                match crate::prerequisites::run_script_bounded(
                    &script,
                    &[
                        "-Component",
                        "virtual-audio",
                        "-ResourcesDirectory",
                        &directory,
                        "-ProbeOnly",
                    ],
                    std::time::Duration::from_secs(15),
                ) {
                    Ok(probe) if probe["driverStore"] == true && status.state == "missing" => {
                        status = audio_preparation_state(
                            "driver_present",
                            "驱动已在 Windows 中但端点尚未就绪，请检查设备状态；不会重复安装。必要时请自行重启后重新检测。",
                        );
                    }
                    Err("PREREQUISITE_INSTALL_BUSY") => {
                        status = audio_preparation_state(
                            "installing",
                            "提权安装任务仍在运行，请完成授权或等待安装结束。",
                        )
                    }
                    Err(code) => return audio_preparation_failure(code),
                    _ => {}
                }
            }
            CommandResult::Ok { data: status }
        }
        Err(code) => service_error(code, "无法检测本机音频设备"),
    }
}
blocking_command!(with_events virtual_audio_status, virtual_audio_status_blocking() -> crate::prerequisites::VirtualAudioPreparation);

pub fn virtual_audio_install_blocking<R: tauri::Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> CommandResult<crate::prerequisites::VirtualAudioPreparation> {
    let Ok(_guard) = AUDIO_PREPARATION_LOCK.try_lock() else {
        return attach_preparation_diagnostic(
            &state,
            "checking".into(),
            None,
            audio_preparation_failure("PREREQUISITE_INSTALL_BUSY"),
        );
    };
    let current_phase = std::cell::RefCell::new("checking".to_string());
    let exit_code = std::cell::Cell::new(None);
    let phase = |phase: &str| {
        *current_phase.borrow_mut() = phase.into();
        let _ = app.emit("virtual_audio.preparation.v1", phase);
    };
    let result = (|| {
        phase("checking");
        let bridge = match audio_bridge_path(&app) {
            Ok(path) => path,
            Err(code) => return service_error(code, "缺少 AudioBridge 音频组件"),
        };
        match crate::prerequisites::enumerate_audio_devices(&bridge) {
            Ok(devices) => {
                let status = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
                if status.installed || status.state != "missing" {
                    return CommandResult::Ok { data: status };
                }
            }
            Err(code) => return audio_preparation_failure(code),
        }
        let managed = state.paths.data_directory.join("prerequisites");
        if std::fs::create_dir_all(&managed).is_err() {
            return service_error("PREREQUISITE_DIRECTORY_FAILED", "无法创建托管安装目录");
        }
        let fetch = match prerequisite_script_path(&app, "fetch-prerequisites.ps1") {
            Ok(path) => path,
            Err(code) => return service_error(code, "缺少虚拟声卡准备脚本"),
        };
        let install = match prerequisite_script_path(&app, "install-prerequisite.ps1") {
            Ok(path) => path,
            Err(code) => return service_error(code, "缺少虚拟声卡安装脚本"),
        };
        let destination = managed.to_string_lossy().to_string();
        // Preflight the worker lock and existing driver before downloading or retrying.
        match crate::prerequisites::run_script_bounded(
            &install,
            &[
                "-Component",
                "virtual-audio",
                "-ResourcesDirectory",
                &destination,
                "-ProbeOnly",
            ],
            std::time::Duration::from_secs(15),
        ) {
            Ok(probe) if probe["driverStore"] == true => {
                return CommandResult::Ok {
                    data: audio_preparation_state(
                        "driver_present",
                        "驱动已经存在，但端点尚未就绪；不会重复安装。请重新检测设备状态。",
                    ),
                };
            }
            Err(code) => return audio_preparation_failure(code),
            _ => {}
        }
        phase("verifying");
        if let Err(code) = crate::prerequisites::run_preparation_script(
            &fetch,
            &["-Component", "virtual-audio", "-Destination", &destination],
            std::time::Duration::from_secs(300),
            &mut |event| match event {
                crate::prerequisites::PreparationEvent::Phase(value) => phase(&value),
                crate::prerequisites::PreparationEvent::Exit(value) => exit_code.set(value),
            },
        ) {
            return audio_preparation_failure(code);
        }
        phase("authorizing");
        exit_code.set(None);
        let install_result = match crate::prerequisites::run_preparation_script(
            &install,
            &[
                "-Component",
                "virtual-audio",
                "-ResourcesDirectory",
                &destination,
            ],
            std::time::Duration::from_secs(600),
            &mut |event| match event {
                crate::prerequisites::PreparationEvent::Phase(value) => phase(&value),
                crate::prerequisites::PreparationEvent::Exit(value) => exit_code.set(value),
            },
        ) {
            Ok(Some(result)) => result,
            Ok(None) => return audio_preparation_failure("PREREQUISITE_RESULT_INVALID"),
            Err(code) => return audio_preparation_failure(code),
        };
        phase("rechecking");
        if let Ok(devices) = crate::prerequisites::enumerate_audio_devices(&bridge) {
            let status = crate::prerequisites::VirtualAudioPreparation::from_devices(&devices);
            if status.installed {
                return CommandResult::Ok { data: status };
            }
        }
        let reboot_required = install_result["rebootRequired"].as_bool().unwrap_or(false);
        CommandResult::Ok {
            data: crate::prerequisites::VirtualAudioPreparation {
                state: if reboot_required {
                    "reboot_required"
                } else {
                    "failed"
                }
                .into(),
                installed: false,
                reboot_required,
                detail: if reboot_required {
                    "驱动已安装，需要重启 Windows 后继续"
                } else {
                    "安装完成但未检测到虚拟声卡端点"
                }
                .into(),
                render_endpoint_id: None,
                capture_endpoint_id: None,
                diagnostic: None,
            },
        }
    })();
    attach_preparation_diagnostic(&state, current_phase.into_inner(), exit_code.get(), result)
}
blocking_command!(with_events virtual_audio_install, virtual_audio_install_blocking() -> crate::prerequisites::VirtualAudioPreparation);
blocking_command!(session_export, session_export_blocking(session_id: String, format: String) -> SessionExportResult);
blocking_command!(session_list, session_list_blocking() -> Vec<SessionSummary>);
blocking_command!(session_get, session_get_blocking(session_id: String) -> SessionDetail);
blocking_command!(session_delete, session_delete_blocking(session_id: String) -> FoundationStatus);
blocking_command!(with_events session_finalize_utterance, session_finalize_utterance_blocking(text: String) -> SessionTurnView);
blocking_command!(with_events session_trigger_assistant, session_trigger_assistant_blocking() -> SessionTurnView);
blocking_command!(with_events session_agent_command, session_agent_command_blocking(input: AgentCommandInput) -> AgentCommandResult);
blocking_command!(model_provider_save, model_provider_save_blocking(input: ProviderSaveInput) -> ProviderConfig);
blocking_command!(model_provider_test, model_provider_test_blocking(provider_id: String) -> ProviderTestResult);
blocking_command!(model_provider_discover, model_provider_discover_blocking(provider_id: String) -> ModelDiscoveryResult);
blocking_command!(model_provider_activate, model_provider_activate_blocking(provider_id: String) -> ProviderConfig);
blocking_command!(model_provider_delete, model_provider_delete_blocking(provider_id: String) -> FoundationStatus);
blocking_command!(model_provider_dependencies, model_provider_dependencies_blocking(provider_id: String) -> Vec<crate::services::ProviderDependency>);
blocking_command!(speech_route_save, speech_route_save_blocking(input: VoiceRouteSaveInput) -> VoiceRouteConfig);
blocking_command!(speech_route_test, speech_route_test_blocking(route_id: String) -> VoiceRouteTestResult);
blocking_command!(speech_route_activate, speech_route_activate_blocking(route_id: String) -> VoiceRouteConfig);
blocking_command!(speech_route_delete, speech_route_delete_blocking(route_id: String) -> FoundationStatus);
blocking_command!(voice_reference_save, voice_reference_save_blocking(input: VoiceReferenceSaveInput) -> VoiceReferenceSummary);
blocking_command!(voice_reference_save_audio, voice_reference_save_audio_blocking(input: VoiceReferenceAudioSaveInput) -> VoiceReferenceSummary);
blocking_command!(voice_reference_update, voice_reference_update_blocking(input: VoiceReferenceUpdateInput) -> VoiceReferenceSummary);
blocking_command!(voice_reference_list, voice_reference_list_blocking() -> Vec<VoiceReferenceSummary>);
blocking_command!(voice_reference_delete, voice_reference_delete_blocking(id: String) -> FoundationStatus);
blocking_command!(voice_reference_clone, voice_reference_clone_blocking(id: String) -> VoiceReferenceCloneResult);
blocking_command!(role_profile_save, role_profile_save_blocking(input: RoleProfileSaveInput) -> RoleProfileConfig);
blocking_command!(role_profile_copy, role_profile_copy_blocking(input: RoleProfileCopyInput) -> RoleProfileConfig);
blocking_command!(role_profile_activate, role_profile_activate_blocking(role_id: String) -> RoleProfileConfig);
blocking_command!(role_profile_delete, role_profile_delete_blocking(role_id: String) -> FoundationStatus);
blocking_command!(embedding_config_save, embedding_config_save_blocking(input: EmbeddingConfigSaveInput) -> EmbeddingConfig);
blocking_command!(embedding_config_test, embedding_config_test_blocking(embedding_id: String) -> EmbeddingTestResult);
blocking_command!(embedding_config_activate, embedding_config_activate_blocking(embedding_id: String) -> EmbeddingConfig);
blocking_command!(embedding_config_delete, embedding_config_delete_blocking(embedding_id: String) -> FoundationStatus);
blocking_command!(config_restore_last_good, config_restore_last_good_blocking() -> StartupState);
blocking_command!(config_restore_defaults, config_restore_defaults_blocking() -> StartupState);

#[cfg(test)]
#[path = "commands_ipc_tests.rs"]
mod ipc_tests;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        embedding_service_error, provider_service_error, public_config,
        role_service_error, route_service_error,
    };
    use crate::{
        app_state::{AppPaths, AppState},
        providers::ProviderError,
        secrets::MemorySecretStore,
        services::{
            EmbeddingServiceError, ProviderServiceError,
            RoleProfileServiceError, VoiceRouteServiceError,
        },
    };

    #[test]
    fn legacy_migration_status_flags_reenter_without_secret_material() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("legacy-repo");
        std::fs::create_dir_all(repo.join("config")).unwrap();
        std::fs::write(repo.join("config/local.json"), r#"{"configVersion":1}"#).unwrap();
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: vec![crate::migrate::LegacySearchRoot::Repository(repo)],
        };
        let state = AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap();

        let json = serde_json::to_value(super::legacy_migration_status_cmd(&state)).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["data"]["applied"], true);
        assert_eq!(json["data"]["reenterSecrets"], true);
        assert!(json["data"]["omitted"].is_array());
        let encoded = json.to_string().to_ascii_lowercase();
        for needle in [
            "password",
            "secretvalue",
            "sk-live",
            &crate::migrate::legacy_login_cookie_name(),
            "desktop_session",
        ] {
            assert!(!encoded.contains(needle), "leaked {needle}: {encoded}");
        }
    }

    #[test]
    fn legacy_import_source_reads_user_selected_desktop_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("old-install");
        std::fs::create_dir_all(source.join(".desktop-runtime/data")).unwrap();
        let connection =
            rusqlite::Connection::open(source.join(".desktop-runtime/data/app.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE archived_sessions (
                    session_id TEXT PRIMARY KEY,
                    finished_at TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                 );
                 INSERT INTO archived_sessions
                 VALUES (
                    'legacy-session-1',
                    '2026-01-02T03:05:00Z',
                    '{\"sessionId\":\"legacy-session-1\",\"revision\":1,\"status\":\"finished\",\"speakingText\":\"\",\"candidateName\":\"合成姓名\",\"roleName\":\"合成岗位\",\"assistantRole\":\"interviewer\",\"jobDescription\":\"\",\"interviewFocus\":\"\",\"consentConfirmed\":true,\"consentConfirmedAt\":\"2026-01-02T03:00:00Z\",\"startedAt\":\"2026-01-02T03:04:00Z\",\"finishedAt\":\"2026-01-02T03:05:00Z\",\"transcript\":[{\"role\":\"candidate\",\"text\":\"合成会话轮次A\",\"at\":\"2026-01-02T03:04:05Z\"},{\"role\":\"interviewer\",\"text\":\"合成助手回复A\",\"at\":\"2026-01-02T03:04:06Z\"}],\"report\":null,\"resumeIds\":[],\"resumeId\":\"\"}',
                    '2026-01-02T03:05:00Z'
                 );",
            )
            .unwrap();
        drop(connection);
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        };
        let state = AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap();
        let json = serde_json::to_value(super::legacy_import_source_cmd(
            &state,
            source.to_string_lossy().into_owned(),
        ))
        .unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["data"]["sessions"], 1);
        assert_eq!(json["data"]["turns"], 1);
        assert_eq!(
            serde_json::to_value(super::legacy_import_source_cmd(
                &state,
                "relative/old".into()
            ))
            .unwrap()["ok"],
            false
        );
    }

    #[test]
    fn initialize_does_not_auto_import_switched_materials() {
        let directory = tempfile::tempdir().unwrap();
        let repo = directory.path().join("legacy-repo");
        std::fs::create_dir_all(repo.join("config")).unwrap();
        std::fs::create_dir_all(repo.join("data/materials")).unwrap();
        std::fs::write(repo.join("config/local.json"), r#"{"configVersion":1}"#).unwrap();
        std::fs::write(repo.join("data/materials/resume.md"), "工作经历\n").unwrap();
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: vec![crate::migrate::LegacySearchRoot::Repository(repo)],
        };
        let state =
            AppState::initialize(paths.clone(), Arc::new(MemorySecretStore::default())).unwrap();
        assert!(paths.data_directory.join("materials/resume.md").is_file());
        let listed = serde_json::to_value(super::material_list_cmd(&state)).unwrap();
        assert_eq!(listed["ok"], true);
        assert_eq!(listed["data"], serde_json::json!([]));
    }

    #[test]
    fn legacy_migration_status_hides_reenter_when_a_slot_is_configured() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        };
        std::fs::create_dir_all(&paths.data_directory).unwrap();
        std::fs::write(
            paths.data_directory.join("migrated-from"),
            r#"{"archivePath":"C:/tmp/archive","utc":"2026-09-06T00:00:00Z","omitted":[]}"#,
        )
        .unwrap();
        std::fs::write(
            &paths.config_path,
            r#"{"configVersion":1,"models":{"providers":[{"id":"p1","baseUrl":"https://one.example","credential":{"reference":"providers/p1/api-key","configured":true}}]}}"#,
        )
        .unwrap();
        let state = AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap();
        let json = serde_json::to_value(super::legacy_migration_status_cmd(&state)).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["data"]["applied"], true);
        assert_eq!(json["data"]["reenterSecrets"], false);
    }

    #[test]
    fn config_get_public_returns_redacted_config_without_secret_material() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        };
        std::fs::write(
            &paths.config_path,
            r#"{"configVersion":1,"models":{"providers":[{"id":"p1","baseUrl":"https://one.example","credential":{"reference":"providers/p1/api-key","configured":true}}]}}"#,
        )
        .unwrap();
        let state = AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap();

        let json = serde_json::to_string(&public_config(&state)).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["configVersion"], 1);
        let credential = &value["data"]["models"]["providers"][0]["credential"];
        assert_eq!(credential["reference"], "providers/p1/api-key");
        assert_eq!(credential["configured"], true);
        assert_eq!(
            credential.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["configured", "reference"]
        );
        for needle in [
            "must-never-cross",
            "password",
            "secretvalue",
            "secretcontents",
        ] {
            assert!(
                !json.to_ascii_lowercase().contains(needle),
                "leaked secret material: {needle}"
            );
        }
    }

    #[test]
    fn config_get_public_surfaces_load_failure_as_command_error() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        };
        std::fs::write(&paths.config_path, "not-json").unwrap();
        let state = AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap();

        let json = serde_json::to_string(&public_config(&state)).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("CONFIG_INVALID"));
    }

    #[test]
    fn service_errors_preserve_field_and_retry_contracts() {
        let provider = serde_json::to_value(provider_service_error::<()>(
            ProviderServiceError::InvalidId,
        ))
        .unwrap();
        assert_eq!(provider["error"]["field"], "id");
        let route = serde_json::to_value(route_service_error::<()>(
            VoiceRouteServiceError::FieldsInvalid,
        ))
        .unwrap();
        assert_eq!(route["error"]["field"], "route");
        let timeout = serde_json::to_value(provider_service_error::<()>(
            ProviderServiceError::Provider(ProviderError::Timeout),
        ))
        .unwrap();
        assert_eq!(timeout["error"]["retryable"], true);
        let review = serde_json::to_value(role_service_error::<()>(
            RoleProfileServiceError::ReviewRequired,
        ))
        .unwrap();
        assert_eq!(review["error"]["code"], "ROLE_PROFILE_REVIEW_REQUIRED");
        assert_eq!(review["error"]["field"], "id");
        assert!(
            review["error"]["message"]
                .as_str()
                .unwrap()
                .contains("save")
        );
        assert!(
            !review["error"]["message"]
                .as_str()
                .unwrap()
                .contains("Ask one question")
        );
        let embedding = serde_json::to_value(embedding_service_error::<()>(
            EmbeddingServiceError::FieldsInvalid,
        ))
        .unwrap();
        assert_eq!(embedding["error"]["field"], "dimensions");
    }

    #[test]
    fn restore_commands_take_the_service_guard() {
        let source = include_str!("commands.rs");
        let last_good = source
            .split("pub fn config_restore_last_good")
            .nth(1)
            .unwrap()
            .split("pub fn config_restore_defaults")
            .next()
            .unwrap();
        let defaults = source
            .split("pub fn config_restore_defaults")
            .nth(1)
            .unwrap()
            .split("pub fn open_app_directory")
            .next()
            .unwrap();
        assert!(last_good.contains("service_guard"));
        assert!(defaults.contains("service_guard"));
    }

    #[test]
    fn diagnostics_export_omits_role_prompt_bodies() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        };
        std::fs::write(
            &paths.config_path,
            r#"{
                "configVersion":1,
                "roleProfiles":[{
                    "id":"interviewer",
                    "name":"Interviewer",
                    "systemPrompt":"PROMPT-MARKER-MUST-NOT-EXPORT",
                    "openingMessage":"OPENING-MARKER-MUST-NOT-EXPORT",
                    "styleInstructions":"STYLE-MARKER-MUST-NOT-EXPORT",
                    "active":false,
                    "configVersion":1
                }]
            }"#,
        )
        .unwrap();
        let state = AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap();
        let config = state.config.load().unwrap();
        let public = serde_json::to_string(&crate::config::public_view(&config)).unwrap();
        assert!(public.contains("PROMPT-MARKER-MUST-NOT-EXPORT"));
        assert!(public.contains("OPENING-MARKER-MUST-NOT-EXPORT"));
        assert!(public.contains("STYLE-MARKER-MUST-NOT-EXPORT"));
        let destination = directory.path().join("report.json");
        state
            .diagnostics
            .export(
                &destination,
                serde_json::to_value(crate::config::diagnostic_view(&config)).unwrap(),
                serde_json::json!({ "database": "ready" }),
            )
            .unwrap();
        let report = std::fs::read_to_string(destination).unwrap();
        assert!(!report.contains("PROMPT-MARKER-MUST-NOT-EXPORT"));
        assert!(!report.contains("OPENING-MARKER-MUST-NOT-EXPORT"));
        assert!(!report.contains("STYLE-MARKER-MUST-NOT-EXPORT"));
        assert!(!report.contains("systemPrompt"));
        assert!(!report.contains("openingMessage"));
        assert!(!report.contains("styleInstructions"));
        assert!(report.contains("interviewer"));
    }

    fn material_paths(directory: &tempfile::TempDir) -> AppPaths {
        AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        }
    }

    fn material_state(directory: &tempfile::TempDir) -> AppState {
        let paths = material_paths(directory);
        std::fs::write(&paths.config_path, r#"{"configVersion":1}"#).unwrap();
        AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap()
    }

    fn command_error_code<T: serde::Serialize + ts_rs::TS>(
        result: &crate::contracts::CommandResult<T>,
    ) -> String {
        let value = serde_json::to_value(result).unwrap();
        value["error"]["code"].as_str().unwrap().to_owned()
    }

    fn command_error_message<T: serde::Serialize + ts_rs::TS>(
        result: &crate::contracts::CommandResult<T>,
    ) -> String {
        let value = serde_json::to_value(result).unwrap();
        value["error"]["message"].as_str().unwrap().to_owned()
    }

    #[test]
    fn material_write_commands_take_the_service_guard() {
        let source = include_str!("commands.rs");
        let import = source
            .split("pub fn material_import")
            .nth(1)
            .expect("material_import command")
            .split("pub fn material_search")
            .next()
            .unwrap();
        let delete = source
            .split("pub fn material_delete")
            .nth(1)
            .expect("material_delete command")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        assert!(import.contains("service_guard"));
        assert!(delete.contains("service_guard"));
    }

    #[test]
    fn material_read_commands_skip_the_service_guard() {
        let source = include_str!("commands.rs");
        let list = source
            .split("pub fn material_list")
            .nth(1)
            .expect("material_list command")
            .split("pub fn material_import")
            .next()
            .unwrap();
        let search = source
            .split("pub fn material_search")
            .nth(1)
            .expect("material_search command")
            .split("pub fn material_delete")
            .next()
            .unwrap();
        assert!(!list.contains("service_guard"));
        assert!(!search.contains("service_guard"));
    }

    #[test]
    fn material_import_rejects_relative_missing_and_directory_paths() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        let relative = super::material_import_cmd(&state, "notes.txt".into());
        assert_eq!(command_error_code(&relative), "MATERIAL_PATH_INVALID");
        assert!(!command_error_message(&relative).contains("notes.txt"));

        let missing = directory.path().join("missing.txt");
        let missing = super::material_import_cmd(&state, missing.to_string_lossy().into_owned());
        assert_eq!(command_error_code(&missing), "MATERIAL_PATH_INVALID");

        let folder = directory.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let as_dir = super::material_import_cmd(&state, folder.to_string_lossy().into_owned());
        assert_eq!(command_error_code(&as_dir), "MATERIAL_PATH_INVALID");
        assert!(!command_error_message(&as_dir).contains("SELECT"));
    }

    #[test]
    fn material_commands_fail_closed_when_database_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        *state.database.lock().unwrap() = None;
        let listed = super::material_list_cmd(&state);
        assert_eq!(command_error_code(&listed), "DATABASE_OPERATION_FAILED");
        let searched = super::material_search_cmd(&state, "订单服务".into(), None);
        assert_eq!(command_error_code(&searched), "DATABASE_OPERATION_FAILED");
    }

    #[test]
    fn material_commands_list_import_search_and_delete() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        let source = directory.path().join("resume.md");
        std::fs::write(
            &source,
            "工作经历\n2019.03-2021.06 阿里巴巴 高级工程师\n负责订单服务与 Kafka 链路。",
        )
        .unwrap();

        let imported = serde_json::to_value(super::material_import_cmd(
            &state,
            source.to_string_lossy().into_owned(),
        ))
        .unwrap();
        assert_eq!(imported["ok"], true);
        assert_eq!(imported["data"]["fileName"], "resume.md");
        assert!(imported["data"].get("extractedText").is_none());
        let id = imported["data"]["id"].as_str().unwrap().to_owned();

        let listed = serde_json::to_value(super::material_list_cmd(&state)).unwrap();
        assert_eq!(listed["ok"], true);
        assert_eq!(listed["data"][0]["id"], id);
        assert!(listed["data"][0].get("extractedText").is_none());

        let hits =
            serde_json::to_value(super::material_search_cmd(&state, "订单服务".into(), None))
                .unwrap();
        assert_eq!(hits["ok"], true);
        assert_eq!(hits["data"][0]["materialId"], id);
        assert!(
            hits["data"][0]["snippet"]
                .as_str()
                .unwrap()
                .contains("订单服务")
        );
        assert!(hits["data"][0].get("extractedText").is_none());

        let operators = serde_json::to_value(super::material_search_cmd(
            &state,
            "订单服务 OR".into(),
            Some(5),
        ))
        .unwrap();
        assert_eq!(operators["ok"], true);
        assert_ne!(operators["error"]["code"], "MATERIAL_OPERATION_FAILED");

        let deleted = serde_json::to_value(super::material_delete_cmd(&state, id)).unwrap();
        assert_eq!(deleted["ok"], true);
        assert_eq!(deleted["data"]["ready"], true);
        let empty = serde_json::to_value(super::material_list_cmd(&state)).unwrap();
        assert_eq!(empty["data"].as_array().unwrap().len(), 0);
    }

    fn command_body<'a>(source: &'a str, name: &str) -> &'a str {
        source
            .split(&format!("pub fn {name}"))
            .nth(1)
            .unwrap_or_else(|| panic!("missing command {name}"))
    }

    #[test]
    fn session_mutating_commands_take_the_service_guard() {
        let source = include_str!("commands.rs");
        for name in [
            "session_start",
            "session_stop",
            "session_delete",
            "session_export",
            "session_finalize_utterance",
            "session_agent_command",
        ] {
            assert!(
                command_body(source, name).contains("service_guard"),
                "{name} must take service_guard"
            );
        }
    }

    #[test]
    fn session_read_and_mode_commands_skip_the_service_guard() {
        let source = include_str!("commands.rs");
        for name in [
            "session_list",
            "session_get",
            "session_set_mode",
            "runtime_get_status",
        ] {
            let body = command_body(source, name);
            let until_next = body
                .split("\nfn ")
                .next()
                .and_then(|chunk| chunk.split("\npub fn ").next())
                .unwrap_or(body);
            assert!(
                !until_next.contains("service_guard"),
                "{name} must not take service_guard"
            );
        }
    }

    pub(super) fn ready_session_config() -> String {
        r#"{
            "configVersion":1,
            "models":{"providers":[
                {"id":"asr-1","baseUrl":"https://asr.example.test/v1","credential":{"reference":"providers/asr-1/api-key","configured":true}},
                {"id":"llm-1","baseUrl":"https://llm.example.test/v1","credential":{"reference":"providers/llm-1/api-key","configured":true}},
                {"id":"tts-1","baseUrl":"https://tts.example.test/v1","credential":{"reference":"providers/tts-1/api-key","configured":true}}
            ]},
            "speech":{"voiceRoutes":[{
                "id":"route-1","name":"Default","mode":"cascaded",
                "asrProviderId":"asr-1","asrModelId":"whisper",
                "llmProviderId":"llm-1","llmModelId":"gpt",
                "ttsProviderId":"tts-1","ttsModelId":"tts-model",
                "voiceId":"alloy","active":true,"ready":true,"status":"ready","configVersion":1
            }],"activeVoiceRouteId":"route-1"},
            "roleProfiles":[{
                "id":"role-1","name":"Interviewer",
                "systemPrompt":"PROMPT-BODY","openingMessage":"OPENING-BODY","styleInstructions":"STYLE-BODY",
                "active":true,"configVersion":1
            }],
            "activeRoleProfileId":"role-1"
        }"#
        .into()
    }

    fn session_state(directory: &tempfile::TempDir, config: &str) -> AppState {
        let paths = AppPaths {
            data_directory: directory.path().join("data"),
            logs_directory: directory.path().join("logs"),
            config_path: directory.path().join("config.json"),
            legacy_search_roots: Vec::new(),
        };
        std::fs::write(&paths.config_path, config).unwrap();
        AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap()
    }

    #[test]
    fn configured_but_missing_provider_credential_fails_before_network() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(
            &directory,
            r#"{"configVersion":1,"models":{"providers":[{"id":"qwen","baseUrl":"https://example.test/v1","credential":{"reference":"providers/qwen/api-key","configured":true}}]}}"#,
        );
        let config = super::load_session_config(&state).unwrap();
        let result = super::read_provider_secret(&state, &config, Some("qwen"));
        assert!(matches!(result, Err(ref e) if e.code == "PROVIDER_CREDENTIAL_MISSING"));
    }

    #[test]
    fn audio_ready_poll_does_not_block_while_a_turn_holds_the_session() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let state_ref = &state;
            scope.spawn(move || {
                let _guard = state_ref.sessions.lock().unwrap();
                locked_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(std::time::Duration::from_millis(500));
            });
            locked_rx.recv().unwrap();
            let start = std::time::Instant::now();
            let result = serde_json::to_value(super::session_audio_ready_cmd(&state)).unwrap();
            let elapsed = start.elapsed();
            let _ = release_tx.send(());
            assert_eq!(result["data"]["ready"], false);
            assert!(elapsed < std::time::Duration::from_millis(200));
        });
    }

    #[test]
    fn session_start_returns_blocked_or_started_without_secret_or_pcm() {
        let empty_dir = tempfile::tempdir().unwrap();
        let empty = session_state(&empty_dir, r#"{"configVersion":1}"#);
        let blocked = serde_json::to_value(super::session_start_cmd(&empty)).unwrap();
        assert_eq!(blocked["ok"], true);
        assert_eq!(blocked["data"]["kind"], "blocked");
        let issues = blocked["data"]["issues"].as_array().unwrap();
        assert!(
            issues
                .iter()
                .any(|issue| issue["code"] == "SESSION_ROUTE_REQUIRED")
        );
        assert!(
            !issues
                .iter()
                .any(|issue| issue["code"] == "SESSION_ROLE_REQUIRED")
        );
        assert!(!serde_json::to_string(&blocked).unwrap().contains("pcm"));

        let ready_dir = tempfile::tempdir().unwrap();
        let ready = session_state(&ready_dir, &ready_session_config());
        let started = serde_json::to_value(super::session_start_cmd(&ready)).unwrap();
        assert_eq!(started["ok"], true, "{started}");
        assert_eq!(started["data"]["kind"], "started");
        assert_eq!(started["data"]["session"]["status"], "listening");
        assert_eq!(started["data"]["session"]["transportMode"], "direct");
        let json = serde_json::to_string(&started).unwrap();
        assert!(!json.contains("PROMPT-BODY"));
        assert!(!json.contains("pcm"));
        assert!(!json.to_ascii_lowercase().contains("sk-"));
    }

    #[test]
    fn selected_role_is_session_local_and_snapshot_survives_config_changes() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let started = serde_json::to_value(super::session_start_selected_cmd(
            &state,
            Some("preset-hr"),
            Some("route-1"),
            false,
        ))
        .unwrap();
        assert_eq!(started["data"]["session"]["roleProfileId"], "preset-hr");
        let before = super::load_session_config(&state).unwrap();
        assert_eq!(
            state
                .config
                .load()
                .unwrap()
                .active_role_profile_id
                .as_deref(),
            Some("role-1")
        );
        state
            .config
            .update(|config| {
                config
                    .role_profiles
                    .iter_mut()
                    .find(|role| role.id == "preset-hr")
                    .unwrap()
                    .system_prompt = "changed during session".into();
                Ok(())
            })
            .unwrap();
        assert_eq!(super::load_session_config(&state).unwrap(), before);
        super::session_stop_cmd(&state);
        assert_ne!(super::load_session_config(&state).unwrap(), before);
    }

    #[test]
    fn invalid_session_selection_does_not_start_or_mutate_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let before = state.config.load().unwrap();
        for (role, route, code) in [
            (Some("missing"), None, "SESSION_ROLE_REQUIRED"),
            (None, Some("missing"), "SESSION_ROUTE_REQUIRED"),
        ] {
            let result = serde_json::to_value(super::session_start_selected_cmd(
                &state, role, route, false,
            ))
            .unwrap();
            assert_eq!(result["error"]["code"], code);
            assert!(state.sessions.lock().unwrap().session_id().is_none());
            assert_eq!(state.config.load().unwrap(), before);
        }
    }

    #[test]
    fn web_source_opener_rejects_non_web_and_credential_urls() {
        for url in [
            "file:///C:/Windows/System32/cmd.exe",
            "javascript:alert(1)",
            "https://user:secret@example.com",
            "https://user@example.com",
            "ms-settings:privacy",
            "not a url",
        ] {
            let result = serde_json::to_value(super::open_web_source(url.into())).unwrap();
            assert_eq!(result["error"]["code"], "WEB_SOURCE_INVALID");
        }
    }

    #[test]
    fn meeting_start_never_falls_back_when_capture_is_missing_or_pid_is_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let enumerator = crate::processes::InjectedProcessEnumerator::new(vec![
            crate::processes::MeetingProcess {
                pid: 123,
                name: "zoom.exe".into(),
                title: "Synthetic meeting".into(),
            },
        ]);
        let missing = directory.path().join("missing-AudioBridge.exe");
        for (pid, code) in [
            (123, "SESSION_SIDECAR_MISSING"),
            (456, "MEETING_PROCESS_NOT_AVAILABLE"),
            (0, "SESSION_SIDECAR_INVALID_PID"),
        ] {
            let result = serde_json::to_value(super::session_start_capture_cmd(
                &state,
                None,
                None,
                false,
                Some(crate::services::MeetingCapture {
                    exe: &missing,
                    pid,
                    enumerator: &enumerator,
                }),
            ))
            .unwrap();
            assert_eq!(result["error"]["code"], code);
            assert!(state.sessions.lock().unwrap().session_id().is_none());
        }
    }


    #[test]
    fn session_commands_list_get_export_delete_and_status() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let started = serde_json::to_value(super::session_start_cmd(&state)).unwrap();
        let id = started["data"]["session"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let listed = serde_json::to_value(super::session_list_cmd(&state)).unwrap();
        assert_eq!(listed["ok"], true);
        assert_eq!(listed["data"][0]["id"], id);

        let detail = serde_json::to_value(super::session_get_cmd(&state, id.clone())).unwrap();
        assert_eq!(detail["ok"], true);
        assert_eq!(detail["data"]["session"]["id"], id);
        assert!(detail["data"]["turns"].as_array().unwrap().is_empty());
        assert!(detail["data"].get("extractedText").is_none());

        let exported = serde_json::to_value(super::session_export_cmd(
            &state,
            id.clone(),
            "markdown".into(),
        ))
        .unwrap();
        assert_eq!(exported["ok"], true);
        let path = exported["data"]["path"].as_str().unwrap();
        assert!(path.contains("exports"));
        assert!(std::path::Path::new(path).is_file());
        let export_text = std::fs::read_to_string(path).unwrap();
        assert!(export_text.contains(&id));
        assert!(!export_text.contains("PROMPT-BODY"));

        let status = serde_json::to_value(super::runtime_get_status_cmd(&state)).unwrap();
        assert_eq!(status["ok"], true);
        assert_eq!(status["data"]["phase"], "listening");
        assert_eq!(status["data"]["mode"], "ai_active");
        assert!(status["data"]["seq"].as_u64().is_some());
        assert_eq!(status["data"]["unusedMaterials"], false);
        assert!(status["data"]["lastErrorCode"].is_null());
        assert_eq!(status["data"]["revision"], 0);

        let mode = serde_json::to_value(super::session_set_mode_cmd(
            &state,
            "operator_speaking".into(),
        ))
        .unwrap();
        assert_eq!(mode["ok"], true);
        assert_eq!(mode["data"]["mode"], "operator_speaking");

        let deleted = serde_json::to_value(super::session_delete_cmd(&state, id)).unwrap();
        assert_eq!(deleted["ok"], true);
        let empty = serde_json::to_value(super::session_list_cmd(&state)).unwrap();
        assert_eq!(empty["data"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn session_stop_keeps_failed_status() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        assert_eq!(
            serde_json::to_value(super::session_start_cmd(&state)).unwrap()["ok"],
            true
        );
        {
            let db = state.database.lock().unwrap();
            let mut sessions = state.sessions.lock().unwrap();
            sessions.capture().mark_sidecar_exited();
            sessions.poll_sidecar(db.as_ref().unwrap()).unwrap();
            sessions.capture().mark_sidecar_exited();
            let error = sessions
                .poll_sidecar(db.as_ref().unwrap())
                .expect_err("second crash");
            assert_eq!(error.code(), "SESSION_SIDECAR_FAILED");
        }
        let stopped = serde_json::to_value(super::session_stop_cmd(&state)).unwrap();
        assert_eq!(stopped["ok"], true);
        assert_eq!(stopped["data"]["status"], "failed");
    }

    #[test]
    fn session_event_payloads_carry_incrementing_seq_without_pcm() {
        let first = super::runtime_status_event(1, "listening", "ai_active", false, None, 0);
        let second = super::transcript_event(2, "hello");
        let third = super::reply_event(3, "world");
        let level = super::audio_level_event(4, 0.42);
        assert_eq!(first["seq"], 1);
        assert_eq!(second["seq"], 2);
        assert_eq!(third["seq"], 3);
        assert_eq!(level["seq"], 4);
        assert_eq!(level["peak"], 0.42);
        for payload in [first, second, third, level] {
            let json = payload.to_string();
            assert!(!json.contains("pcm"));
            assert!(!json.contains("extractedText"));
        }
        assert_eq!(
            super::transcript_event(5, &"x".repeat(200))["text"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            160
        );
    }

    #[test]
    fn finalize_event_seqs_bump_separately() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let first = super::bump_event_seq(&state);
        let second = super::bump_event_seq(&state);
        assert_eq!(second, first + 1);
        assert_ne!(first, second);
    }

    struct ScriptedAsr;
    struct ScriptedLlm(&'static str);
    struct ScriptedTts;
    struct UnusedEmbed;

    impl crate::providers::SpeechToText for ScriptedAsr {
        fn transcribe(
            &self,
            _: &crate::providers::ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[u8],
            _: u32,
        ) -> Result<String, crate::providers::CascadeError> {
            Ok("ignored".into())
        }
    }

    impl crate::providers::ChatModel for ScriptedLlm {
        fn complete(
            &self,
            _: &crate::providers::ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[crate::providers::ChatMessage],
        ) -> Result<String, crate::providers::CascadeError> {
            Ok(self.0.into())
        }
    }

    struct FailingLlm(crate::providers::CascadeError);
    struct GateLlm {
        entered: std::sync::Arc<std::sync::atomic::AtomicBool>,
        proceed: std::sync::Arc<std::sync::atomic::AtomicBool>,
        calls: std::sync::atomic::AtomicU32,
    }

    impl crate::providers::ChatModel for FailingLlm {
        fn complete(
            &self,
            _: &crate::providers::ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[crate::providers::ChatMessage],
        ) -> Result<String, crate::providers::CascadeError> {
            Err(self.0)
        }
    }

    impl crate::providers::ChatModel for GateLlm {
        fn complete(
            &self,
            _: &crate::providers::ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[crate::providers::ChatMessage],
        ) -> Result<String, crate::providers::CascadeError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.entered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            while !self.proceed.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok("慢回复".into())
        }
    }

    struct CountingTts(std::sync::Arc<std::sync::atomic::AtomicU32>);

    impl crate::providers::TextToSpeech for CountingTts {
        fn synthesize(
            &self,
            _: &crate::providers::ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, crate::providers::CascadeError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![0x01])
        }
    }

    impl crate::providers::TextToSpeech for ScriptedTts {
        fn synthesize(
            &self,
            _: &crate::providers::ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<Vec<u8>, crate::providers::CascadeError> {
            Ok(vec![0x01, 0x02])
        }
    }

    struct UnusedRealtime;

    impl crate::providers::RealtimeModel for UnusedRealtime {
        fn transcribe_turn(
            &self,
            _: crate::providers::RealtimeAudioRequest<'_>,
            _: &std::sync::atomic::AtomicBool,
        ) -> Result<crate::providers::RealtimeTurn, crate::providers::RealtimeError> {
            panic!("cascaded command test must not call Realtime")
        }
    }

    impl crate::providers::EmbeddingProbe for UnusedEmbed {
        fn embed(
            &self,
            _: &crate::providers::ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: u32,
            _: &str,
        ) -> Result<Vec<f32>, crate::providers::EmbeddingError> {
            Err(crate::providers::EmbeddingError::RequestFailed)
        }
    }

    #[test]
    fn session_finalize_utterance_persists_turn_without_pcm() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        assert_eq!(
            serde_json::to_value(super::session_start_cmd(&state)).unwrap()["ok"],
            true
        );
        let asr = ScriptedAsr;
        let llm = ScriptedLlm("这是一个后端岗位");
        let tts = ScriptedTts;
        let embed = UnusedEmbed;
        let probes = crate::services::SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        let finalized = serde_json::to_value(super::session_finalize_utterance_cmd(
            &state,
            &probes,
            crate::runtime::CascadeCredentials::default(),
            Some("请介绍岗位"),
        ))
        .unwrap();
        assert_eq!(finalized["ok"], true, "{finalized}");
        assert_eq!(finalized["data"]["userText"], "请介绍岗位");
        assert_eq!(finalized["data"]["assistantText"], "这是一个后端岗位");
        assert_eq!(finalized["data"]["materialsUsed"], false);
        let json = finalized.to_string();
        assert!(!json.contains("pcm"));
        assert!(!json.contains("extractedText"));

        let id = serde_json::to_value(super::session_list_cmd(&state)).unwrap()["data"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let detail = serde_json::to_value(super::session_get_cmd(&state, id)).unwrap();
        assert_eq!(detail["data"]["turns"].as_array().unwrap().len(), 1);
        assert_eq!(
            detail["data"]["turns"][0]["assistantText"],
            "这是一个后端岗位"
        );

        let missing = serde_json::to_value(super::session_finalize_utterance_cmd(
            &session_state(&tempfile::tempdir().unwrap(), &ready_session_config()),
            &probes,
            crate::runtime::CascadeCredentials::default(),
            Some("hi"),
        ))
        .unwrap();
        assert_eq!(missing["ok"], false);
        assert_eq!(missing["error"]["code"], "SESSION_NOT_FOUND");
    }

    fn voiced_pcm(frames: usize) -> Vec<u8> {
        let mut pcm = Vec::with_capacity(frames * 1920);
        for _ in 0..frames {
            for sample in 0..960i16 {
                let value = if sample % 2 == 0 { 2000i16 } else { -2000i16 };
                pcm.extend_from_slice(&value.to_le_bytes());
            }
        }
        pcm
    }

    fn silence_pcm(frames: usize) -> Vec<u8> {
        vec![0x00; frames * 1920]
    }

    fn push_mic_pcm(state: &AppState, pcm: &[u8], sample_rate: u32) -> serde_json::Value {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(pcm);
        serde_json::to_value(super::session_push_mic_pcm_cmd(state, &encoded, sample_rate))
            .unwrap()
    }

    #[test]
    fn session_push_mic_pcm_feeds_segmenter_and_finalizes_via_asr() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        assert_eq!(
            serde_json::to_value(super::session_start_cmd(&state)).unwrap()["ok"],
            true
        );

        // 240ms 语音 + 800ms 静音：超过 START/MIN_SPEECH/END_SILENCE 阈值，应产出一条待转写语句。
        let pcm = voiced_pcm(12);
        let pcm = [pcm, silence_pcm(40)].concat();
        let pushed = push_mic_pcm(&state, &pcm, 48_000);
        assert_eq!(pushed["ok"], true, "{pushed}");
        assert_eq!(pushed["data"]["accepted"], true);

        let ready = serde_json::to_value(super::session_audio_ready_cmd(&state)).unwrap();
        assert_eq!(ready["data"]["ready"], true);

        let asr = ScriptedAsr;
        let llm = ScriptedLlm("收到");
        let tts = ScriptedTts;
        let embed = UnusedEmbed;
        let probes = crate::services::SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        let finalized = serde_json::to_value(super::session_finalize_utterance_cmd(
            &state,
            &probes,
            crate::runtime::CascadeCredentials::default(),
            None,
        ))
        .unwrap();
        assert_eq!(finalized["ok"], true, "{finalized}");
        assert_eq!(finalized["data"]["userText"], "ignored");
        assert_eq!(finalized["data"]["assistantText"], "收到");
    }

    #[test]
    fn session_push_mic_pcm_resamples_non_48k_input() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        assert_eq!(
            serde_json::to_value(super::session_start_cmd(&state)).unwrap()["ok"],
            true
        );

        // 16kHz 采集：300ms 语音 + 900ms 静音。若命令忽略 sample_rate，
        // 这段音频会被当成 48kHz 解析，语句永远不会达到断句阈值。
        let mut speech_16k = Vec::new();
        for sample in 0..4_800i16 {
            let value = if sample % 2 == 0 { 2000i16 } else { -2000i16 };
            speech_16k.extend_from_slice(&value.to_le_bytes());
        }
        let silence_16k = vec![0x00; 14_400 * 2];
        let pcm = [speech_16k, silence_16k].concat();
        let pushed = push_mic_pcm(&state, &pcm, 16_000);
        assert_eq!(pushed["ok"], true, "{pushed}");

        let ready = serde_json::to_value(super::session_audio_ready_cmd(&state)).unwrap();
        assert_eq!(ready["data"]["ready"], true);
    }

    #[test]
    fn session_push_mic_pcm_without_session_reports_not_found() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let pushed = push_mic_pcm(&state, &voiced_pcm(3), 48_000);
        assert_eq!(pushed["ok"], false, "{pushed}");
        assert_eq!(pushed["error"]["code"], "SESSION_NOT_FOUND");
    }

    #[test]
    fn session_finalize_after_llm_error_is_not_state_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        assert_eq!(
            serde_json::to_value(super::session_start_cmd(&state)).unwrap()["ok"],
            true
        );
        let asr = ScriptedAsr;
        let fail = FailingLlm(crate::providers::CascadeError::RequestFailed(
            crate::providers::CascadeStage::Llm,
        ));
        let tts = ScriptedTts;
        let embed = UnusedEmbed;
        let failed = serde_json::to_value(super::session_finalize_utterance_cmd(
            &state,
            &crate::services::SessionProbes {
                asr: &asr,
                llm: &fail,
                tts: &tts,
                embed: &embed,
                realtime: &UnusedRealtime,
            },
            crate::runtime::CascadeCredentials::default(),
            Some("第一轮"),
        ))
        .unwrap();
        assert_eq!(failed["ok"], false, "{failed}");
        assert_eq!(failed["error"]["code"], "LLM_REQUEST_FAILED");
        assert_ne!(failed["error"]["code"], "SESSION_STATE_INVALID");

        let ok_llm = ScriptedLlm("第二轮回复");
        let second = serde_json::to_value(super::session_finalize_utterance_cmd(
            &state,
            &crate::services::SessionProbes {
                asr: &asr,
                llm: &ok_llm,
                tts: &tts,
                embed: &embed,
                realtime: &UnusedRealtime,
            },
            crate::runtime::CascadeCredentials::default(),
            Some("第二轮"),
        ))
        .unwrap();
        assert_eq!(second["ok"], true, "{second}");
        assert_eq!(second["data"]["assistantText"], "第二轮回复");
    }

    #[test]
    fn session_stop_sets_cancel_while_finalize_holds_locks() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        assert_eq!(
            serde_json::to_value(super::session_start_cmd(&state)).unwrap()["ok"],
            true
        );
        let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let proceed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let tts_calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let asr = ScriptedAsr;
        let llm = GateLlm {
            entered: std::sync::Arc::clone(&entered),
            proceed: std::sync::Arc::clone(&proceed),
            calls: std::sync::atomic::AtomicU32::new(0),
        };
        let tts = CountingTts(std::sync::Arc::clone(&tts_calls));
        let embed = UnusedEmbed;

        std::thread::scope(|scope| {
            let finalize = scope.spawn(|| {
                serde_json::to_value(super::session_finalize_utterance_cmd(
                    &state,
                    &crate::services::SessionProbes {
                        asr: &asr,
                        llm: &llm,
                        tts: &tts,
                        embed: &embed,
                        realtime: &UnusedRealtime,
                    },
                    crate::runtime::CascadeCredentials::default(),
                    Some("慢轮"),
                ))
                .unwrap()
            });
            while !entered.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let started = std::time::Instant::now();
            let stopped = serde_json::to_value(super::session_stop_cmd(&state)).unwrap();
            assert!(
                started.elapsed() < std::time::Duration::from_millis(200),
                "stop waited for finalize: {:?}",
                started.elapsed()
            );
            assert_eq!(stopped["ok"], true, "{stopped}");
            assert!(state.session_control.is_cancelled());
            proceed.store(true, std::sync::atomic::Ordering::SeqCst);
            let finalized = finalize.join().expect("finalize thread");
            assert_eq!(finalized["ok"], false, "{finalized}");
            assert_eq!(finalized["error"]["code"], "SESSION_CANCELLED");
            assert_eq!(
                tts_calls.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "tts must not run after cancel"
            );
        });
    }

    #[test]
    fn session_agent_command_say_retry_correct_report_without_pcm() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        assert_eq!(
            serde_json::to_value(super::session_start_cmd(&state)).unwrap()["ok"],
            true
        );
        let asr = ScriptedAsr;
        let llm = ScriptedLlm("助手回复");
        let tts = ScriptedTts;
        let embed = UnusedEmbed;
        let probes = crate::services::SessionProbes {
            asr: &asr,
            llm: &llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        let credentials = crate::runtime::CascadeCredentials::default();
        assert_eq!(
            serde_json::to_value(super::session_finalize_utterance_cmd(
                &state,
                &probes,
                credentials,
                Some("你好"),
            ))
            .unwrap()["ok"],
            true
        );
        let after_finalize = serde_json::to_value(super::runtime_get_status_cmd(&state)).unwrap();
        assert_eq!(after_finalize["data"]["revision"], 1);

        let say = serde_json::to_value(super::session_agent_command_cmd(
            &state,
            &probes,
            credentials,
            super::AgentCommandInput {
                id: "cmd-say".into(),
                action: "say".into(),
                text: Some("请开始".into()),
                answer: None,
                mode: None,
                expected_revision: 1,
            },
        ))
        .unwrap();
        assert_eq!(say["ok"], true, "{say}");
        assert_eq!(say["data"]["commandId"], "cmd-say");
        assert_eq!(say["data"]["action"], "say");
        assert_eq!(say["data"]["ok"], true);
        assert_eq!(say["data"]["error"], "");
        assert_eq!(say["data"]["result"]["text"], "请开始");
        let say_json = say.to_string();
        assert!(!say_json.contains("pcm"));
        assert!(!say_json.to_ascii_lowercase().contains("sk-"));

        let stale = serde_json::to_value(super::session_agent_command_cmd(
            &state,
            &probes,
            credentials,
            super::AgentCommandInput {
                id: "cmd-stale".into(),
                action: "retry".into(),
                text: None,
                answer: None,
                mode: None,
                expected_revision: 0,
            },
        ))
        .unwrap();
        assert_eq!(stale["ok"], true, "{stale}");
        assert_eq!(stale["data"]["ok"], false);
        assert_eq!(stale["data"]["error"], "SESSION_CHANGED");

        let retry_llm = ScriptedLlm("新问题");
        let retry_probes = crate::services::SessionProbes {
            asr: &asr,
            llm: &retry_llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        let retry = serde_json::to_value(super::session_agent_command_cmd(
            &state,
            &retry_probes,
            credentials,
            super::AgentCommandInput {
                id: "cmd-retry".into(),
                action: "retry".into(),
                text: None,
                answer: None,
                mode: None,
                expected_revision: 1,
            },
        ))
        .unwrap();
        assert_eq!(retry["ok"], true, "{retry}");
        assert_eq!(retry["data"]["ok"], true);
        assert_eq!(retry["data"]["result"]["question"], "新问题");

        let correct = serde_json::to_value(super::session_agent_command_cmd(
            &state,
            &probes,
            credentials,
            super::AgentCommandInput {
                id: "cmd-correct".into(),
                action: "correct".into(),
                text: None,
                answer: Some("改成这句".into()),
                mode: None,
                expected_revision: 2,
            },
        ))
        .unwrap();
        assert_eq!(correct["ok"], true, "{correct}");
        assert_eq!(correct["data"]["result"]["answer"], "改成这句");

        let report_llm = ScriptedLlm(
            r#"{"summary":"纪要","strengths":[],"followUps":[],"limitations":[],"evidence":[]}"#,
        );
        let report_probes = crate::services::SessionProbes {
            asr: &asr,
            llm: &report_llm,
            tts: &tts,
            embed: &embed,
            realtime: &UnusedRealtime,
        };
        let report = serde_json::to_value(super::session_agent_command_cmd(
            &state,
            &report_probes,
            credentials,
            super::AgentCommandInput {
                id: "cmd-report".into(),
                action: "report".into(),
                text: None,
                answer: None,
                mode: None,
                expected_revision: 3,
            },
        ))
        .unwrap();
        assert_eq!(report["ok"], true, "{report}");
        assert_eq!(report["data"]["result"]["summary"], "纪要");
        assert!(!report.to_string().contains("pcm"));
    }

    #[test]
    fn livestream_success_advances_only_the_confirmed_bounded_script() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        let mut script = crate::livestream::LivestreamScript::draft(
            "产品".into(),
            vec![
                crate::livestream::LivestreamSegment::draft(
                    "一".into(),
                    "第一段".into(),
                    1,
                    vec![],
                ),
                crate::livestream::LivestreamSegment::draft(
                    "二".into(),
                    "第二段".into(),
                    1,
                    vec![],
                ),
            ],
            false,
        )
        .unwrap();
        script.confirm().unwrap();
        script.start().unwrap();
        *state.livestream.lock().unwrap() = Some(script);
        *state.livestream_stage.lock().unwrap() = Some(crate::livestream::LivestreamStageState {
            product_title: "产品".into(),
            current_subtitle: "第一段".into(),
            next_hint: "下一段：二".into(),
            state: crate::livestream::LivestreamState::Playing,
            media_path: None,
            media_kind: None,
            output_state: crate::livestream::LivestreamOutputState::Playing,
            output_error_code: None,
        });
        let token = state.livestream_playback_cancel.lock().unwrap().clone();

        assert_eq!(
            super::advance_livestream_after_success(&state, &token).as_deref(),
            Some("第二段")
        );
        assert!(super::advance_livestream_after_success(&state, &token).is_none());
        let script = state.livestream.lock().unwrap();
        assert_eq!(
            script.as_ref().unwrap().state,
            crate::livestream::LivestreamState::Finished
        );
        let stage = state.livestream_stage.lock().unwrap();
        assert_eq!(
            stage.as_ref().unwrap().output_state,
            crate::livestream::LivestreamOutputState::Played
        );
    }

    fn playing_two_segment_runtime(state: &AppState) {
        let mut script = crate::livestream::LivestreamScript::draft(
            "产品".into(),
            vec![
                crate::livestream::LivestreamSegment::draft(
                    "一".into(),
                    "第一段".into(),
                    1,
                    vec![],
                ),
                crate::livestream::LivestreamSegment::draft(
                    "二".into(),
                    "第二段".into(),
                    1,
                    vec![],
                ),
            ],
            false,
        )
        .unwrap();
        script.confirm().unwrap();
        script.start().unwrap();
        *state.livestream.lock().unwrap() = Some(script);
        *state.livestream_stage.lock().unwrap() = Some(crate::livestream::LivestreamStageState {
            product_title: "产品".into(),
            current_subtitle: "第一段".into(),
            next_hint: "下一段：二".into(),
            state: crate::livestream::LivestreamState::Playing,
            media_path: None,
            media_kind: None,
            output_state: crate::livestream::LivestreamOutputState::Playing,
            output_error_code: None,
        });
    }

    #[test]
    fn livestream_stale_playback_token_does_not_advance_or_mutate_stage() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        playing_two_segment_runtime(&state);
        let stale = state.livestream_playback_cancel.lock().unwrap().clone();
        *state.livestream_playback_cancel.lock().unwrap() =
            Arc::new(std::sync::atomic::AtomicBool::new(false));
        let current = state.livestream_playback_cancel.lock().unwrap().clone();

        assert!(super::advance_livestream_after_success(&state, &stale).is_none());
        super::set_livestream_output_state(
            &state,
            &stale,
            crate::livestream::LivestreamOutputState::Failed,
            Some("LIVESTREAM_TTS_FAILED"),
        );

        {
            let script = state.livestream.lock().unwrap();
            assert_eq!(script.as_ref().unwrap().current_index, Some(0));
            assert_eq!(
                script.as_ref().unwrap().state,
                crate::livestream::LivestreamState::Playing
            );
            let stage = state.livestream_stage.lock().unwrap();
            assert_eq!(
                stage.as_ref().unwrap().output_state,
                crate::livestream::LivestreamOutputState::Playing
            );
            assert_eq!(stage.as_ref().unwrap().output_error_code, None);
        }

        assert_eq!(
            super::advance_livestream_after_success(&state, &current).as_deref(),
            Some("第二段")
        );
    }

    #[test]
    fn livestream_cancelled_token_does_not_advance() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        playing_two_segment_runtime(&state);
        let token = state.livestream_playback_cancel.lock().unwrap().clone();
        token.store(true, std::sync::atomic::Ordering::SeqCst);

        assert!(super::advance_livestream_after_success(&state, &token).is_none());
        let script = state.livestream.lock().unwrap();
        assert_eq!(script.as_ref().unwrap().current_index, Some(0));
    }

    #[test]
    fn livestream_completion_cannot_restart_a_paused_script() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        playing_two_segment_runtime(&state);
        let token = state.livestream_playback_cancel.lock().unwrap().clone();
        state.livestream.lock().unwrap().as_mut().unwrap().pause().unwrap();
        assert!(super::advance_livestream_after_success(&state, &token).is_none());
        let script = state.livestream.lock().unwrap();
        assert_eq!(script.as_ref().unwrap().current_index, Some(0));
        assert_eq!(script.as_ref().unwrap().state, crate::livestream::LivestreamState::Paused);
    }

    #[test]
    fn livestream_cancelled_worker_cannot_report_failure_on_current_script() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        playing_two_segment_runtime(&state);
        let token = state.livestream_playback_cancel.lock().unwrap().clone();
        token.store(true, std::sync::atomic::Ordering::SeqCst);
        super::set_livestream_output_state(&state, &token,
            crate::livestream::LivestreamOutputState::Failed, Some("LIVESTREAM_TTS_FAILED"));
        assert_eq!(state.livestream_stage.lock().unwrap().as_ref().unwrap().output_error_code, None);
        assert_eq!(state.livestream.lock().unwrap().as_ref().unwrap().state, crate::livestream::LivestreamState::Playing);
    }

    #[test]
    fn replacing_livestream_script_cancels_the_previous_playback_token() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        playing_two_segment_runtime(&state);
        let stale = state.livestream_playback_cancel.lock().unwrap().clone();
        *state.livestream_voice.lock().unwrap() =
            Some(crate::livestream::LivestreamVoiceSnapshot {
                provider_id: "old".into(),
                base_url: "https://example.test".into(),
                model_id: "old-voice".into(),
                voice_id: "alloy".into(),
            });
        let result = super::save_livestream_runtime(
            &state,
            "新产品".into(),
            vec![crate::livestream::LivestreamSegment::draft(
                "一".into(),
                "新讲稿".into(),
                1,
                vec![],
            )],
            false,
            None,
            None,
        );
        assert!(matches!(result, crate::contracts::CommandResult::Ok { .. }));
        assert!(stale.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!std::sync::Arc::ptr_eq(
            &stale,
            &*state.livestream_playback_cancel.lock().unwrap()
        ));
        assert!(super::advance_livestream_after_success(&state, &stale).is_none());
        assert!(state.livestream_voice.lock().unwrap().is_none());
        let script = state.livestream.lock().unwrap();
        assert_eq!(script.as_ref().unwrap().title, "新产品");
        assert_eq!(
            script.as_ref().unwrap().state,
            crate::livestream::LivestreamState::Draft
        );
    }

    #[test]
    fn livestream_voice_snapshot_ignores_later_config_changes() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        let frozen = crate::livestream::LivestreamVoiceSnapshot {
            provider_id: "frozen".into(),
            base_url: "https://frozen.test/v1".into(),
            model_id: "tts-frozen".into(),
            voice_id: "coral".into(),
        };
        *state.livestream_voice.lock().unwrap() = Some(frozen.clone());
        let resolved = super::resolve_livestream_voice(&state).unwrap();
        assert_eq!(resolved, frozen);
    }

    #[test]
    fn livestream_manual_complete_does_not_mark_output_played() {
        let source = include_str!("commands.rs");
        let control = source
            .split("pub fn livestream_control")
            .nth(1)
            .expect("livestream_control")
            .split("pub async fn livestream_insert_question")
            .next()
            .unwrap();
        assert!(control.contains("Manual skip is not proof of playback"));
        assert!(control.contains("LivestreamOutputState::Cancelled"));
        assert!(!control.contains("LivestreamOutputState::Played"));
    }

    #[test]
    fn session_start_rollback_keeps_routing_when_restore_fails() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        let change = crate::prerequisites::AudioRoutingChange {
            bridge: directory.path().join("missing-audio-bridge.exe"),
            previous_id: "mic-original".into(),
            cable_id: "cable-output".into(),
            changed: true,
        };
        let code = super::rollback_session_routing(&state, change);
        assert!(code.is_some());
        assert!(state.audio_routing.lock().unwrap().is_some());
        assert!(
            directory
                .path()
                .join("data/prerequisites/audio-routing.json")
                .exists()
                || state
                    .paths
                    .data_directory
                    .join("prerequisites/audio-routing.json")
                    .exists()
        );
    }
}
