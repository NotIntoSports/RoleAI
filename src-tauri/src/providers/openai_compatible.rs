use std::{io::Read, time::Duration};

use reqwest::{StatusCode, Url, blocking::Client, redirect::Policy};
use serde::Deserialize;

use super::{DiscoveredModel, ProviderEndpoint, ProviderError, ProviderProbe, TextToSpeech};

pub(crate) const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

fn build_client(timeout: Duration) -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(5))
        .redirect(Policy::none())
        .no_proxy()
        .tls_backend_rustls()
        .build()
}

pub(crate) fn build_bounded_client() -> Result<Client, reqwest::Error> {
    build_client(Duration::from_secs(10))
}

pub(crate) fn build_cascade_client() -> Result<Client, reqwest::Error> {
    build_client(Duration::from_secs(30))
}

pub(crate) enum BoundedBodyError {
    TooLarge,
    Failed,
}

pub(crate) fn read_bounded_body(
    response: reqwest::blocking::Response,
) -> Result<Vec<u8>, BoundedBodyError> {
    read_bounded_body_limited(response, MAX_RESPONSE_BYTES)
}

pub(crate) fn read_bounded_body_limited(
    response: reqwest::blocking::Response,
    max_bytes: u64,
) -> Result<Vec<u8>, BoundedBodyError> {
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes)
    {
        return Err(BoundedBodyError::TooLarge);
    }
    let mut bytes = Vec::new();
    response
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| BoundedBodyError::Failed)?;
    if bytes.len() as u64 > max_bytes {
        return Err(BoundedBodyError::TooLarge);
    }
    Ok(bytes)
}

pub struct OpenAiCompatibleProbe {
    client: Client,
}

impl OpenAiCompatibleProbe {
    pub fn new() -> Result<Self, ProviderError> {
        let client = build_bounded_client().map_err(|_| ProviderError::ClientUnavailable)?;
        Ok(Self { client })
    }
}

impl ProviderProbe for OpenAiCompatibleProbe {
    fn discover_models(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        let url = normalize_models_url(&endpoint.base_url)?;
        let mut request = self
            .client
            .get(url.clone())
            .header("Accept", "application/json");
        if let Some(credential) = credential.filter(|value| !value.is_empty()) {
            request = request.bearer_auth(credential);
        }
        let response = request.send().map_err(|error| {
            if error.is_timeout() {
                ProviderError::Timeout
            } else {
                ProviderError::RequestFailed
            }
        })?;
        if response.status() == StatusCode::UNAUTHORIZED
            || response.status() == StatusCode::FORBIDDEN
        {
            return Err(ProviderError::Unauthorized);
        }
        if !response.status().is_success() {
            return Err(ProviderError::RequestFailed);
        }
        let bytes = read_bounded_body(response).map_err(|error| match error {
            BoundedBodyError::TooLarge => ProviderError::ResponseTooLarge,
            BoundedBodyError::Failed => ProviderError::RequestFailed,
        })?;
        let mut models = parse_model_catalog(&bytes)?;
        merge_builtin_catalog(&url, &mut models);
        Ok(models)
    }
}

pub(crate) fn normalize_models_url(base_url: &str) -> Result<Url, ProviderError> {
    let mut url = Url::parse(base_url).map_err(|_| ProviderError::EndpointInvalid)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ProviderError::EndpointInvalid);
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/models") {
        path.to_owned()
    } else if path.is_empty() {
        "/models".to_owned()
    } else {
        format!("{path}/models")
    };
    url.set_path(&path);
    Ok(url)
}

#[derive(Deserialize)]
struct ModelCatalog {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

pub(crate) fn parse_model_catalog(bytes: &[u8]) -> Result<Vec<DiscoveredModel>, ProviderError> {
    let catalog: ModelCatalog =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::ResponseInvalid)?;
    let mut ids = catalog
        .data
        .into_iter()
        .map(|entry| entry.id.trim().to_owned())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    Ok(ids.into_iter().map(|id| DiscoveredModel { id }).collect())
}

/// 智谱开放平台的 /models 只返回对话模型；语音、向量、实时模型 ID
/// 依据官方文档内置（2026-09-24 核对 docs.bigmodel.cn），仅作界面填写建议。
const ZHIPU_BUILTIN_MODELS: &[&str] = &[
    "embedding-2",
    "embedding-3",
    "glm-asr-2512",
    "glm-realtime",
    "glm-realtime-air",
    "glm-realtime-flash",
    "glm-tts",
];

/// 按接入地址主机名合并内置已知模型 ID；结果保持排序去重，用户仍可手填目录外的模型。
pub(crate) fn merge_builtin_catalog(url: &Url, models: &mut Vec<DiscoveredModel>) {
    let known = match url.host_str() {
        Some(host) if host.eq_ignore_ascii_case("open.bigmodel.cn") => ZHIPU_BUILTIN_MODELS,
        _ => return,
    };
    let mut ids = models
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    for id in known {
        if !ids.iter().any(|existing| existing.as_str() == *id) {
            ids.push((*id).to_owned());
        }
    }
    ids.sort();
    ids.dedup();
    *models = ids
        .into_iter()
        .map(|id| DiscoveredModel { id })
        .collect();
}

/// 线路测试的阶段探测实现：e2e 走真实 realtime 握手，级联走一次最小 TTS 合成。
pub struct StandardRouteProbe;

impl StandardRouteProbe {
    pub fn new() -> Result<Self, ProviderError> {
        // 仅验证本机 HTTP 栈可用，真正探测时按线路供应商逐个发起请求。
        build_bounded_client().map_err(|_| ProviderError::ClientUnavailable).map(|_| Self)
    }
}

fn realtime_probe_error(error: crate::providers::RealtimeError) -> super::RouteProbeError {
    super::RouteProbeError {
        code: error.code().to_owned(),
        message: error.public_message(),
    }
}

fn tts_probe_error(error: crate::providers::CascadeError) -> super::RouteProbeError {
    super::RouteProbeError {
        code: error.code().to_owned(),
        message: format!(
            "语音合成测试未通过（{}），请确认账号已开通语音合成并有余量。",
            error.code()
        ),
    }
}

impl super::RouteStageProbe for StandardRouteProbe {
    fn probe_realtime_session(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
    ) -> Result<(), super::RouteProbeError> {
        crate::providers::openai_realtime::probe_realtime_session(endpoint, credential, model_id)
            .map_err(realtime_probe_error)
    }

    fn probe_tts(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        voice_id: Option<&str>,
    ) -> Result<(), super::RouteProbeError> {
        let cascade = crate::providers::OpenAiCompatibleCascade::new()
            .map_err(tts_probe_error)?;
        cascade
            .synthesize(endpoint, credential, model_id, voice_id.unwrap_or(""), "你好")
            .map(|_| ())
            .map_err(tts_probe_error)
    }
}
