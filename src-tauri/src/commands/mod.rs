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
mod tests {
    use std::sync::Arc;

    use super::{
        embedding_service_error, provider_service_error, public_config, role_service_error,
        route_service_error,
    };
    use crate::{
        app_state::{AppPaths, AppState},
        providers::ProviderError,
        secrets::MemorySecretStore,
        services::{
            EmbeddingServiceError, ProviderServiceError, RoleProfileServiceError,
            VoiceRouteServiceError,
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
        // R1 拆分后 restore 命令位于 commands/config.rs，扫描目标同步指向新文件。
        let source = include_str!("config.rs");
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
            .split("blocking_command!(")
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
        // R1 拆分后 material 命令位于 commands/materials.rs，扫描目标同步指向新文件。
        let source = include_str!("materials.rs");
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
            .split("pub fn material_index")
            .next()
            .unwrap();
        assert!(import.contains("service_guard"));
        assert!(delete.contains("service_guard"));
    }

    #[test]
    fn material_read_commands_skip_the_service_guard() {
        let source = include_str!("materials.rs");
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
        // R1 拆分后 session 命令位于 commands/sessions.rs，扫描目标同步指向新文件。
        let source = include_str!("sessions.rs");
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
        let source = include_str!("sessions.rs");
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
        let blocked = serde_json::to_value(super::session_start_cmd(&empty, None)).unwrap();
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
        let started = serde_json::to_value(super::session_start_cmd(&ready, None)).unwrap();
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
    fn session_start_passes_allow_barge_in_to_the_service() {
        // 意义：命令层的会话级语音打断开关必须透传到服务（capture 旗标接控制端）；
        // 未传（None）时保持关闭——置位旗标也不得影响控制端。
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let started = serde_json::to_value(super::session_start_selected_cmd(
            &state,
            None,
            None,
            false,
            Some(true),
        ))
        .unwrap();
        assert_eq!(started["ok"], true, "{started}");
        assert_eq!(started["data"]["kind"], "started");
        {
            let sessions = state.sessions.lock().unwrap();
            sessions
                .capture()
                .barge_in_flag()
                .store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(sessions.control().barge_in_requested());
        }
        super::session_stop_cmd(&state);

        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        let started = serde_json::to_value(super::session_start_selected_cmd(
            &state, None, None, false, None,
        ))
        .unwrap();
        assert_eq!(started["ok"], true, "{started}");
        assert_eq!(started["data"]["kind"], "started");
        let sessions = state.sessions.lock().unwrap();
        sessions
            .capture()
            .barge_in_flag()
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!sessions.control().barge_in_requested());
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
            None,
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
                &state, role, route, false, None,
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
                None,
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
        let started = serde_json::to_value(super::session_start_cmd(&state, None)).unwrap();
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
            serde_json::to_value(super::session_start_cmd(&state, None)).unwrap()["ok"],
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
        // 流式事件需要携带完整前缀（上限 EVENT_TEXT_LIMIT），不再截到 160。
        assert_eq!(
            super::transcript_event(5, &"x".repeat(super::EVENT_TEXT_LIMIT + 200))["text"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            super::EVENT_TEXT_LIMIT
        );
        // done 标记区分流式快照与落库收尾。
        assert_eq!(super::transcript_event(6, "hi")["done"], true,);
        assert_eq!(super::reply_event(7, "hi")["done"], true);
    }

    /// Tauri 2 只接受字母数字与 `-` `/` `:` `_`；含点的事件名 emit 会静默失败、
    /// 前端 listen 抛错，所有实时字幕、状态和 WebAudio 音频都到不了页面。
    #[test]
    fn event_names_are_valid_tauri_event_names() {
        for name in [
            super::EVENT_RUNTIME_STATUS,
            super::EVENT_AUDIO_LEVEL,
            super::EVENT_SESSION_TRANSCRIPT,
            super::EVENT_SESSION_REPLY,
            super::EVENT_SESSION_AUDIO,
            super::EVENT_SESSION_PLAYBACK_CONTROL,
            "virtual_audio:preparation:v1",
        ] {
            assert!(
                name.chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '-' | '/' | ':' | '_')),
                "invalid tauri event name: {name}"
            );
        }
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
            serde_json::to_value(super::session_start_cmd(&state, None)).unwrap()["ok"],
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
        serde_json::to_value(super::session_push_mic_pcm_cmd(
            state,
            &encoded,
            sample_rate,
        ))
        .unwrap()
    }

    #[test]
    fn session_push_mic_pcm_feeds_segmenter_and_finalizes_via_asr() {
        // 管线联通性测试依赖能量分段器的确定性提交语义（合成方波不被 Silero 判为语音），
        // 经 AI_VOICE_VAD=off 开关强制能量实现，与工厂用例共用锁串行执行。
        crate::audio::segmenter::factory_test_support::with_vad_off(|| {
            let directory = tempfile::tempdir().unwrap();
            let state = session_state(&directory, &ready_session_config());
            assert_eq!(
                serde_json::to_value(super::session_start_cmd(&state, None)).unwrap()["ok"],
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
        });
    }

    #[test]
    fn session_push_mic_pcm_resamples_non_48k_input() {
        // 同上：重采样断言依赖能量分段器确定性阈值，强制能量实现。
        crate::audio::segmenter::factory_test_support::with_vad_off(|| {
            let directory = tempfile::tempdir().unwrap();
            let state = session_state(&directory, &ready_session_config());
            assert_eq!(
                serde_json::to_value(super::session_start_cmd(&state, None)).unwrap()["ok"],
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
        });
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
    fn session_push_video_frame_validates_input_and_reports_acceptance() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());

        // 空帧与超限帧（base64 > 300KB）拒绝，不触碰会话锁语义。
        for invalid in ["", &"a".repeat(super::VIDEO_FRAME_BASE64_LIMIT + 1)] {
            let rejected =
                serde_json::to_value(super::session_push_video_frame_cmd(&state, invalid)).unwrap();
            assert_eq!(rejected["ok"], false, "{rejected}");
            assert_eq!(rejected["error"]["code"], "SESSION_VIDEO_FRAME_INVALID");
        }

        // 无泵会话（级联/未装配实时路线）：帧合法但无接收方，accepted=false。
        let idle =
            serde_json::to_value(super::session_push_video_frame_cmd(&state, "aGk=")).unwrap();
        assert_eq!(idle["ok"], true, "{idle}");
        assert_eq!(idle["data"]["accepted"], false);
    }

    #[test]
    fn session_finalize_after_llm_error_is_not_state_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let state = session_state(&directory, &ready_session_config());
        assert_eq!(
            serde_json::to_value(super::session_start_cmd(&state, None)).unwrap()["ok"],
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
            serde_json::to_value(super::session_start_cmd(&state, None)).unwrap()["ok"],
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
            serde_json::to_value(super::session_start_cmd(&state, None)).unwrap()["ok"],
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
        state
            .livestream
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .pause()
            .unwrap();
        assert!(super::advance_livestream_after_success(&state, &token).is_none());
        let script = state.livestream.lock().unwrap();
        assert_eq!(script.as_ref().unwrap().current_index, Some(0));
        assert_eq!(
            script.as_ref().unwrap().state,
            crate::livestream::LivestreamState::Paused
        );
    }

    #[test]
    fn livestream_cancelled_worker_cannot_report_failure_on_current_script() {
        let directory = tempfile::tempdir().unwrap();
        let state = material_state(&directory);
        playing_two_segment_runtime(&state);
        let token = state.livestream_playback_cancel.lock().unwrap().clone();
        token.store(true, std::sync::atomic::Ordering::SeqCst);
        super::set_livestream_output_state(
            &state,
            &token,
            crate::livestream::LivestreamOutputState::Failed,
            Some("LIVESTREAM_TTS_FAILED"),
        );
        assert_eq!(
            state
                .livestream_stage
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .output_error_code,
            None
        );
        assert_eq!(
            state.livestream.lock().unwrap().as_ref().unwrap().state,
            crate::livestream::LivestreamState::Playing
        );
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
        // R1 拆分后 livestream 命令位于 commands/livestream.rs，扫描目标同步指向新文件。
        let source = include_str!("livestream.rs");
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
