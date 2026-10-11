//! 把本地角色同步成 Tavus PAL，并开关一场中文视频通话。
//! 密钥只经过凭据管理器；本模块不把密钥写入配置或返回值。

use serde_json::Value;
use zeroize::Zeroizing;

use crate::{
    config::{ConfigError, ConfigStore},
    contracts::SecretStatus,
    providers::tavus::{
        TavusApi, TavusConversationStarted, TavusError, TavusFace, api_key_is_valid,
        build_conversation_body, build_pal_body, persona_too_long, tavus_id_is_valid,
    },
    secrets::{SecretError, SecretService},
};

pub const API_KEY_REFERENCE: &str = "tavus/api-key";

#[derive(Debug)]
pub enum TavusCallError {
    KeyMissing,
    KeyInvalid,
    FaceRequired,
    PersonaTooLong,
    RoleNotFound,
    ConversationInvalid,
    Tavus(TavusError),
    Secret(SecretError),
    Config(ConfigError),
}

impl TavusCallError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::KeyMissing => "TAVUS_KEY_MISSING",
            Self::KeyInvalid => "TAVUS_KEY_INVALID",
            Self::FaceRequired => "TAVUS_FACE_REQUIRED",
            Self::PersonaTooLong => "TAVUS_PERSONA_TOO_LONG",
            Self::RoleNotFound => "TAVUS_ROLE_NOT_FOUND",
            Self::ConversationInvalid => "TAVUS_CONVERSATION_INVALID",
            Self::Tavus(error) => error.code(),
            Self::Secret(error) => error.code(),
            Self::Config(error) => error.code(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::KeyMissing => "请先在设置里保存 Tavus 密钥".into(),
            Self::KeyInvalid => "Tavus 密钥无效".into(),
            Self::FaceRequired => "请先为这个角色选择视频形象".into(),
            Self::PersonaTooLong => "人设超过 2 万字符，请缩短后再开始视频通话".into(),
            Self::RoleNotFound => "找不到这个角色".into(),
            Self::ConversationInvalid => "通话编号无效".into(),
            Self::Tavus(TavusError::Unauthorized) => "Tavus 密钥被拒绝".into(),
            Self::Tavus(TavusError::Unreachable) => "连不上 Tavus".into(),
            Self::Tavus(TavusError::Rejected(status)) => {
                format!("Tavus 拒绝了请求（HTTP {status}）")
            }
            Self::Tavus(TavusError::NotFound) => "Tavus 上找不到对应资源".into(),
            Self::Tavus(TavusError::BadResponse) => "Tavus 返回了无法使用的结果".into(),
            Self::Secret(_) => "凭据存储不可用".into(),
            Self::Config(_) => "本地配置不可用".into(),
        }
    }
}

pub struct TavusCallService<'a> {
    config: &'a ConfigStore,
    secrets: &'a SecretService,
    api: &'a dyn TavusApi,
}

impl<'a> TavusCallService<'a> {
    pub fn new(config: &'a ConfigStore, secrets: &'a SecretService, api: &'a dyn TavusApi) -> Self {
        Self {
            config,
            secrets,
            api,
        }
    }

    pub fn credential_status(&self) -> Result<SecretStatus, TavusCallError> {
        self.secrets
            .status(API_KEY_REFERENCE)
            .map_err(TavusCallError::Secret)
    }

    pub fn save_credential(&self, api_key: &str) -> Result<SecretStatus, TavusCallError> {
        if api_key.is_empty() {
            return self
                .secrets
                .delete(API_KEY_REFERENCE)
                .map_err(TavusCallError::Secret);
        }
        if !api_key_is_valid(api_key) {
            return Err(TavusCallError::KeyInvalid);
        }
        self.secrets
            .set(API_KEY_REFERENCE, api_key)
            .map_err(TavusCallError::Secret)
    }

    pub fn list_faces(&self) -> Result<Vec<TavusFace>, TavusCallError> {
        let key = self.read_key()?;
        self.api
            .list_faces(key.as_str())
            .map_err(TavusCallError::Tavus)
    }

    pub fn start(&self, role_id: &str) -> Result<TavusConversationStarted, TavusCallError> {
        let key = self.read_key()?;
        let config = self.config.load().map_err(TavusCallError::Config)?;
        let role = config
            .role_profiles
            .iter()
            .find(|profile| profile.id == role_id)
            .ok_or(TavusCallError::RoleNotFound)?;
        if persona_too_long(&role.system_prompt, &role.style_instructions) {
            return Err(TavusCallError::PersonaTooLong);
        }
        let face_id = role
            .tavus_face_id
            .clone()
            .filter(|id| tavus_id_is_valid(id))
            .ok_or(TavusCallError::FaceRequired)?;
        let body = build_pal_body(
            &role.name,
            &role.system_prompt,
            &role.style_instructions,
            &role.opening_message,
            &face_id,
        );
        let version = role.config_version;
        let role_id = role.id.clone();
        let opening = role.opening_message.clone();
        let name = role.name.clone();
        let pal_id = self.ensure_pal(key.as_str(), role, &body)?;
        self.remember_pal(&role_id, version, &pal_id)?;
        let started = self
            .api
            .create_conversation(
                key.as_str(),
                &build_conversation_body(&pal_id, &name, &opening),
            )
            .map_err(TavusCallError::Tavus)?;
        Ok(started)
    }

    pub fn end(&self, conversation_id: &str) -> Result<(), TavusCallError> {
        if !tavus_id_is_valid(conversation_id) {
            return Err(TavusCallError::ConversationInvalid);
        }
        let key = self.read_key()?;
        self.api
            .end_conversation(key.as_str(), conversation_id)
            .map_err(TavusCallError::Tavus)
    }

    fn ensure_pal(
        &self,
        api_key: &str,
        role: &crate::config::RoleProfileConfig,
        body: &Value,
    ) -> Result<String, TavusCallError> {
        let Some(existing) = role.tavus_pal_id.clone().filter(|id| tavus_id_is_valid(id)) else {
            return self
                .api
                .create_pal(api_key, body)
                .map_err(TavusCallError::Tavus);
        };
        if role.tavus_synced_config_version == Some(role.config_version) {
            return Ok(existing);
        }
        match self.api.update_pal(api_key, &existing, body) {
            Ok(()) => Ok(existing),
            Err(TavusError::NotFound) => self
                .api
                .create_pal(api_key, body)
                .map_err(TavusCallError::Tavus),
            Err(error) => Err(TavusCallError::Tavus(error)),
        }
    }

    fn remember_pal(
        &self,
        role_id: &str,
        version: u32,
        pal_id: &str,
    ) -> Result<(), TavusCallError> {
        self.config
            .update(|config| {
                if let Some(profile) = config
                    .role_profiles
                    .iter_mut()
                    .find(|profile| profile.id == role_id)
                    && profile.config_version == version
                {
                    profile.tavus_pal_id = Some(pal_id.to_owned());
                    profile.tavus_synced_config_version = Some(version);
                }
                Ok(())
            })
            .map(|_| ())
            .map_err(TavusCallError::Config)
    }

    fn read_key(&self) -> Result<Zeroizing<String>, TavusCallError> {
        self.secrets
            .read(API_KEY_REFERENCE)
            .map_err(TavusCallError::Secret)?
            .filter(|value| api_key_is_valid(value))
            .ok_or(TavusCallError::KeyMissing)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};

    use super::*;
    use crate::{
        config::ConfigStore,
        providers::tavus::{CHINESE_INSTRUCTION, TavusApi, TavusConversationStarted, TavusFace},
        secrets::{MemorySecretStore, SecretService},
        services::{RoleProfileSaveInput, RoleProfileService},
    };

    struct Scripted {
        update_missing: bool,
        calls: Mutex<Vec<String>>,
        bodies: Mutex<Vec<Value>>,
    }

    impl Scripted {
        fn new(update_missing: bool) -> Self {
            Self {
                update_missing,
                calls: Mutex::new(Vec::new()),
                bodies: Mutex::new(Vec::new()),
            }
        }
    }

    impl TavusApi for Scripted {
        fn list_faces(&self, api_key: &str) -> Result<Vec<TavusFace>, TavusError> {
            assert!(!api_key.is_empty());
            self.calls.lock().unwrap().push("list".into());
            Ok(vec![TavusFace {
                id: "r9d30b0e55ac".into(),
                name: "Luna".into(),
            }])
        }

        fn create_pal(&self, _api_key: &str, body: &Value) -> Result<String, TavusError> {
            self.calls.lock().unwrap().push("create-pal".into());
            self.bodies.lock().unwrap().push(body.clone());
            Ok("pal123".into())
        }

        fn update_pal(&self, _api_key: &str, pal_id: &str, body: &Value) -> Result<(), TavusError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("update-pal:{pal_id}"));
            self.bodies.lock().unwrap().push(body.clone());
            if self.update_missing {
                Err(TavusError::NotFound)
            } else {
                Ok(())
            }
        }

        fn create_conversation(
            &self,
            _api_key: &str,
            body: &Value,
        ) -> Result<TavusConversationStarted, TavusError> {
            self.calls
                .lock()
                .unwrap()
                .push("create-conversation".into());
            self.bodies.lock().unwrap().push(body.clone());
            Ok(TavusConversationStarted {
                conversation_id: "c123".into(),
                conversation_url: "https://tavus.daily.co/c123".into(),
            })
        }

        fn end_conversation(
            &self,
            _api_key: &str,
            conversation_id: &str,
        ) -> Result<(), TavusError> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("end:{conversation_id}"));
            Ok(())
        }
    }

    fn harness(update_missing: bool) -> (tempfile::TempDir, ConfigStore, SecretService, Scripted) {
        let directory = tempfile::tempdir().unwrap();
        let config = ConfigStore::new(directory.path().join("config.json"));
        let secrets = SecretService::new("test", Arc::new(MemorySecretStore::default())).unwrap();
        secrets.set(API_KEY_REFERENCE, "abcd1234abcd1234").unwrap();
        let api = Scripted::new(update_missing);
        (directory, config, secrets, api)
    }

    fn save_role(config: &ConfigStore, face: Option<&str>, prompt: &str) -> String {
        RoleProfileService::new(config)
            .save(RoleProfileSaveInput {
                id: Some("role-1".into()),
                name: "面试官".into(),
                system_prompt: prompt.into(),
                opening_message: "你好，我们开始吧".into(),
                style_instructions: "语气平稳".into(),
                tavus_face_id: face.map(str::to_owned),
            })
            .unwrap()
            .id
    }

    #[test]
    fn missing_key_face_and_long_persona_do_not_call_tavus() {
        let (_dir, config, secrets, api) = harness(false);
        secrets.delete(API_KEY_REFERENCE).unwrap();
        let service = TavusCallService::new(&config, &secrets, &api);
        assert_eq!(
            service.start("missing").unwrap_err().code(),
            "TAVUS_KEY_MISSING"
        );
        secrets.set(API_KEY_REFERENCE, "abcd1234abcd1234").unwrap();
        let id = save_role(&config, None, "一次只问一个问题");
        assert_eq!(
            service.start(&id).unwrap_err().code(),
            "TAVUS_FACE_REQUIRED"
        );
        let id = save_role(&config, Some("r9d30b0e55ac"), &"a".repeat(20_001));
        assert_eq!(
            service.start(&id).unwrap_err().code(),
            "TAVUS_PERSONA_TOO_LONG"
        );
        assert!(api.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn start_creates_chinese_pal_then_reuses_it_until_the_role_changes() {
        let (_dir, config, secrets, api) = harness(false);
        let id = save_role(&config, Some("r9d30b0e55ac"), "一次只问一个问题");
        let service = TavusCallService::new(&config, &secrets, &api);
        let started = service.start(&id).unwrap();
        assert_eq!(started.conversation_id, "c123");
        assert_eq!(started.conversation_url, "https://tavus.daily.co/c123");
        let stored = config.load().unwrap();
        let role = stored
            .role_profiles
            .iter()
            .find(|role| role.id == id)
            .unwrap();
        assert_eq!(role.tavus_pal_id.as_deref(), Some("pal123"));
        assert_eq!(role.tavus_synced_config_version, Some(role.config_version));
        service.start(&id).unwrap();
        let calls = api.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec!["create-pal", "create-conversation", "create-conversation"]
        );
        let body = api.bodies.lock().unwrap()[0].clone();
        assert_eq!(body["languages"], json!(["zh"]));
        assert!(
            body["system_prompt"]
                .as_str()
                .unwrap()
                .contains(CHINESE_INSTRUCTION)
        );
        assert!(!body.to_string().contains("abcd1234"));
        save_role(&config, Some("r9d30b0e55ac"), "改成两个问题");
        service.start(&id).unwrap();
        assert!(
            api.calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call == "update-pal:pal123")
        );
    }

    #[test]
    fn missing_remote_pal_is_created_again() {
        let (_dir, config, secrets, api) = harness(true);
        let id = save_role(&config, Some("r9d30b0e55ac"), "一次只问一个问题");
        config
            .update(|config| {
                let role = config
                    .role_profiles
                    .iter_mut()
                    .find(|role| role.id == id)
                    .unwrap();
                role.tavus_pal_id = Some("oldpal".into());
                role.tavus_synced_config_version = Some(0);
                Ok(())
            })
            .unwrap();
        TavusCallService::new(&config, &secrets, &api)
            .start(&id)
            .unwrap();
        let calls = api.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec!["update-pal:oldpal", "create-pal", "create-conversation"]
        );
    }

    #[test]
    fn end_rejects_unsafe_ids_and_posts_a_safe_one() {
        let (_dir, config, secrets, api) = harness(false);
        let service = TavusCallService::new(&config, &secrets, &api);
        assert_eq!(
            service.end("../end").unwrap_err().code(),
            "TAVUS_CONVERSATION_INVALID"
        );
        service.end("c123").unwrap();
        assert_eq!(api.calls.lock().unwrap().as_slice(), ["end:c123"]);
    }
}
