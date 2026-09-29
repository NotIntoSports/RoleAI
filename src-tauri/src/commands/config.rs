//! config 域命令：启动状态、公开配置、模型供应商 / 语音线路 / 角色画像 /
//! Embedding 配置的维护与恢复。纯搬移自 commands.rs，不含行为变更。
use super::*;

#[tauri::command]
pub fn config_get_startup_state(state: State<'_, AppState>) -> CommandResult<StartupState> {
    CommandResult::Ok {
        data: state.startup_state(),
    }
}

pub(super) fn public_config(state: &AppState) -> CommandResult<PublicConfig> {
    match state.config.load() {
        Ok(config) => CommandResult::Ok {
            data: public_view(&config),
        },
        Err(error) => CommandResult::Err {
            error: PublicError::new(error.code(), "Configuration is unavailable", false),
        },
    }
}

pub fn config_get_public_blocking(state: State<'_, AppState>) -> CommandResult<PublicConfig> {
    public_config(&state)
}

pub(super) fn provider_service_error<T: ts_rs::TS>(
    error: ProviderServiceError,
) -> CommandResult<T> {
    let code = error.code();
    let mut public = PublicError::new(
        code,
        match code {
            "PROVIDER_IN_USE" => "供应商仍被语音线路或 Embedding 配置引用，请先处理关联配置。",
            "SECRET_CLEANUP_FAILED" => "供应商配置已删除，但密钥清理失败，可以重试清理。",
            "CONFIG_WRITE_FAILED" => "无法写入配置，删除未完成。请检查目录写入权限。",
            "CONFIG_READ_FAILED" => "无法读取本地配置，操作未完成。",
            "PROVIDER_NOT_FOUND" => "供应商不存在，请刷新配置列表。",
            _ => "供应商操作未完成，请检查配置后重试。",
        },
        matches!(
            code,
            "PROVIDER_TIMEOUT" | "PROVIDER_REQUEST_FAILED" | "SECRET_CLEANUP_FAILED"
        ),
    );
    if let Some(field) = match code {
        "PROVIDER_ID_INVALID" => Some("id"),
        "CONFIG_URL_INVALID" | "PROVIDER_ENDPOINT_INVALID" => Some("baseUrl"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

pub(super) fn role_service_error<T: ts_rs::TS>(error: RoleProfileServiceError) -> CommandResult<T> {
    let code = error.code();
    let message = if code == "ROLE_PROFILE_REVIEW_REQUIRED" {
        "Review and save this role profile before activating or copying it"
    } else {
        "Role profile operation failed"
    };
    let mut public = PublicError::new(code, message, false);
    if let Some(field) = match code {
        "ROLE_PROFILE_ID_INVALID"
        | "ROLE_PROFILE_COPY_ID_IN_USE"
        | "ROLE_PROFILE_REVIEW_REQUIRED"
        | "ROLE_PROFILE_NOT_FOUND" => Some("id"),
        "ROLE_PROFILE_FIELDS_INVALID" => Some("name"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

pub(super) fn embedding_service_error<T: ts_rs::TS>(
    error: EmbeddingServiceError,
) -> CommandResult<T> {
    let code = error.code();
    let mut public = PublicError::new(
        code,
        "Embedding configuration operation failed",
        matches!(code, "EMBEDDING_TIMEOUT" | "EMBEDDING_REQUEST_FAILED"),
    );
    if let Some(field) = match code {
        "EMBEDDING_ID_INVALID"
        | "EMBEDDING_NOT_FOUND"
        | "EMBEDDING_NOT_READY"
        | "EMBEDDING_STALE" => Some("id"),
        "EMBEDDING_FIELDS_INVALID" => Some("dimensions"),
        "EMBEDDING_SOURCE_INVALID" | "CONFIG_REFERENCE_MISSING" => Some("providerId"),
        "CONFIG_URL_INVALID" | "EMBEDDING_ENDPOINT_INVALID" => Some("baseUrl"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

pub(super) fn route_service_error<T: ts_rs::TS>(error: VoiceRouteServiceError) -> CommandResult<T> {
    let code = error.code();
    let probe_message = if let VoiceRouteServiceError::Probe(probe) = &error {
        Some(probe.message.clone())
    } else {
        None
    };
    let mut public = PublicError::new(
        code,
        probe_message.unwrap_or_else(|| "Voice route operation failed".into()),
        matches!(
            code,
            "PROVIDER_TIMEOUT" | "PROVIDER_REQUEST_FAILED" | "VOICE_ROUTE_PROBE_FAILED"
        ),
    );
    if let Some(field) = match code {
        "VOICE_ROUTE_ID_INVALID" => Some("id"),
        "VOICE_ROUTE_FIELDS_INVALID" | "VOICE_ROUTE_MODEL_NOT_FOUND" => Some("route"),
        _ => None,
    } {
        public = public.with_field(field);
    }
    CommandResult::Err { error: public }
}

fn provider_probe<T: ts_rs::TS>() -> Result<OpenAiCompatibleProbe, CommandResult<T>> {
    OpenAiCompatibleProbe::new()
        .map_err(|error| service_error(error.code(), "Provider client is unavailable"))
}

fn route_stage_probe<T: ts_rs::TS>() -> Result<StandardRouteProbe, CommandResult<T>> {
    StandardRouteProbe::new()
        .map_err(|error| service_error(error.code(), "Provider client is unavailable"))
}

pub fn model_provider_save_blocking(
    state: State<'_, AppState>,
    input: ProviderSaveInput,
) -> CommandResult<ProviderConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .save(input)
        .map_or_else(provider_service_error, |data| CommandResult::Ok { data })
}

pub fn model_provider_test_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<ProviderTestResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let service = ProviderService::new(&state.config, &state.secrets, &probe);
    let discovered = match service.discover(&provider_id) {
        Ok(discovered) => discovered,
        Err(error) => return provider_service_error(error),
    };
    let config = match state.config.load() {
        Ok(config) => public_view(&config),
        Err(error) => return service_error(error.code(), "模型配置不可用"),
    };
    let Some(provider) = config
        .models
        .providers
        .iter()
        .find(|item| item.id == provider_id)
    else {
        return service_error("PROVIDER_NOT_FOUND", "模型供应商不存在");
    };
    let capability = provider.web_capability.unwrap_or_default();
    let mut data = ProviderTestResult {
        provider_id: provider_id.clone(),
        reachable: true,
        model_count: discovered.models.len(),
        web_status: crate::providers::web_search::WebCapabilityStatus::Disabled,
        web_source_count: 0,
    };
    if capability != crate::providers::web_search::WebCapability::None {
        let Some(model) = discovered.models.first() else {
            data.web_status = crate::providers::web_search::WebCapabilityStatus::ModelUnsupported;
            return CommandResult::Ok { data };
        };
        let secret = match read_provider_secret(&state, &config, Some(&provider_id)) {
            Ok(secret) => secret,
            Err(error) => return CommandResult::Err { error },
        };
        let model_client = match OpenAiCompatibleCascade::new() {
            Ok(client) => client.with_web_capability(capability),
            Err(_) => {
                data.web_status =
                    crate::providers::web_search::WebCapabilityStatus::NetworkUnreachable;
                return CommandResult::Ok { data };
            }
        };
        let result = model_client.complete(
            &ProviderEndpoint {
                provider_id: provider.id.clone(),
                base_url: provider.base_url.clone(),
            },
            secret.as_deref().map(|value| value.as_str()),
            &model.id,
            &[ChatMessage {
                role: "user".into(),
                content: "请使用联网搜索回答当前 UTC 日期，并提供来源。".into(),
            }],
        );
        let web = model_client.web_result();
        data.web_source_count = web.sources.len();
        data.web_status = crate::providers::web_search::probe_status_from_result(
            result.as_ref().map(|_| &web).map_err(|error| *error),
        );
    }
    CommandResult::Ok { data }
}

pub fn model_provider_discover_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<ModelDiscoveryResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .discover(&provider_id)
        .map_or_else(provider_service_error, |data| CommandResult::Ok { data })
}

pub fn model_provider_activate_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<ProviderConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .activate(&provider_id)
        .map_or_else(provider_service_error, |data| CommandResult::Ok { data })
}

pub fn model_provider_delete_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .delete(&provider_id)
        .map_or_else(provider_service_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

pub fn model_provider_dependencies_blocking(
    state: State<'_, AppState>,
    provider_id: String,
) -> CommandResult<Vec<crate::services::ProviderDependency>> {
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    ProviderService::new(&state.config, &state.secrets, &probe)
        .dependencies(&provider_id)
        .map_or_else(provider_service_error, |data| CommandResult::Ok { data })
}

pub fn speech_route_save_blocking(
    state: State<'_, AppState>,
    input: VoiceRouteSaveInput,
) -> CommandResult<VoiceRouteConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let stage_probe = match route_stage_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    VoiceRouteService::new(&state.config, &state.secrets, &probe, &stage_probe)
        .save(input)
        .map_or_else(route_service_error, |data| CommandResult::Ok { data })
}

pub fn speech_route_test_blocking(
    state: State<'_, AppState>,
    route_id: String,
) -> CommandResult<VoiceRouteTestResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let stage_probe = match route_stage_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    VoiceRouteService::new(&state.config, &state.secrets, &probe, &stage_probe)
        .test(&route_id)
        .map_or_else(route_service_error, |data| CommandResult::Ok { data })
}

pub fn speech_route_activate_blocking(
    state: State<'_, AppState>,
    route_id: String,
) -> CommandResult<VoiceRouteConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let stage_probe = match route_stage_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    VoiceRouteService::new(&state.config, &state.secrets, &probe, &stage_probe)
        .activate(&route_id)
        .map_or_else(route_service_error, |data| CommandResult::Ok { data })
}

pub fn speech_route_delete_blocking(
    state: State<'_, AppState>,
    route_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match provider_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    let stage_probe = match route_stage_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    VoiceRouteService::new(&state.config, &state.secrets, &probe, &stage_probe)
        .delete(&route_id)
        .map_or_else(route_service_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

pub fn role_profile_save_blocking(
    state: State<'_, AppState>,
    input: RoleProfileSaveInput,
) -> CommandResult<RoleProfileConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    RoleProfileService::new(&state.config)
        .save(input)
        .map_or_else(role_service_error, |data| CommandResult::Ok { data })
}

pub fn role_profile_copy_blocking(
    state: State<'_, AppState>,
    input: RoleProfileCopyInput,
) -> CommandResult<RoleProfileConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    RoleProfileService::new(&state.config)
        .copy(input)
        .map_or_else(role_service_error, |data| CommandResult::Ok { data })
}

pub fn role_profile_activate_blocking(
    state: State<'_, AppState>,
    role_id: String,
) -> CommandResult<RoleProfileConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    RoleProfileService::new(&state.config)
        .activate(&role_id)
        .map_or_else(role_service_error, |data| CommandResult::Ok { data })
}

pub fn role_profile_delete_blocking(
    state: State<'_, AppState>,
    role_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    RoleProfileService::new(&state.config)
        .delete(&role_id)
        .map_or_else(role_service_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

pub fn embedding_config_save_blocking(
    state: State<'_, AppState>,
    input: EmbeddingConfigSaveInput,
) -> CommandResult<EmbeddingConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    EmbeddingService::new(&state.config, &state.secrets, &probe)
        .save(input)
        .map_or_else(embedding_service_error, |data| CommandResult::Ok { data })
}

pub fn embedding_config_test_blocking(
    state: State<'_, AppState>,
    embedding_id: String,
) -> CommandResult<EmbeddingTestResult> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    EmbeddingService::new(&state.config, &state.secrets, &probe)
        .test(&embedding_id)
        .map_or_else(embedding_service_error, |data| CommandResult::Ok { data })
}

pub fn embedding_config_activate_blocking(
    state: State<'_, AppState>,
    embedding_id: String,
) -> CommandResult<EmbeddingConfig> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    EmbeddingService::new(&state.config, &state.secrets, &probe)
        .activate(&embedding_id)
        .map_or_else(embedding_service_error, |data| CommandResult::Ok { data })
}

pub fn embedding_config_delete_blocking(
    state: State<'_, AppState>,
    embedding_id: String,
) -> CommandResult<FoundationStatus> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    let probe = match embedding_probe() {
        Ok(probe) => probe,
        Err(error) => return error,
    };
    EmbeddingService::new(&state.config, &state.secrets, &probe)
        .delete(&embedding_id)
        .map_or_else(embedding_service_error, |_| CommandResult::Ok {
            data: FoundationStatus { ready: true },
        })
}

pub fn config_restore_last_good_blocking(
    state: State<'_, AppState>,
) -> CommandResult<StartupState> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    CommandResult::Ok {
        data: state.restore_last_good(),
    }
}

pub fn config_restore_defaults_blocking(state: State<'_, AppState>) -> CommandResult<StartupState> {
    let _guard = match service_guard(&state) {
        Ok(guard) => guard,
        Err(error) => return error,
    };
    CommandResult::Ok {
        data: state.restore_defaults(),
    }
}

blocking_command!(
    config_get_public,
    config_get_public_blocking() -> PublicConfig
);
blocking_command!(model_provider_save, model_provider_save_blocking(input: ProviderSaveInput) -> ProviderConfig);
blocking_command!(model_provider_test, model_provider_test_blocking(provider_id: String) -> ProviderTestResult);
blocking_command!(model_provider_discover, model_provider_discover_blocking(provider_id: String) -> ModelDiscoveryResult);
blocking_command!(model_provider_activate, model_provider_activate_blocking(provider_id: String) -> ProviderConfig);
blocking_command!(model_provider_delete, model_provider_delete_blocking(provider_id: String) -> FoundationStatus);
blocking_command!(model_provider_dependencies, model_provider_dependencies_blocking(provider_id: String) -> Vec<crate::services::ProviderDependency>);
blocking_command!(speech_route_save, speech_route_save_blocking(input: VoiceRouteSaveInput) -> VoiceRouteConfig);
blocking_command!(speech_route_test, speech_route_test_blocking(route_id: String) -> VoiceRouteTestResult);
blocking_command!(speech_route_activate, speech_route_activate_blocking(route_id: String) -> VoiceRouteConfig);
blocking_command!(speech_route_delete, speech_route_delete_blocking(route_id: String) -> FoundationStatus);
blocking_command!(role_profile_save, role_profile_save_blocking(input: RoleProfileSaveInput) -> RoleProfileConfig);
blocking_command!(role_profile_copy, role_profile_copy_blocking(input: RoleProfileCopyInput) -> RoleProfileConfig);
blocking_command!(role_profile_activate, role_profile_activate_blocking(role_id: String) -> RoleProfileConfig);
blocking_command!(role_profile_delete, role_profile_delete_blocking(role_id: String) -> FoundationStatus);
blocking_command!(embedding_config_save, embedding_config_save_blocking(input: EmbeddingConfigSaveInput) -> EmbeddingConfig);
blocking_command!(embedding_config_test, embedding_config_test_blocking(embedding_id: String) -> EmbeddingTestResult);
blocking_command!(embedding_config_activate, embedding_config_activate_blocking(embedding_id: String) -> EmbeddingConfig);
blocking_command!(embedding_config_delete, embedding_config_delete_blocking(embedding_id: String) -> FoundationStatus);
blocking_command!(config_restore_last_good, config_restore_last_good_blocking() -> StartupState);
blocking_command!(config_restore_defaults, config_restore_defaults_blocking() -> StartupState);
