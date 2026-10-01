use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use crate::{
    config::{ConfigStore, EmbeddingDistance},
    providers::{
        DiscoveredModel, EmbeddingError, EmbeddingProbe, ProviderEndpoint, ProviderError,
        ProviderProbe, RouteProbeError, RouteStageProbe,
    },
    secrets::{MemorySecretStore, SecretError, SecretService, SecretStore},
};

use super::{
    EmbeddingConfigSaveInput, EmbeddingService, ProviderSaveInput, ProviderService,
    RoleProfileCopyInput, RoleProfileSaveInput, RoleProfileService,
};

fn role_input(id: &str) -> RoleProfileSaveInput {
    RoleProfileSaveInput {
        id: Some(id.into()),
        name: " Interviewer ".into(),
        system_prompt: " Ask one question ".into(),
        opening_message: " Hello ".into(),
        style_instructions: " Concise ".into(),
    }
}

#[test]
fn role_save_creates_trimmed_profile_and_edit_resets_active_with_new_version() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    let service = RoleProfileService::new(&config);

    let saved = service.save(role_input("interviewer")).unwrap();
    assert_eq!(saved.id, "interviewer");
    assert_eq!(saved.name, "Interviewer");
    assert_eq!(saved.system_prompt, "Ask one question");
    assert_eq!(saved.opening_message, "Hello");
    assert_eq!(saved.style_instructions, "Concise");
    assert_eq!(saved.config_version, 1);
    assert!(!saved.active);

    assert!(service.activate("interviewer").unwrap().active);
    let edited = service
        .save(RoleProfileSaveInput {
            system_prompt: "Ask two questions".into(),
            ..role_input("interviewer")
        })
        .unwrap();
    assert_eq!(edited.config_version, 2);
    assert!(!edited.active);
    assert!(config.load().unwrap().active_role_profile_id.is_none());
}

#[test]
fn role_copy_clones_content_as_distinct_inactive_version_one_profile() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    let service = RoleProfileService::new(&config);
    let source = service.save(role_input("interviewer")).unwrap();
    service.activate("interviewer").unwrap();

    let copied = service
        .copy(RoleProfileCopyInput {
            source_id: "interviewer".into(),
            id: Some("panelist".into()),
        })
        .unwrap();

    assert_eq!(copied.id, "panelist");
    assert_eq!(copied.name, format!("{} 副本", source.name));
    assert_eq!(copied.system_prompt, source.system_prompt);
    assert_eq!(copied.opening_message, source.opening_message);
    assert_eq!(copied.style_instructions, source.style_instructions);
    assert_eq!(copied.config_version, 1);
    assert!(!copied.active);
    assert_eq!(
        config.load().unwrap().active_role_profile_id.as_deref(),
        Some("interviewer")
    );
}

#[test]
fn role_copy_rejects_duplicate_destination_id() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    let service = RoleProfileService::new(&config);
    service.save(role_input("interviewer")).unwrap();

    assert_eq!(
        service
            .copy(RoleProfileCopyInput {
                source_id: "interviewer".into(),
                id: Some("interviewer".into()),
            })
            .unwrap_err()
            .code(),
        "ROLE_PROFILE_COPY_ID_IN_USE"
    );
}

#[test]
fn role_save_generates_uuid_when_id_is_omitted() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    let service = RoleProfileService::new(&config);

    let saved = service
        .save(RoleProfileSaveInput {
            id: None,
            ..role_input("unused")
        })
        .unwrap();
    assert!(uuid::Uuid::parse_str(&saved.id).is_ok());
    assert_eq!(saved.name, "Interviewer");
}

#[test]
fn role_copy_generates_uuid_and_appends_copy_suffix_when_id_is_omitted() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    let service = RoleProfileService::new(&config);
    let source = service.save(role_input("interviewer")).unwrap();

    let copied = service
        .copy(RoleProfileCopyInput {
            source_id: "interviewer".into(),
            id: None,
        })
        .unwrap();

    assert!(uuid::Uuid::parse_str(&copied.id).is_ok());
    assert_ne!(copied.id, source.id);
    assert_eq!(copied.name, "Interviewer 副本");
    assert_eq!(copied.system_prompt, source.system_prompt);
}

#[test]
fn role_save_enforces_id_name_and_all_content_length_limits() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    let service = RoleProfileService::new(&config);

    let mut at_limits = role_input(&"a".repeat(64));
    at_limits.system_prompt = "p".repeat(32 * 1024);
    at_limits.opening_message = "o".repeat(4 * 1024);
    at_limits.style_instructions = "s".repeat(8 * 1024);
    assert!(service.save(at_limits).is_ok());

    for id in ["Uppercase", &"a".repeat(65)] {
        assert_eq!(
            service.save(role_input(id)).unwrap_err().code(),
            "ROLE_PROFILE_ID_INVALID"
        );
    }
    let mut empty_name = role_input("empty-name");
    empty_name.name = " \t ".into();
    assert_eq!(
        service.save(empty_name).unwrap_err().code(),
        "ROLE_PROFILE_FIELDS_INVALID"
    );
    for (id, field) in [
        ("prompt-too-long", "prompt"),
        ("opening-too-long", "opening"),
        ("style-too-long", "style"),
    ] {
        let mut input = role_input(id);
        match field {
            "prompt" => input.system_prompt = "p".repeat(32 * 1024 + 1),
            "opening" => input.opening_message = "o".repeat(4 * 1024 + 1),
            "style" => input.style_instructions = "s".repeat(8 * 1024 + 1),
            _ => unreachable!(),
        }
        assert_eq!(
            service.save(input).unwrap_err().code(),
            "ROLE_PROFILE_FIELDS_INVALID"
        );
    }
}

#[test]
fn role_activation_is_singleton_and_delete_clears_active_id_and_reports_missing_roles() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    let service = RoleProfileService::new(&config);
    service.save(role_input("interviewer")).unwrap();
    service.save(role_input("panelist")).unwrap();

    service.activate("interviewer").unwrap();
    assert!(service.activate("panelist").unwrap().active);
    let loaded = config.load().unwrap();
    assert_eq!(loaded.active_role_profile_id.as_deref(), Some("panelist"));
    assert_eq!(
        loaded
            .role_profiles
            .iter()
            .filter(|profile| profile.active)
            .map(|profile| profile.id.as_str())
            .collect::<Vec<_>>(),
        vec!["panelist"]
    );

    service.delete("panelist").unwrap();
    assert!(config.load().unwrap().active_role_profile_id.is_none());
    assert_eq!(
        service.activate("missing").unwrap_err().code(),
        "ROLE_PROFILE_NOT_FOUND"
    );
    assert_eq!(
        service.delete("missing").unwrap_err().code(),
        "ROLE_PROFILE_NOT_FOUND"
    );
}

#[test]
fn role_legacy_profile_requires_review_before_activation_until_saved() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"roleProfiles":[{"id":"legacy","instructions":"Ask one question"}]}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let service = RoleProfileService::new(&config);

    let before = config.load().unwrap();
    let legacy = &before.role_profiles[0];
    assert_eq!(legacy.system_prompt, "Ask one question");
    assert_eq!(legacy.config_version, 0);
    assert!(!legacy.active);
    assert_eq!(
        service.activate("legacy").unwrap_err().code(),
        "ROLE_PROFILE_REVIEW_REQUIRED"
    );
    assert_eq!(config.load().unwrap(), before);

    let saved = service.save(role_input("legacy")).unwrap();
    assert_eq!(saved.config_version, 1);
    assert!(service.activate("legacy").unwrap().active);
}

#[test]
fn role_oversized_legacy_profile_stays_quarantined_without_activation_or_copy() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let instructions = "legacy content ".repeat(3_000);
    assert!(instructions.len() > 32 * 1024);
    std::fs::write(
        &path,
        serde_json::json!({
            "configVersion": 1,
            "roleProfiles": [{
                "id": "legacy-oversized",
                "instructions": instructions,
            }],
        })
        .to_string(),
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let service = RoleProfileService::new(&config);

    let before = config.load().unwrap();
    assert_eq!(before.role_profiles[0].config_version, 0);
    assert!(before.role_profiles[0].system_prompt.len() > 32 * 1024);
    assert_eq!(
        service.activate("legacy-oversized").unwrap_err().code(),
        "ROLE_PROFILE_REVIEW_REQUIRED"
    );
    assert_eq!(
        service
            .copy(RoleProfileCopyInput {
                source_id: "legacy-oversized".into(),
                id: Some("review-copy".into()),
            })
            .unwrap_err()
            .code(),
        "ROLE_PROFILE_REVIEW_REQUIRED"
    );
    assert_eq!(config.load().unwrap(), before);
}

struct FakeProbe;

impl ProviderProbe for FakeProbe {
    fn discover_models(
        &self,
        _: &ProviderEndpoint,
        credential: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        assert_eq!(credential, Some("credential-value"));
        Ok(vec![DiscoveredModel {
            id: "model-a".into(),
        }])
    }
}

#[test]
fn provider_save_keeps_secret_out_of_config_and_discovers_models() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    config.restore_defaults().unwrap();
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let service = ProviderService::new(&config, &secrets, &FakeProbe);

    let saved = service
        .save(ProviderSaveInput {
            web_capability: None,
            id: Some("openai".into()),
            name: Some("OpenAI compatible".into()),
            base_url: "https://example.test/v1".into(),
            api_key: Some("credential-value".into()),
        })
        .unwrap();
    assert_eq!(saved.id, "openai");
    assert!(saved.credential.unwrap().configured);
    assert!(
        !std::fs::read_to_string(directory.path().join("config.json"))
            .unwrap()
            .contains("credential-value")
    );

    let result = service.discover("openai").unwrap();
    assert_eq!(result.models[0].id, "model-a");
}

#[test]
fn provider_save_generates_uuid_when_id_is_omitted() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    config.restore_defaults().unwrap();
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let service = ProviderService::new(&config, &secrets, &FakeProbe);

    let saved = service
        .save(ProviderSaveInput {
            web_capability: None,
            id: None,
            name: Some("OpenAI compatible".into()),
            base_url: "https://example.test/v1".into(),
            api_key: Some("credential-value".into()),
        })
        .unwrap();
    assert!(uuid::Uuid::parse_str(&saved.id).is_ok());
    assert_eq!(saved.name.as_deref(), Some("OpenAI compatible"));
}

#[test]
fn provider_delete_rejects_referenced_provider() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"p1","baseUrl":"https://example.test"}]},"speech":{"voiceRoutes":[{"id":"r1","name":"route","mode":"e2e","e2eProviderId":"p1","e2eModelId":"m1"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let service = ProviderService::new(&config, &secrets, &FakeProbe);

    assert_eq!(service.delete("p1").unwrap_err().code(), "PROVIDER_IN_USE");
    let dependencies = service.dependencies("p1").unwrap();
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].id, "r1");
    assert_eq!(dependencies[0].name, "route");
    assert_eq!(dependencies[0].kind, "voiceRoute");
    config
        .update(|config| {
            config.speech.voice_routes.clear();
            Ok(())
        })
        .unwrap();
    assert!(service.dependencies("p1").unwrap().is_empty());
    service.delete("p1").unwrap();
    let reopened = ConfigStore::new(directory.path().join("config.json"));
    assert!(reopened.load().unwrap().models.providers.is_empty());
}

#[test]
fn provider_delete_sees_references_added_after_dependency_check() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"p1","baseUrl":"https://example.test"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let service = ProviderService::new(&config, &secrets, &FakeProbe);
    assert!(service.dependencies("p1").unwrap().is_empty());
    config
        .update(|config| {
            config
                .speech
                .voice_routes
                .push(crate::config::VoiceRouteConfig {
                    id: "r1".into(),
                    name: "route".into(),
                    mode: crate::config::VoiceRouteMode::E2e,
                    asr_provider_id: None,
                    asr_model_id: None,
                    llm_provider_id: None,
                    llm_model_id: None,
                    tts_provider_id: None,
                    tts_model_id: None,
                    voice_id: None,
                    e2e_provider_id: Some("p1".into()),
                    e2e_model_id: Some("m1".into()),
                    active: false,
                    ready: false,
                    status: None,
                    config_version: 1,
                });
            Ok(())
        })
        .unwrap();
    assert_eq!(service.delete("p1").unwrap_err().code(), "PROVIDER_IN_USE");
    assert_eq!(service.dependencies("p1").unwrap().len(), 1);
}

// The read-only attribute blocks the atomic replace only on Windows; a Unix rename
// replaces a read-only file because permissions live on the directory.
#[cfg(windows)]
#[test]
fn provider_delete_keeps_provider_when_config_write_fails() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"p1","baseUrl":"https://example.test"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path.clone());
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let service = ProviderService::new(&config, &secrets, &FakeProbe);
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).unwrap();
    let error = service.delete("p1").unwrap_err();
    #[allow(clippy::permissions_set_readonly_false)]
    {
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(false);
        std::fs::set_permissions(&path, permissions).unwrap();
    }
    assert_eq!(error.code(), "CONFIG_WRITE_FAILED");
    let reopened = ConfigStore::new(path);
    assert_eq!(reopened.load().unwrap().models.providers.len(), 1);
}

#[test]
fn provider_delete_rejects_provider_referenced_only_by_embedding_config() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{
            "configVersion":1,
            "models":{"providers":[{"id":"p1","baseUrl":"https://example.test"}]},
            "knowledge":{"embeddingConfigs":[{
                "id":"embedding-1","providerId":"p1","modelId":"embed-1",
                "dimensions":1536,"distance":"cosine","normalized":true,
                "active":false,"ready":false,"status":null,"configVersion":1
            }]}
        }"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let service = ProviderService::new(&config, &secrets, &FakeProbe);

    assert_eq!(service.delete("p1").unwrap_err().code(), "PROVIDER_IN_USE");
    let dependencies = service.dependencies("p1").unwrap();
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].id, "embedding-1");
    assert_eq!(dependencies[0].kind, "embedding");
}

#[test]
fn provider_cleanup_can_retry_after_config_is_already_deleted() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    let config = ConfigStore::new(path.clone());
    let backend = Arc::new(ScriptedSecretStore::new());
    let secrets = SecretService::new("test", backend.clone()).unwrap();
    let service = ProviderService::new(&config, &secrets, &FakeProbe);
    service
        .save(ProviderSaveInput {
            id: Some("p1".into()),
            name: Some("test".into()),
            base_url: "https://example.test".into(),
            api_key: Some("synthetic-secret".into()),
            web_capability: None,
        })
        .unwrap();
    *backend.fail_delete_suffix.lock().unwrap() = Some("api-key".into());
    assert_eq!(
        service.delete("p1").unwrap_err().code(),
        "SECRET_CLEANUP_FAILED"
    );
    assert!(
        ConfigStore::new(path)
            .load()
            .unwrap()
            .models
            .providers
            .is_empty()
    );
    *backend.fail_delete_suffix.lock().unwrap() = None;
    service.delete("p1").unwrap();
    assert!(!secrets.status("providers/p1/api-key").unwrap().configured);
}

struct OpenProbe;

impl ProviderProbe for OpenProbe {
    fn discover_models(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        Ok(["model-a", "asr-1", "llm-1", "tts-1", "realtime"]
            .into_iter()
            .map(|id| DiscoveredModel { id: id.into() })
            .collect())
    }
}

/// 线路阶段探测桩：realtime/tts 各自可配置成败，并记录调用次数以断言 test() 真正发起了探测。
struct StageProbeStub {
    realtime_allowed: bool,
    tts_allowed: bool,
    realtime_calls: AtomicU32,
    tts_calls: AtomicU32,
}

impl StageProbeStub {
    fn open() -> Self {
        Self {
            realtime_allowed: true,
            tts_allowed: true,
            realtime_calls: AtomicU32::new(0),
            tts_calls: AtomicU32::new(0),
        }
    }

    fn denied() -> Self {
        Self {
            realtime_allowed: false,
            tts_allowed: false,
            realtime_calls: AtomicU32::new(0),
            tts_calls: AtomicU32::new(0),
        }
    }
}

fn probe_denied() -> RouteProbeError {
    RouteProbeError {
        code: "downstream_reconnect_exceeded".into(),
        message: "实时语音服务拒绝了本次请求，请检查模型配置或稍后重试。".into(),
    }
}

impl RouteStageProbe for StageProbeStub {
    fn probe_realtime_session(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
    ) -> Result<(), RouteProbeError> {
        self.realtime_calls.fetch_add(1, Ordering::SeqCst);
        if self.realtime_allowed {
            Ok(())
        } else {
            Err(probe_denied())
        }
    }

    fn probe_tts(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: Option<&str>,
    ) -> Result<(), RouteProbeError> {
        self.tts_calls.fetch_add(1, Ordering::SeqCst);
        if self.tts_allowed {
            Ok(())
        } else {
            Err(RouteProbeError {
                code: "TTS_RATE_LIMITED".into(),
                message: "语音合成测试未通过，请确认账号已开通语音合成并有余量。".into(),
            })
        }
    }
}

struct FailProbe;

impl ProviderProbe for FailProbe {
    fn discover_models(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        Err(ProviderError::Timeout)
    }
}

struct NoAuthProbe;

impl ProviderProbe for NoAuthProbe {
    fn discover_models(
        &self,
        _: &ProviderEndpoint,
        credential: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        assert!(credential.is_none());
        Ok(vec![DiscoveredModel { id: "local".into() }])
    }
}

#[test]
fn provider_with_unconfigured_credential_slot_supports_no_auth_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, r#"{"configVersion":1,"models":{"providers":[{"id":"local","baseUrl":"http://127.0.0.1:11434/v1","credential":{"reference":"providers/local/api-key","configured":false}}]}}"#).unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let service = ProviderService::new(&config, &secrets, &NoAuthProbe);

    assert_eq!(service.discover("local").unwrap().models[0].id, "local");
}

#[test]
fn voice_route_requires_test_before_single_activation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"asr","baseUrl":"https://asr.test/v1"},{"id":"llm","baseUrl":"https://llm.test/v1"},{"id":"tts","baseUrl":"https://tts.test/v1"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::open();
    let service = super::VoiceRouteService::new(&config, &secrets, &OpenProbe, &stage);
    let route = service
        .save(super::VoiceRouteSaveInput {
            id: Some("default".into()),
            name: "Default".into(),
            mode: crate::config::VoiceRouteMode::Cascaded,
            asr_provider_id: Some("asr".into()),
            asr_model_id: Some("asr-1".into()),
            llm_provider_id: Some("llm".into()),
            llm_model_id: Some("llm-1".into()),
            tts_provider_id: Some("tts".into()),
            tts_model_id: Some("tts-1".into()),
            voice_id: None,
            e2e_provider_id: None,
            e2e_model_id: None,
        })
        .unwrap();
    assert!(!route.ready);
    assert_eq!(
        service.activate("default").unwrap_err().code(),
        "VOICE_ROUTE_NOT_READY"
    );

    let tested = service.test("default").unwrap();
    assert!(tested.ready);
    assert_eq!(tested.checked_provider_ids, vec!["asr", "llm", "tts"]);
    let active = service.activate("default").unwrap();
    assert!(active.active);
    assert_eq!(
        config
            .load()
            .unwrap()
            .speech
            .active_voice_route_id
            .as_deref(),
        Some("default")
    );
}

#[test]
fn voice_route_save_generates_uuid_when_id_is_omitted() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"e2e","baseUrl":"https://e2e.test/v1"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::open();
    let service = super::VoiceRouteService::new(&config, &secrets, &OpenProbe, &stage);
    let route = service
        .save(super::VoiceRouteSaveInput {
            id: None,
            name: "Generated".into(),
            mode: crate::config::VoiceRouteMode::E2e,
            asr_provider_id: None,
            asr_model_id: None,
            llm_provider_id: None,
            llm_model_id: None,
            tts_provider_id: None,
            tts_model_id: None,
            voice_id: None,
            e2e_provider_id: Some("e2e".into()),
            e2e_model_id: Some("realtime".into()),
        })
        .unwrap();
    assert!(uuid::Uuid::parse_str(&route.id).is_ok());
    assert_eq!(route.name, "Generated");
}

#[test]
fn e2e_route_rejects_cascaded_fields() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    config.restore_defaults().unwrap();
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::open();
    let service = super::VoiceRouteService::new(&config, &secrets, &OpenProbe, &stage);
    let error = service
        .save(super::VoiceRouteSaveInput {
            id: Some("bad".into()),
            name: "Bad".into(),
            mode: crate::config::VoiceRouteMode::E2e,
            asr_provider_id: Some("asr".into()),
            asr_model_id: Some("asr-1".into()),
            llm_provider_id: None,
            llm_model_id: None,
            tts_provider_id: None,
            tts_model_id: None,
            voice_id: None,
            e2e_provider_id: Some("e2e".into()),
            e2e_model_id: Some("realtime".into()),
        })
        .unwrap_err();
    assert_eq!(error.code(), "VOICE_ROUTE_FIELDS_INVALID");
}

#[test]
fn failed_retest_deactivates_and_unreadies_route() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(&path, r#"{"configVersion":1,"models":{"providers":[{"id":"e2e","baseUrl":"https://e2e.test/v1"}]},"speech":{"voiceRoutes":[{"id":"route","name":"Route","mode":"e2e","e2eProviderId":"e2e","e2eModelId":"realtime","active":true,"ready":true,"status":"ready","configVersion":1}],"activeVoiceRouteId":"route"}}"#).unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::denied();
    let service = super::VoiceRouteService::new(&config, &secrets, &FailProbe, &stage);
    assert_eq!(
        service.test("route").unwrap_err().code(),
        "PROVIDER_TIMEOUT"
    );
    let loaded = config.load().unwrap();
    assert!(loaded.speech.active_voice_route_id.is_none());
    assert!(!loaded.speech.voice_routes[0].active);
    assert!(!loaded.speech.voice_routes[0].ready);
}

#[test]
fn voice_route_test_rejects_a_model_missing_from_provider_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"e2e","baseUrl":"https://e2e.test/v1"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::open();
    let service = super::VoiceRouteService::new(&config, &secrets, &OpenProbe, &stage);
    service
        .save(super::VoiceRouteSaveInput {
            id: Some("missing-model".into()),
            name: "Missing model".into(),
            mode: crate::config::VoiceRouteMode::E2e,
            asr_provider_id: None,
            asr_model_id: None,
            llm_provider_id: None,
            llm_model_id: None,
            tts_provider_id: None,
            tts_model_id: None,
            voice_id: None,
            e2e_provider_id: Some("e2e".into()),
            e2e_model_id: Some("does-not-exist".into()),
        })
        .unwrap();

    assert_eq!(
        service.test("missing-model").unwrap_err().code(),
        "VOICE_ROUTE_MODEL_NOT_FOUND"
    );
    let route = &config.load().unwrap().speech.voice_routes[0];
    assert!(!route.ready);
    assert_eq!(route.status.as_deref(), Some("test_failed"));
}

#[test]
fn e2e_route_test_fails_when_realtime_handshake_is_denied() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"e2e","baseUrl":"https://e2e.test/v1"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::denied();
    let service = super::VoiceRouteService::new(&config, &secrets, &OpenProbe, &stage);
    service
        .save(super::VoiceRouteSaveInput {
            id: Some("voice".into()),
            name: "Voice".into(),
            mode: crate::config::VoiceRouteMode::E2e,
            asr_provider_id: None,
            asr_model_id: None,
            llm_provider_id: None,
            llm_model_id: None,
            tts_provider_id: None,
            tts_model_id: None,
            voice_id: None,
            e2e_provider_id: Some("e2e".into()),
            e2e_model_id: Some("realtime".into()),
        })
        .unwrap();

    // 模型在目录里也必须真实握手：账号无实时语音权限时测试必须失败。
    let error = service.test("voice").unwrap_err();
    assert_eq!(error.code(), "VOICE_ROUTE_PROBE_FAILED");
    assert_eq!(stage.realtime_calls.load(Ordering::SeqCst), 1);
    let route = &config.load().unwrap().speech.voice_routes[0];
    assert!(!route.ready);
    assert_eq!(route.status.as_deref(), Some("test_failed"));
}

#[test]
fn e2e_route_test_passes_when_realtime_handshake_succeeds() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"e2e","baseUrl":"https://e2e.test/v1"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::open();
    let service = super::VoiceRouteService::new(&config, &secrets, &OpenProbe, &stage);
    service
        .save(super::VoiceRouteSaveInput {
            id: Some("voice".into()),
            name: "Voice".into(),
            mode: crate::config::VoiceRouteMode::E2e,
            asr_provider_id: None,
            asr_model_id: None,
            llm_provider_id: None,
            llm_model_id: None,
            tts_provider_id: None,
            tts_model_id: None,
            voice_id: None,
            e2e_provider_id: Some("e2e".into()),
            e2e_model_id: Some("realtime".into()),
        })
        .unwrap();

    let tested = service.test("voice").unwrap();
    assert!(tested.ready);
    assert_eq!(stage.realtime_calls.load(Ordering::SeqCst), 1);
    assert_eq!(stage.tts_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cascaded_route_test_fails_when_tts_probe_is_denied() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"models":{"providers":[{"id":"asr","baseUrl":"https://asr.test/v1"},{"id":"llm","baseUrl":"https://llm.test/v1"},{"id":"tts","baseUrl":"https://tts.test/v1"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::denied();
    let service = super::VoiceRouteService::new(&config, &secrets, &OpenProbe, &stage);
    service
        .save(super::VoiceRouteSaveInput {
            id: Some("cascade".into()),
            name: "Cascade".into(),
            mode: crate::config::VoiceRouteMode::Cascaded,
            asr_provider_id: Some("asr".into()),
            asr_model_id: Some("asr-1".into()),
            llm_provider_id: Some("llm".into()),
            llm_model_id: Some("llm-1".into()),
            tts_provider_id: Some("tts".into()),
            tts_model_id: Some("tts-1".into()),
            voice_id: None,
            e2e_provider_id: None,
            e2e_model_id: None,
        })
        .unwrap();

    let error = service.test("cascade").unwrap_err();
    assert_eq!(error.code(), "VOICE_ROUTE_PROBE_FAILED");
    assert_eq!(stage.tts_calls.load(Ordering::SeqCst), 1);
    assert_eq!(stage.realtime_calls.load(Ordering::SeqCst), 0);
    let route = &config.load().unwrap().speech.voice_routes[0];
    assert!(!route.ready);
}

#[test]
fn incomplete_legacy_route_cannot_be_tested_or_activated() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"configVersion":1,"speech":{"voiceRoutes":[{"id":"legacy","name":"Legacy","mode":"cascaded"}]}}"#,
    )
    .unwrap();
    let config = ConfigStore::new(path);
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let stage = StageProbeStub::open();
    let service = super::VoiceRouteService::new(&config, &secrets, &OpenProbe, &stage);

    assert_eq!(
        service.test("legacy").unwrap_err().code(),
        "VOICE_ROUTE_FIELDS_INVALID"
    );
    assert_eq!(
        service.activate("legacy").unwrap_err().code(),
        "VOICE_ROUTE_NOT_READY"
    );
}

struct ReadyEmbeddingProbe;

impl EmbeddingProbe for ReadyEmbeddingProbe {
    fn embed(
        &self,
        _: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        dimensions: u32,
        input: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        assert_eq!(credential, Some("credential-value"));
        assert_eq!(model_id, "embed-3");
        assert_eq!(input, "AI Virtual Assistant embedding connectivity test");
        Ok(vec![0.25; dimensions as usize])
    }
}

struct FailEmbeddingProbe;

impl EmbeddingProbe for FailEmbeddingProbe {
    fn embed(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: u32,
        _: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        Err(EmbeddingError::Timeout)
    }
}

struct WrongDimensionProbe;

impl EmbeddingProbe for WrongDimensionProbe {
    fn embed(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: u32,
        _: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        Ok(vec![1.0, 2.0])
    }
}

fn embedding_input(id: &str) -> EmbeddingConfigSaveInput {
    EmbeddingConfigSaveInput {
        id: Some(id.into()),
        provider_id: "openai".into(),
        base_url: None,
        api_key: None,
        model_id: "embed-3".into(),
        dimensions: 3,
        normalized: true,
    }
}

fn seeded_embedding_store() -> (tempfile::TempDir, ConfigStore, SecretService) {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    config.restore_defaults().unwrap();
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let providers = ProviderService::new(&config, &secrets, &FakeProbe);
    providers
        .save(ProviderSaveInput {
            web_capability: None,
            id: Some("openai".into()),
            name: Some("OpenAI compatible".into()),
            base_url: "https://example.test/v1".into(),
            api_key: Some("credential-value".into()),
        })
        .unwrap();
    (directory, config, secrets)
}

#[test]
fn embedding_save_validates_provider_model_and_dimensions_and_resets_readiness() {
    let (_directory, config, secrets) = seeded_embedding_store();
    let service = EmbeddingService::new(&config, &secrets, &ReadyEmbeddingProbe);

    assert_eq!(
        service
            .save(EmbeddingConfigSaveInput {
                provider_id: "missing".into(),
                ..embedding_input("primary")
            })
            .unwrap_err()
            .code(),
        "CONFIG_REFERENCE_MISSING"
    );
    assert_eq!(
        service
            .save(EmbeddingConfigSaveInput {
                model_id: " ".into(),
                ..embedding_input("primary")
            })
            .unwrap_err()
            .code(),
        "EMBEDDING_FIELDS_INVALID"
    );
    assert_eq!(
        service
            .save(EmbeddingConfigSaveInput {
                dimensions: 0,
                ..embedding_input("primary")
            })
            .unwrap_err()
            .code(),
        "EMBEDDING_FIELDS_INVALID"
    );

    let saved = service.save(embedding_input("primary")).unwrap();
    assert_eq!(saved.config_version, 1);
    assert!(!saved.ready);
    assert!(!saved.active);
    assert_eq!(saved.status.as_deref(), Some("not_tested"));
    assert_eq!(saved.distance, EmbeddingDistance::Cosine);

    let tested = service.test("primary").unwrap();
    assert!(tested.ready);
    assert!(service.activate("primary").unwrap().active);

    let edited = service
        .save(EmbeddingConfigSaveInput {
            dimensions: 8,
            ..embedding_input("primary")
        })
        .unwrap();
    assert_eq!(edited.config_version, 2);
    assert!(!edited.ready);
    assert!(!edited.active);
    assert!(
        config
            .load()
            .unwrap()
            .knowledge
            .active_embedding_config_id
            .is_none()
    );
}

#[test]
fn embedding_save_generates_uuid_when_id_is_omitted() {
    let (_directory, config, secrets) = seeded_embedding_store();
    let service = EmbeddingService::new(&config, &secrets, &ReadyEmbeddingProbe);
    let saved = service
        .save(EmbeddingConfigSaveInput {
            id: None,
            ..embedding_input("unused")
        })
        .unwrap();
    assert!(uuid::Uuid::parse_str(&saved.id).is_ok());
    assert_eq!(saved.model_id, "embed-3");
}

#[test]
fn embedding_save_accepts_custom_url_without_provider() {
    let directory = tempfile::tempdir().unwrap();
    let config = ConfigStore::new(directory.path().join("config.json"));
    config.restore_defaults().unwrap();
    let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
    let calls = std::sync::Mutex::new(Vec::<(String, Option<String>)>::new());
    struct CaptureProbe<'a>(&'a std::sync::Mutex<Vec<(String, Option<String>)>>);
    impl EmbeddingProbe for CaptureProbe<'_> {
        fn embed(
            &self,
            endpoint: &crate::providers::ProviderEndpoint,
            credential: Option<&str>,
            _: &str,
            dimensions: u32,
            _: &str,
        ) -> Result<Vec<f32>, EmbeddingError> {
            self.0
                .lock()
                .unwrap()
                .push((endpoint.base_url.clone(), credential.map(str::to_owned)));
            Ok(vec![0.1; dimensions as usize])
        }
    }
    let probe = CaptureProbe(&calls);
    let service = EmbeddingService::new(&config, &secrets, &probe);
    assert_eq!(
        service
            .save(EmbeddingConfigSaveInput {
                provider_id: String::new(),
                base_url: None,
                ..embedding_input("primary")
            })
            .unwrap_err()
            .code(),
        "EMBEDDING_SOURCE_INVALID"
    );
    let saved = service
        .save(EmbeddingConfigSaveInput {
            provider_id: String::new(),
            base_url: Some("http://127.0.0.1:8080/v1".into()),
            api_key: Some("custom-key".into()),
            ..embedding_input("primary")
        })
        .unwrap();
    assert!(saved.provider_id.is_empty());
    assert_eq!(saved.base_url.as_deref(), Some("http://127.0.0.1:8080/v1"));
    assert!(
        saved
            .credential
            .as_ref()
            .is_some_and(|slot| slot.configured)
    );
    service.test("primary").unwrap();
    let captured = calls.lock().unwrap();
    assert_eq!(captured[0].0, "http://127.0.0.1:8080/v1");
    assert_eq!(captured[0].1.as_deref(), Some("custom-key"));
}

#[test]
fn embedding_test_reads_internal_credential_and_requires_exact_dimension() {
    let (_directory, config, secrets) = seeded_embedding_store();
    let service = EmbeddingService::new(&config, &secrets, &ReadyEmbeddingProbe);
    service.save(embedding_input("primary")).unwrap();

    let tested = service.test("primary").unwrap();
    assert!(tested.ready);
    assert_eq!(tested.dimensions, 3);

    let mismatch = EmbeddingService::new(&config, &secrets, &WrongDimensionProbe);
    assert_eq!(
        mismatch.test("primary").unwrap_err().code(),
        "EMBEDDING_DIMENSION_MISMATCH"
    );
    let loaded = config.load().unwrap();
    assert!(!loaded.knowledge.embedding_configs[0].ready);
    assert_eq!(
        loaded.knowledge.embedding_configs[0].status.as_deref(),
        Some("test_failed")
    );
}

#[test]
fn embedding_activation_requires_test_and_keeps_a_single_active_config() {
    let (_directory, config, secrets) = seeded_embedding_store();
    let service = EmbeddingService::new(&config, &secrets, &ReadyEmbeddingProbe);
    service.save(embedding_input("primary")).unwrap();
    service.save(embedding_input("secondary")).unwrap();

    assert_eq!(
        service.activate("primary").unwrap_err().code(),
        "EMBEDDING_NOT_READY"
    );
    service.test("primary").unwrap();
    service.test("secondary").unwrap();
    assert!(service.activate("primary").unwrap().active);
    assert!(service.activate("secondary").unwrap().active);

    let loaded = config.load().unwrap();
    assert_eq!(
        loaded.knowledge.active_embedding_config_id.as_deref(),
        Some("secondary")
    );
    assert!(!loaded.knowledge.embedding_configs[0].active);
    assert!(loaded.knowledge.embedding_configs[1].active);
}

struct VersionBumpProbe {
    path: std::path::PathBuf,
}

impl EmbeddingProbe for VersionBumpProbe {
    fn embed(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: u32,
        _: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        let mut current: crate::config::AppConfigV1 =
            serde_json::from_str(&std::fs::read_to_string(&self.path).unwrap()).unwrap();
        current.knowledge.embedding_configs[0].config_version += 1;
        std::fs::write(&self.path, serde_json::to_string(&current).unwrap()).unwrap();
        Ok(vec![0.25, 0.25, 0.25])
    }
}

#[test]
fn failed_retest_deactivates_embedding_and_stale_test_is_rejected() {
    let (directory, config, secrets) = seeded_embedding_store();
    let service = EmbeddingService::new(&config, &secrets, &ReadyEmbeddingProbe);
    service.save(embedding_input("primary")).unwrap();
    service.test("primary").unwrap();
    service.activate("primary").unwrap();

    let failed = EmbeddingService::new(&config, &secrets, &FailEmbeddingProbe);
    assert_eq!(
        failed.test("primary").unwrap_err().code(),
        "EMBEDDING_TIMEOUT"
    );
    let loaded = config.load().unwrap();
    assert!(loaded.knowledge.active_embedding_config_id.is_none());
    assert!(!loaded.knowledge.embedding_configs[0].active);
    assert!(!loaded.knowledge.embedding_configs[0].ready);

    service.save(embedding_input("primary")).unwrap();
    let stale_probe = VersionBumpProbe {
        path: directory.path().join("config.json"),
    };
    let stale = EmbeddingService::new(&config, &secrets, &stale_probe);
    assert_eq!(stale.test("primary").unwrap_err().code(), "EMBEDDING_STALE");
    assert!(!config.load().unwrap().knowledge.embedding_configs[0].ready);
}

#[test]
fn embedding_delete_clears_active_id_and_provider_edit_invalidates_references() {
    let (_directory, config, secrets) = seeded_embedding_store();
    let service = EmbeddingService::new(&config, &secrets, &ReadyEmbeddingProbe);
    service.save(embedding_input("primary")).unwrap();
    service.test("primary").unwrap();
    service.activate("primary").unwrap();

    service.delete("primary").unwrap();
    let after_delete = config.load().unwrap();
    assert!(after_delete.knowledge.embedding_configs.is_empty());
    assert!(after_delete.knowledge.active_embedding_config_id.is_none());

    service.save(embedding_input("primary")).unwrap();
    service.test("primary").unwrap();
    service.activate("primary").unwrap();
    ProviderService::new(&config, &secrets, &FakeProbe)
        .save(ProviderSaveInput {
            web_capability: None,
            id: Some("openai".into()),
            name: Some("Renamed".into()),
            base_url: "https://example.test/v2".into(),
            api_key: None,
        })
        .unwrap();
    let loaded = config.load().unwrap();
    assert!(!loaded.knowledge.embedding_configs[0].ready);
    assert!(!loaded.knowledge.embedding_configs[0].active);
    assert_eq!(
        loaded.knowledge.embedding_configs[0].status.as_deref(),
        Some("configuration_changed")
    );
    assert!(loaded.knowledge.active_embedding_config_id.is_none());
}

struct ScriptedSecretStore {
    inner: MemorySecretStore,
    fail_set_suffix: std::sync::Mutex<Option<String>>,
    fail_delete_suffix: std::sync::Mutex<Option<String>>,
    reads: std::sync::Mutex<u32>,
}

impl ScriptedSecretStore {
    fn new() -> Self {
        Self {
            inner: MemorySecretStore::default(),
            fail_set_suffix: std::sync::Mutex::new(None),
            fail_delete_suffix: std::sync::Mutex::new(None),
            reads: std::sync::Mutex::new(0),
        }
    }
}

impl SecretStore for ScriptedSecretStore {
    fn set(&self, reference: &str, value: &str) -> Result<(), SecretError> {
        if self
            .fail_set_suffix
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|suffix| reference.ends_with(suffix))
        {
            return Err(SecretError::Backend);
        }
        self.inner.set(reference, value)
    }

    fn get(&self, reference: &str) -> Result<Option<zeroize::Zeroizing<String>>, SecretError> {
        *self.reads.lock().unwrap() += 1;
        self.inner.get(reference)
    }

    fn delete(&self, reference: &str) -> Result<bool, SecretError> {
        if self
            .fail_delete_suffix
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|suffix| reference.ends_with(suffix))
        {
            return Err(SecretError::Backend);
        }
        self.inner.delete(reference)
    }

    fn contains(&self, reference: &str) -> Result<bool, SecretError> {
        self.inner.contains(reference)
    }
}

mod voice_references {
    use std::sync::{Arc, Mutex};

    use base64::Engine as _;

    use crate::{
        config::ConfigStore,
        database::Database,
        providers::{ProviderEndpoint, ProviderError},
        secrets::{MemorySecretStore, SecretService},
        services::{
            ProviderSaveInput, ProviderService, VoiceCloneError, VoiceCloneGateway,
            VoiceReferenceSaveInput, VoiceReferenceService,
        },
    };

    struct Environment {
        directory: tempfile::TempDir,
        database: Database,
        config: ConfigStore,
        secrets: SecretService,
    }

    fn environment() -> Environment {
        let directory = tempfile::tempdir().unwrap();
        let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
        database.migrate().unwrap();
        let config = ConfigStore::new(directory.path().join("config.json"));
        config.restore_defaults().unwrap();
        let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
        Environment {
            directory,
            database,
            config,
            secrets,
        }
    }

    fn wav_bytes(sample_rate: u32, channels: u16, bits: u16, data_len: usize) -> Vec<u8> {
        let byte_rate = sample_rate * u32::from(channels) * u32::from(bits) / 8;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&byte_rate.to_le_bytes());
        bytes.extend_from_slice(&((channels * bits) / 8).to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        bytes.extend(std::iter::repeat_n(0_u8, data_len));
        bytes
    }

    fn save_input(
        id: Option<&str>,
        name: &str,
        provider: Option<&str>,
        path: &str,
    ) -> VoiceReferenceSaveInput {
        VoiceReferenceSaveInput {
            id: id.map(str::to_owned),
            name: name.to_owned(),
            provider_id: provider.map(str::to_owned),
            target_model: None,
            transcript: Some(" 你好世界 ".into()),
            audio_path: path.to_owned(),
        }
    }

    fn service<'a>(
        environment: &'a Environment,
        gateway: &'a dyn VoiceCloneGateway,
    ) -> VoiceReferenceService<'a> {
        VoiceReferenceService::new(
            &environment.database,
            &environment.config,
            &environment.secrets,
            gateway,
        )
    }

    #[derive(Clone)]
    struct CapturedCloneReference {
        endpoint: ProviderEndpoint,
        credential: Option<String>,
        voice_name: String,
        transcript: String,
        target_model: Option<String>,
        file_name: String,
        mime_type: String,
        bytes_len: usize,
    }

    struct OkGateway {
        outcome_voice_id: &'static str,
        outcome_remote_file_id: Option<&'static str>,
        clone: Mutex<Option<CapturedCloneReference>>,
    }

    impl OkGateway {
        fn new() -> Self {
            Self {
                outcome_voice_id: "voice_clone_9",
                outcome_remote_file_id: Some("file_remote_1"),
                clone: Mutex::new(None),
            }
        }

        fn qwen() -> Self {
            Self {
                outcome_voice_id: "qwen-omni-vc-roleai-1",
                outcome_remote_file_id: None,
                clone: Mutex::new(None),
            }
        }
    }

    impl VoiceCloneGateway for OkGateway {
        fn clone_reference(
            &self,
            endpoint: &ProviderEndpoint,
            credential: Option<&str>,
            voice_name: &str,
            transcript: &str,
            target_model: Option<&str>,
            file_name: &str,
            mime_type: &str,
            bytes: Vec<u8>,
        ) -> Result<crate::providers::VoiceCloneOutcome, VoiceCloneError> {
            *self.clone.lock().unwrap() = Some(CapturedCloneReference {
                endpoint: endpoint.clone(),
                credential: credential.map(str::to_owned),
                voice_name: voice_name.to_owned(),
                transcript: transcript.to_owned(),
                target_model: target_model.map(str::to_owned),
                file_name: file_name.to_owned(),
                mime_type: mime_type.to_owned(),
                bytes_len: bytes.len(),
            });
            Ok(crate::providers::VoiceCloneOutcome {
                voice_id: self.outcome_voice_id.to_owned(),
                remote_file_id: self.outcome_remote_file_id.map(str::to_owned),
            })
        }
    }

    struct ErrGateway;

    impl VoiceCloneGateway for ErrGateway {
        fn clone_reference(
            &self,
            _endpoint: &ProviderEndpoint,
            _credential: Option<&str>,
            _voice_name: &str,
            _transcript: &str,
            _target_model: Option<&str>,
            _file_name: &str,
            _mime_type: &str,
            _bytes: Vec<u8>,
        ) -> Result<crate::providers::VoiceCloneOutcome, VoiceCloneError> {
            Err(VoiceCloneError {
                kind: ProviderError::Unauthorized,
                provider_message: None,
            })
        }
    }

    fn provider_with_credential(environment: &Environment) {
        ProviderService::new(&environment.config, &environment.secrets, &super::FakeProbe)
            .save(ProviderSaveInput {
                web_capability: None,
                id: Some("bigmodel".into()),
                name: Some("BigModel".into()),
                base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
                api_key: Some("credential-value".into()),
            })
            .unwrap();
    }

    #[test]
    fn save_parses_wav_duration_and_summary_excludes_audio_bytes() {
        let environment = environment();
        let file = environment.directory.path().join("sample.wav");
        std::fs::write(&file, wav_bytes(16_000, 1, 16, 32_000)).unwrap();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);

        let summary = service
            .save(save_input(
                None,
                " 我的音色 ",
                Some("bigmodel"),
                &file.to_string_lossy(),
            ))
            .unwrap();

        assert_eq!(summary.name, "我的音色");
        assert_eq!(summary.mime_type, "audio/wav");
        assert_eq!(summary.byte_size, 44 + 32_000);
        assert_eq!(summary.duration_ms, Some(1000));
        assert_eq!(summary.transcript, "你好世界");
        assert_eq!(summary.provider_id.as_deref(), Some("bigmodel"));
        assert_eq!(summary.clone_status, "pending");
        assert!(summary.voice_id.is_none());
        let payload = serde_json::to_value(&summary).unwrap();
        assert!(payload.get("audio").is_none());
        assert!(service.list().unwrap().len() == 1);
    }

    #[test]
    fn save_rejects_bad_extension_relative_paths_and_oversize_audio() {
        let environment = environment();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);

        let text_file = environment.directory.path().join("note.txt");
        std::fs::write(&text_file, b"hello").unwrap();
        let error = service
            .save(save_input(None, "a", None, &text_file.to_string_lossy()))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_AUDIO_INVALID");

        let error = service
            .save(save_input(None, "a", None, "relative/sample.wav"))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_AUDIO_INVALID");

        let big = environment.directory.path().join("big.wav");
        std::fs::write(&big, vec![0_u8; 10 * 1024 * 1024 + 1]).unwrap();
        let error = service
            .save(save_input(None, "a", None, &big.to_string_lossy()))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_AUDIO_TOO_LARGE");
    }

    #[test]
    fn save_updates_existing_row_and_resets_clone_state() {
        let environment = environment();
        let file = environment.directory.path().join("sample.wav");
        std::fs::write(&file, wav_bytes(16_000, 1, 16, 32_000)).unwrap();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);
        let saved = service
            .save(save_input(
                Some("voice-1"),
                "音色",
                Some("bigmodel"),
                &file.to_string_lossy(),
            ))
            .unwrap();

        environment
            .database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE voice_references SET voice_id='voice_old', clone_status='cloned', remote_file_id='file_old' WHERE id='voice-1'",
                    [],
                )
                .map(|_| ())
            })
            .unwrap();

        let updated = service
            .save(save_input(
                Some("voice-1"),
                "音色",
                Some("bigmodel"),
                &file.to_string_lossy(),
            ))
            .unwrap();
        assert_eq!(updated.id, saved.id);
        assert_eq!(updated.clone_status, "pending");
        assert!(updated.voice_id.is_none());
    }

    #[test]
    fn delete_removes_reference_and_reports_missing() {
        let environment = environment();
        let file = environment.directory.path().join("sample.mp3");
        std::fs::write(&file, b"mp3-bytes").unwrap();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);
        let saved = service
            .save(save_input(None, "mp3", None, &file.to_string_lossy()))
            .unwrap();
        assert_eq!(saved.mime_type, "audio/mpeg");
        assert_eq!(saved.duration_ms, None);

        service.delete(&saved.id).unwrap();
        assert!(service.list().unwrap().is_empty());
        let error = service.delete(&saved.id).unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_NOT_FOUND");
    }

    #[test]
    fn clone_passes_context_to_gateway_and_persists_outcome() {
        let environment = environment();
        provider_with_credential(&environment);
        let file = environment.directory.path().join("sample.wav");
        std::fs::write(&file, wav_bytes(16_000, 1, 16, 32_000)).unwrap();
        let gateway = OkGateway::new();
        let service = service(&environment, &gateway);
        let saved = service
            .save(save_input(
                None,
                " 音色 ",
                Some("bigmodel"),
                &file.to_string_lossy(),
            ))
            .unwrap();

        let result = service.clone_voice(&saved.id).unwrap();

        assert_eq!(result.voice_id, "voice_clone_9");
        assert_eq!(result.remote_file_id, "file_remote_1");
        let clone = gateway.clone.lock().unwrap().clone().unwrap();
        assert_eq!(
            clone.endpoint.base_url,
            "https://open.bigmodel.cn/api/paas/v4"
        );
        assert_eq!(clone.credential.as_deref(), Some("credential-value"));
        assert_eq!(clone.mime_type, "audio/wav");
        assert_eq!(clone.target_model, None);
        assert!(clone.file_name.ends_with(".wav"));
        assert_eq!(clone.bytes_len, 44 + 32_000);
        assert_eq!(clone.transcript, "你好世界");
        assert!(clone.voice_name.starts_with("roleai_"));
        assert!(clone.voice_name.len() <= 30);
        let listed = service.list().unwrap().remove(0);
        assert_eq!(listed.clone_status, "cloned");
        assert_eq!(listed.voice_id.as_deref(), Some("voice_clone_9"));
        assert_eq!(listed.remote_file_id.as_deref(), Some("file_remote_1"));
    }

    #[test]
    fn clone_with_qwen_target_model_persists_voice_without_remote_file() {
        let environment = environment();
        provider_with_credential(&environment);
        let file = environment.directory.path().join("sample.wav");
        std::fs::write(&file, wav_bytes(16_000, 1, 16, 32_000)).unwrap();
        let gateway = OkGateway::qwen();
        let service = service(&environment, &gateway);
        let mut input = save_input(None, "音色", Some("bigmodel"), &file.to_string_lossy());
        input.target_model = Some(" qwen3.8-omni-flash-realtime ".into());
        let saved = service.save(input).unwrap();

        let result = service.clone_voice(&saved.id).unwrap();

        assert_eq!(result.voice_id, "qwen-omni-vc-roleai-1");
        assert_eq!(result.remote_file_id, "");
        let clone = gateway.clone.lock().unwrap().clone().unwrap();
        assert_eq!(
            clone.target_model.as_deref(),
            Some("qwen3.8-omni-flash-realtime")
        );
        let listed = service.list().unwrap().remove(0);
        assert_eq!(listed.clone_status, "cloned");
        assert_eq!(listed.voice_id.as_deref(), Some("qwen-omni-vc-roleai-1"));
        assert!(listed.remote_file_id.is_none());
        assert_eq!(
            listed.target_model.as_deref(),
            Some("qwen3.8-omni-flash-realtime")
        );
    }

    #[test]
    fn clone_failure_marks_reference_failed_with_stable_code() {
        let environment = environment();
        provider_with_credential(&environment);
        let file = environment.directory.path().join("sample.wav");
        std::fs::write(&file, wav_bytes(16_000, 1, 16, 32_000)).unwrap();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);
        let saved = service
            .save(save_input(
                None,
                "音色",
                Some("bigmodel"),
                &file.to_string_lossy(),
            ))
            .unwrap();

        let error = service.clone_voice(&saved.id).unwrap_err();

        assert_eq!(error.code(), "PROVIDER_UNAUTHORIZED");
        let listed = service.list().unwrap().remove(0);
        assert_eq!(listed.clone_status, "failed");
        assert_eq!(
            listed.clone_error.as_deref(),
            Some("音色克隆失败：PROVIDER_UNAUTHORIZED")
        );
    }

    #[test]
    fn clone_requires_a_provider_reference() {
        let environment = environment();
        let file = environment.directory.path().join("sample.wav");
        std::fs::write(&file, wav_bytes(16_000, 1, 16, 32_000)).unwrap();
        let gateway = OkGateway::new();
        let service = service(&environment, &gateway);
        let saved = service
            .save(save_input(None, "音色", None, &file.to_string_lossy()))
            .unwrap();

        let error = service.clone_voice(&saved.id).unwrap_err();

        assert_eq!(error.code(), "VOICE_REFERENCE_PROVIDER_MISSING");
    }

    fn audio_input(
        id: Option<&str>,
        name: &str,
        base64_audio: &str,
    ) -> super::super::VoiceReferenceAudioSaveInput {
        super::super::VoiceReferenceAudioSaveInput {
            id: id.map(str::to_owned),
            name: name.to_owned(),
            provider_id: None,
            target_model: None,
            transcript: Some(" 你好世界 ".into()),
            audio_base64: base64_audio.to_owned(),
        }
    }

    fn update_input(
        id: &str,
        name: &str,
        provider: Option<&str>,
    ) -> super::super::VoiceReferenceUpdateInput {
        super::super::VoiceReferenceUpdateInput {
            id: id.to_owned(),
            name: name.to_owned(),
            provider_id: provider.map(str::to_owned),
            target_model: None,
            transcript: Some(" 更新文字 ".into()),
        }
    }

    #[test]
    fn update_metadata_changes_fields_and_preserves_audio_and_clone_state() {
        let environment = environment();
        let gateway = OkGateway::new();
        let service = service(&environment, &gateway);
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(wav_bytes(16_000, 1, 16, 128_000));
        let saved = service
            .save_audio(audio_input(None, "原名", &encoded))
            .unwrap();

        let with_provider = service
            .update_metadata(update_input(&saved.id, "原名", Some("bigmodel")))
            .unwrap();
        assert_eq!(with_provider.provider_id.as_deref(), Some("bigmodel"));
        provider_with_credential(&environment);
        let cloned = service.clone_voice(&saved.id).unwrap();

        let updated = service
            .update_metadata(update_input(&saved.id, " 新名 ", Some("other")))
            .unwrap();

        assert_eq!(updated.name, "新名");
        assert_eq!(updated.provider_id.as_deref(), Some("other"));
        assert_eq!(updated.transcript, "更新文字");
        assert_eq!(updated.byte_size, saved.byte_size);
        assert_eq!(updated.duration_ms, saved.duration_ms);
        assert_eq!(updated.voice_id.as_deref(), Some(cloned.voice_id.as_str()));
        assert_eq!(updated.clone_status, "cloned");
    }

    #[test]
    fn update_metadata_rejects_empty_name_and_missing_rows() {
        let environment = environment();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);

        let error = service
            .update_metadata(update_input("ghost", "   ", None))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_FIELDS_INVALID");

        let error = service
            .update_metadata(update_input("ghost", "名", None))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_NOT_FOUND");
    }

    #[test]
    fn save_audio_persists_base64_wav_with_duration() {
        let environment = environment();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);
        let wav = wav_bytes(16_000, 1, 16, 128_000);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&wav);

        let summary = service
            .save_audio(audio_input(None, " 录音音色 ", &encoded))
            .unwrap();

        assert_eq!(summary.name, "录音音色");
        assert_eq!(summary.mime_type, "audio/wav");
        assert_eq!(summary.byte_size, 44 + 128_000);
        assert_eq!(summary.duration_ms, Some(4000));
        assert_eq!(summary.clone_status, "pending");
        let payload = serde_json::to_value(&summary).unwrap();
        assert!(payload.get("audio").is_none());
    }

    #[test]
    fn save_audio_accepts_mp3_payload_without_duration() {
        let environment = environment();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);
        let mut mp3 = Vec::new();
        mp3.extend_from_slice(b"ID3\x04\x00\x00\x00\x00\x00\x00");
        mp3.extend(std::iter::repeat_n(0xFF_u8, 2048));
        let encoded = base64::engine::general_purpose::STANDARD.encode(&mp3);

        let summary = service
            .save_audio(audio_input(None, "mp3 音色", &encoded))
            .unwrap();

        assert_eq!(summary.mime_type, "audio/mpeg");
        assert_eq!(summary.duration_ms, None);
    }

    #[test]
    fn save_audio_rejects_short_audio_and_bad_payloads() {
        let environment = environment();
        let gateway = ErrGateway;
        let service = service(&environment, &gateway);

        let short =
            base64::engine::general_purpose::STANDARD.encode(wav_bytes(16_000, 1, 16, 16_000));
        let error = service
            .save_audio(audio_input(None, "短录音", &short))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_AUDIO_TOO_SHORT");

        let error = service
            .save_audio(audio_input(None, "坏编码", "not-base64!!"))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_AUDIO_INVALID");

        let not_audio = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let error = service
            .save_audio(audio_input(None, "非音频", &not_audio))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_AUDIO_INVALID");

        let oversize =
            base64::engine::general_purpose::STANDARD.encode(vec![0_u8; 10 * 1024 * 1024 + 1]);
        let error = service
            .save_audio(audio_input(None, "过大", &oversize))
            .unwrap_err();
        assert_eq!(error.code(), "VOICE_REFERENCE_AUDIO_TOO_LARGE");
    }
}
