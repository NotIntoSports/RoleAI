mod embeddings;
mod ids;
mod livekit;
mod materials;
mod providers;
mod roles;
mod sessions;
pub use sessions::MeetingCapture;
mod voice_references;
mod voice_routes;

pub use embeddings::{
    EmbeddingConfigSaveInput, EmbeddingService, EmbeddingServiceError, EmbeddingTestResult,
    embedding_credential_slot, embedding_endpoint, embedding_space_provider_id,
};
pub use livekit::{
    LiveKitJoinToken, LiveKitSettingsError, LiveKitSettingsSaveInput, LiveKitSettingsService,
    LiveKitTestResult,
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
pub use crate::providers::VoiceCloneError;
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
