//! 高层服务门面：sessions（会话服务）、realtime_pump（实时音频泵）、
//! echo_guard（播报回声的文本过滤）、materials/providers 组装、ids（会话/轮次 id）。

mod echo_guard;
mod embeddings;
mod ids;
mod materials;
mod providers;
mod realtime_pump;
mod roles;
pub(crate) mod sessions;

/// 基准支撑（C32）：把 crate 内部热路径以最小面暴露给 `src-tauri/benches/`。
/// 仅 benches/*.rs 允许使用；不为基准把内部 API 公开成正式接口（手册 C31）。
#[doc(hidden)]
pub mod bench_support {
    pub use super::echo_guard::{
        gate_drain_deadline_from_bytes, gate_timers_expired, is_echo, normalize,
    };
}

pub(crate) use realtime_pump::{PlaybackControl, PumpLive, RealtimePlaybackMode};
pub use sessions::MeetingCapture;
pub(crate) use sessions::{RealtimePumpDeps, active_session_role_scenario, e2e_instructions};
mod voice_references;
mod voice_routes;

pub use crate::providers::VoiceCloneError;
pub use embeddings::{
    EmbeddingConfigSaveInput, EmbeddingService, EmbeddingServiceError, EmbeddingTestResult,
    embedding_credential_slot, embedding_endpoint, embedding_space_provider_id,
};
pub use materials::{
    EmbeddingSpace, MaterialIndexResult, MaterialSearchHit, MaterialService, MaterialServiceError,
    MaterialSummary,
};
pub use providers::{
    DiscoveredModelDto, ModelDiscoveryResult, ProviderDependency, ProviderSaveInput,
    ProviderService, ProviderServiceError, ProviderTestResult,
};
pub use roles::{
    RoleProfileCopyInput, RoleProfileSaveInput, RoleProfileService, RoleProfileServiceError,
};
pub use sessions::{
    SessionControl, SessionProbes, SessionService, SessionServiceError, SessionStartOutcome,
};
pub use voice_references::{
    VoiceCloneGateway, VoiceReferenceAudioSaveInput, VoiceReferenceCloneResult,
    VoiceReferenceSaveInput, VoiceReferenceService, VoiceReferenceServiceError,
    VoiceReferenceSummary, VoiceReferenceUpdateInput,
};
pub use voice_routes::{
    VoiceRouteSaveInput, VoiceRouteService, VoiceRouteServiceError, VoiceRouteTestResult,
};

#[cfg(test)]
mod tests;
