use serde::Deserialize;
#[cfg(test)]
use std::path::Path;

#[cfg(test)]
use crate::config::{
    ApplicationConfig, DiagnosticsConfig, EmbeddingConfig, EmbeddingDistance, KnowledgeConfig,
    ModelConfig, ProviderConfig, PublicConfig, RoleProfileConfig, RoleScenario, SecretSlot,
    SpeechConfig, StorageConfig, VoiceRouteConfig, VoiceRouteMode,
};
#[cfg(test)]
use crate::services::{
    DiscoveredModelDto, EmbeddingConfigSaveInput, EmbeddingTestResult, MaterialIndexResult,
    MaterialSearchHit, MaterialSummary, ModelDiscoveryResult, ProviderSaveInput,
    ProviderTestResult, RoleProfileCopyInput, RoleProfileSaveInput, VoiceRouteSaveInput,
    VoiceRouteTestResult,
};

use serde::{Serialize, Serializer, ser::SerializeMap};
use ts_rs::TS;

pub use crate::error::PublicError;
pub use crate::migrate::{LegacyMigrationStatus, LegacySessionImport};
pub use crate::runtime::PreflightIssue;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct FoundationStatus {
    pub ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MicPcmAcceptance {
    pub accepted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct VideoFrameAcceptance {
    pub accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SecretStatus {
    pub reference: String,
    pub configured: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DiagnosticsExportResult {
    pub exported: bool,
}

/// 单条语音线路的首响延迟与入口丢帧汇总（只读诊断）。
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RouteLatencySummary {
    pub route_id: String,
    pub route_label: String,
    pub samples: u32,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub ingress_dropped_total: u32,
}

/// 按语音线路汇总的最近会话首响延迟概览（只读诊断）。
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DiagnosticsLatencySummary {
    pub sessions_scanned: u32,
    pub routes: Vec<RouteLatencySummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(tag = "kind", rename_all = "camelCase")]
pub enum StartupState {
    Ready,
    Migrated,
    Recoverable { error: PublicError },
    Invalid { error: PublicError },
}

#[derive(Debug, Clone, PartialEq, Eq, TS)]
#[ts(type = "{ ok: true; data: T } | { ok: false; error: PublicError }")]
pub enum CommandResult<T: TS> {
    Ok { data: T },
    Err { error: PublicError },
}

impl<T: Serialize + TS> Serialize for CommandResult<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(2))?;
        match self {
            Self::Ok { data } => {
                map.serialize_entry("ok", &true)?;
                map.serialize_entry("data", data)?;
            }
            Self::Err { error } => {
                map.serialize_entry("ok", &false)?;
                map.serialize_entry("error", error)?;
            }
        }
        map.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub status: String,
    pub role_profile_id: String,
    pub voice_route_id: String,
    pub transport_mode: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub updated_at: String,
}

impl From<crate::sessions::SessionRecord> for SessionSummary {
    fn from(record: crate::sessions::SessionRecord) -> Self {
        Self {
            id: record.id,
            status: record.status,
            role_profile_id: record.role_profile_id,
            voice_route_id: record.voice_route_id,
            transport_mode: record.transport_mode,
            started_at: record.started_at,
            finished_at: record.finished_at,
            updated_at: record.updated_at,
        }
    }
}

/// Tagged start outcome. `Blocked` carries every preflight issue, not just the first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(tag = "kind", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)]
pub enum SessionStartResult {
    Started { session: SessionSummary },
    Blocked { issues: Vec<PreflightIssue> },
}

/// Live runtime snapshot. Frontend must drop events or snapshots whose `seq`
/// is less than or equal to the last applied seq.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub phase: String,
    pub mode: String,
    #[ts(type = "number")]
    pub seq: u64,
    pub unused_materials: bool,
    pub last_error_code: Option<String>,
    #[ts(type = "number")]
    pub revision: u64,
    /// 实时语音 WS 链路状态：idle（无实时路线）/ connected / reconnecting /
    /// failed / unknown（会话锁被占用暂时不可读）。
    pub realtime_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionExportResult {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionCitationView {
    pub material_id: String,
    pub chunk_id: String,
    pub snippet: String,
}

/// 公开契约复用的轮次分阶段时间线（services 层同一结构，camelCase 序列化）。
pub use crate::services::realtime_pump::TurnTimeline;

/// 单轮延迟时间线视图：线路、模式、打断标注与分阶段毫秒数。
/// 旧记录没有 timeline 时整段缺省，前端显示「无数据」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct TurnLatencyView {
    pub route_id: String,
    /// "cascade" | "realtime"（读侧自 turn_meta.latencyMode 归一，缺省 realtime）。
    pub mode: String,
    pub interrupted: bool,
    pub timeline: TurnTimeline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionTurnView {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub web_sources: Option<Vec<crate::providers::web_search::WebSource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub web_degraded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub trigger_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub user_confirmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub playback_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub latency: Option<TurnLatencyView>,
    pub id: String,
    #[ts(type = "number")]
    pub turn_index: i64,
    pub user_text: String,
    pub assistant_text: String,
    pub materials_used: bool,
    pub citations: Vec<SessionCitationView>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionDetail {
    pub session: SessionSummary,
    pub turns: Vec<SessionTurnView>,
}

/// Transcript event. Frontend must drop this payload when `seq` is stale.
/// `done=false` 为流式增量快照（文本为已生成前缀，轮次尚未落库），
/// `done=true` 表示该轮已落库，前端此时才应拉取会话详情刷新列表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionTranscriptEvent {
    #[ts(type = "number")]
    pub seq: u64,
    pub text: String,
    pub done: bool,
}

/// Reply event. Frontend must drop this payload when `seq` is stale.
/// `done` 语义与 `SessionTranscriptEvent` 一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionReplyEvent {
    #[ts(type = "number")]
    pub seq: u64,
    pub text: String,
    pub done: bool,
}

/// Realtime WebAudio playback delta. PCM 只经本机 Tauri 事件传输，不入库，
/// 也不进入公开 bindings（公开契约禁止暴露 PCM）；前端用本地接口类型接收。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionAudioEvent {
    pub seq: u64,
    pub pcm_base64: String,
    pub sample_rate: u32,
}

/// WebAudio 播放队列控制事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SessionPlaybackControlEvent {
    #[ts(type = "number")]
    pub seq: u64,
    /// 目前只有 `clear`：清空前端播放队列。
    pub action: String,
}

/// Peak-only level event. Never includes PCM. Frontend must drop stale `seq`.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AudioLevelEvent {
    pub peak: f64,
    #[ts(type = "number")]
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AgentCommandInput {
    pub id: String,
    pub action: String,
    pub text: Option<String>,
    pub answer: Option<String>,
    pub mode: Option<String>,
    #[ts(type = "number")]
    pub expected_revision: u64,
}

/// Agent command result. `error` is a code only. Never includes PCM or provider bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AgentCommandResult {
    pub command_id: String,
    pub action: String,
    pub ok: bool,
    #[ts(type = "Record<string, unknown>")]
    pub result: serde_json::Value,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct LivestreamSegmentDraftInput {
    pub title: String,
    pub text: String,
    pub estimated_seconds: u32,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct LivestreamDraftInput {
    pub title: String,
    pub segments: Vec<LivestreamSegmentDraftInput>,
    pub loop_enabled: bool,
    pub media_path: Option<String>,
    pub media_kind: Option<crate::livestream::LivestreamMediaKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct LivestreamGenerateInput {
    pub title: String,
    pub material_ids: Vec<String>,
    pub language: String,
    pub max_segments: u8,
    pub loop_enabled: bool,
    pub media_path: Option<String>,
    pub media_kind: Option<crate::livestream::LivestreamMediaKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct LivestreamRuntime {
    pub script: crate::livestream::LivestreamScript,
    pub stage: crate::livestream::LivestreamStageState,
}

#[cfg(test)]
fn generated_bindings() -> String {
    let config = ts_rs::Config::default();
    // Registry of public DTO declarations. Emit order == registry order; adding a
    // new DTO to the public contract only requires adding one more `decl` entry.
    let decls = [
        PublicError::decl(&config),
        FoundationStatus::decl(&config),
        MicPcmAcceptance::decl(&config),
        VideoFrameAcceptance::decl(&config),
        SecretStatus::decl(&config),
        DiagnosticsExportResult::decl(&config),
        DiagnosticsLatencySummary::decl(&config),
        RouteLatencySummary::decl(&config),
        StartupState::decl(&config),
        crate::migrate::LegacyMigrationStatus::decl(&config),
        crate::migrate::LegacySessionImport::decl(&config),
        // Config contract DTOs (Phase 3 Stage B): the redacted public
        // configuration tree. Providers only carry SecretSlot references; no
        // secret value is ever part of this contract.
        SecretSlot::decl(&config),
        ApplicationConfig::decl(&config),
        ProviderConfig::decl(&config),
        crate::providers::web_search::WebCapability::decl(&config),
        crate::providers::web_search::WebCapabilityStatus::decl(&config),
        crate::providers::web_search::WebSource::decl(&config),
        crate::processes::MeetingProcess::decl(&config),
        crate::audio::playback::AudioOutputDevice::decl(&config),
        crate::prerequisites::LocalAudioDevice::decl(&config),
        crate::prerequisites::VirtualAudioPreparation::decl(&config),
        crate::prerequisites::PreparationDiagnostic::decl(&config),
        ModelConfig::decl(&config),
        VoiceRouteMode::decl(&config),
        VoiceRouteConfig::decl(&config),
        SpeechConfig::decl(&config),
        EmbeddingDistance::decl(&config),
        EmbeddingConfig::decl(&config),
        KnowledgeConfig::decl(&config),
        StorageConfig::decl(&config),
        RoleScenario::decl(&config),
        RoleProfileConfig::decl(&config),
        DiagnosticsConfig::decl(&config),
        PublicConfig::decl(&config),
        ProviderSaveInput::decl(&config),
        ProviderTestResult::decl(&config),
        crate::services::ProviderDependency::decl(&config),
        DiscoveredModelDto::decl(&config),
        ModelDiscoveryResult::decl(&config),
        VoiceRouteSaveInput::decl(&config),
        VoiceRouteTestResult::decl(&config),
        crate::services::VoiceReferenceSaveInput::decl(&config),
        crate::services::VoiceReferenceAudioSaveInput::decl(&config),
        crate::services::VoiceReferenceUpdateInput::decl(&config),
        crate::services::VoiceReferenceSummary::decl(&config),
        crate::services::VoiceReferenceCloneResult::decl(&config),
        RoleProfileSaveInput::decl(&config),
        RoleProfileCopyInput::decl(&config),
        EmbeddingConfigSaveInput::decl(&config),
        EmbeddingTestResult::decl(&config),
        MaterialSummary::decl(&config),
        MaterialIndexResult::decl(&config),
        MaterialSearchHit::decl(&config),
        PreflightIssue::decl(&config),
        SessionSummary::decl(&config),
        SessionStartResult::decl(&config),
        RuntimeStatus::decl(&config),
        SessionExportResult::decl(&config),
        SessionCitationView::decl(&config),
        TurnTimeline::decl(&config),
        TurnLatencyView::decl(&config),
        SessionTurnView::decl(&config),
        SessionDetail::decl(&config),
        SessionTranscriptEvent::decl(&config),
        SessionReplyEvent::decl(&config),
        SessionPlaybackControlEvent::decl(&config),
        AudioLevelEvent::decl(&config),
        AgentCommandInput::decl(&config),
        AgentCommandResult::decl(&config),
        crate::livestream::LivestreamState::decl(&config),
        crate::livestream::LivestreamSegmentStatus::decl(&config),
        crate::livestream::LivestreamOutputState::decl(&config),
        crate::livestream::LivestreamMediaKind::decl(&config),
        crate::livestream::LivestreamSegment::decl(&config),
        crate::livestream::LivestreamScript::decl(&config),
        crate::livestream::LivestreamStageState::decl(&config),
        LivestreamSegmentDraftInput::decl(&config),
        LivestreamDraftInput::decl(&config),
        LivestreamGenerateInput::decl(&config),
        LivestreamRuntime::decl(&config),
        crate::obs::ObsRuntimeStatus::decl(&config),
        CommandResult::<FoundationStatus>::decl(&config),
    ];
    let header = "// Generated by ts-rs. Do not edit.\n";
    let body = decls
        .iter()
        .map(|decl| format!("export {decl}\n"))
        .collect::<Vec<String>>()
        .join("\n");
    format!("{header}\n{body}")
}

#[cfg(test)]
fn write_bindings(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create generated binding directory");
    }
    std::fs::write(path, generated_bindings()).expect("write generated TypeScript bindings");
}

#[cfg(test)]
mod tests {
    use super::{CommandResult, FoundationStatus, PublicError, generated_bindings, write_bindings};
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn contracts_serialize_with_stable_shapes() {
        assert_eq!(
            serde_json::to_value(CommandResult::Ok {
                data: FoundationStatus { ready: true },
            })
            .unwrap(),
            json!({"ok": true, "data": {"ready": true}})
        );
        assert_eq!(
            serde_json::to_value(CommandResult::<FoundationStatus>::Err {
                error: PublicError::new("CONFIG_INVALID", "配置无效", false)
                    .with_field("providers"),
            })
            .unwrap()["ok"],
            false
        );
        assert_eq!(
            PublicError::new("CONFIG_INVALID", "配置无效", false).code,
            "CONFIG_INVALID"
        );
    }

    #[test]
    fn export_bindings() {
        let bindings = generated_bindings();
        assert!(
            bindings.contains("export type CommandResult<T>"),
            "unexpected bindings:\n{bindings}"
        );
        assert!(bindings.contains("ok: true"));
        assert!(bindings.contains("ok: false"));
        assert!(bindings.contains("export type SecretStatus"));
        assert!(bindings.contains("export type DiagnosticsExportResult"));
        assert!(bindings.contains("export type StartupState"));
        assert!(bindings.contains("export type LegacyMigrationStatus"));
        assert!(bindings.contains("export type LegacySessionImport"));
        assert!(bindings.contains("reenterSecrets"));
        assert!(bindings.contains("export type PublicConfig"));
        assert!(bindings.contains("export type VoiceRouteMode"));
        assert!(bindings.contains("export type VoiceRouteConfig"));
        assert!(bindings.contains("export type RoleProfileConfig"));
        assert!(bindings.contains("export type RoleScenario"));
        assert!(bindings.contains("export type EmbeddingConfig"));
        assert!(bindings.contains("export type RoleProfileSaveInput"));
        assert!(bindings.contains("export type EmbeddingConfigSaveInput"));
        assert!(
            bindings.contains("export type MaterialSummary"),
            "unexpected bindings:\n{bindings}"
        );
        assert!(
            bindings.contains("export type MaterialSearchHit"),
            "unexpected bindings:\n{bindings}"
        );
        assert!(
            bindings.contains("export type MaterialIndexResult"),
            "unexpected bindings:\n{bindings}"
        );
        assert!(
            !bindings.contains("extractedText") && !bindings.contains("extracted_text"),
            "public bindings must not expose extracted document text:\n{bindings}"
        );
        for name in [
            "SessionSummary",
            "SessionStartResult",
            "PreflightIssue",
            "RuntimeStatus",
            "SessionExportResult",
            "SessionDetail",
            "SessionTurnView",
            "SessionCitationView",
            "TurnTimeline",
            "TurnLatencyView",
            "SessionTranscriptEvent",
            "SessionReplyEvent",
            "SessionPlaybackControlEvent",
            "AudioLevelEvent",
            "AgentCommandInput",
            "AgentCommandResult",
            "VoiceReferenceSummary",
            "VoiceReferenceSaveInput",
            "VoiceReferenceAudioSaveInput",
            "VoiceReferenceUpdateInput",
            "VoiceReferenceCloneResult",
            "LivestreamDraftInput",
            "LivestreamGenerateInput",
            "LivestreamRuntime",
            "ObsRuntimeStatus",
        ] {
            assert!(
                bindings.contains(&format!("export type {name}")),
                "missing {name} in bindings:\n{bindings}"
            );
        }
        assert!(
            !bindings.contains("pcm") && !bindings.contains("PCM"),
            "public bindings must not expose PCM:\n{bindings}"
        );
        assert!(
            bindings.contains("unusedMaterials")
                && bindings.contains("lastErrorCode")
                && bindings.contains("revision"),
            "RuntimeStatus must expose unusedMaterials, lastErrorCode, and revision:\n{bindings}"
        );
        write_bindings(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/generated/bindings.ts"));
    }
}
