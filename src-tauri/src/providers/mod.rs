mod cascade;
mod embedding;
mod openai_compatible;
mod openai_realtime;
mod realtime_protocol;
pub(crate) mod realtime_session;
mod voice_clone;
mod voice_clone_dashscope;
pub mod web_search;

use std::fmt;

pub use cascade::{
    CascadeError, CascadeStage, ChatMessage, ChatModel, OpenAiCompatibleCascade, SpeechToText,
    TextToSpeech,
};
#[cfg(test)]
pub(crate) use cascade::{
    build_asr_multipart, build_llm_request, build_tts_request, json_body_too_large,
    normalize_chat_completions_url, normalize_speech_url, normalize_transcriptions_url,
    parse_chat_completion, parse_transcript, parse_tts_pcm, pcm_to_wav, stream_sse_text,
    tts_body_too_large,
};
pub use embedding::{EmbeddingError, EmbeddingProbe, OpenAiCompatibleEmbeddingProbe};
pub use openai_compatible::{OpenAiCompatibleProbe, StandardRouteProbe};
#[cfg(test)]
pub(crate) use openai_compatible::{
    merge_builtin_catalog, normalize_models_url, parse_model_catalog,
};
#[cfg(test)]
pub(crate) use openai_realtime::{
    InputTranscriptAssembler, RealtimeDialectName, RealtimeTransport, dialect_input_rate,
    realtime_dialect, realtime_url, session_update_event, wait_session_updated,
};
pub use openai_realtime::{
    OpenAiCompatibleRealtime, RealtimeAudioRequest, RealtimeError, RealtimeModel,
    RealtimeTextRequest, RealtimeTurn, probe_realtime_session,
};
pub use voice_clone::{
    VOICE_CLONE_TRIAL_TEXT, VoiceCloneError, VoiceCloneOutcome, VoiceCloneProbe,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEndpoint {
    pub provider_id: String,
    pub base_url: String,
}

/// 单轮流式回调：收到的是"截至当前的完整文本快照"而非增量碎片，
/// 与前端事件通道的整体替换语义一致，重试重启时发送方重发空串即可复位。
/// 任一回调缺省时对应阶段静默跳过，行为退化为非流式。
#[derive(Clone, Copy)]
pub struct TurnStreamHooks<'a> {
    pub user_text: Option<&'a dyn Fn(&str)>,
    pub assistant_text: Option<&'a dyn Fn(&str)>,
}

impl<'a> TurnStreamHooks<'a> {
    pub fn none() -> Self {
        Self {
            user_text: None,
            assistant_text: None,
        }
    }

    pub fn notify_user(&self, text: &str) {
        if let Some(hook) = self.user_text {
            hook(text);
        }
    }

    pub fn notify_assistant(&self, text: &str) {
        if let Some(hook) = self.assistant_text {
            hook(text);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredModel {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderError {
    EndpointInvalid,
    ClientUnavailable,
    Timeout,
    Unauthorized,
    RequestFailed,
    /// 请求到达但供应商返回了非 2xx/401/403 的 HTTP 状态；携带状态码用于诊断。
    RequestFailedWithStatus(u16),
    ResponseTooLarge,
    ResponseInvalid,
}

impl ProviderError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::EndpointInvalid => "PROVIDER_ENDPOINT_INVALID",
            Self::ClientUnavailable => "PROVIDER_CLIENT_UNAVAILABLE",
            Self::Timeout => "PROVIDER_TIMEOUT",
            Self::Unauthorized => "PROVIDER_UNAUTHORIZED",
            Self::RequestFailed | Self::RequestFailedWithStatus(_) => "PROVIDER_REQUEST_FAILED",
            Self::ResponseTooLarge => "PROVIDER_RESPONSE_TOO_LARGE",
            Self::ResponseInvalid => "PROVIDER_RESPONSE_INVALID",
        }
    }

    /// 供用户界面展示的完整说明；带状态码的变体会附上 HTTP 状态。
    pub fn detail(self) -> String {
        match self {
            Self::RequestFailedWithStatus(status) => {
                format!("Provider request failed (HTTP {status})")
            }
            other => other.code().to_owned(),
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail())
    }
}

impl std::error::Error for ProviderError {}

pub trait ProviderProbe: Send + Sync {
    fn discover_models(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
    ) -> Result<Vec<DiscoveredModel>, ProviderError>;
}

/// 线路阶段探测失败：code 保留供应商原始错误码，message 为可直接展示的中文说明。
#[derive(Debug, Clone)]
pub struct RouteProbeError {
    pub code: String,
    pub message: String,
}

/// 线路测试的真实链路探测：目录比对无法发现账号级权限/余额问题，
/// 只有真正发起一次最小调用才能在测试阶段暴露（如未开通实时语音模型）。
pub trait RouteStageProbe: Send + Sync {
    /// e2e 线路：完成一次 realtime 会话握手（session.update → session.updated），
    /// 不发送任何音频。
    fn probe_realtime_session(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
    ) -> Result<(), RouteProbeError>;

    /// 级联线路：合成一个短字符，验证 TTS 阶段的权限与可用性。
    fn probe_tts(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        voice_id: Option<&str>,
    ) -> Result<(), RouteProbeError>;
}

#[cfg(test)]
mod tests;
