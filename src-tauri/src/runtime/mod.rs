//! 会话运行时编排：cascade 组装一轮问答依赖、commands 定义泵控制命令、
//! preflight 启动前自检、snapshot 会话快照构建、state 全局运行状态（阶段/代次/修订号）。

pub mod cascade;
pub mod commands;
pub mod preflight;
pub mod snapshot;
pub mod state;

pub use cascade::{
    CascadeCredentials, CascadeTurn, CascadeTurnDeps, CascadeTurnRequest, HistoryTurn,
    TurnCitation, run_cascade_turn,
};
pub use commands::{
    AgentCommand, AgentCommandAction, AgentCommandError, AgentCommandOutcome,
    assert_expected_revision, command_requirements, execute_agent_command, parse_agent_command,
    result_packet,
};
pub use preflight::{PreflightIssue, preflight};
pub use snapshot::{SessionRuntimeSnapshot, build_snapshot};
pub use state::{AgentMode, RuntimeError, RuntimeEvent, SessionPhase, SessionRuntime};

use crate::config::{EmbeddingConfig, PublicConfig, RoleProfileConfig, VoiceRouteConfig};

pub(crate) fn active_voice_route(config: &PublicConfig) -> Option<&VoiceRouteConfig> {
    let id = config.speech.active_voice_route_id.as_deref()?;
    config
        .speech
        .voice_routes
        .iter()
        .find(|route| route.id == id && route.active)
}

pub(crate) fn active_role_profile(config: &PublicConfig) -> Option<&RoleProfileConfig> {
    let id = config.active_role_profile_id.as_deref()?;
    config
        .role_profiles
        .iter()
        .find(|profile| profile.id == id && profile.active)
}

pub(crate) fn active_embedding(config: &PublicConfig) -> Option<&EmbeddingConfig> {
    let id = config.knowledge.active_embedding_config_id.as_deref()?;
    config
        .knowledge
        .embedding_configs
        .iter()
        .find(|embedding| embedding.id == id && embedding.active)
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::config::{
        AppConfigV1, EmbeddingConfig, EmbeddingDistance, KnowledgeConfig, ModelConfig,
        ProviderConfig, PublicConfig, RoleProfileConfig, SecretSlot, SpeechConfig,
        VoiceRouteConfig, VoiceRouteMode, public_view,
    };

    pub const PROMPT_MARKERS: &[&str] = &[
        "UNIQUE_PROMPT_BODY_DO_NOT_SNAPSHOT",
        "UNIQUE_OPENING_DO_NOT_SNAPSHOT",
        "UNIQUE_STYLE_DO_NOT_SNAPSHOT",
    ];

    pub fn empty_public_config() -> PublicConfig {
        public_view(&AppConfigV1::default())
    }

    pub fn ready_public_config() -> PublicConfig {
        public_view(&AppConfigV1 {
            models: ModelConfig {
                providers: vec![
                    provider("asr-1", "https://asr.example.test/v1"),
                    provider("llm-1", "https://llm.example.test/v1"),
                    provider("tts-1", "https://tts.example.test/v1"),
                    provider("emb-1", "https://emb.example.test/v1"),
                ],
                active_provider_id: None,
            },
            speech: SpeechConfig {
                voice_routes: vec![VoiceRouteConfig {
                    id: "route-1".into(),
                    name: "Default".into(),
                    mode: VoiceRouteMode::Cascaded,
                    asr_provider_id: Some("asr-1".into()),
                    asr_model_id: Some("whisper".into()),
                    llm_provider_id: Some("llm-1".into()),
                    llm_model_id: Some("gpt".into()),
                    tts_provider_id: Some("tts-1".into()),
                    tts_model_id: Some("tts-model".into()),
                    voice_id: Some("alloy".into()),
                    e2e_provider_id: None,
                    e2e_model_id: None,
                    active: true,
                    ready: true,
                    status: Some("ready".into()),
                    config_version: 1,
                }],
                active_voice_route_id: Some("route-1".into()),
            },
            knowledge: KnowledgeConfig {
                embedding_configs: vec![EmbeddingConfig {
                    id: "emb-space".into(),
                    provider_id: "emb-1".into(),
                    base_url: None,
                    credential: None,
                    model_id: "bge".into(),
                    dimensions: 1024,
                    distance: EmbeddingDistance::Cosine,
                    normalized: true,
                    active: true,
                    ready: true,
                    status: Some("ready".into()),
                    config_version: 1,
                }],
                active_embedding_config_id: Some("emb-space".into()),
            },
            role_profiles: vec![RoleProfileConfig {
                id: "role-1".into(),
                name: "Interviewer".into(),
                system_prompt: PROMPT_MARKERS[0].into(),
                opening_message: PROMPT_MARKERS[1].into(),
                style_instructions: PROMPT_MARKERS[2].into(),
                scenario: None,
                active: true,
                config_version: 1,
            }],
            active_role_profile_id: Some("role-1".into()),
            ..AppConfigV1::default()
        })
    }

    pub fn ready_e2e_public_config() -> PublicConfig {
        public_view(&AppConfigV1 {
            models: ModelConfig {
                providers: vec![
                    provider("e2e-1", "https://e2e.example.test/v1"),
                    provider("emb-1", "https://emb.example.test/v1"),
                ],
                active_provider_id: None,
            },
            speech: SpeechConfig {
                voice_routes: vec![VoiceRouteConfig {
                    id: "route-1".into(),
                    name: "Realtime".into(),
                    mode: VoiceRouteMode::E2e,
                    asr_provider_id: None,
                    asr_model_id: None,
                    llm_provider_id: None,
                    llm_model_id: None,
                    tts_provider_id: None,
                    tts_model_id: None,
                    voice_id: Some("alloy".into()),
                    e2e_provider_id: Some("e2e-1".into()),
                    e2e_model_id: Some("gpt-realtime".into()),
                    active: true,
                    ready: true,
                    status: Some("ready".into()),
                    config_version: 1,
                }],
                active_voice_route_id: Some("route-1".into()),
            },
            knowledge: KnowledgeConfig {
                embedding_configs: vec![EmbeddingConfig {
                    id: "emb-space".into(),
                    provider_id: "emb-1".into(),
                    base_url: None,
                    credential: None,
                    model_id: "bge".into(),
                    dimensions: 1024,
                    distance: EmbeddingDistance::Cosine,
                    normalized: true,
                    active: true,
                    ready: true,
                    status: Some("ready".into()),
                    config_version: 1,
                }],
                active_embedding_config_id: Some("emb-space".into()),
            },
            role_profiles: vec![RoleProfileConfig {
                id: "role-1".into(),
                name: "Interviewer".into(),
                system_prompt: PROMPT_MARKERS[0].into(),
                opening_message: PROMPT_MARKERS[1].into(),
                style_instructions: PROMPT_MARKERS[2].into(),
                scenario: None,
                active: true,
                config_version: 1,
            }],
            active_role_profile_id: Some("role-1".into()),
            ..AppConfigV1::default()
        })
    }

    fn provider(id: &str, base_url: &str) -> ProviderConfig {
        ProviderConfig {
            web_capability: None,
            id: id.into(),
            name: None,
            base_url: base_url.into(),
            credential: Some(SecretSlot {
                reference: format!("providers/{id}/api-key"),
                configured: true,
            }),
        }
    }
}
