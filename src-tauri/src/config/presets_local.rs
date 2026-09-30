//! 本地模型服务预设：Ollama / LM Studio / OpenAI 兼容本地 Whisper（speaches）/
//! OpenAI 兼容本地 TTS（Kokoro-FastAPI）。全部通过既有 OpenAI 兼容协议接入，
//! 不引入新协议代码；模板只补缺失项，与角色预设同一策略（用户已配置的条目
//! 优先，已删除的模板在下次启动时回补）。端点与默认模型见 `guide/local-models.md`。

use super::{AppConfigV1, EmbeddingConfig, EmbeddingDistance, ProviderConfig, VoiceRouteConfig};

/// 本地模型预设供应商模板 ID（与角色预设 `PRESET_IDS` 并列）。
pub const LOCAL_PRESET_PROVIDER_IDS: [&str; 4] = [
    "local-preset-ollama",
    "local-preset-lmstudio",
    "local-preset-whisper",
    "local-preset-kokoro",
];

const LOCAL_PRESET_ROUTE_ID: &str = "local-preset-route";
const LOCAL_PRESET_EMBEDDING_ID: &str = "local-preset-embedding";

/// 播种缺失的本地模型模板。返回是否发生了变更（调用方据此决定是否落盘）。
pub fn ensure_local_model_presets(config: &mut AppConfigV1) -> bool {
    // 端点均来自各服务官方文档（2026-09-30 核对）：
    // Ollama OpenAI 兼容层 https://github.com/ollama/ollama/blob/main/docs/openai.md
    // LM Studio 本地服务器 https://lmstudio.ai/docs/app/api
    // speaches（faster-whisper）默认 uvicorn 端口 8000 https://speaches.ai/configuration/
    // Kokoro-FastAPI 默认端口 8880 https://github.com/remsky/Kokoro-FastAPI
    let definitions = [
        (
            "local-preset-ollama",
            "Ollama（本机）",
            "http://127.0.0.1:11434/v1",
        ),
        (
            "local-preset-lmstudio",
            "LM Studio（本机）",
            "http://127.0.0.1:1234/v1",
        ),
        (
            "local-preset-whisper",
            "本地 Whisper（speaches）",
            "http://127.0.0.1:8000/v1",
        ),
        (
            "local-preset-kokoro",
            "本地 TTS（Kokoro-FastAPI）",
            "http://127.0.0.1:8880/v1",
        ),
    ];
    let mut changed = false;
    for (id, name, base_url) in definitions {
        if config
            .models
            .providers
            .iter()
            .any(|provider| provider.id == id)
        {
            continue;
        }
        config.models.providers.push(ProviderConfig {
            web_capability: None,
            id: id.into(),
            name: Some(name.into()),
            base_url: base_url.into(),
            // 本地服务默认不需要 API Key；需要时用户可在供应商设置里补填。
            credential: None,
        });
        changed = true;
    }
    if !config
        .speech
        .voice_routes
        .iter()
        .any(|route| route.id == LOCAL_PRESET_ROUTE_ID)
    {
        // 模板模型 ID 是各服务生态的常见默认值，用户按自己已拉取的模型修改；
        // ready=false + model_not_ready 表示线路未经测试，不会影响既有线路。
        config.speech.voice_routes.push(VoiceRouteConfig {
            id: LOCAL_PRESET_ROUTE_ID.into(),
            name: "本机全本地线路（模板）".into(),
            mode: Default::default(),
            asr_provider_id: Some("local-preset-whisper".into()),
            asr_model_id: Some("Systran/faster-whisper-small".into()),
            llm_provider_id: Some("local-preset-ollama".into()),
            llm_model_id: Some("qwen2.5:7b".into()),
            tts_provider_id: Some("local-preset-kokoro".into()),
            tts_model_id: Some("kokoro".into()),
            voice_id: Some("af_heart".into()),
            e2e_provider_id: None,
            e2e_model_id: None,
            active: false,
            ready: false,
            status: Some("model_not_ready".into()),
            config_version: 1,
        });
        changed = true;
    }
    if !config
        .knowledge
        .embedding_configs
        .iter()
        .any(|embedding| embedding.id == LOCAL_PRESET_EMBEDDING_ID)
    {
        // nomic-embed-text 的向量维度是 768（Ollama 官方模型卡）。
        config.knowledge.embedding_configs.push(EmbeddingConfig {
            id: LOCAL_PRESET_EMBEDDING_ID.into(),
            provider_id: "local-preset-ollama".into(),
            base_url: None,
            credential: None,
            model_id: "nomic-embed-text".into(),
            dimensions: 768,
            distance: EmbeddingDistance::Cosine,
            normalized: true,
            active: false,
            ready: false,
            status: Some("not_tested".into()),
            config_version: 1,
        });
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_once_and_passes_config_validation() {
        let mut config = AppConfigV1::default();
        assert!(ensure_local_model_presets(&mut config));
        assert_eq!(
            config.models.providers.len(),
            LOCAL_PRESET_PROVIDER_IDS.len()
        );
        config.validate().unwrap();
        // 第二次启动不再变更（幂等）。
        assert!(!ensure_local_model_presets(&mut config));
        config.validate().unwrap();
    }

    #[test]
    fn coexists_with_role_presets_and_existing_entries() {
        let mut config = AppConfigV1::default();
        assert!(super::super::presets::ensure_role_presets(&mut config));
        assert!(ensure_local_model_presets(&mut config));
        assert!(config.validate().is_ok());
        // 用户已存在的同名供应商优先，不被覆盖。
        let before = config.models.providers.clone();
        assert!(!ensure_local_model_presets(&mut config));
        assert_eq!(config.models.providers, before);
    }

    #[test]
    fn preset_endpoints_stay_on_loopback_and_without_credentials() {
        // 隐私边界：本地模型预设永远指向环回地址，且不携带任何凭据槽。
        let mut config = AppConfigV1::default();
        ensure_local_model_presets(&mut config);
        for provider in &config.models.providers {
            if !LOCAL_PRESET_PROVIDER_IDS.contains(&provider.id.as_str()) {
                continue;
            }
            let url = tauri::Url::parse(&provider.base_url).unwrap();
            let host = url.host_str().unwrap();
            assert!(
                host == "127.0.0.1" || host == "localhost",
                "本地预设端点必须指向环回地址，实际 {host}"
            );
            assert!(provider.credential.is_none());
        }
    }

    #[test]
    fn preset_route_references_only_seeded_providers() {
        let mut config = AppConfigV1::default();
        ensure_local_model_presets(&mut config);
        let route = config
            .speech
            .voice_routes
            .iter()
            .find(|route| route.id == LOCAL_PRESET_ROUTE_ID)
            .unwrap();
        for provider_id in [
            route.asr_provider_id.as_ref(),
            route.llm_provider_id.as_ref(),
            route.tts_provider_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                LOCAL_PRESET_PROVIDER_IDS.contains(&provider_id.as_str()),
                "路由引用了未播种的供应商 {provider_id}"
            );
        }
        assert!(!route.ready && !route.active);
        // 模板不改变用户当前激活线路。
        assert!(config.speech.active_voice_route_id.is_none());
    }
}
