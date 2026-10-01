use std::{collections::HashMap, ffi::OsString, path::PathBuf};

use super::{
    AppConfigV1, ConfigDirs, ConfigError, ConfigPatch, ConfigStore, DiagnosticsPatch,
    EmbeddingDistance, VoiceRouteMode, locate_config, public_view,
};

fn dirs() -> ConfigDirs {
    ConfigDirs {
        repository: PathBuf::from(r"E:\source\assistant"),
        roaming_app_data: PathBuf::from(r"C:\Users\tester\AppData\Roaming"),
    }
}

#[cfg(windows)]
#[test]
fn config_path_precedence_is_deterministic() {
    use super::ConfigSource;

    let mut env = HashMap::new();
    env.insert(
        "AI_VIRTUAL_ASSISTANT_CONFIG".to_owned(),
        OsString::from(r"C:\env\config.json"),
    );
    env.insert("UNRELATED_SECRET".to_owned(), OsString::from("ignored"));

    let cli = locate_config(
        &[
            OsString::from("app"),
            OsString::from("--config"),
            OsString::from(r"D:\cli\config.json"),
        ],
        &env,
        &dirs(),
        true,
    )
    .unwrap();
    assert_eq!(cli.source, ConfigSource::CommandLine);
    assert_eq!(cli.path, PathBuf::from(r"D:\cli\config.json"));

    let from_env = locate_config(&[OsString::from("app")], &env, &dirs(), true).unwrap();
    assert_eq!(from_env.source, ConfigSource::Environment);
    assert_eq!(from_env.path, PathBuf::from(r"C:\env\config.json"));

    env.clear();
    let development = locate_config(&[], &env, &dirs(), true).unwrap();
    assert_eq!(development.source, ConfigSource::DevelopmentDefault);
    assert_eq!(
        development.path,
        PathBuf::from(r"E:\source\assistant\config\local.json")
    );

    let release = locate_config(&[], &env, &dirs(), false).unwrap();
    assert_eq!(release.source, ConfigSource::ReleaseDefault);
    assert_eq!(
        release.path,
        PathBuf::from(r"C:\Users\tester\AppData\Roaming\AI Virtual Assistant\config.json")
    );
}

#[test]
fn config_paths_from_cli_and_environment_must_be_absolute() {
    let error = locate_config(
        &[
            OsString::from("app"),
            OsString::from("--config"),
            OsString::from("relative.json"),
        ],
        &HashMap::new(),
        &dirs(),
        true,
    )
    .unwrap_err();
    assert_eq!(error.code(), "CONFIG_PATH_NOT_ABSOLUTE");

    let env = HashMap::from([(
        "AI_VIRTUAL_ASSISTANT_CONFIG".to_owned(),
        OsString::from("relative.json"),
    )]);
    assert_eq!(
        locate_config(&[], &env, &dirs(), false).unwrap_err().code(),
        "CONFIG_PATH_NOT_ABSOLUTE"
    );
}

fn parse_error(json: &str) -> ConfigError {
    AppConfigV1::from_json(json).unwrap_err()
}

/// Scan for fields that could carry secret material. `apiKey` and `apiSecret`
/// are canonical public `SecretSlot` fields, so they are checked structurally
/// by the public projection test rather than rejected by name.
fn contains_secret_material(json: &str) -> bool {
    let lower = json.to_ascii_lowercase();
    ["password", "secretvalue", "secretcontents", "\"token\""]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[test]
fn minimal_configuration_uses_safe_defaults() {
    let config = AppConfigV1::from_json(r#"{"configVersion":1}"#).unwrap();
    assert!(config.models.providers.is_empty());
    assert_eq!(config.diagnostics.log_retention_days, 14);
}

#[test]
fn tracked_example_is_valid_and_contains_no_configured_secret() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("config/local.example.json");
    let json = std::fs::read_to_string(path).unwrap();
    let config = AppConfigV1::from_json(&json).unwrap();

    assert!(config.models.providers.iter().all(|provider| {
        provider
            .credential
            .as_ref()
            .is_none_or(|slot| !slot.configured)
    }));
    assert_eq!(config.role_profiles.len(), 1);
    assert!(config.active_role_profile_id.is_none());
    assert_eq!(config.knowledge.embedding_configs.len(), 1);
    assert!(config.knowledge.active_embedding_config_id.is_none());
}

#[test]
fn rejects_unknown_versions_and_broken_references() {
    assert_eq!(
        parse_error(r#"{"configVersion":2}"#).code(),
        "CONFIG_VERSION_UNSUPPORTED"
    );
    assert_eq!(
        parse_error(r#"{"configVersion":1,"models":{"providers":[{"id":"same","baseUrl":"https://one.example"},{"id":"same","baseUrl":"https://two.example"}]}}"#).code(),
        "CONFIG_DUPLICATE_ID"
    );
    assert_eq!(
        parse_error(r#"{"configVersion":1,"models":{"activeProviderId":"missing"}}"#).code(),
        "CONFIG_REFERENCE_MISSING"
    );
}

#[test]
fn rejects_unsafe_urls_inline_secrets_and_invalid_retention() {
    assert_eq!(
        parse_error(r#"{"configVersion":1,"models":{"providers":[{"id":"bad","baseUrl":"https://example.com/v1?api_key=leak"}]}}"#).code(),
        "CONFIG_URL_INVALID"
    );
    assert_eq!(
        parse_error(r#"{"configVersion":1,"models":{"providers":[{"id":"bad","baseUrl":"ftp://example.com"}]}}"#).code(),
        "CONFIG_URL_INVALID"
    );
    assert_eq!(
        parse_error(r#"{"configVersion":1,"models":{"providers":[{"id":"bad","baseUrl":"https://example.com","apiKey":"do-not-store"}]}}"#).code(),
        "CONFIG_SECRET_INLINE_FORBIDDEN"
    );
    assert_eq!(
        parse_error(r#"{"configVersion":1,"diagnostics":{"logRetentionDays":0}}"#).code(),
        "CONFIG_FIELD_INVALID"
    );
}

#[test]
fn active_voice_route_flags_must_match_the_active_id() {
    let base = r#"{"configVersion":1,"speech":{"voiceRoutes":[{"id":"r1","active":true,"ready":true,"status":"ready","configVersion":1},{"id":"r2","active":true,"ready":true,"status":"ready","configVersion":1}],"activeVoiceRouteId":"r1"}}"#;
    assert_eq!(parse_error(base).code(), "CONFIG_ACTIVE_ROUTE_INVALID");
    let mismatch = r#"{"configVersion":1,"speech":{"voiceRoutes":[{"id":"r1","active":false,"ready":true,"status":"ready","configVersion":1}],"activeVoiceRouteId":"r1"}}"#;
    assert_eq!(parse_error(mismatch).code(), "CONFIG_ACTIVE_ROUTE_INVALID");
}

#[test]
fn voice_route_defaults_are_backward_compatible() {
    let config =
        AppConfigV1::from_json(r#"{"configVersion":1,"speech":{"voiceRoutes":[{"id":"r"}]}}"#)
            .unwrap();
    let route = &config.speech.voice_routes[0];
    assert_eq!(route.mode, VoiceRouteMode::Cascaded);
    assert!(route.name.is_empty());
    assert!(!route.active);
    assert!(!route.ready);
    assert_eq!(route.config_version, 0);
    assert!(route.asr_provider_id.is_none());
    assert!(route.e2e_provider_id.is_none());
}

#[test]
fn voice_route_rejects_unknown_mode_and_missing_provider_reference() {
    assert_eq!(
        parse_error(r#"{"configVersion":1,"speech":{"voiceRoutes":[{"id":"r","mode":"turbo"}]}}"#)
            .code(),
        "CONFIG_INVALID"
    );
    assert_eq!(
        parse_error(r#"{"configVersion":1,"speech":{"voiceRoutes":[{"id":"r","mode":"cascaded","asrProviderId":"missing"}]}}"#).code(),
        "CONFIG_REFERENCE_MISSING"
    );
}

#[test]
fn legacy_role_embedding_and_removed_transport_fields_upgrade_to_safe_canonical_state() {
    let legacy_json = r#"{
        "configVersion": 1,
        "roleProfiles": [{"id":"interviewer","instructions":"Ask one question"}],
        "knowledge": {"embeddingProviderId":"legacy-provider"},
        "transport": {"livekitUrl":"wss://legacy.example.com"}
    }"#;

    let config = AppConfigV1::from_json(legacy_json).unwrap();
    assert_eq!(config.role_profiles[0].name, "interviewer");
    assert_eq!(config.role_profiles[0].system_prompt, "Ask one question");
    assert!(config.role_profiles[0].opening_message.is_empty());
    assert!(config.role_profiles[0].style_instructions.is_empty());
    assert!(!config.role_profiles[0].active);
    assert_eq!(config.role_profiles[0].config_version, 0);
    assert!(config.active_role_profile_id.is_none());
    assert!(config.knowledge.embedding_configs.is_empty());
    assert!(config.knowledge.active_embedding_config_id.is_none());

    let canonical_json = serde_json::to_string(&config).unwrap();
    assert!(canonical_json.contains("\"systemPrompt\":\"Ask one question\""));
    assert!(canonical_json.contains("\"embeddingConfigs\":[]"));
    assert!(!canonical_json.contains("instructions"));
    assert!(!canonical_json.contains("livekitUrl"));
    assert!(!canonical_json.contains("livekit"));
    assert!(!canonical_json.contains("embeddingProviderId"));
}

#[test]
fn legacy_role_migration_preserves_noncanonical_id_and_oversized_instructions_safely() {
    let legacy_id = "Interviewer / Panel 🧭";
    let legacy_instructions = "旧版角色说明。".repeat(5_000);
    assert!(legacy_instructions.len() > 32 * 1024);
    let legacy_json = serde_json::json!({
        "configVersion": 1,
        "roleProfiles": [{
            "id": legacy_id,
            "instructions": legacy_instructions
        }]
    })
    .to_string();

    let config = AppConfigV1::from_json(&legacy_json).unwrap();
    let profile = &config.role_profiles[0];
    assert!(profile.id.starts_with("legacy-"));
    assert!(profile.id.len() <= 64);
    assert!(profile.id.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
    }));
    assert_eq!(profile.name, legacy_id);
    assert_eq!(profile.system_prompt, legacy_instructions);
    assert!(!profile.active);
    assert_eq!(profile.config_version, 0);

    let canonical_json = serde_json::to_string(&config).unwrap();
    assert!(!canonical_json.contains("instructions"));
    assert_eq!(AppConfigV1::from_json(&canonical_json).unwrap(), config);
}

#[test]
fn canonical_role_embedding_and_removed_transport_configuration_loads() {
    let config = AppConfigV1::from_json(
        r#"{
            "configVersion": 1,
            "models": {
                "providers": [{"id":"provider-1","baseUrl":"https://api.example.com/v1"}],
                "activeProviderId": "provider-1"
            },
            "activeRoleProfileId": "role-1",
            "roleProfiles": [{
                "id":"role-1","name":"Interviewer","systemPrompt":"Ask one question",
                "openingMessage":"Welcome","styleInstructions":"Be concise",
                "active":true,"configVersion":1
            }],
            "knowledge": {
                "embeddingConfigs": [{
                    "id":"embedding-1","providerId":"provider-1","modelId":"embed-3",
                    "dimensions":1536,"distance":"cosine","normalized":true,
                    "active":true,"ready":true,"status":"ready","configVersion":1
                }],
                "activeEmbeddingConfigId":"embedding-1"
            },
            "transport": {"livekit": {
                "enabled":true,"url":"wss://rtc.example.com",
                "apiKey":{"reference":"transport/livekit/api-key","configured":true},
                "apiSecret":{"reference":"transport/livekit/api-secret","configured":true},
                "ready":true,"status":"ready","configVersion":1
            }}
        }"#,
    )
    .unwrap();

    assert_eq!(config.active_role_profile_id.as_deref(), Some("role-1"));
    assert_eq!(
        config.knowledge.embedding_configs[0].distance,
        EmbeddingDistance::Cosine
    );
    // LiveKit transport 配置已整体移除：旧键被接受但不再进入规范化配置。
    let canonical_json = serde_json::to_string(&config).unwrap();
    assert!(!canonical_json.contains("livekit"));
}

#[test]
fn role_active_id_must_be_unique_present_and_match_derived_active() {
    assert_eq!(
        parse_error(
            r#"{"configVersion":1,"roleProfiles":[{"id":"same","name":"One","systemPrompt":"","openingMessage":"","styleInstructions":"","active":false,"configVersion":1},{"id":"same","name":"Two","systemPrompt":"","openingMessage":"","styleInstructions":"","active":false,"configVersion":1}]}"#
        )
        .code(),
        "CONFIG_DUPLICATE_ID"
    );
    assert_eq!(
        parse_error(r#"{"configVersion":1,"activeRoleProfileId":"missing"}"#).code(),
        "CONFIG_REFERENCE_MISSING"
    );
    assert_eq!(
        parse_error(
            r#"{"configVersion":1,"activeRoleProfileId":"r1","roleProfiles":[{"id":"r1","name":"One","systemPrompt":"","openingMessage":"","styleInstructions":"","active":false,"configVersion":1}]}"#
        )
        .code(),
        "CONFIG_ACTIVE_ROLE_INVALID"
    );
    assert_eq!(
        parse_error(
            r#"{"configVersion":1,"roleProfiles":[{"id":"r1","name":"One","systemPrompt":"","openingMessage":"","styleInstructions":"","active":true,"configVersion":1}]}"#
        )
        .code(),
        "CONFIG_ACTIVE_ROLE_INVALID"
    );
}

#[test]
fn embedding_active_id_must_be_unique_present_ready_and_match_derived_active() {
    let duplicate = r#"{
        "configVersion":1,
        "models":{"providers":[{"id":"p1","baseUrl":"https://api.example.com"}]},
        "knowledge":{"embeddingConfigs":[
            {"id":"same","providerId":"p1","modelId":"m1","dimensions":1,"distance":"cosine","normalized":true,"active":false,"ready":false,"status":null,"configVersion":1},
            {"id":"same","providerId":"p1","modelId":"m2","dimensions":2,"distance":"cosine","normalized":true,"active":false,"ready":false,"status":null,"configVersion":1}
        ]}
    }"#;
    assert_eq!(parse_error(duplicate).code(), "CONFIG_DUPLICATE_ID");
    assert_eq!(
        parse_error(r#"{"configVersion":1,"knowledge":{"activeEmbeddingConfigId":"missing"}}"#)
            .code(),
        "CONFIG_REFERENCE_MISSING"
    );

    let inconsistent = r#"{
        "configVersion":1,
        "models":{"providers":[{"id":"p1","baseUrl":"https://api.example.com"}]},
        "knowledge":{"embeddingConfigs":[{"id":"e1","providerId":"p1","modelId":"m1","dimensions":1,"distance":"cosine","normalized":true,"active":false,"ready":true,"status":"ready","configVersion":1}],"activeEmbeddingConfigId":"e1"}
    }"#;
    assert_eq!(
        parse_error(inconsistent).code(),
        "CONFIG_ACTIVE_EMBEDDING_INVALID"
    );

    let not_ready = r#"{
        "configVersion":1,
        "models":{"providers":[{"id":"p1","baseUrl":"https://api.example.com"}]},
        "knowledge":{"embeddingConfigs":[{"id":"e1","providerId":"p1","modelId":"m1","dimensions":1,"distance":"cosine","normalized":true,"active":true,"ready":false,"status":null,"configVersion":1}],"activeEmbeddingConfigId":"e1"}
    }"#;
    assert_eq!(
        parse_error(not_ready).code(),
        "CONFIG_ACTIVE_EMBEDDING_INVALID"
    );
}

#[test]
fn role_content_limits_are_enforced_in_bytes() {
    fn role_json(name: &str, prompt: &str, opening: &str, style: &str) -> String {
        serde_json::json!({
            "configVersion": 1,
            "roleProfiles": [{
                "id": "role-1",
                "name": name,
                "systemPrompt": prompt,
                "openingMessage": opening,
                "styleInstructions": style,
                "active": false,
                "configVersion": 1
            }]
        })
        .to_string()
    }

    assert_eq!(
        parse_error(&role_json(" ", "", "", "")).code(),
        "CONFIG_FIELD_INVALID"
    );
    assert_eq!(
        parse_error(&role_json("Role", &"a".repeat(32 * 1024 + 1), "", "")).code(),
        "CONFIG_FIELD_INVALID"
    );
    assert_eq!(
        parse_error(&role_json("Role", "", &"a".repeat(4 * 1024 + 1), "")).code(),
        "CONFIG_FIELD_INVALID"
    );
    assert_eq!(
        parse_error(&role_json("Role", "", "", &"a".repeat(8 * 1024 + 1))).code(),
        "CONFIG_FIELD_INVALID"
    );

    AppConfigV1::from_json(&role_json(
        "Role",
        &"a".repeat(32 * 1024),
        &"a".repeat(4 * 1024),
        &"a".repeat(8 * 1024),
    ))
    .unwrap();
}

#[test]
fn embedding_dimensions_and_provider_reference_are_validated() {
    for dimensions in [0, 65_537] {
        let json = format!(
            r#"{{"configVersion":1,"models":{{"providers":[{{"id":"p1","baseUrl":"https://api.example.com"}}]}},"knowledge":{{"embeddingConfigs":[{{"id":"e1","providerId":"p1","modelId":"m1","dimensions":{dimensions},"distance":"cosine","normalized":true,"active":false,"ready":false,"status":null,"configVersion":1}}]}}}}"#
        );
        assert_eq!(parse_error(&json).code(), "CONFIG_FIELD_INVALID");
    }

    assert_eq!(
        parse_error(
            r#"{"configVersion":1,"knowledge":{"embeddingConfigs":[{"id":"e1","providerId":"missing","modelId":"m1","dimensions":1,"distance":"cosine","normalized":true,"active":false,"ready":false,"status":null,"configVersion":1}]}}"#
        )
        .code(),
        "CONFIG_REFERENCE_MISSING"
    );
}

#[test]
fn public_view_exposes_secret_references_but_never_secret_material() {
    let config = AppConfigV1::from_json(
        r#"{
            "configVersion": 1,
            "models": {
                "providers": [{
                    "id": "p1",
                    "name": "Provider One",
                    "baseUrl": "https://one.example",
                    "credential": { "reference": "providers/p1/api-key", "configured": true }
                }],
                "activeProviderId": "p1"
            },
            "speech": {
                "voiceRoutes": [{
                    "id": "r1",
                    "name": "Route One",
                    "mode": "cascaded",
                    "asrProviderId": "p1",
                    "asrModelId": "asr-1",
                    "llmProviderId": "p1",
                    "llmModelId": "llm-1",
                    "ttsProviderId": "p1",
                    "ttsModelId": "tts-1",
                    "voiceId": "voice-1",
                    "active": true,
                    "ready": true,
                    "status": "ready",
                    "configVersion": 3
                }],
                "activeVoiceRouteId": "r1"
            }
        }"#,
    )
    .unwrap();

    let public = public_view(&config);
    let json = serde_json::to_string(&public).unwrap();

    // The credential survives only as a SecretSlot reference + configured flag.
    assert!(json.contains("providers/p1/api-key"));
    assert!(json.contains("\"configured\":true"));
    assert!(!json.contains("\"apiKey\":\""));
    assert!(!json.contains("\"apiSecret\":\""));
    // Voice-route projection carries the full cascaded wiring.
    assert!(json.contains("\"asrModelId\":\"asr-1\""));
    assert!(json.contains("\"mode\":\"cascaded\""));
    // No secret material keyword may appear anywhere in the public projection.
    assert!(
        !contains_secret_material(&json),
        "public config leaked secret material: {json}"
    );
}

#[test]
fn store_saves_atomically_and_keeps_a_last_good_copy() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, r#"{"configVersion":1}"#).unwrap();
    let store = ConfigStore::new(path.clone());

    let saved = store
        .save_patch(ConfigPatch {
            diagnostics: Some(DiagnosticsPatch {
                log_retention_days: Some(30),
            }),
        })
        .unwrap();
    assert_eq!(saved.diagnostics.log_retention_days, 30);
    assert_eq!(store.load().unwrap(), saved);
    assert_eq!(
        AppConfigV1::from_json(&std::fs::read_to_string(store.last_good_path()).unwrap()).unwrap(),
        saved
    );

    let before = std::fs::read_to_string(&path).unwrap();
    let error = store
        .save_patch(ConfigPatch {
            diagnostics: Some(DiagnosticsPatch {
                log_retention_days: Some(0),
            }),
        })
        .unwrap_err();
    assert_eq!(error.code(), "CONFIG_FIELD_INVALID");
    assert_eq!(std::fs::read_to_string(path).unwrap(), before);
}

#[test]
fn failed_last_good_write_does_not_commit_primary_config() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, r#"{"configVersion":1}"#).unwrap();
    let store = ConfigStore::new(path.clone());
    std::fs::create_dir(store.last_good_path()).unwrap();

    assert!(
        store
            .save_patch(ConfigPatch {
                diagnostics: Some(DiagnosticsPatch {
                    log_retention_days: Some(30)
                }),
            })
            .is_err()
    );
    assert_eq!(store.load().unwrap().diagnostics.log_retention_days, 14);
}

#[test]
fn concurrent_updates_do_not_lose_each_others_changes() {
    use std::{sync::mpsc, thread, time::Duration};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let store = ConfigStore::new(path);
    store.restore_defaults().unwrap();
    let first = store.clone();
    let second = store.clone();
    let (loaded_tx, loaded_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        first
            .update(|config| {
                loaded_tx.send(()).unwrap();
                thread::sleep(Duration::from_millis(100));
                config.application.locale = Some("zh-CN".into());
                Ok(())
            })
            .unwrap();
    });
    loaded_rx.recv().unwrap();
    second
        .update(|config| {
            config.diagnostics.log_retention_days = 30;
            Ok(())
        })
        .unwrap();
    worker.join().unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(loaded.application.locale.as_deref(), Some("zh-CN"));
    assert_eq!(loaded.diagnostics.log_retention_days, 30);
}
