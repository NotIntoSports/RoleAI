//! Tauri 命令层公共设施：`CommandResult` 信封、错误码映射、事件发射与去抖。
//! 具体命令按域拆在子模块（config/sessions/materials/voice/obs/system/livestream），
//! 与前端唯一入口 src/api/commands.ts 一一对应；注册表见 lib.rs invoke_handler。

use std::sync::{
    Arc, TryLockError,
    atomic::{AtomicBool, Ordering},
};

use tauri::{AppHandle, Emitter, Manager, State};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::{
    app_state::AppState,
    config::{
        EmbeddingConfig, ProviderConfig, PublicConfig, RoleProfileConfig, VoiceRouteConfig,
        diagnostic_view, public_view,
    },
    contracts::{
        AgentCommandInput, AgentCommandResult, AudioLevelEvent, CommandResult,
        DiagnosticsExportResult, DiagnosticsLatencySummary, FoundationStatus,
        LegacyMigrationStatus, LegacySessionImport, LivestreamDraftInput, LivestreamGenerateInput,
        LivestreamRuntime, MicPcmAcceptance, RuntimeStatus, SessionAudioEvent, SessionCitationView,
        SessionDetail, SessionExportResult, SessionPlaybackControlEvent, SessionReplyEvent,
        SessionStartResult, SessionSummary, SessionTranscriptEvent, SessionTurnView, StartupState,
        VideoFrameAcceptance,
    },
    error::PublicError,
    providers::{
        ChatMessage, ChatModel, OpenAiCompatibleCascade, OpenAiCompatibleEmbeddingProbe,
        OpenAiCompatibleProbe, OpenAiCompatibleRealtime, ProviderEndpoint, StandardRouteProbe,
        TextToSpeech, TurnStreamHooks, VoiceCloneProbe,
    },
    runtime::{
        AgentMode, CascadeCredentials, active_embedding, active_voice_route, parse_agent_command,
        preflight,
    },
    services::{
        EmbeddingConfigSaveInput, EmbeddingService, EmbeddingServiceError, EmbeddingTestResult,
        MaterialIndexResult, MaterialSearchHit, MaterialService, MaterialServiceError,
        MaterialSummary, ModelDiscoveryResult, ProviderSaveInput, ProviderService,
        ProviderServiceError, ProviderTestResult, RoleProfileCopyInput, RoleProfileSaveInput,
        RoleProfileService, RoleProfileServiceError, SessionProbes, SessionServiceError,
        SessionStartOutcome, VoiceReferenceAudioSaveInput, VoiceReferenceCloneResult,
        VoiceReferenceSaveInput, VoiceReferenceService, VoiceReferenceServiceError,
        VoiceReferenceSummary, VoiceReferenceUpdateInput, VoiceRouteSaveInput, VoiceRouteService,
        VoiceRouteServiceError, VoiceRouteTestResult,
    },
    sessions::{SessionExportError, SessionExportFormat, SessionStore, export_session},
};

fn service_error<T: ts_rs::TS>(code: &str, message: &str) -> CommandResult<T> {
    CommandResult::Err {
        error: PublicError::new(code, message, false),
    }
}

fn embedding_probe<T: ts_rs::TS>() -> Result<OpenAiCompatibleEmbeddingProbe, CommandResult<T>> {
    OpenAiCompatibleEmbeddingProbe::new()
        .map_err(|error| service_error(error.code(), "Embedding client is unavailable"))
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

mod config;
pub use self::config::*;
mod materials;
pub use self::materials::*;
mod sessions;
pub use self::sessions::*;
mod livestream;
pub use self::livestream::*;
mod obs;
pub use self::obs::*;
mod voice;
pub use self::voice::*;
mod system;
pub use self::system::*;

#[cfg(test)]
#[path = "../commands_ipc_tests.rs"]
mod ipc_tests;

#[cfg(test)]
mod tests;
