//! 阿里云 DashScope（百炼）声音复刻适配：getPolicy 上传参考音频到临时 OSS，
//! 再调用 voice-enrollment 定制接口创建音色。接口契约见
//! help.aliyun.com《声音复刻 HTTP API 参考》与《上传本地文件获取临时URL》。
//!
//! 与智谱路径的差异：上传产物是公网临时 URL（48 小时有效，仅注册时使用），
//! 复刻请求体不收文字稿，音色名以 ≤10 字符字母数字前缀表达。

use serde::Deserialize;
use serde_json::json;

use reqwest::{Url, blocking::Client};

use super::openai_compatible::read_bounded_body;
use super::voice_clone::{VoiceCloneError, VoiceCloneOutcome, check_status};
use super::{ProviderEndpoint, ProviderError};

pub const DASHSCOPE_ENROLLMENT_MODEL: &str = "voice-enrollment";
/// 复刻目标模型：克隆出的音色只能配同名模型合成，语音线路 TTS 需选择一致。
pub const DASHSCOPE_TARGET_MODEL: &str = "cosyvoice-v2";
/// qwen 全模态/实时系复刻模型：qwen-voice-enrollment 单调用复刻，音频 base64 直传。
const QWEN_ENROLLMENT_MODEL: &str = "qwen-voice-enrollment";

/// base_url 指向 DashScope（含国际站）时走本适配层；仅按主机名精确匹配，
/// 避免把伪装域名的第三方网关误判进来。
pub(crate) fn is_dashscope_base(base_url: &str) -> bool {
    matches!(
        Url::parse(base_url.trim())
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .as_deref(),
        Some("dashscope.aliyuncs.com") | Some("dashscope-intl.aliyuncs.com")
    )
}

/// 目标模型属于 qwen 全模态/实时系（qwen-voice-enrollment 复刻体系）。
/// cosyvoice 系音色驱动 qwen realtime 会被服务端以
/// "Voice '…' is not supported" 拒绝，两类复刻不可混用。
pub(crate) fn is_qwen_target(target_model: &str) -> bool {
    let target = target_model.trim().to_ascii_lowercase();
    target.starts_with("qwen") || target.contains("omni")
}

/// 以 base_url 的 scheme+host 为根拼 /api/v1 下的接口地址；
/// base_url 的路径（如 /compatible-mode/v1）与复刻接口不在同一前缀下，一律忽略。
fn dashscope_api_url(base_url: &str, path: &str) -> Result<Url, VoiceCloneError> {
    let mut url = Url::parse(base_url.trim()).map_err(|_| VoiceCloneError {
        kind: ProviderError::EndpointInvalid,
        provider_message: None,
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(VoiceCloneError {
            kind: ProviderError::EndpointInvalid,
            provider_message: None,
        });
    }
    url.set_path(&format!("/api/v1{path}"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn bearer(credential: Option<&str>) -> Option<String> {
    credential
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("Bearer {}", value.trim()))
}

#[derive(Debug, Deserialize)]
struct UploadPolicyResponse {
    data: UploadPolicyData,
}

#[derive(Debug, Deserialize)]
struct UploadPolicyData {
    policy: String,
    signature: String,
    upload_dir: String,
    upload_host: String,
    oss_access_key_id: String,
    x_oss_object_acl: String,
    x_oss_forbid_overwrite: String,
}

/// 上传参考音频：先取上传策略，再直传 OSS，返回可公网访问的临时 URL。
pub(crate) fn upload_sample(
    client: &Client,
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
    file_name: &str,
    mime_type: &str,
    bytes: Vec<u8>,
) -> Result<String, VoiceCloneError> {
    let mut policy_url = dashscope_api_url(&endpoint.base_url, "/uploads")?;
    policy_url
        .query_pairs_mut()
        .append_pair("action", "getPolicy")
        .append_pair("model", DASHSCOPE_ENROLLMENT_MODEL);
    let mut request = client.get(policy_url);
    if let Some(authorization) = bearer(credential) {
        request = request.header("Authorization", authorization);
    }
    let response = request.send().map_err(super::voice_clone::map_send_error)?;
    let response = check_status(response)?;
    let body = read_bounded_body(response).map_err(super::voice_clone::map_body_error)?;
    let policy: UploadPolicyResponse =
        serde_json::from_slice(&body).map_err(|_| VoiceCloneError {
            kind: ProviderError::ResponseInvalid,
            provider_message: None,
        })?;

    let object_key = format!("{}/{}", policy.data.upload_dir, file_name);
    let form = reqwest::blocking::multipart::Form::new()
        .text("OSSAccessKeyId", policy.data.oss_access_key_id)
        .text("Signature", policy.data.signature)
        .text("policy", policy.data.policy)
        .text("x-oss-object-acl", policy.data.x_oss_object_acl)
        .text("x-oss-forbid-overwrite", policy.data.x_oss_forbid_overwrite)
        .text("key", object_key.clone())
        .text("success_action_status", "200")
        // OSS PostObject 规范要求 file 为最后一个表单字段。
        .part(
            "file",
            reqwest::blocking::multipart::Part::bytes(bytes)
                .file_name(file_name.to_owned())
                .mime_str(mime_type)
                .map_err(|_| VoiceCloneError {
                    kind: ProviderError::EndpointInvalid,
                    provider_message: None,
                })?,
        );
    let oss_url = Url::parse(&policy.data.upload_host).map_err(|_| VoiceCloneError {
        kind: ProviderError::EndpointInvalid,
        provider_message: None,
    })?;
    let response = client
        .post(oss_url)
        .multipart(form)
        .send()
        .map_err(super::voice_clone::map_send_error)?;
    check_status(response)?;

    // DashScope 约定：以 oss:// 引用刚上传的临时文件，请求带
    // X-DashScope-OssResourceResolve: enable 由服务端解析（公网 https 直链会 403）。
    Ok(format!("oss://{object_key}"))
}

#[derive(Debug, Deserialize)]
struct CloneResponse {
    output: CloneOutput,
}

#[derive(Debug, Deserialize)]
struct CloneOutput {
    voice_id: String,
}

#[derive(Debug, Deserialize)]
struct QwenCloneResponse {
    output: QwenCloneOutput,
}

#[derive(Debug, Deserialize)]
struct QwenCloneOutput {
    voice: String,
}

/// 按目标模型分发复刻：qwen 全模态/实时系走 qwen-voice-enrollment 单调用
/// （音频 base64 直传，无独立上传步骤）；cosyvoice 系走既有 上传→create_voice。
#[allow(clippy::too_many_arguments)]
pub(crate) fn clone_reference(
    client: &Client,
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
    voice_name: &str,
    target_model: &str,
    file_name: &str,
    mime_type: &str,
    bytes: Vec<u8>,
) -> Result<VoiceCloneOutcome, VoiceCloneError> {
    if is_qwen_target(target_model) {
        let voice_id = clone_voice_qwen(
            client,
            endpoint,
            credential,
            voice_name,
            target_model,
            file_name,
            mime_type,
            bytes,
        )?;
        Ok(VoiceCloneOutcome {
            voice_id,
            remote_file_id: None,
        })
    } else {
        let target = if target_model.trim().is_empty() {
            DASHSCOPE_TARGET_MODEL
        } else {
            target_model.trim()
        };
        let sample_url = upload_sample(client, endpoint, credential, file_name, mime_type, bytes)?;
        let voice_id = clone_voice_cosyvoice(
            client,
            endpoint,
            credential,
            voice_name,
            target,
            &sample_url,
        )?;
        Ok(VoiceCloneOutcome {
            voice_id,
            remote_file_id: Some(sample_url),
        })
    }
}

/// qwen 全模态/实时系复刻：qwen-voice-enrollment，音频以 data URI 内嵌，
/// target_model 必须与后续 realtime/omni 调用模型一致，否则合成被拒。
#[allow(clippy::too_many_arguments)]
fn clone_voice_qwen(
    client: &Client,
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
    voice_name: &str,
    target_model: &str,
    _file_name: &str,
    mime_type: &str,
    bytes: Vec<u8>,
) -> Result<String, VoiceCloneError> {
    use base64::Engine as _;
    let data_uri = format!(
        "data:{mime_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    );
    let clone_url = dashscope_api_url(&endpoint.base_url, "/services/audio/tts/customization")?;
    let payload = json!({
        "model": QWEN_ENROLLMENT_MODEL,
        "parameters": { "voice_clone_mode": "normal" },
        "input": {
            "action": "create",
            "target_model": target_model.trim(),
            "preferred_name": voice_prefix(voice_name),
            "audio": { "data": data_uri },
        },
    });
    let mut request = client.post(clone_url).json(&payload);
    if let Some(authorization) = bearer(credential) {
        request = request.header("Authorization", authorization);
    }
    let response = request.send().map_err(super::voice_clone::map_send_error)?;
    let response = check_status(response)?;
    let body = read_bounded_body(response).map_err(super::voice_clone::map_body_error)?;
    let clone: QwenCloneResponse = serde_json::from_slice(&body).map_err(|_| VoiceCloneError {
        kind: ProviderError::ResponseInvalid,
        provider_message: None,
    })?;
    Ok(clone.output.voice)
}

/// 创建音色：以临时 URL 注册复刻音色，返回供应商音色 ID。
fn clone_voice_cosyvoice(
    client: &Client,
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
    voice_name: &str,
    target_model: &str,
    sample_url: &str,
) -> Result<String, VoiceCloneError> {
    let clone_url = dashscope_api_url(&endpoint.base_url, "/services/audio/tts/customization")?;
    let payload = json!({
        "model": DASHSCOPE_ENROLLMENT_MODEL,
        "input": {
            "action": "create_voice",
            "target_model": target_model,
            "prefix": voice_prefix(voice_name),
            "url": sample_url,
        },
    });
    let mut request = client.post(clone_url).json(&payload);
    request = request.header("X-DashScope-OssResourceResolve", "enable");
    if let Some(authorization) = bearer(credential) {
        request = request.header("Authorization", authorization);
    }
    let response = request.send().map_err(super::voice_clone::map_send_error)?;
    let response = check_status(response)?;
    let body = read_bounded_body(response).map_err(super::voice_clone::map_body_error)?;
    let clone: CloneResponse = serde_json::from_slice(&body).map_err(|_| VoiceCloneError {
        kind: ProviderError::ResponseInvalid,
        provider_message: None,
    })?;
    Ok(clone.output.voice_id)
}

/// DashScope 限制前缀 ≤10 字符且仅字母数字：复用智谱侧生成的音色名，
/// 过滤后截前 10 位，保持与该条参考音频的对应关系可人工辨认。
fn voice_prefix(voice_name: &str) -> String {
    let filtered: String = voice_name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let prefix: String = filtered.chars().take(10).collect();
    if prefix.is_empty() {
        "roleai0000".to_owned()
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_prefix_is_at_most_ten_alphanumeric_characters() {
        assert_eq!(voice_prefix("roleai_0123456789abcdef0123456"), "roleai0123");
        assert_eq!(voice_prefix("roleai_abcdefghij"), "roleaiabcd");
        assert_eq!(voice_prefix(""), "roleai0000");
    }
}
