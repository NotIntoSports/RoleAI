use super::{AppConfigV1, RoleProfileConfig, RoleScenario};

pub const PRESET_IDS: [&str; 7] = [
    "preset-interviewer",
    "preset-hr",
    "preset-candidate",
    "preset-meeting",
    "preset-presenter",
    "preset-strict-interviewer",
    "preset-expression-coach",
];

pub fn is_preset(id: &str) -> bool {
    PRESET_IDS.contains(&id)
}

/// Seed only missing templates. Existing profiles and the user's active role win.
pub fn ensure_role_presets(config: &mut AppConfigV1) -> bool {
    let definitions = [
        (
            "面试官",
            "你是面试官。围绕用户提供的岗位与候选人资料，一次提出一个明确问题，根据回答追问。不要捏造履历，不作自动录用决定。",
            "你好，我们开始面试。请先简要介绍与你应聘岗位相关的经历。",
        ),
        (
            "HR",
            "你是HR面试助手。了解求职动机、岗位匹配、沟通协作与到岗安排，一次问一个问题。结论交由人工复核。",
            "你好，我是AI面试助手。你为什么对这个岗位感兴趣？",
        ),
        (
            "求职者",
            "你帮助求职者准备回答。只依据用户提供的真实经历和资料，缺少事实时说明需要补充，禁止编造项目或资历。给出简洁、可由用户确认的回答建议。",
            "",
        ),
        (
            "会议助手",
            "你是会议助手。根据会议上下文和指定资料回答点名提问，区分事实、推测和待确认事项。保持简洁，不主动打断讨论。",
            "",
        ),
        (
            "直播讲解员",
            "你是产品直播讲解员。仅依据指定产品资料介绍功能、适用场景和限制，不编造价格、库存、优惠或效果承诺。按确认后的讲稿分段讲解。",
            "",
        ),
        (
            "严苛面试官",
            "你是严苛的技术面试官。围绕用户提供的岗位与简历连续追问，优先考察项目深度、决策取舍与事实边界；发现含糊、矛盾或编造迹象时直接指出并要求澄清。一次只问一个问题，问题具体、有压力但不人身攻击。面试结论交由人工复核，不作自动录用决定。",
            "你好，我们开始。请用两分钟介绍你最有代表性的项目，我会针对细节追问。",
        ),
        (
            "表达教练",
            "你是表达教练。用户给出一段回答或陈述后，先指出最影响效果的少量问题（结构、重点、冗余、口头禅），再给出一条更清晰的改写示范，并说明改动理由。只依据用户提供的真实内容改写，不虚构经历或事实。每次聚焦一个改进点，避免一次性堆砌建议。",
            "",
        ),
    ];
    let mut changed = false;
    for (id, (name, prompt, opening)) in PRESET_IDS.into_iter().zip(definitions) {
        if let Some(existing) = config.role_profiles.iter_mut().find(|role| role.id == id) {
            // 场景回填只针对有映射的预设；纯对话预设（strict/coach）scenario
            // 恒为 None，重复赋值会让「已播种」误报为变更。
            if existing.scenario.is_none() && let Some(scenario) = RoleScenario::from_preset_id(id)
            {
                existing.scenario = Some(scenario);
                changed = true;
            }
            continue;
        }
        config.role_profiles.push(RoleProfileConfig {
            id: id.into(),
            name: name.into(),
            system_prompt: prompt.into(),
            opening_message: opening.into(),
            style_instructions: "使用自然、简洁的中文，每次只处理当前问题。".into(),
            scenario: RoleScenario::from_preset_id(id),
            active: false,
            config_version: 1,
        });
        changed = true;
    }
    // Do not silently switch an existing custom role or activate a legacy role.
    if config.active_role_profile_id.is_none()
        && !config.role_profiles.iter().any(|r| r.active)
        && let Some(role) = config
            .role_profiles
            .iter_mut()
            .find(|r| r.id == "preset-meeting")
    {
        role.active = true;
        config.active_role_profile_id = Some(role.id.clone());
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seeds_once_and_preserves_user_edits_and_selection() {
        let mut config = AppConfigV1::default();
        assert!(ensure_role_presets(&mut config));
        assert_eq!(config.role_profiles.len(), PRESET_IDS.len());
        assert!(!ensure_role_presets(&mut config));
        config.role_profiles[0].system_prompt = "existing user text".into();
        assert!(!ensure_role_presets(&mut config));
        assert_eq!(config.role_profiles[0].system_prompt, "existing user text");
        config.validate().unwrap();
    }

    #[test]
    fn preserves_custom_active_role() {
        let mut config = AppConfigV1::default();
        config.role_profiles.push(RoleProfileConfig {
            id: "custom".into(),
            name: "我的助手".into(),
            system_prompt: "custom instructions".into(),
            opening_message: String::new(),
            style_instructions: String::new(),
            scenario: None,
            active: true,
            config_version: 1,
        });
        config.active_role_profile_id = Some("custom".into());
        ensure_role_presets(&mut config);
        assert_eq!(config.active_role_profile_id.as_deref(), Some("custom"));
        assert_eq!(
            config
                .role_profiles
                .iter()
                .filter(|role| role.active)
                .count(),
            1
        );
        config.validate().unwrap();
    }

    #[test]
    fn backfills_scenario_on_existing_presets_without_overwriting_content() {
        let mut config = AppConfigV1::default();
        config.role_profiles.push(RoleProfileConfig {
            id: "preset-candidate".into(),
            name: "我的求职者模板".into(),
            system_prompt: "keep this".into(),
            opening_message: String::new(),
            style_instructions: String::new(),
            scenario: None,
            active: false,
            config_version: 3,
        });
        assert!(ensure_role_presets(&mut config));
        let role = config
            .role_profiles
            .iter()
            .find(|role| role.id == "preset-candidate")
            .unwrap();
        assert_eq!(role.system_prompt, "keep this");
        assert_eq!(role.scenario, Some(RoleScenario::Candidate));
    }

    #[test]
    fn startup_persists_templates_and_template_edits_make_personal_copies() {
        use crate::services::{RoleProfileSaveInput, RoleProfileService};
        let directory = tempfile::tempdir().unwrap();
        let store = super::super::ConfigStore::new(directory.path().join("config.json"));
        store.load_for_startup().unwrap();
        let original = store.load().unwrap();
        store.load_for_startup().unwrap();
        assert_eq!(store.load().unwrap(), original);
        let roles = RoleProfileService::new(&store);
        let saved = roles
            .save(RoleProfileSaveInput {
                id: Some("preset-meeting".into()),
                name: "自定义会议助手".into(),
                system_prompt: "personal instructions".into(),
                opening_message: String::new(),
                style_instructions: String::new(),
            })
            .unwrap();
        assert!(!is_preset(&saved.id));
        assert_eq!(saved.scenario, Some(super::RoleScenario::MeetingAssistant));
        assert!(roles.delete("preset-meeting").is_err());
        let next = store.load().unwrap();
        assert_eq!(next.role_profiles.len(), PRESET_IDS.len() + 1);
        assert_eq!(
            next.role_profiles.iter().find(|r| r.id == "preset-meeting"),
            original
                .role_profiles
                .iter()
                .find(|r| r.id == "preset-meeting")
        );
    }
}
