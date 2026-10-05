//! 音频输入路径诊断：把麦克风选型的实测证据（候选设备、名称特征、探针 RMS）
//! 交给当前激活线路的大模型复核，产出受白名单约束的结论（建议改绑的设备 id
//! 必须在证据表内）。LLM 只做诊断与建议，不直接改绑定、不碰系统状态；
//! 调用失败/输出不合法由调用方降级为确定性选择。

use crate::contracts::{AudioPathEvidence, AudioPathReview};
use crate::providers::{CascadeError, ChatMessage, ChatModel, ProviderEndpoint};

/// 复核失败原因：稳定错误码，前端一律降级、不阻断会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDiagnosisError {
    /// 证据表为空或字段非法。
    EvidenceInvalid,
    /// 未配置激活线路 / LLM provider / 模型。
    EndpointMissing,
    /// LLM 调用失败（网络/鉴权/限流等）。
    RequestFailed,
    /// LLM 输出无法解析为约定的 JSON。
    ResponseInvalid,
}

impl AudioDiagnosisError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::EvidenceInvalid => "AUDIO_EVIDENCE_INVALID",
            Self::EndpointMissing => "AUDIO_DIAGNOSIS_UNAVAILABLE",
            Self::RequestFailed => "AUDIO_DIAGNOSIS_FAILED",
            Self::ResponseInvalid => "AUDIO_DIAGNOSIS_INVALID",
        }
    }

    pub fn public_message(&self) -> &'static str {
        match self {
            Self::EvidenceInvalid => "音频设备证据无效，已使用默认选型。",
            Self::EndpointMissing => "尚未配置可用的语音线路，无法进行智能复核。",
            Self::RequestFailed => "智能复核调用失败，已使用默认选型。",
            Self::ResponseInvalid => "智能复核返回格式异常，已使用默认选型。",
        }
    }
}

const DIAGNOSIS_LIMIT: usize = 300;
const ADVICE_LIMIT: usize = 300;

/// 证据表基础校验：至少一个候选、字段合法。
pub fn validate_evidence(evidence: &AudioPathEvidence) -> Result<(), AudioDiagnosisError> {
    if evidence.inputs.is_empty() {
        return Err(AudioDiagnosisError::EvidenceInvalid);
    }
    for device in &evidence.inputs {
        if device.device_id.trim().is_empty()
            || !device.rms_peak.is_finite()
            || device.rms_peak < 0.0
        {
            return Err(AudioDiagnosisError::EvidenceInvalid);
        }
    }
    Ok(())
}

/// 组装复核消息。证据以 JSON 内嵌，系统提示固定输出契约（严格 JSON、
/// 按界面语言输出文本），保证解析端稳定。
pub fn review_messages(evidence: &AudioPathEvidence) -> Vec<ChatMessage> {
    let locale = evidence.locale.as_deref().unwrap_or("zh-CN");
    let system = format!(
        "你是 Windows 桌面语音应用的音频输入路由诊断助手。应用在会话开始时为麦克风采集逐个候选设备试绑并实测了峰值 RMS（rmsPeak，0..1，纯静音为 0）。请判断当前绑定（boundDeviceId）是否正确；若不正确，从候选中选出应绑定的设备。\n\
         判定参考：\n\
         1. virtualEndpoint 为 true 的是虚拟回环端点（VB-CABLE/CABLE Output 等），本机麦克风会话里它们通常只回环系统播放音，不是真人语音，除非证据显示其中确有语音信号（例如用户想听系统回放），否则不要选它。\n\
         2. rmsPeak 为 0 的设备当前送入的是纯静音：可能是设备被 Fn 静音键/系统静音、隐私遮罩、驱动异常或线路未插入；不要把静音设备当作绑定目标。\n\
         3. 若所有物理设备 rmsPeak 都是 0，保持绑定不变，重点在 diagnosis 里说明静音的可能原因（Fn 静音键/系统静音、隐私遮罩、未插好、驱动异常等）。\n\
         4. 绑定正确且无值得用户知道的情况时，diagnosis 与 advice 都输出空字符串（不要输出客套话）。\n\
         只输出一个 JSON 对象，禁止输出代码栅栏或其它文字，字段：\n\
         {{\"useDeviceId\": \"<候选 deviceId；维持现状填 null>\", \"diagnosis\": \"<不超过 120 字>\", \"advice\": \"<不超过 120 字，无建议则空字符串>\"}}\n\
         diagnosis 与 advice 用 {locale} 书写。"
    );
    let user = serde_json::to_string(&evidence).unwrap_or_else(|_| "{}".to_string());
    vec![
        ChatMessage {
            role: "system".into(),
            content: system,
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

/// 解析并校验 LLM 结论：剥离代码栅栏，取首个 JSON 对象；
/// switchDeviceId 不在证据表内时按“维持现状”处理（不整体作废文本结论）。
pub fn parse_review(
    text: &str,
    evidence: &AudioPathEvidence,
) -> Result<AudioPathReview, AudioDiagnosisError> {
    let value = extract_json_object(text).ok_or(AudioDiagnosisError::ResponseInvalid)?;
    let switch = match value.get("useDeviceId") {
        Some(serde_json::Value::String(id)) => {
            let known = evidence.inputs.iter().any(|device| device.device_id == *id);
            known.then(|| id.clone())
        }
        _ => None,
    };
    let text_field = |key: &str, limit: usize| -> String {
        match value.get(key) {
            Some(serde_json::Value::String(text)) => {
                let text = text.trim();
                if text.chars().count() > limit {
                    text.chars().take(limit).collect()
                } else {
                    text.to_owned()
                }
            }
            _ => String::new(),
        }
    };
    Ok(AudioPathReview {
        switch_device_id: switch,
        diagnosis: text_field("diagnosis", DIAGNOSIS_LIMIT),
        advice: text_field("advice", ADVICE_LIMIT),
    })
}

/// 从 LLM 文本中提取首个平衡的 JSON 对象（容忍 ```json 栅栏与前后闲话）。
fn extract_json_object(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, character) in text.char_indices().skip_while(|(index, _)| *index < start) {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return serde_json::from_str(&text[start..=index]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

/// 完整复核：装配消息 → LLM → 解析校验。
pub fn review_audio_path(
    llm: &dyn ChatModel,
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
    model_id: &str,
    evidence: &AudioPathEvidence,
) -> Result<AudioPathReview, AudioDiagnosisError> {
    validate_evidence(evidence)?;
    let messages = review_messages(evidence);
    let text = llm
        .complete(endpoint, credential, model_id, &messages)
        .map_err(|error| map_cascade_error(&error))?;
    parse_review(&text, evidence)
}

fn map_cascade_error(error: &CascadeError) -> AudioDiagnosisError {
    match error {
        CascadeError::EndpointInvalid(_) => AudioDiagnosisError::EndpointMissing,
        CascadeError::Unauthorized(_) => AudioDiagnosisError::RequestFailed,
        _ => AudioDiagnosisError::RequestFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::AudioPathDeviceEvidence;
    use crate::providers::CascadeStage;

    fn evidence_json() -> AudioPathEvidence {
        AudioPathEvidence {
            inputs: vec![
                AudioPathDeviceEvidence {
                    device_id: "guid-cable".into(),
                    label: "CABLE Output (VB-Audio Virtual Cable)".into(),
                    virtual_endpoint: true,
                    rms_peak: 0.0,
                },
                AudioPathDeviceEvidence {
                    device_id: "guid-mic".into(),
                    label: "Microphone Array (Intel SST)".into(),
                    virtual_endpoint: false,
                    rms_peak: 0.02,
                },
            ],
            bound_device_id: "guid-cable".into(),
            os_default_label: Some("默认 - CABLE Output (VB-Audio Virtual Cable)".into()),
            locale: Some("zh-CN".into()),
        }
    }

    struct FakeLlm {
        response: Result<String, CascadeError>,
    }

    impl ChatModel for FakeLlm {
        fn complete(
            &self,
            _endpoint: &ProviderEndpoint,
            _credential: Option<&str>,
            _model_id: &str,
            _messages: &[ChatMessage],
        ) -> Result<String, CascadeError> {
            self.response.clone()
        }
    }

    #[test]
    fn evidence_rejects_empty_or_invalid_inputs() {
        let mut evidence = evidence_json();
        assert!(validate_evidence(&evidence).is_ok());
        evidence.inputs.clear();
        assert_eq!(
            validate_evidence(&evidence).unwrap_err(),
            AudioDiagnosisError::EvidenceInvalid
        );
        let mut evidence = evidence_json();
        evidence.inputs[0].rms_peak = f64::NAN;
        assert_eq!(
            validate_evidence(&evidence).unwrap_err(),
            AudioDiagnosisError::EvidenceInvalid
        );
        let mut evidence = evidence_json();
        evidence.inputs[0].device_id = "  ".into();
        assert_eq!(
            validate_evidence(&evidence).unwrap_err(),
            AudioDiagnosisError::EvidenceInvalid
        );
    }

    #[test]
    fn parse_review_accepts_fenced_json_and_unknown_device_falls_back_to_keep() {
        let evidence = evidence_json();
        let text = "结论如下\n```json\n{\"useDeviceId\": \"guid-mic\", \"diagnosis\": \"系统默认被虚拟声卡占用\", \"advice\": \"已改用真实麦克风\"}\n```";
        let review = parse_review(text, &evidence).expect("parse ok");
        assert_eq!(review.switch_device_id.as_deref(), Some("guid-mic"));
        assert_eq!(review.diagnosis, "系统默认被虚拟声卡占用");

        let text = "{\"useDeviceId\": \"guid-unknown\", \"diagnosis\": \"d\", \"advice\": \"a\"}";
        let review = parse_review(text, &evidence).expect("parse ok");
        assert_eq!(review.switch_device_id, None);
    }

    #[test]
    fn parse_review_rejects_non_json_and_null_switch() {
        let evidence = evidence_json();
        assert_eq!(
            parse_review("抱歉，我无法判断。", &evidence).unwrap_err(),
            AudioDiagnosisError::ResponseInvalid
        );
        let review = parse_review(
            "{\"useDeviceId\": null, \"diagnosis\": \"绑定正确\", \"advice\": \"\"}",
            &evidence,
        )
        .expect("parse ok");
        assert_eq!(review.switch_device_id, None);
        assert_eq!(review.diagnosis, "绑定正确");
    }

    #[test]
    fn parse_review_clamps_oversized_text_fields() {
        let evidence = evidence_json();
        let long = "长".repeat(500);
        let text =
            format!("{{\"useDeviceId\": null, \"diagnosis\": \"{long}\", \"advice\": \"\"}}");
        let review = parse_review(&text, &evidence).expect("parse ok");
        assert_eq!(review.diagnosis.chars().count(), DIAGNOSIS_LIMIT);
    }

    #[test]
    fn review_maps_llm_failure_to_stable_code() {
        let evidence = evidence_json();
        let endpoint = ProviderEndpoint {
            provider_id: "p".into(),
            base_url: "https://example.invalid".into(),
        };
        let llm = FakeLlm {
            response: Err(CascadeError::Unauthorized(CascadeStage::Llm)),
        };
        assert_eq!(
            review_audio_path(&llm, &endpoint, None, "model", &evidence).unwrap_err(),
            AudioDiagnosisError::RequestFailed
        );
    }

    #[test]
    fn review_end_to_end_with_fake_llm() {
        let evidence = evidence_json();
        let endpoint = ProviderEndpoint {
            provider_id: "p".into(),
            base_url: "https://example.invalid".into(),
        };
        let llm = FakeLlm {
            response: Ok("{\"useDeviceId\": \"guid-mic\", \"diagnosis\": \"默认被 CABLE 占用\", \"advice\": \"已选真实麦克风\"}".into()),
        };
        let review =
            review_audio_path(&llm, &endpoint, None, "model", &evidence).expect("review ok");
        assert_eq!(review.switch_device_id.as_deref(), Some("guid-mic"));
    }
}
