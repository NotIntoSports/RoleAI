use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    config::{ConfigError, ConfigStore, VoiceRouteConfig, VoiceRouteMode},
    providers::{ProviderEndpoint, ProviderProbe, RouteProbeError, RouteStageProbe},
    secrets::SecretService,
};

use super::{ProviderService, ProviderServiceError};

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct VoiceRouteSaveInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub mode: VoiceRouteMode,
    pub asr_provider_id: Option<String>,
    pub asr_model_id: Option<String>,
    pub llm_provider_id: Option<String>,
    pub llm_model_id: Option<String>,
    pub tts_provider_id: Option<String>,
    pub tts_model_id: Option<String>,
    pub voice_id: Option<String>,
    pub e2e_provider_id: Option<String>,
    pub e2e_model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct VoiceRouteTestResult {
    pub route_id: String,
    pub ready: bool,
    pub checked_provider_ids: Vec<String>,
}

#[derive(Debug)]
pub enum VoiceRouteServiceError {
    InvalidId,
    FieldsInvalid,
    NotFound,
    NotReady,
    ModelNotFound,
    Probe(RouteProbeError),
    Config(ConfigError),
    Provider(ProviderServiceError),
}

impl VoiceRouteServiceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidId => "VOICE_ROUTE_ID_INVALID",
            Self::FieldsInvalid => "VOICE_ROUTE_FIELDS_INVALID",
            Self::NotFound => "VOICE_ROUTE_NOT_FOUND",
            Self::NotReady => "VOICE_ROUTE_NOT_READY",
            Self::ModelNotFound => "VOICE_ROUTE_MODEL_NOT_FOUND",
            Self::Probe(_) => "VOICE_ROUTE_PROBE_FAILED",
            Self::Config(error) => error.code(),
            Self::Provider(error) => error.code(),
        }
    }
}

pub struct VoiceRouteService<'a> {
    config: &'a ConfigStore,
    secrets: &'a SecretService,
    probe: &'a dyn ProviderProbe,
    stage_probe: &'a dyn RouteStageProbe,
}

impl<'a> VoiceRouteService<'a> {
    pub fn new(
        config: &'a ConfigStore,
        secrets: &'a SecretService,
        probe: &'a dyn ProviderProbe,
        stage_probe: &'a dyn RouteStageProbe,
    ) -> Self {
        Self {
            config,
            secrets,
            probe,
            stage_probe,
        }
    }

    pub fn save(
        &self,
        input: VoiceRouteSaveInput,
    ) -> Result<VoiceRouteConfig, VoiceRouteServiceError> {
        let id = super::ids::resolve_optional_id(input.id.as_deref())
            .map_err(|_| VoiceRouteServiceError::InvalidId)?;
        validate_route_fields(&input)?;
        let mut saved = None;
        self.config
            .update(|config| {
                let version = config
                    .speech
                    .voice_routes
                    .iter()
                    .find(|route| route.id == id)
                    .map(|route| route.config_version.saturating_add(1))
                    .unwrap_or(1);
                let route = VoiceRouteConfig {
                    id: id.clone(),
                    name: input.name.trim().to_owned(),
                    mode: input.mode,
                    asr_provider_id: clean(input.asr_provider_id.clone()),
                    asr_model_id: clean(input.asr_model_id.clone()),
                    llm_provider_id: clean(input.llm_provider_id.clone()),
                    llm_model_id: clean(input.llm_model_id.clone()),
                    tts_provider_id: clean(input.tts_provider_id.clone()),
                    tts_model_id: clean(input.tts_model_id.clone()),
                    voice_id: clean(input.voice_id.clone()),
                    e2e_provider_id: clean(input.e2e_provider_id.clone()),
                    e2e_model_id: clean(input.e2e_model_id.clone()),
                    active: false,
                    ready: false,
                    status: Some("not_tested".into()),
                    config_version: version,
                };
                if let Some(existing) = config
                    .speech
                    .voice_routes
                    .iter_mut()
                    .find(|item| item.id == route.id)
                {
                    *existing = route.clone();
                } else {
                    config.speech.voice_routes.push(route.clone());
                }
                if config.speech.active_voice_route_id.as_deref() == Some(&route.id) {
                    config.speech.active_voice_route_id = None;
                }
                saved = Some(route);
                Ok(())
            })
            .map_err(VoiceRouteServiceError::Config)?;
        saved.ok_or(VoiceRouteServiceError::NotFound)
    }

    pub fn test(&self, route_id: &str) -> Result<VoiceRouteTestResult, VoiceRouteServiceError> {
        let config = self.config.load().map_err(VoiceRouteServiceError::Config)?;
        let route = config
            .speech
            .voice_routes
            .iter()
            .find(|route| route.id == route_id)
            .cloned()
            .ok_or(VoiceRouteServiceError::NotFound)?;
        validate_stored_route(&route)?;
        let provider_models = provider_models(&route);
        let providers = provider_models.keys().cloned().collect::<Vec<_>>();
        let provider_service = ProviderService::new(self.config, self.secrets, self.probe);
        for (provider_id, required_models) in &provider_models {
            let discovered = match provider_service.discover(provider_id) {
                Ok(discovered) => discovered,
                Err(error) => {
                    mark_test_failed(self.config, route_id)?;
                    return Err(VoiceRouteServiceError::Provider(error));
                }
            };
            let available = discovered
                .models
                .into_iter()
                .map(|model| model.id)
                .collect::<BTreeSet<_>>();
            if !required_models.is_subset(&available) {
                mark_test_failed(self.config, route_id)?;
                return Err(VoiceRouteServiceError::ModelNotFound);
            }
        }
        // 目录比对无法发现账号级的权限/余额问题（例如未开通实时语音模型），
        // 必须真实调用一次线路的阶段链路才能在测试阶段暴露。
        if let Err(error) = self.probe_route_stages(&route) {
            mark_test_failed(self.config, route_id)?;
            return Err(VoiceRouteServiceError::Probe(error));
        }
        self.config
            .update(|config| {
                let current = config
                    .speech
                    .voice_routes
                    .iter_mut()
                    .find(|item| item.id == route_id)
                    .ok_or_else(|| {
                        ConfigError::new("VOICE_ROUTE_NOT_FOUND", "Voice route not found")
                    })?;
                if current.config_version != route.config_version {
                    return Err(ConfigError::new(
                        "VOICE_ROUTE_STALE",
                        "Voice route changed during test",
                    ));
                }
                current.ready = true;
                current.status = Some("ready".into());
                Ok(())
            })
            .map_err(VoiceRouteServiceError::Config)?;
        Ok(VoiceRouteTestResult {
            route_id: route_id.into(),
            ready: true,
            checked_provider_ids: providers,
        })
    }

    fn probe_route_stages(&self, route: &VoiceRouteConfig) -> Result<(), RouteProbeError> {
        match route.mode {
            VoiceRouteMode::E2e => {
                let provider_id =
                    clean(route.e2e_provider_id.clone()).ok_or_else(probe_config_error)?;
                let model_id = clean(route.e2e_model_id.clone()).ok_or_else(probe_config_error)?;
                let (endpoint, credential) = self.stage_endpoint_credential(&provider_id)?;
                self.stage_probe
                    .probe_realtime_session(&endpoint, credential.as_deref(), &model_id)
            }
            VoiceRouteMode::Cascaded => {
                let provider_id =
                    clean(route.tts_provider_id.clone()).ok_or_else(probe_config_error)?;
                let model_id = clean(route.tts_model_id.clone()).ok_or_else(probe_config_error)?;
                let voice_id = clean(route.voice_id.clone());
                let (endpoint, credential) = self.stage_endpoint_credential(&provider_id)?;
                self.stage_probe.probe_tts(
                    &endpoint,
                    credential.as_deref(),
                    &model_id,
                    voice_id.as_deref(),
                )
            }
        }
    }

    fn stage_endpoint_credential(
        &self,
        provider_id: &str,
    ) -> Result<(ProviderEndpoint, Option<String>), RouteProbeError> {
        let config = self.config.load().map_err(|error| RouteProbeError {
            code: error.code().to_owned(),
            message: "线路测试无法读取本地配置。".into(),
        })?;
        let provider = config
            .models
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .ok_or_else(probe_config_error)?;
        let credential = provider
            .credential
            .as_ref()
            .filter(|slot| slot.configured)
            .map(|slot| self.secrets.read(&slot.reference))
            .transpose()
            .map_err(|_| RouteProbeError {
                code: "SECRET_BACKEND_UNAVAILABLE".into(),
                message: "线路测试无法读取供应商密钥，请重新保存 API Key 后重试。".into(),
            })?
            .flatten()
            .map(|value| value.to_string());
        Ok((
            ProviderEndpoint {
                provider_id: provider.id.clone(),
                base_url: provider.base_url.clone(),
            },
            credential,
        ))
    }

    pub fn activate(&self, route_id: &str) -> Result<VoiceRouteConfig, VoiceRouteServiceError> {
        let mut activated = None;
        self.config
            .update(|config| {
                let target = config
                    .speech
                    .voice_routes
                    .iter()
                    .find(|route| route.id == route_id)
                    .ok_or_else(|| {
                        ConfigError::new("VOICE_ROUTE_NOT_FOUND", "Voice route not found")
                    })?;
                if !target.ready || target.status.as_deref() != Some("ready") {
                    return Err(ConfigError::new(
                        "VOICE_ROUTE_NOT_READY",
                        "Voice route must pass a test before activation",
                    ));
                }
                for route in &mut config.speech.voice_routes {
                    route.active = route.id == route_id;
                    if route.active {
                        activated = Some(route.clone());
                    }
                }
                config.speech.active_voice_route_id = Some(route_id.into());
                Ok(())
            })
            .map_err(|error| match error.code() {
                "VOICE_ROUTE_NOT_FOUND" => VoiceRouteServiceError::NotFound,
                "VOICE_ROUTE_NOT_READY" => VoiceRouteServiceError::NotReady,
                _ => VoiceRouteServiceError::Config(error),
            })?;
        activated.ok_or(VoiceRouteServiceError::NotFound)
    }

    pub fn delete(&self, route_id: &str) -> Result<(), VoiceRouteServiceError> {
        self.config
            .update(|config| {
                let original = config.speech.voice_routes.len();
                config
                    .speech
                    .voice_routes
                    .retain(|route| route.id != route_id);
                if original == config.speech.voice_routes.len() {
                    return Err(ConfigError::new(
                        "VOICE_ROUTE_NOT_FOUND",
                        "Voice route not found",
                    ));
                }
                if config.speech.active_voice_route_id.as_deref() == Some(route_id) {
                    config.speech.active_voice_route_id = None;
                }
                Ok(())
            })
            .map_err(|error| {
                if error.code() == "VOICE_ROUTE_NOT_FOUND" {
                    VoiceRouteServiceError::NotFound
                } else {
                    VoiceRouteServiceError::Config(error)
                }
            })?;
        Ok(())
    }
}

fn validate_route_fields(input: &VoiceRouteSaveInput) -> Result<(), VoiceRouteServiceError> {
    if input.name.trim().is_empty() {
        return Err(VoiceRouteServiceError::FieldsInvalid);
    }
    let cascaded = [
        input.asr_provider_id.as_deref(),
        input.asr_model_id.as_deref(),
        input.llm_provider_id.as_deref(),
        input.llm_model_id.as_deref(),
        input.tts_provider_id.as_deref(),
        input.tts_model_id.as_deref(),
    ];
    let e2e = [
        input.e2e_provider_id.as_deref(),
        input.e2e_model_id.as_deref(),
    ];
    let valid = match input.mode {
        VoiceRouteMode::Cascaded => {
            cascaded.into_iter().all(present) && e2e.into_iter().all(|value| !present(value))
        }
        VoiceRouteMode::E2e => {
            e2e.into_iter().all(present) && cascaded.into_iter().all(|value| !present(value))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(VoiceRouteServiceError::FieldsInvalid)
    }
}

fn present(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.trim().is_empty())
}

fn probe_config_error() -> RouteProbeError {
    RouteProbeError {
        code: "VOICE_ROUTE_FIELDS_INVALID".into(),
        message: "线路配置不完整，无法发起链路测试。".into(),
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_owned();
        (!value.is_empty()).then_some(value)
    })
}

fn provider_models(route: &VoiceRouteConfig) -> BTreeMap<String, BTreeSet<String>> {
    let mut providers = BTreeMap::<String, BTreeSet<String>>::new();
    for (provider, model) in [
        (&route.asr_provider_id, &route.asr_model_id),
        (&route.llm_provider_id, &route.llm_model_id),
        (&route.tts_provider_id, &route.tts_model_id),
        (&route.e2e_provider_id, &route.e2e_model_id),
    ] {
        if let (Some(provider), Some(model)) = (provider, model) {
            providers
                .entry(provider.clone())
                .or_default()
                .insert(model.clone());
        }
    }
    providers
}

fn mark_test_failed(
    config_store: &ConfigStore,
    route_id: &str,
) -> Result<(), VoiceRouteServiceError> {
    config_store
        .update(|config| {
            let current = config
                .speech
                .voice_routes
                .iter_mut()
                .find(|item| item.id == route_id)
                .ok_or_else(|| {
                    ConfigError::new("VOICE_ROUTE_NOT_FOUND", "Voice route not found")
                })?;
            current.ready = false;
            current.active = false;
            current.status = Some("test_failed".into());
            if config.speech.active_voice_route_id.as_deref() == Some(route_id) {
                config.speech.active_voice_route_id = None;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(VoiceRouteServiceError::Config)
}

fn validate_stored_route(route: &VoiceRouteConfig) -> Result<(), VoiceRouteServiceError> {
    validate_route_fields(&VoiceRouteSaveInput {
        id: Some(route.id.clone()),
        name: route.name.clone(),
        mode: route.mode,
        asr_provider_id: route.asr_provider_id.clone(),
        asr_model_id: route.asr_model_id.clone(),
        llm_provider_id: route.llm_provider_id.clone(),
        llm_model_id: route.llm_model_id.clone(),
        tts_provider_id: route.tts_provider_id.clone(),
        tts_model_id: route.tts_model_id.clone(),
        voice_id: route.voice_id.clone(),
        e2e_provider_id: route.e2e_provider_id.clone(),
        e2e_model_id: route.e2e_model_id.clone(),
    })
}
