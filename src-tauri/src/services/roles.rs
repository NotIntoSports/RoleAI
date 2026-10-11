use serde::Deserialize;
use ts_rs::TS;

use crate::config::{ConfigError, ConfigStore, RoleProfileConfig, RoleScenario};

const MAX_SYSTEM_PROMPT_BYTES: usize = 32 * 1024;
const MAX_OPENING_MESSAGE_BYTES: usize = 4 * 1024;
const MAX_STYLE_INSTRUCTIONS_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct RoleProfileSaveInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub system_prompt: String,
    pub opening_message: String,
    pub style_instructions: String,
    #[serde(default)]
    #[ts(optional)]
    pub tavus_face_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct RoleProfileCopyInput {
    pub source_id: String,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug)]
pub enum RoleProfileServiceError {
    InvalidId,
    FieldsInvalid,
    CopyIdInUse,
    ReviewRequired,
    NotFound,
    Config(ConfigError),
}

impl RoleProfileServiceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidId => "ROLE_PROFILE_ID_INVALID",
            Self::FieldsInvalid => "ROLE_PROFILE_FIELDS_INVALID",
            Self::CopyIdInUse => "ROLE_PROFILE_COPY_ID_IN_USE",
            Self::ReviewRequired => "ROLE_PROFILE_REVIEW_REQUIRED",
            Self::NotFound => "ROLE_PROFILE_NOT_FOUND",
            Self::Config(error) => error.code(),
        }
    }
}

pub struct RoleProfileService<'a> {
    config: &'a ConfigStore,
}

impl<'a> RoleProfileService<'a> {
    pub fn new(config: &'a ConfigStore) -> Self {
        Self { config }
    }

    pub fn save(
        &self,
        input: RoleProfileSaveInput,
    ) -> Result<RoleProfileConfig, RoleProfileServiceError> {
        let input = clean_save_input(input);
        let preset_scenario = input.id.as_deref().and_then(RoleScenario::from_preset_id);
        // 内置角色不能原地改：写回同名的个人角色（见 select_same_name_personal_role），
        // 没有同名个人角色时才第一次 fork 出新 id。
        let editing_preset = input
            .id
            .as_deref()
            .is_some_and(crate::config::presets::is_preset);
        let requested_id = input
            .id
            .as_deref()
            .filter(|id| !crate::config::presets::is_preset(id));
        let generated_id = super::ids::resolve_optional_id(requested_id)
            .map_err(|_| RoleProfileServiceError::InvalidId)?;
        validate_save_fields(&input)?;
        let mut saved = None;
        self.config
            .update(|config| {
                let id = if editing_preset {
                    select_same_name_personal_role(config, &input.name)
                        .unwrap_or_else(|| generated_id.clone())
                } else {
                    // 个人角色（编辑与新建）名字必须唯一：另一个 id 已用同名（trim 后）时拒绝保存。
                    if config.role_profiles.iter().any(|profile| {
                        profile.id != generated_id && profile.name.trim() == input.name
                    }) {
                        return Err(ConfigError::new(
                            "ROLE_PROFILE_FIELDS_INVALID",
                            "Role name is already used by another role profile",
                        ));
                    }
                    generated_id.clone()
                };
                let config_version = config
                    .role_profiles
                    .iter()
                    .find(|profile| profile.id == id)
                    .map(|profile| profile.config_version.saturating_add(1))
                    .unwrap_or(1);
                let preserved = config
                    .role_profiles
                    .iter()
                    .find(|profile| profile.id == id)
                    .map(|profile| {
                        (
                            profile.scenario.clone(),
                            profile.tavus_pal_id.clone(),
                            profile.tavus_synced_config_version,
                        )
                    });
                let (saved_scenario, tavus_pal_id, tavus_synced_config_version) =
                    preserved.unwrap_or((None, None, None));
                let scenario = preset_scenario.clone().or(saved_scenario);
                let profile = RoleProfileConfig {
                    id: id.clone(),
                    name: input.name.clone(),
                    system_prompt: input.system_prompt.clone(),
                    opening_message: input.opening_message.clone(),
                    style_instructions: input.style_instructions.clone(),
                    scenario,
                    tavus_pal_id,
                    tavus_face_id: input.tavus_face_id.clone(),
                    tavus_synced_config_version,
                    active: true,
                    config_version,
                };
                if let Some(existing) = config
                    .role_profiles
                    .iter_mut()
                    .find(|existing| existing.id == profile.id)
                {
                    *existing = profile.clone();
                } else {
                    config.role_profiles.push(profile.clone());
                }
                for existing in &mut config.role_profiles {
                    existing.active = existing.id == id;
                }
                config.active_role_profile_id = Some(id.clone());
                saved = Some(profile);
                Ok(())
            })
            .map_err(RoleProfileServiceError::Config)?;
        saved.ok_or(RoleProfileServiceError::NotFound)
    }

    pub fn copy(
        &self,
        input: RoleProfileCopyInput,
    ) -> Result<RoleProfileConfig, RoleProfileServiceError> {
        let source_id = input.source_id.trim();
        let id = super::ids::resolve_optional_id(input.id.as_deref())
            .map_err(|_| RoleProfileServiceError::InvalidId)?;
        let mut copied = None;
        self.config
            .update(|config| {
                if config.role_profiles.iter().any(|profile| profile.id == id) {
                    return Err(ConfigError::new(
                        "ROLE_PROFILE_COPY_ID_IN_USE",
                        "Role profile copy ID is already in use",
                    ));
                }
                let source = config
                    .role_profiles
                    .iter()
                    .find(|profile| profile.id == source_id)
                    .ok_or_else(|| {
                        ConfigError::new("ROLE_PROFILE_NOT_FOUND", "Role profile not found")
                    })?;
                if source.config_version == 0 {
                    return Err(ConfigError::new(
                        "ROLE_PROFILE_REVIEW_REQUIRED",
                        "Legacy role profile must be saved before use",
                    ));
                }
                let profile = RoleProfileConfig {
                    id: id.clone(),
                    name: next_copy_name(config, source.name.trim()),
                    system_prompt: source.system_prompt.clone(),
                    opening_message: source.opening_message.clone(),
                    style_instructions: source.style_instructions.clone(),
                    scenario: source
                        .scenario
                        .clone()
                        .or_else(|| RoleScenario::from_preset_id(&source.id)),
                    tavus_pal_id: None,
                    tavus_face_id: source.tavus_face_id.clone(),
                    tavus_synced_config_version: None,
                    active: false,
                    config_version: 1,
                };
                config.role_profiles.push(profile.clone());
                copied = Some(profile);
                Ok(())
            })
            .map_err(map_config_error)?;
        copied.ok_or(RoleProfileServiceError::NotFound)
    }

    pub fn activate(&self, id: &str) -> Result<RoleProfileConfig, RoleProfileServiceError> {
        let id = id.trim();
        let mut activated = None;
        self.config
            .update(|config| {
                let target = config
                    .role_profiles
                    .iter()
                    .find(|profile| profile.id == id)
                    .ok_or_else(|| {
                        ConfigError::new("ROLE_PROFILE_NOT_FOUND", "Role profile not found")
                    })?;
                if target.config_version == 0 {
                    return Err(ConfigError::new(
                        "ROLE_PROFILE_REVIEW_REQUIRED",
                        "Legacy role profile must be saved before use",
                    ));
                }
                for profile in &mut config.role_profiles {
                    profile.active = profile.id == id;
                    if profile.active {
                        activated = Some(profile.clone());
                    }
                }
                config.active_role_profile_id = Some(id.into());
                Ok(())
            })
            .map_err(map_config_error)?;
        activated.ok_or(RoleProfileServiceError::NotFound)
    }

    pub fn delete(&self, id: &str) -> Result<(), RoleProfileServiceError> {
        let id = id.trim();
        if crate::config::presets::is_preset(id) {
            return Err(RoleProfileServiceError::FieldsInvalid);
        }
        self.config
            .update(|config| {
                let count = config.role_profiles.len();
                config.role_profiles.retain(|profile| profile.id != id);
                if config.role_profiles.len() == count {
                    return Err(ConfigError::new(
                        "ROLE_PROFILE_NOT_FOUND",
                        "Role profile not found",
                    ));
                }
                if config.active_role_profile_id.as_deref() == Some(id) {
                    config.active_role_profile_id = None;
                }
                Ok(())
            })
            .map(|_| ())
            .map_err(map_config_error)
    }
}

// 同名个人角色的保留优先级：当前 active 优先，其次 config_version 最高，
// 版本相同则优先已有 tavus_face_id 的。
fn personal_role_priority(profile: &RoleProfileConfig) -> (bool, u32, bool) {
    (
        profile.active,
        profile.config_version,
        profile.tavus_face_id.is_some(),
    )
}

fn select_same_name_personal_role(config: &crate::config::AppConfigV1, name: &str) -> Option<String> {
    let mut best: Option<&RoleProfileConfig> = None;
    for profile in &config.role_profiles {
        if crate::config::presets::is_preset(&profile.id) || profile.name.trim() != name {
            continue;
        }
        let better = match best {
            None => true,
            Some(current) => personal_role_priority(profile) > personal_role_priority(current),
        };
        if better {
            best = Some(profile);
        }
    }
    best.map(|profile| profile.id.clone())
}

/// 复制角色的名字：「{原名} 副本」，同名冲突则「{原名} 副本 2」「副本 3」……
fn next_copy_name(config: &crate::config::AppConfigV1, source_name: &str) -> String {
    let mut candidate = format!("{} 副本", source_name);
    let mut ordinal = 2;
    while config
        .role_profiles
        .iter()
        .any(|profile| profile.name.trim() == candidate)
    {
        candidate = format!("{} 副本 {}", source_name, ordinal);
        ordinal += 1;
    }
    candidate
}

fn clean_save_input(mut input: RoleProfileSaveInput) -> RoleProfileSaveInput {
    input.id = input
        .id
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty());
    input.name = input.name.trim().into();
    input.system_prompt = input.system_prompt.trim().into();
    input.opening_message = input.opening_message.trim().into();
    input.style_instructions = input.style_instructions.trim().into();
    input.tavus_face_id = input
        .tavus_face_id
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty());
    input
}

fn validate_save_fields(input: &RoleProfileSaveInput) -> Result<(), RoleProfileServiceError> {
    let face_valid = input
        .tavus_face_id
        .as_deref()
        .is_none_or(crate::providers::tavus_id_is_valid);
    let fields_valid = !input.name.is_empty()
        && face_valid
        && input.system_prompt.len() <= MAX_SYSTEM_PROMPT_BYTES
        && input.opening_message.len() <= MAX_OPENING_MESSAGE_BYTES
        && input.style_instructions.len() <= MAX_STYLE_INSTRUCTIONS_BYTES;
    if fields_valid {
        Ok(())
    } else {
        Err(RoleProfileServiceError::FieldsInvalid)
    }
}

fn map_config_error(error: ConfigError) -> RoleProfileServiceError {
    match error.code() {
        "ROLE_PROFILE_COPY_ID_IN_USE" => RoleProfileServiceError::CopyIdInUse,
        "ROLE_PROFILE_REVIEW_REQUIRED" => RoleProfileServiceError::ReviewRequired,
        "ROLE_PROFILE_NOT_FOUND" => RoleProfileServiceError::NotFound,
        _ => RoleProfileServiceError::Config(error),
    }
}
