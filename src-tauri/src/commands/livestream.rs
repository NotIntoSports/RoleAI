//! livestream 域命令：数字人直播草稿、生成、控制、插入提问与播放推进。
//! 纯搬移自 commands.rs，不含行为变更。
use super::*;

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
    let mut cancel_slot = state
        .livestream_playback_cancel
        .lock()
        .map_err(|_| livestream_busy())?;
    let mut script_slot = match state.livestream.lock() {
        Ok(slot) => slot,
        Err(_) => return Err(livestream_busy()),
    };
    let mut stage_slot = match state.livestream_stage.lock() {
        Ok(slot) => slot,
        Err(_) => return Err(livestream_busy()),
    };
    let (Some(script), Some(stage)) = (script_slot.as_mut(), stage_slot.as_mut()) else {
        return Err(PublicError::new(
            "LIVESTREAM_NOT_FOUND",
            "尚未创建直播讲稿",
            false,
        ));
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
    let speech_text = matches!(action, "start" | "resume" | "previous" | "next" | "replay")
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

pub(super) fn advance_livestream_after_success(
    state: &AppState,
    token: &Arc<AtomicBool>,
) -> Option<String> {
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

pub(super) fn set_livestream_output_state(
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

pub(super) fn save_livestream_runtime(
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
        if slot.is_none()
            && let Ok(snapshot) = &candidate
        {
            *slot = Some(snapshot.clone());
        }
        if let Some(snapshot) = slot.as_ref() {
            return Ok(snapshot.clone());
        }
    }
    candidate
}

pub(super) fn resolve_livestream_voice(
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
