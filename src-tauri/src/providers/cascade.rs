use std::fmt;

use reqwest::{StatusCode, Url, blocking::Client};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    ProviderEndpoint,
    openai_compatible::{
        BoundedBodyError, build_cascade_client, read_bounded_body, read_bounded_body_limited,
    },
};

pub(crate) const JSON_BODY_LIMIT: u64 = 1024 * 1024;
pub(crate) const TTS_BODY_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CascadeStage {
    Asr,
    Llm,
    Tts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 级联链路（ASR→LLM→TTS）的稳定错误码，前端据此提示，不携带敏感细节。
pub enum CascadeError {
    ClientUnavailable,
    EndpointInvalid(CascadeStage),
    Timeout(CascadeStage),
    Unauthorized(CascadeStage),
    RequestFailed(CascadeStage),
    ResponseTooLarge(CascadeStage),
    ResponseInvalid(CascadeStage),
    ResponseEmpty(CascadeStage),
    PcmInvalid,
    RateLimited {
        stage: CascadeStage,
        retry_after_secs: Option<u64>,
    },
    ServerError(CascadeStage),
    ConnectionReset(CascadeStage),
    Cancelled,
    AnswerBlocked,
}

impl CascadeError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ClientUnavailable => "CASCADE_CLIENT_UNAVAILABLE",
            Self::EndpointInvalid(CascadeStage::Asr) => "ASR_ENDPOINT_INVALID",
            Self::EndpointInvalid(CascadeStage::Llm) => "LLM_ENDPOINT_INVALID",
            Self::EndpointInvalid(CascadeStage::Tts) => "TTS_ENDPOINT_INVALID",
            Self::Timeout(CascadeStage::Asr) => "ASR_TIMEOUT",
            Self::Timeout(CascadeStage::Llm) => "LLM_TIMEOUT",
            Self::Timeout(CascadeStage::Tts) => "TTS_TIMEOUT",
            Self::Unauthorized(CascadeStage::Asr) => "ASR_UNAUTHORIZED",
            Self::Unauthorized(CascadeStage::Llm) => "LLM_UNAUTHORIZED",
            Self::Unauthorized(CascadeStage::Tts) => "TTS_UNAUTHORIZED",
            Self::RequestFailed(CascadeStage::Asr) => "ASR_REQUEST_FAILED",
            Self::RequestFailed(CascadeStage::Llm) => "LLM_REQUEST_FAILED",
            Self::RequestFailed(CascadeStage::Tts) => "TTS_REQUEST_FAILED",
            Self::ResponseTooLarge(CascadeStage::Asr) => "ASR_RESPONSE_TOO_LARGE",
            Self::ResponseTooLarge(CascadeStage::Llm) => "LLM_RESPONSE_TOO_LARGE",
            Self::ResponseTooLarge(CascadeStage::Tts) => "TTS_RESPONSE_TOO_LARGE",
            Self::ResponseInvalid(CascadeStage::Asr) => "ASR_RESPONSE_INVALID",
            Self::ResponseInvalid(CascadeStage::Llm) => "LLM_RESPONSE_INVALID",
            Self::ResponseInvalid(CascadeStage::Tts) => "TTS_RESPONSE_INVALID",
            Self::ResponseEmpty(CascadeStage::Asr) => "ASR_RESPONSE_INVALID",
            Self::ResponseEmpty(CascadeStage::Llm) => "LLM_RESPONSE_EMPTY",
            Self::ResponseEmpty(CascadeStage::Tts) => "TTS_PCM_INVALID",
            Self::PcmInvalid => "TTS_PCM_INVALID",
            Self::RateLimited {
                stage: CascadeStage::Asr,
                ..
            } => "ASR_RATE_LIMITED",
            Self::RateLimited {
                stage: CascadeStage::Llm,
                ..
            } => "LLM_RATE_LIMITED",
            Self::RateLimited {
                stage: CascadeStage::Tts,
                ..
            } => "TTS_RATE_LIMITED",
            Self::ServerError(CascadeStage::Asr) => "ASR_SERVER_ERROR",
            Self::ServerError(CascadeStage::Llm) => "LLM_SERVER_ERROR",
            Self::ServerError(CascadeStage::Tts) => "TTS_SERVER_ERROR",
            Self::ConnectionReset(CascadeStage::Asr) => "ASR_CONNECTION_RESET",
            Self::ConnectionReset(CascadeStage::Llm) => "LLM_CONNECTION_RESET",
            Self::ConnectionReset(CascadeStage::Tts) => "TTS_CONNECTION_RESET",
            Self::Cancelled => "SESSION_CANCELLED",
            Self::AnswerBlocked => "SESSION_ANSWER_BLOCKED",
        }
    }
}

impl fmt::Display for CascadeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

impl std::error::Error for CascadeError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// 语音转写（ASR）供应商能力：把 16kHz 单声道 PCM 转成文本。
pub trait SpeechToText: Send + Sync {
    fn transcribe(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        pcm16le: &[u8],
        sample_rate: u32,
    ) -> Result<String, CascadeError>;
}

/// 文本对话（LLM）供应商能力：按消息列表补全一轮回答。
pub trait ChatModel: Send + Sync {
    fn complete(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        messages: &[ChatMessage],
    ) -> Result<String, CascadeError>;

    /// 流式补全：每收到增量就以"截至当前的完整文本"回调 `on_snapshot`，
    /// 供事件通道整体替换上屏。默认实现退化为一次性 `complete`，
    /// 结束时回调全文，保证调用方语义一致。
    fn complete_streaming(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        messages: &[ChatMessage],
        on_snapshot: &dyn Fn(&str),
    ) -> Result<String, CascadeError> {
        let text = self.complete(endpoint, credential, model_id, messages)?;
        on_snapshot(&text);
        Ok(text)
    }
}

/// 语音合成（TTS）供应商能力：把回答文本合成为可播放的 PCM。
pub trait TextToSpeech: Send + Sync {
    fn synthesize(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        voice_id: &str,
        text: &str,
    ) -> Result<Vec<u8>, CascadeError>;
}

pub struct OpenAiCompatibleCascade {
    client: Client,
    web_capability: super::web_search::WebCapability,
    last_web: std::sync::Mutex<super::web_search::WebCompletion>,
}

impl OpenAiCompatibleCascade {
    pub fn new() -> Result<Self, CascadeError> {
        let client = build_cascade_client().map_err(|_| CascadeError::ClientUnavailable)?;
        Ok(Self {
            client,
            web_capability: Default::default(),
            last_web: Default::default(),
        })
    }

    /// 测试专用：短超时客户端，用于钉住「超时不重试」路径（生产固定 30s 不变）。
    #[cfg(test)]
    pub(crate) fn with_timeout_for_tests(timeout: std::time::Duration) -> Self {
        let client =
            super::openai_compatible::build_client(timeout).expect("test client must build");
        Self {
            client,
            web_capability: Default::default(),
            last_web: Default::default(),
        }
    }

    pub fn with_web_capability(mut self, capability: super::web_search::WebCapability) -> Self {
        self.web_capability = capability;
        self
    }

    pub fn web_result(&self) -> super::web_search::WebCompletion {
        self.last_web
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// 单次 HTTP 尝试的内部分类；`Connect`/`RetryableStatus` 触发有界重试，
    /// 其余臂一次定性（不进公共错误枚举）。
    fn send_once(&self, request: &CascadeHttpRequest<'_>) -> SendOutcome {
        let mut http = self
            .client
            .post(request.url.clone())
            .header("Accept", request.accept)
            .header("Content-Type", request.content_type)
            .body(request.body.clone());
        if let Some(credential) = request.credential.filter(|value| !value.is_empty()) {
            http = http.bearer_auth(credential);
        }
        match http.send() {
            Ok(response) => {
                let status = response.status();
                if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                    SendOutcome::Unauthorized
                } else if matches!(status.as_u16(), 429 | 502 | 503 | 504) {
                    SendOutcome::RetryableStatus
                } else if !status.is_success() {
                    SendOutcome::RequestFailed
                } else {
                    SendOutcome::Response(response)
                }
            }
            Err(error) if error.is_connect() => SendOutcome::Connect,
            Err(error) if error.is_timeout() => SendOutcome::Timeout,
            Err(_) => SendOutcome::RequestFailed,
        }
    }

    /// 瞬时失败有界重试：连接失败与 HTTP 429/502/503/504 最多重试 2 次
    /// （间隔 300ms/900ms），最终失败仍返回原稳定错误码。超时（单次已 30 秒，
    /// 重试会成倍拉长用户等待）、401/403、其余 4xx、响应超限与解析失败一律不重试。
    /// 取消令牌在调用方 await/锁层生效，provider 层无 cancel 通道
    /// （调研见 docs/dependency-decisions.md F2）。
    fn send(&self, request: CascadeHttpRequest<'_>) -> Result<Vec<u8>, CascadeError> {
        const RETRY_DELAYS: [std::time::Duration; 2] = [
            std::time::Duration::from_millis(300),
            std::time::Duration::from_millis(900),
        ];
        let mut attempt = 0usize;
        loop {
            match self.send_once(&request) {
                SendOutcome::Response(response) => {
                    let bytes = if request.max_bytes == JSON_BODY_LIMIT {
                        read_bounded_body(response)
                    } else {
                        read_bounded_body_limited(response, request.max_bytes)
                    }
                    .map_err(|error| match error {
                        BoundedBodyError::TooLarge => CascadeError::ResponseTooLarge(request.stage),
                        BoundedBodyError::Failed => CascadeError::RequestFailed(request.stage),
                    })?;
                    if oversized_for_stage(bytes.len() as u64, request.stage) {
                        return Err(CascadeError::ResponseTooLarge(request.stage));
                    }
                    return Ok(bytes);
                }
                SendOutcome::Unauthorized => return Err(CascadeError::Unauthorized(request.stage)),
                SendOutcome::Timeout => return Err(CascadeError::Timeout(request.stage)),
                outcome => {
                    let retryable =
                        matches!(outcome, SendOutcome::Connect | SendOutcome::RetryableStatus);
                    if !retryable || attempt >= RETRY_DELAYS.len() {
                        return Err(CascadeError::RequestFailed(request.stage));
                    }
                    std::thread::sleep(RETRY_DELAYS[attempt]);
                    attempt += 1;
                }
            }
        }
    }
}

impl SpeechToText for OpenAiCompatibleCascade {
    fn transcribe(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        pcm16le: &[u8],
        sample_rate: u32,
    ) -> Result<String, CascadeError> {
        let url = normalize_transcriptions_url(&endpoint.base_url)?;
        let wav = pcm_to_wav(pcm16le, sample_rate);
        let boundary = format!("voice-route-{}", Uuid::new_v4().simple());
        let (body, content_type) = build_asr_multipart(model_id, &wav, &boundary);
        let bytes = self.send(CascadeHttpRequest {
            url,
            credential,
            content_type: &content_type,
            accept: "application/json",
            body,
            stage: CascadeStage::Asr,
            max_bytes: JSON_BODY_LIMIT,
        })?;
        parse_transcript(&bytes)
    }
}

impl ChatModel for OpenAiCompatibleCascade {
    fn complete(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        messages: &[ChatMessage],
    ) -> Result<String, CascadeError> {
        use super::web_search::{WebCapability, parse_search_response, search_request};
        *self.last_web.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
        if self.web_capability != WebCapability::None {
            let suffix = if self.web_capability == WebCapability::QwenChatEnableSearch {
                "/chat/completions"
            } else {
                "/responses"
            };
            let request = search_request(self.web_capability, model_id, messages);
            let searched = self
                .send(CascadeHttpRequest {
                    url: normalize_cascade_url(&endpoint.base_url, suffix, CascadeStage::Llm)?,
                    credential,
                    content_type: "application/json",
                    accept: "application/json",
                    body: serde_json::to_vec(&request)
                        .map_err(|_| CascadeError::RequestFailed(CascadeStage::Llm))?,
                    stage: CascadeStage::Llm,
                    max_bytes: JSON_BODY_LIMIT,
                })
                .and_then(|bytes| parse_search_response(self.web_capability, &bytes));
            match searched {
                Ok(result) => {
                    let text = result.text.clone();
                    *self.last_web.lock().unwrap_or_else(|e| e.into_inner()) = result;
                    return Ok(text);
                }
                Err(_) => {
                    self.last_web
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .degraded = true
                }
            }
        }
        let url = normalize_chat_completions_url(&endpoint.base_url)?;
        let payload = build_llm_request(model_id, messages);
        let body = serde_json::to_vec(&payload)
            .map_err(|_| CascadeError::RequestFailed(CascadeStage::Llm))?;
        let bytes = self.send(CascadeHttpRequest {
            url,
            credential,
            content_type: "application/json",
            accept: "application/json",
            body,
            stage: CascadeStage::Llm,
            max_bytes: JSON_BODY_LIMIT,
        })?;
        parse_chat_completion(&bytes)
    }

    fn complete_streaming(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        messages: &[ChatMessage],
        on_snapshot: &dyn Fn(&str),
    ) -> Result<String, CascadeError> {
        use super::web_search::{WebCapability, parse_search_response, search_request};
        *self.last_web.lock().unwrap_or_else(|e| e.into_inner()) = Default::default();
        if self.web_capability != WebCapability::None {
            // 联网搜索协议没有流式形态：先同步搜索命中，成功则全文一次性回调；
            // 失败降级走下面的流式补全，与 complete() 的降级顺序一致。
            let suffix = if self.web_capability == WebCapability::QwenChatEnableSearch {
                "/chat/completions"
            } else {
                "/responses"
            };
            let request = search_request(self.web_capability, model_id, messages);
            let searched = self
                .send(CascadeHttpRequest {
                    url: normalize_cascade_url(&endpoint.base_url, suffix, CascadeStage::Llm)?,
                    credential,
                    content_type: "application/json",
                    accept: "application/json",
                    body: serde_json::to_vec(&request)
                        .map_err(|_| CascadeError::RequestFailed(CascadeStage::Llm))?,
                    stage: CascadeStage::Llm,
                    max_bytes: JSON_BODY_LIMIT,
                })
                .and_then(|bytes| parse_search_response(self.web_capability, &bytes));
            match searched {
                Ok(result) => {
                    let text = result.text.clone();
                    *self.last_web.lock().unwrap_or_else(|e| e.into_inner()) = result;
                    on_snapshot(&text);
                    return Ok(text);
                }
                Err(_) => {
                    self.last_web
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .degraded = true
                }
            }
        }
        let url = normalize_chat_completions_url(&endpoint.base_url)?;
        let mut payload = build_llm_request(model_id, messages);
        payload["stream"] = json!(true);
        let body = serde_json::to_vec(&payload)
            .map_err(|_| CascadeError::RequestFailed(CascadeStage::Llm))?;
        let mut http = self
            .client
            .post(url)
            .header("Accept", "text/event-stream")
            .header("Content-Type", "application/json")
            .body(body);
        if let Some(credential) = credential.filter(|value| !value.is_empty()) {
            http = http.bearer_auth(credential);
        }
        let response = http.send().map_err(|error| {
            if error.is_timeout() {
                CascadeError::Timeout(CascadeStage::Llm)
            } else {
                CascadeError::RequestFailed(CascadeStage::Llm)
            }
        })?;
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            return Err(CascadeError::Unauthorized(CascadeStage::Llm));
        }
        if !status.is_success() {
            return Err(CascadeError::RequestFailed(CascadeStage::Llm));
        }
        stream_sse_text(response, on_snapshot)
    }
}

/// SSE 累计文本上限：与 JSON 体积上限同量级，防止异常服务端无限推送。
const STREAM_TEXT_CHAR_LIMIT: usize = 64 * 1024;

/// 逐行读取 OpenAI 兼容 SSE 流，把 `choices[0].delta.content` 拼成全文，
/// 每次拼接后以完整快照回调。`[DONE]` 或连接结束时返回累计文本。
/// 兼容性回退：个别供应商会忽略 `stream:true` 直接返回整段 JSON，
/// 全程未出现 SSE 帧时按普通补全解析。入参泛型化便于用 `Cursor` 做单元测试。
pub(crate) fn stream_sse_text<R: std::io::Read>(
    response: R,
    on_snapshot: &dyn Fn(&str),
) -> Result<String, CascadeError> {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(response);
    let mut line = String::new();
    let mut text = String::new();
    let mut raw = Vec::new();
    let mut saw_sse_frame = false;
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|_| CascadeError::ConnectionReset(CascadeStage::Llm))?;
        if read == 0 {
            break;
        }
        if raw.len() + read <= super::openai_compatible::MAX_RESPONSE_BYTES as usize {
            raw.extend_from_slice(line.as_bytes());
        }
        let Some(data) = line.trim_end_matches(['\r', '\n']).strip_prefix("data:") else {
            continue;
        };
        saw_sse_frame = true;
        let data = data.trim_start();
        if data == "[DONE]" {
            break;
        }
        if data.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        if let Some(delta) = event["choices"][0]["delta"]["content"].as_str()
            && !delta.is_empty()
        {
            text.push_str(delta);
            if text.chars().count() > STREAM_TEXT_CHAR_LIMIT {
                return Err(CascadeError::ResponseTooLarge(CascadeStage::Llm));
            }
            on_snapshot(&text);
        }
    }
    if !saw_sse_frame {
        let text = parse_chat_completion(&raw)?;
        on_snapshot(&text);
        return Ok(text);
    }
    let text = text.trim();
    if text.is_empty() {
        return Err(CascadeError::ResponseEmpty(CascadeStage::Llm));
    }
    Ok(text.to_owned())
}

impl TextToSpeech for OpenAiCompatibleCascade {
    fn synthesize(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        voice_id: &str,
        text: &str,
    ) -> Result<Vec<u8>, CascadeError> {
        let url = normalize_speech_url(&endpoint.base_url)?;
        let payload = build_tts_request(model_id, voice_id, text);
        let body = serde_json::to_vec(&payload)
            .map_err(|_| CascadeError::RequestFailed(CascadeStage::Tts))?;
        let bytes = self.send(CascadeHttpRequest {
            url,
            credential,
            content_type: "application/json",
            accept: "application/octet-stream",
            body,
            stage: CascadeStage::Tts,
            max_bytes: TTS_BODY_LIMIT,
        })?;
        parse_tts_pcm(&bytes)
    }
}

pub(crate) fn pcm_to_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(pcm.len()).unwrap_or(u32::MAX);
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate.saturating_mul(2)).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

pub(crate) fn normalize_transcriptions_url(base_url: &str) -> Result<Url, CascadeError> {
    normalize_cascade_url(base_url, "/audio/transcriptions", CascadeStage::Asr)
}

pub(crate) fn normalize_chat_completions_url(base_url: &str) -> Result<Url, CascadeError> {
    normalize_cascade_url(base_url, "/chat/completions", CascadeStage::Llm)
}

pub(crate) fn normalize_speech_url(base_url: &str) -> Result<Url, CascadeError> {
    normalize_cascade_url(base_url, "/audio/speech", CascadeStage::Tts)
}

fn normalize_cascade_url(
    base_url: &str,
    suffix: &str,
    stage: CascadeStage,
) -> Result<Url, CascadeError> {
    let url = Url::parse(base_url).map_err(|_| CascadeError::EndpointInvalid(stage))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CascadeError::EndpointInvalid(stage));
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with(suffix) {
        path.to_owned()
    } else if path.is_empty() {
        suffix.to_owned()
    } else {
        format!("{path}{suffix}")
    };
    let mut url = url;
    url.set_path(&path);
    Ok(url)
}

struct CascadeHttpRequest<'a> {
    url: Url,
    credential: Option<&'a str>,
    content_type: &'a str,
    accept: &'a str,
    body: Vec<u8>,
    stage: CascadeStage,
    max_bytes: u64,
}

/// `send_once` 的内部分类结果。
enum SendOutcome {
    Response(reqwest::blocking::Response),
    Connect,
    Timeout,
    Unauthorized,
    RetryableStatus,
    RequestFailed,
}

pub(crate) fn json_body_too_large(len: u64) -> bool {
    len > JSON_BODY_LIMIT
}

pub(crate) fn tts_body_too_large(len: u64) -> bool {
    len > TTS_BODY_LIMIT
}

fn oversized_for_stage(len: u64, stage: CascadeStage) -> bool {
    match stage {
        CascadeStage::Tts => tts_body_too_large(len),
        CascadeStage::Asr | CascadeStage::Llm => json_body_too_large(len),
    }
}

pub(crate) fn build_asr_multipart(
    model_id: &str,
    wav_audio: &[u8],
    boundary: &str,
) -> (Vec<u8>, String) {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model_id}\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav_audio);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (body, format!("multipart/form-data; boundary={boundary}"))
}

pub(crate) fn build_llm_request(model_id: &str, messages: &[ChatMessage]) -> Value {
    json!({
        "model": model_id,
        "messages": messages,
        "temperature": 0.3,
    })
}

pub(crate) fn build_tts_request(model_id: &str, voice_id: &str, text: &str) -> Value {
    json!({
        "model": model_id,
        "input": text.trim(),
        "voice": voice_id.trim(),
        "response_format": "pcm",
    })
}

#[derive(Deserialize)]
struct TranscriptResponse {
    text: Option<String>,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
}

pub(crate) fn parse_transcript(bytes: &[u8]) -> Result<String, CascadeError> {
    let response: TranscriptResponse = serde_json::from_slice(bytes)
        .map_err(|_| CascadeError::ResponseInvalid(CascadeStage::Asr))?;
    let text = response
        .text
        .ok_or(CascadeError::ResponseInvalid(CascadeStage::Asr))?;
    let text = text.trim();
    if text.is_empty() {
        return Err(CascadeError::ResponseInvalid(CascadeStage::Asr));
    }
    Ok(text.to_owned())
}

pub(crate) fn parse_chat_completion(bytes: &[u8]) -> Result<String, CascadeError> {
    let response: ChatCompletionResponse = serde_json::from_slice(bytes)
        .map_err(|_| CascadeError::ResponseInvalid(CascadeStage::Llm))?;
    let content = response
        .choices
        .into_iter()
        .next()
        .ok_or(CascadeError::ResponseInvalid(CascadeStage::Llm))?
        .message
        .content
        .ok_or(CascadeError::ResponseEmpty(CascadeStage::Llm))?;
    let content = content.trim();
    if content.is_empty() {
        return Err(CascadeError::ResponseEmpty(CascadeStage::Llm));
    }
    Ok(content.to_owned())
}

pub(crate) fn parse_tts_pcm(bytes: &[u8]) -> Result<Vec<u8>, CascadeError> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return Err(CascadeError::PcmInvalid);
    }
    Ok(bytes.to_vec())
}
