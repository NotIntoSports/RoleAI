use serde::Deserialize;
use serde_json::json;

use std::fmt;

use reqwest::{StatusCode, Url, blocking::Client};

use super::openai_compatible::{
    BoundedBodyError, build_cascade_client, read_bounded_body,
};
use super::{ProviderEndpoint, ProviderError};

pub const VOICE_CLONE_MODEL: &str = "glm-tts-clone";
pub const VOICE_CLONE_INPUT_PURPOSE: &str = "voice-clone-input";
pub const VOICE_CLONE_TRIAL_TEXT: &str = "你好，这是一段用于确认克隆音色效果的试听文本。";

pub struct VoiceCloneProbe {
    client: Client,
}

impl VoiceCloneProbe {
    pub fn new() -> Result<Self, ProviderError> {
        Ok(Self {
            client: build_cascade_client().map_err(|_| ProviderError::ClientUnavailable)?,
        })
    }

    pub fn upload_sample(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String, VoiceCloneError> {
        let url = clone_api_url(&endpoint.base_url, "files")?;
        let part = reqwest::blocking::multipart::Part::bytes(bytes)
            .file_name(file_name.to_owned())
            .mime_str(mime_type)
            .map_err(|_| VoiceCloneError { kind: ProviderError::EndpointInvalid, provider_message: None })?;
        let form = reqwest::blocking::multipart::Form::new()
            .text("purpose", VOICE_CLONE_INPUT_PURPOSE)
            .part("file", part);
        let mut request = self.client.post(url).multipart(form);
        if let Some(credential) = credential.filter(|value| !value.is_empty()) {
            request = request.bearer_auth(credential);
        }
        let response = request.send().map_err(map_send_error)?;
        let response = check_status(response)?;
        let bytes = read_bounded_body(response).map_err(map_body_error)?;
        #[derive(Deserialize)]
        struct FileObject {
            id: String,
        }
        let file: FileObject =
            serde_json::from_slice(&bytes).map_err(|_| VoiceCloneError { kind: ProviderError::ResponseInvalid, provider_message: None })?;
        Ok(file.id)
    }

    pub fn clone_voice(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        voice_name: &str,
        transcript: &str,
        file_id: &str,
    ) -> Result<String, VoiceCloneError> {
        let url = clone_api_url(&endpoint.base_url, "voice/clone")?;
        let mut payload = json!({
            "model": VOICE_CLONE_MODEL,
            "voice_name": voice_name,
            "input": VOICE_CLONE_TRIAL_TEXT,
            "file_id": file_id,
        });
        let transcript = transcript.trim();
        if !transcript.is_empty() {
            payload["text"] = json!(transcript);
        }
        let mut request = self.client.post(url).json(&payload);
        if let Some(credential) = credential.filter(|value| !value.is_empty()) {
            request = request.bearer_auth(credential);
        }
        let response = request.send().map_err(map_send_error)?;
        let response = check_status(response)?;
        let bytes = read_bounded_body(response).map_err(map_body_error)?;
        #[derive(Deserialize)]
        struct CloneResponse {
            voice: String,
        }
        let clone: CloneResponse =
            serde_json::from_slice(&bytes).map_err(|_| VoiceCloneError { kind: ProviderError::ResponseInvalid, provider_message: None })?;
        Ok(clone.voice)
    }
}

fn clone_api_url(base_url: &str, suffix: &str) -> Result<Url, ProviderError> {
    let mut url = Url::parse(base_url).map_err(|_| ProviderError::EndpointInvalid)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ProviderError::EndpointInvalid);
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/');
    url.set_path(&format!("{path}/{suffix}"));
    Ok(url)
}

fn map_send_error(error: reqwest::Error) -> VoiceCloneError {
    let kind = if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::RequestFailed
    };
    VoiceCloneError {
        kind,
        provider_message: None,
    }
}

fn map_body_error(error: BoundedBodyError) -> VoiceCloneError {
    let kind = match error {
        BoundedBodyError::TooLarge => ProviderError::ResponseTooLarge,
        BoundedBodyError::Failed => ProviderError::RequestFailed,
    };
    VoiceCloneError {
        kind,
        provider_message: None,
    }
}

/// 音色克隆链路错误：保留通用分类，并尽量透出供应商返回的错误消息（限长、不含密钥）。
#[derive(Debug, Clone)]
pub struct VoiceCloneError {
    pub kind: ProviderError,
    pub provider_message: Option<String>,
}

impl VoiceCloneError {
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    pub fn detail(&self) -> String {
        match &self.provider_message {
            Some(message) => format!("{}（{}）", self.kind.detail(), message),
            None => self.kind.detail(),
        }
    }
}

impl fmt::Display for VoiceCloneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail())
    }
}

impl From<ProviderError> for VoiceCloneError {
    fn from(kind: ProviderError) -> Self {
        Self {
            kind,
            provider_message: None,
        }
    }
}

impl From<VoiceCloneError> for ProviderError {
    fn from(error: VoiceCloneError) -> Self {
        error.kind
    }
}

fn provider_error_message(bytes: &[u8]) -> Option<String> {
    #[derive(Deserialize)]
    struct ErrorBody {
        error: Option<ErrorInner>,
        message: Option<String>,
    }
    #[derive(Deserialize)]
    struct ErrorInner {
        message: Option<String>,
    }
    let parsed: ErrorBody = serde_json::from_slice(bytes).ok()?;
    let message = parsed.error.and_then(|inner| inner.message).or(parsed.message)?;
    Some(message.chars().take(300).collect())
}

fn check_status(response: reqwest::blocking::Response) -> Result<reqwest::blocking::Response, VoiceCloneError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let kind = if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        ProviderError::Unauthorized
    } else {
        ProviderError::RequestFailedWithStatus(status.as_u16())
    };
    let provider_message = read_bounded_body(response)
        .ok()
        .and_then(|bytes| provider_error_message(&bytes));
    Err(VoiceCloneError {
        kind,
        provider_message,
    })
}
