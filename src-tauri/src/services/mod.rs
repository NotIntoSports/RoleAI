mod echo_guard;
mod embeddings;
mod ids;
mod materials;
mod providers;
mod realtime_pump;
mod roles;
mod sessions;
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
