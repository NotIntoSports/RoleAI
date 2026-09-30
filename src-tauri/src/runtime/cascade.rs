use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use crate::{
    config::{PublicConfig, RoleProfileConfig},
    database::Database,
    materials::hybrid::search_hybrid,
    providers::{
        CascadeError, CascadeStage, ChatMessage, ChatModel, EmbeddingError, EmbeddingProbe,
        ProviderEndpoint, SpeechToText, TextToSpeech, TurnStreamHooks,
    },
    runtime::{SessionRuntime, active_embedding, active_role_profile, active_voice_route},
};

const HISTORY_LIMIT: usize = 20;
const SNIPPET_CHARS: usize = 160;
const TIMEOUT_BACKOFF_BASE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTurn {
    pub user_text: String,
    pub assistant_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnCitation {
    pub material_id: String,
    pub chunk_id: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CascadeTurn {
    pub user_text: String,
    pub assistant_text: String,
    pub tts_pcm: Vec<u8>,
    pub citations: Vec<TurnCitation>,
    pub materials_used: bool,
    pub error_code: Option<&'static str>,
    /// 分阶段延迟时间线（相对本函数入口；落库进 turn_meta，见 finalize.rs）。
    pub timeline: crate::services::realtime_pump::TurnTimeline,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CascadeCredentials<'a> {
    pub asr: Option<&'a str>,
    pub llm: Option<&'a str>,
    pub tts: Option<&'a str>,
    pub embed: Option<&'a str>,
    pub e2e: Option<&'a str>,
}

pub struct CascadeTurnDeps<'a> {
    pub asr: &'a dyn SpeechToText,
    pub llm: &'a dyn ChatModel,
    pub tts: &'a dyn TextToSpeech,
    pub embed: &'a dyn EmbeddingProbe,
    pub database: &'a Database,
    pub runtime: &'a SessionRuntime,
    pub sleep: &'a dyn Fn(Duration),
}

pub struct CascadeTurnRequest<'a> {
    pub config: &'a PublicConfig,
    pub credentials: CascadeCredentials<'a>,
    pub pcm: Option<&'a [u8]>,
    pub sample_rate: u32,
    pub user_text: Option<&'a str>,
    pub history: &'a [HistoryTurn],
    /// 会话级滚动摘要（可为 None）；由 build_messages 并入系统提示。
    pub context_summary: Option<&'a str>,
}

pub fn run_cascade_turn(
    deps: &CascadeTurnDeps<'_>,
    request: CascadeTurnRequest<'_>,
    cancel: &AtomicBool,
    hooks: &TurnStreamHooks<'_>,
) -> Result<CascadeTurn, CascadeError> {
    let started = std::time::Instant::now();
    let mut timeline = crate::services::realtime_pump::TurnTimeline::default();
    let since = |slot: &mut Option<u64>| {
        *slot = Some(started.elapsed().as_millis() as u64);
    };
    cancelled(cancel)?;
    if !deps.runtime.can_answer() {
        return Err(CascadeError::AnswerBlocked);
    }

    let route = active_voice_route(request.config)
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Llm))?;
    let role = active_role_profile(request.config)
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Llm))?;
    let asr_endpoint = required_endpoint(
        request.config,
        route.asr_provider_id.as_deref(),
        CascadeStage::Asr,
    )?;
    let llm_endpoint = required_endpoint(
        request.config,
        route.llm_provider_id.as_deref(),
        CascadeStage::Llm,
    )?;
    let tts_endpoint = required_endpoint(
        request.config,
        route.tts_provider_id.as_deref(),
        CascadeStage::Tts,
    )?;
    let asr_model_id = route
        .asr_model_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Asr))?;
    let llm_model_id = route
        .llm_model_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Llm))?;
    let tts_model_id = route
        .tts_model_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(CascadeStage::Tts))?;
    let voice_id = route.voice_id.as_deref().filter(|id| !id.is_empty());

    cancelled(cancel)?;
    let user_text = resolve_user_text(deps, &request, &asr_endpoint, asr_model_id)?;
    since(&mut timeline.asr_done_ms);
    cancelled(cancel)?;
    let citations = retrieve(deps, &request, &user_text);
    since(&mut timeline.retrieval_done_ms);
    cancelled(cancel)?;
    let messages = build_messages(
        role,
        request.context_summary,
        request.history,
        &user_text,
        &citations,
    );
    let assistant_text = {
        // 首个增量快照即 LLM 首 token：包一层回调记录时间点后转发原钩子。
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
        let result = run_with_retry(deps.sleep, classify_cascade, || {
            deps.llm.complete_streaming(
                &llm_endpoint,
                request.credentials.llm,
                llm_model_id,
                &messages,
                &on_snapshot,
            )
        });
        timeline.llm_first_token_ms = first_token.get().copied().flatten();
        since(&mut timeline.llm_done_ms);
        result?
    };
    cancelled(cancel)?;
    let (tts_pcm, error_code) = match voice_id {
        Some(voice_id) => match run_with_retry(deps.sleep, classify_cascade, || {
            deps.tts.synthesize(
                &tts_endpoint,
                request.credentials.tts,
                tts_model_id,
                voice_id,
                &assistant_text,
            )
        }) {
            Ok(pcm) => {
                since(&mut timeline.tts_done_ms);
                (pcm, None)
            }
            Err(_) => (Vec::new(), Some("TTS_FAILED")),
        },
        None => (Vec::new(), Some("TTS_FAILED")),
    };

    Ok(CascadeTurn {
        user_text,
        assistant_text,
        tts_pcm,
        materials_used: !citations.is_empty(),
        citations,
        error_code,
        timeline,
    })
}

#[derive(Debug, Clone, Copy)]
enum RetryClass {
    TimeoutReset,
    RateLimited { retry_after_secs: Option<u64> },
    ServerError,
    None,
}

fn classify_cascade(error: &CascadeError) -> RetryClass {
    match error {
        CascadeError::Timeout(_) | CascadeError::ConnectionReset(_) => RetryClass::TimeoutReset,
        CascadeError::RateLimited {
            retry_after_secs, ..
        } => RetryClass::RateLimited {
            retry_after_secs: *retry_after_secs,
        },
        CascadeError::ServerError(_) => RetryClass::ServerError,
        _ => RetryClass::None,
    }
}

fn classify_embed(error: &EmbeddingError) -> RetryClass {
    match error {
        EmbeddingError::Timeout => RetryClass::TimeoutReset,
        _ => RetryClass::None,
    }
}

fn retry_delay(class: RetryClass, failure_count: u32) -> Option<Duration> {
    match class {
        RetryClass::TimeoutReset if failure_count < 3 => {
            Some(TIMEOUT_BACKOFF_BASE * 2u32.pow(failure_count.saturating_sub(1)))
        }
        RetryClass::RateLimited { retry_after_secs } if failure_count < 2 => {
            Some(Duration::from_secs(retry_after_secs.unwrap_or(1)))
        }
        RetryClass::ServerError if failure_count < 2 => Some(TIMEOUT_BACKOFF_BASE),
        _ => None,
    }
}

fn run_with_retry<T, E>(
    sleep: &dyn Fn(Duration),
    classify: impl Fn(&E) -> RetryClass,
    mut op: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut failure_count = 0_u32;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(error) => {
                failure_count += 1;
                match retry_delay(classify(&error), failure_count) {
                    Some(delay) => sleep(delay),
                    None => return Err(error),
                }
            }
        }
    }
}

fn cancelled(cancel: &AtomicBool) -> Result<(), CascadeError> {
    if cancel.load(Ordering::SeqCst) {
        Err(CascadeError::Cancelled)
    } else {
        Ok(())
    }
}

fn resolve_user_text(
    deps: &CascadeTurnDeps<'_>,
    request: &CascadeTurnRequest<'_>,
    asr_endpoint: &ProviderEndpoint,
    asr_model_id: &str,
) -> Result<String, CascadeError> {
    if let Some(pcm) = request.pcm.filter(|bytes| !bytes.is_empty()) {
        return run_with_retry(deps.sleep, classify_cascade, || {
            deps.asr.transcribe(
                asr_endpoint,
                request.credentials.asr,
                asr_model_id,
                pcm,
                request.sample_rate,
            )
        });
    }
    request
        .user_text
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .ok_or(CascadeError::ResponseInvalid(CascadeStage::Asr))
}

pub(crate) fn retrieve(
    deps: &CascadeTurnDeps<'_>,
    request: &CascadeTurnRequest<'_>,
    user_text: &str,
) -> Vec<TurnCitation> {
    let query_vector = active_embedding(request.config).and_then(|embedding| {
        let endpoint = crate::services::embedding_endpoint(&request.config.models, embedding)?;
        if endpoint.base_url.is_empty() {
            return None;
        }
        run_with_retry(deps.sleep, classify_embed, || {
            deps.embed.embed(
                &endpoint,
                request.credentials.embed,
                &embedding.model_id,
                embedding.dimensions,
                user_text,
            )
        })
        .ok()
    });
    match search_hybrid(deps.database, user_text, query_vector.as_deref(), None) {
        Ok(hits) => hits
            .into_iter()
            .map(|hit| TurnCitation {
                material_id: hit.material_id,
                chunk_id: hit.chunk_id,
                snippet: hit.snippet.chars().take(SNIPPET_CHARS).collect(),
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn provider_endpoint(config: &PublicConfig, provider_id: &str) -> Option<ProviderEndpoint> {
    config.models.providers.iter().find_map(|provider| {
        (provider.id == provider_id).then(|| ProviderEndpoint {
            provider_id: provider.id.clone(),
            base_url: provider.base_url.clone(),
        })
    })
}

fn required_endpoint(
    config: &PublicConfig,
    provider_id: Option<&str>,
    stage: CascadeStage,
) -> Result<ProviderEndpoint, CascadeError> {
    let provider_id = provider_id
        .filter(|id| !id.is_empty())
        .ok_or(CascadeError::EndpointInvalid(stage))?;
    provider_endpoint(config, provider_id)
        .filter(|endpoint| !endpoint.base_url.is_empty())
        .ok_or(CascadeError::EndpointInvalid(stage))
}

fn build_messages(
    role: &RoleProfileConfig,
    context_summary: Option<&str>,
    history: &[HistoryTurn],
    user_text: &str,
    citations: &[TurnCitation],
) -> Vec<ChatMessage> {
    let mut system = role.system_prompt.clone();
    if !role.style_instructions.is_empty() {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str(&role.style_instructions);
    }
    if let Some(summary) = context_summary.filter(|s| !s.trim().is_empty()) {
        if !system.is_empty() {
            system.push_str("\n\n");
        }
        system.push_str("此前对话摘要：");
        system.push_str(summary);
    }
    let mut messages = vec![ChatMessage {
        role: "system".into(),
        content: system,
    }];
    let history_start = history.len().saturating_sub(HISTORY_LIMIT);
    for turn in &history[history_start..] {
        messages.push(ChatMessage {
            role: "user".into(),
            content: turn.user_text.clone(),
        });
        messages.push(ChatMessage {
            role: "assistant".into(),
            content: turn.assistant_text.clone(),
        });
    }
    let mut user_content = user_text.to_owned();
    if !citations.is_empty() {
        user_content.push_str("\n\n");
        for citation in citations {
            user_content.push_str("- ");
            user_content.push_str(&citation.snippet);
            user_content.push('\n');
        }
    }
    messages.push(ChatMessage {
        role: "user".into(),
        content: user_content,
    });
    messages
}

#[cfg(test)]
mod tests;
