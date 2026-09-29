//! 应用配置模型与持久化：`ApplicationConfig`（v1 schema）定义全部可序列化配置，
//! locator 按优先级定位配置文件，store 负责读写、校验与 schema 迁移，
//! presets 是内置角色与路由预设。

mod locator;
pub mod presets;
mod store;

#[cfg(test)]
mod tests;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use ts_rs::TS;

pub use locator::{ConfigDirs, ConfigLocation, ConfigSource, locate_config};
pub use store::{ConfigLoadOutcome, ConfigStore};

const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct ConfigError {
    code: &'static str,
    message: String,
}

impl ConfigError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct SecretSlot {
    pub reference: String,
    #[serde(default)]
    pub configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ApplicationConfig {
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ProviderConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub web_capability: Option<crate::providers::web_search::WebCapability>,
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub base_url: String,
    #[serde(default)]
    pub credential: Option<SecretSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct ModelConfig {
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub active_provider_id: Option<String>,
}

/// Voice pipeline mode. Cascaded runs ASR -> retrieval -> LLM -> TTS; E2e runs a
/// single Realtime round-trip. Exactly one route is active at a time (design §12.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub enum VoiceRouteMode {
    #[default]
    #[serde(rename = "cascaded")]
    #[ts(rename = "cascaded")]
    Cascaded,
    #[serde(rename = "e2e")]
    #[ts(rename = "e2e")]
    E2e,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct VoiceRouteConfig {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mode: VoiceRouteMode,
    #[serde(default)]
    pub asr_provider_id: Option<String>,
    #[serde(default)]
    pub asr_model_id: Option<String>,
    #[serde(default)]
    pub llm_provider_id: Option<String>,
    #[serde(default)]
    pub llm_model_id: Option<String>,
    #[serde(default)]
    pub tts_provider_id: Option<String>,
    #[serde(default)]
    pub tts_model_id: Option<String>,
    #[serde(default)]
    pub voice_id: Option<String>,
    #[serde(default)]
    pub e2e_provider_id: Option<String>,
    #[serde(default)]
    pub e2e_model_id: Option<String>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub config_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct SpeechConfig {
    #[serde(default)]
    pub voice_routes: Vec<VoiceRouteConfig>,
    #[serde(default)]
    pub active_voice_route_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct KnowledgeConfig {
    #[serde(default)]
    pub embedding_configs: Vec<EmbeddingConfig>,
    #[serde(default)]
    pub active_embedding_config_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct StorageConfig {
    #[serde(default)]
    pub export_directory: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub enum EmbeddingDistance {
    #[default]
    #[serde(rename = "cosine")]
    #[ts(rename = "cosine")]
    Cosine,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub enum RoleScenario {
    Interviewer,
    Hr,
    Candidate,
    MeetingAssistant,
    LivestreamPresenter,
}

impl RoleScenario {
    pub fn from_preset_id(id: &str) -> Option<Self> {
        match id {
            "preset-interviewer" => Some(Self::Interviewer),
            "preset-hr" => Some(Self::Hr),
            "preset-candidate" => Some(Self::Candidate),
            "preset-meeting" => Some(Self::MeetingAssistant),
            "preset-presenter" => Some(Self::LivestreamPresenter),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct RoleProfileConfig {
    pub id: String,
    pub name: String,
    pub system_prompt: String,
    pub opening_message: String,
    pub style_instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scenario: Option<RoleScenario>,
    pub active: bool,
    pub config_version: u32,
}

/// Backward-compatible Rust name for callers compiled against the original
/// configuration module. JSON input compatibility is handled separately.
pub type RoleProfile = RoleProfileConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct EmbeddingConfig {
    pub id: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub credential: Option<SecretSlot>,
    pub model_id: String,
    pub dimensions: u32,
    pub distance: EmbeddingDistance,
    pub normalized: bool,
    pub active: bool,
    pub ready: bool,
    pub status: Option<String>,
    pub config_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct DiagnosticsConfig {
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u16,
}

fn default_log_retention_days() -> u16 {
    14
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            log_retention_days: default_log_retention_days(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConfigV1 {
    pub config_version: u32,
    #[serde(default)]
    pub application: ApplicationConfig,
    #[serde(default)]
    pub models: ModelConfig,
    #[serde(default)]
    pub speech: SpeechConfig,
    #[serde(default)]
    pub knowledge: KnowledgeConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub role_profiles: Vec<RoleProfileConfig>,
    #[serde(default)]
    pub active_role_profile_id: Option<String>,
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppConfigV1Input {
    config_version: u32,
    #[serde(default)]
    application: ApplicationConfig,
    #[serde(default)]
    models: ModelConfig,
    #[serde(default)]
    speech: SpeechConfig,
    /// Deprecated LiveKit transport settings. Accepted so existing config
    /// files keep parsing, then dropped: sessions always run local direct.
    #[serde(default)]
    #[allow(dead_code)]
    transport: Value,
    #[serde(default)]
    knowledge: KnowledgeConfigInput,
    #[serde(default)]
    storage: StorageConfig,
    #[serde(default)]
    role_profiles: Vec<RoleProfileInput>,
    #[serde(default)]
    active_role_profile_id: Option<String>,
    #[serde(default)]
    diagnostics: DiagnosticsConfig,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RoleProfileInput {
    Canonical(RoleProfileConfig),
    Legacy(LegacyRoleProfile),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyRoleProfile {
    id: String,
    instructions: String,
}

impl From<RoleProfileInput> for RoleProfileConfig {
    fn from(input: RoleProfileInput) -> Self {
        match input {
            RoleProfileInput::Canonical(profile) => profile,
            RoleProfileInput::Legacy(profile) => Self {
                name: profile.id.clone(),
                id: migrate_legacy_role_id(&profile.id),
                system_prompt: profile.instructions,
                opening_message: String::new(),
                style_instructions: String::new(),
                scenario: None,
                active: false,
                config_version: 0,
            },
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KnowledgeConfigInput {
    #[serde(default)]
    embedding_configs: Option<Vec<EmbeddingConfig>>,
    #[serde(default)]
    active_embedding_config_id: Option<String>,
    #[serde(default)]
    embedding_provider_id: Option<String>,
}

impl<'de> Deserialize<'de> for AppConfigV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let input = AppConfigV1Input::deserialize(deserializer)?;
        if input.knowledge.embedding_configs.is_some()
            && input.knowledge.embedding_provider_id.is_some()
        {
            return Err(serde::de::Error::custom(
                "embeddingConfigs and legacy embeddingProviderId cannot both be present",
            ));
        }

        Ok(Self {
            config_version: input.config_version,
            application: input.application,
            models: input.models,
            speech: input.speech,
            knowledge: KnowledgeConfig {
                embedding_configs: input.knowledge.embedding_configs.unwrap_or_default(),
                active_embedding_config_id: input.knowledge.active_embedding_config_id,
            },
            storage: input.storage,
            role_profiles: input
                .role_profiles
                .into_iter()
                .map(RoleProfileConfig::from)
                .collect(),
            active_role_profile_id: input.active_role_profile_id,
            diagnostics: input.diagnostics,
        })
    }
}

/// Redacted, IPC-safe projection of the local configuration (design §7.3/§8.2).
///
/// Providers only ever carry a [`SecretSlot`] (`reference` + `configured`); the
/// secret *value* lives exclusively in the Windows Credential Manager and is
/// never present here, in JSON, SQLite, logs, or the frontend. This is the DTO
/// returned by the read-only `config_get_public` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PublicConfig {
    pub config_version: u32,
    pub application: ApplicationConfig,
    pub models: ModelConfig,
    pub speech: SpeechConfig,
    pub knowledge: KnowledgeConfig,
    pub storage: StorageConfig,
    pub role_profiles: Vec<RoleProfileConfig>,
    pub active_role_profile_id: Option<String>,
    pub diagnostics: DiagnosticsConfig,
}

/// Project the internal [`AppConfigV1`] onto its redacted public contract.
///
/// Read-only: clones the non-secret configuration sections. No secret value is
/// ever read or copied — credentials stay as [`SecretSlot`] references.
pub fn public_view(config: &AppConfigV1) -> PublicConfig {
    PublicConfig {
        config_version: config.config_version,
        application: config.application.clone(),
        models: config.models.clone(),
        speech: config.speech.clone(),
        knowledge: config.knowledge.clone(),
        storage: config.storage.clone(),
        role_profiles: config.role_profiles.clone(),
        active_role_profile_id: config.active_role_profile_id.clone(),
        diagnostics: config.diagnostics.clone(),
    }
}

/// Diagnostic-safe projection: same redacted public tree, but role prompt bodies
/// are omitted so they cannot enter exported reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRoleProfile {
    pub id: String,
    pub name: String,
    pub active: bool,
    pub config_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticPublicConfig {
    pub config_version: u32,
    pub application: ApplicationConfig,
    pub models: ModelConfig,
    pub speech: SpeechConfig,
    pub knowledge: KnowledgeConfig,
    pub storage: StorageConfig,
    pub role_profiles: Vec<DiagnosticRoleProfile>,
    pub active_role_profile_id: Option<String>,
    pub diagnostics: DiagnosticsConfig,
}

pub fn diagnostic_view(config: &AppConfigV1) -> DiagnosticPublicConfig {
    DiagnosticPublicConfig {
        config_version: config.config_version,
        application: config.application.clone(),
        models: config.models.clone(),
        speech: config.speech.clone(),
        knowledge: config.knowledge.clone(),
        storage: config.storage.clone(),
        role_profiles: config
            .role_profiles
            .iter()
            .map(|profile| DiagnosticRoleProfile {
                id: profile.id.clone(),
                name: profile.name.clone(),
                active: profile.active,
                config_version: profile.config_version,
            })
            .collect(),
        active_role_profile_id: config.active_role_profile_id.clone(),
        diagnostics: config.diagnostics.clone(),
    }
}

impl Default for AppConfigV1 {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            application: ApplicationConfig::default(),
            models: ModelConfig::default(),
            speech: SpeechConfig::default(),
            knowledge: KnowledgeConfig::default(),
            storage: StorageConfig::default(),
            role_profiles: Vec::new(),
            active_role_profile_id: None,
            diagnostics: DiagnosticsConfig::default(),
        }
    }
}

impl AppConfigV1 {
    pub fn from_json(json: &str) -> Result<Self, ConfigError> {
        let value: Value = serde_json::from_str(json)
            .map_err(|error| ConfigError::new("CONFIG_INVALID", error.to_string()))?;
        reject_inline_secrets(&value)?;
        match value.get("configVersion").and_then(Value::as_u64) {
            Some(version) if version == u64::from(CONFIG_VERSION) => {}
            Some(_) => {
                return Err(ConfigError::new(
                    "CONFIG_VERSION_UNSUPPORTED",
                    "Unsupported configVersion",
                ));
            }
            None => {
                return Err(ConfigError::new(
                    "CONFIG_INVALID",
                    "configVersion is required",
                ));
            }
        }
        let config: Self = serde_json::from_value(value)
            .map_err(|error| ConfigError::new("CONFIG_INVALID", error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.config_version != CONFIG_VERSION {
            return Err(ConfigError::new(
                "CONFIG_VERSION_UNSUPPORTED",
                "Unsupported configVersion",
            ));
        }
        ensure_unique(self.models.providers.iter().map(|item| item.id.as_str()))?;
        ensure_unique(self.speech.voice_routes.iter().map(|item| item.id.as_str()))?;
        ensure_unique(self.role_profiles.iter().map(|item| item.id.as_str()))?;
        ensure_unique(
            self.knowledge
                .embedding_configs
                .iter()
                .map(|item| item.id.as_str()),
        )?;
        for provider in &self.models.providers {
            validate_url(&provider.base_url, &["http", "https"])?;
            if let Some(credential) = &provider.credential
                && credential.reference != format!("providers/{}/api-key", provider.id)
            {
                return Err(ConfigError::new(
                    "CONFIG_SECRET_REFERENCE_INVALID",
                    "Provider credential reference is not canonical",
                ));
            }
        }
        for profile in &self.role_profiles {
            validate_stable_id(&profile.id)?;
            // Version zero is the explicit compatibility marker for an inactive
            // role migrated from the original `{ id, instructions }` shape.
            // Preserve old instructions losslessly; activation/edit services
            // must create a positive-version profile within the current limit.
            let migrated_legacy_profile = profile.config_version == 0 && !profile.active;
            if profile.name.trim().is_empty()
                || (!migrated_legacy_profile && profile.system_prompt.len() > 32 * 1024)
                || profile.opening_message.len() > 4 * 1024
                || profile.style_instructions.len() > 8 * 1024
            {
                return Err(ConfigError::new(
                    "CONFIG_FIELD_INVALID",
                    "Role profile fields exceed their allowed limits",
                ));
            }
        }
        let active_profiles = self
            .role_profiles
            .iter()
            .filter(|profile| profile.active)
            .collect::<Vec<_>>();
        match self.active_role_profile_id.as_deref() {
            Some(active_id)
                if active_profiles.len() == 1
                    && active_profiles[0].id == active_id
                    && active_profiles[0].config_version > 0 => {}
            None if active_profiles.is_empty() => {}
            Some(active_id)
                if !self
                    .role_profiles
                    .iter()
                    .any(|profile| profile.id == active_id) =>
            {
                return Err(ConfigError::new(
                    "CONFIG_REFERENCE_MISSING",
                    "Active role profile does not exist",
                ));
            }
            _ => {
                return Err(ConfigError::new(
                    "CONFIG_ACTIVE_ROLE_INVALID",
                    "Active role profile flags are inconsistent",
                ));
            }
        }
        if let Some(active) = &self.models.active_provider_id
            && !self
                .models
                .providers
                .iter()
                .any(|provider| &provider.id == active)
        {
            return Err(ConfigError::new(
                "CONFIG_REFERENCE_MISSING",
                "Active provider does not exist",
            ));
        }
        if let Some(active) = &self.speech.active_voice_route_id
            && !self
                .speech
                .voice_routes
                .iter()
                .any(|route| &route.id == active)
        {
            return Err(ConfigError::new(
                "CONFIG_REFERENCE_MISSING",
                "Active voice route does not exist",
            ));
        }
        let active_routes = self
            .speech
            .voice_routes
            .iter()
            .filter(|route| route.active)
            .collect::<Vec<_>>();
        match self.speech.active_voice_route_id.as_deref() {
            Some(active_id)
                if active_routes.len() == 1
                    && active_routes[0].id == active_id
                    && active_routes[0].ready
                    && active_routes[0].status.as_deref() == Some("ready")
                    && active_routes[0].config_version > 0 => {}
            None if active_routes.is_empty() => {}
            _ => {
                return Err(ConfigError::new(
                    "CONFIG_ACTIVE_ROUTE_INVALID",
                    "Active voice route flags are inconsistent",
                ));
            }
        }
        for route in &self.speech.voice_routes {
            for provider_id in [
                route.asr_provider_id.as_ref(),
                route.llm_provider_id.as_ref(),
                route.tts_provider_id.as_ref(),
                route.e2e_provider_id.as_ref(),
            ] {
                if let Some(provider_id) = provider_id
                    && !self
                        .models
                        .providers
                        .iter()
                        .any(|provider| &provider.id == provider_id)
                {
                    return Err(ConfigError::new(
                        "CONFIG_REFERENCE_MISSING",
                        "Voice route provider does not exist",
                    ));
                }
            }
        }
        for embedding in &self.knowledge.embedding_configs {
            validate_stable_id(&embedding.id)?;
            if embedding.model_id.trim().is_empty() || !(1..=65_536).contains(&embedding.dimensions)
            {
                return Err(ConfigError::new(
                    "CONFIG_FIELD_INVALID",
                    "Embedding fields are invalid",
                ));
            }
            let provider_id = embedding.provider_id.trim();
            let base_url = embedding
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            match (provider_id.is_empty(), base_url) {
                (false, None) => {
                    if !self
                        .models
                        .providers
                        .iter()
                        .any(|provider| provider.id == provider_id)
                    {
                        return Err(ConfigError::new(
                            "CONFIG_REFERENCE_MISSING",
                            "Embedding provider does not exist",
                        ));
                    }
                }
                (true, Some(url)) => validate_url(url, &["http", "https"])?,
                _ => {
                    return Err(ConfigError::new(
                        "CONFIG_FIELD_INVALID",
                        "Embedding must use a provider or a custom URL",
                    ));
                }
            }
        }
        let active_embeddings = self
            .knowledge
            .embedding_configs
            .iter()
            .filter(|embedding| embedding.active)
            .collect::<Vec<_>>();
        match self.knowledge.active_embedding_config_id.as_deref() {
            Some(active_id)
                if active_embeddings.len() == 1
                    && active_embeddings[0].id == active_id
                    && active_embeddings[0].ready
                    && active_embeddings[0].status.as_deref() == Some("ready")
                    && active_embeddings[0].config_version > 0 => {}
            None if active_embeddings.is_empty() => {}
            Some(active_id)
                if !self
                    .knowledge
                    .embedding_configs
                    .iter()
                    .any(|embedding| embedding.id == active_id) =>
            {
                return Err(ConfigError::new(
                    "CONFIG_REFERENCE_MISSING",
                    "Active embedding configuration does not exist",
                ));
            }
            _ => {
                return Err(ConfigError::new(
                    "CONFIG_ACTIVE_EMBEDDING_INVALID",
                    "Active embedding configuration flags are inconsistent",
                ));
            }
        }
        if !(1..=365).contains(&self.diagnostics.log_retention_days) {
            return Err(ConfigError::new(
                "CONFIG_FIELD_INVALID",
                "logRetentionDays must be between 1 and 365",
            ));
        }
        Ok(())
    }
}

fn ensure_unique<'a>(ids: impl Iterator<Item = &'a str>) -> Result<(), ConfigError> {
    let mut seen = HashSet::new();
    for id in ids {
        if id.trim().is_empty() {
            return Err(ConfigError::new(
                "CONFIG_FIELD_INVALID",
                "IDs cannot be empty",
            ));
        }
        if !seen.insert(id) {
            return Err(ConfigError::new(
                "CONFIG_DUPLICATE_ID",
                "IDs must be unique",
            ));
        }
    }
    Ok(())
}

fn validate_stable_id(id: &str) -> Result<(), ConfigError> {
    if !is_stable_id(id) {
        return Err(ConfigError::new(
            "CONFIG_FIELD_INVALID",
            "ID must use lowercase ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(())
}

pub(crate) fn is_stable_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn migrate_legacy_role_id(id: &str) -> String {
    if is_stable_id(id) {
        return id.to_owned();
    }

    // FNV-1a gives invalid legacy IDs a deterministic, non-secret, canonical
    // identifier without truncating or exposing arbitrary legacy text in refs.
    let hash = id
        .as_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    format!("legacy-{hash:016x}")
}

fn validate_url(raw: &str, schemes: &[&str]) -> Result<(), ConfigError> {
    let url = tauri::Url::parse(raw)
        .map_err(|_| ConfigError::new("CONFIG_URL_INVALID", "URL is invalid"))?;
    if !schemes.contains(&url.scheme())
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConfigError::new(
            "CONFIG_URL_INVALID",
            "URL scheme or authority is invalid",
        ));
    }
    Ok(())
}

fn reject_inline_secrets(value: &Value) -> Result<(), ConfigError> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let normalized = key.to_ascii_lowercase().replace(['_', '-'], "");
                let canonical_secret_slot = matches!(normalized.as_str(), "apikey" | "apisecret")
                    && (child.is_null() || is_secret_slot_value(child));
                let forbidden = normalized.contains("password")
                    || (normalized.contains("apikey") && !canonical_secret_slot)
                    || normalized == "token"
                    || normalized.ends_with("token")
                    || (normalized.contains("secret")
                        && normalized != "secretref"
                        && !canonical_secret_slot);
                if forbidden {
                    return Err(ConfigError::new(
                        "CONFIG_SECRET_INLINE_FORBIDDEN",
                        "Inline secrets are forbidden",
                    ));
                }
                reject_inline_secrets(child)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                reject_inline_secrets(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_secret_slot_value(value: &Value) -> bool {
    let Some(slot) = value.as_object() else {
        return false;
    };
    slot.len() == 2
        && slot.get("reference").is_some_and(Value::is_string)
        && slot.get("configured").is_some_and(Value::is_boolean)
}

#[derive(Debug, Clone, Default)]
pub struct ConfigPatch {
    pub diagnostics: Option<DiagnosticsPatch>,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticsPatch {
    pub log_retention_days: Option<u16>,
}
