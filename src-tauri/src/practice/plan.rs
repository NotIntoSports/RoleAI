//! 题单生成：提示词构建、LLM 响应的 JSON 解析与上限裁剪。
//! LLM 通道复用现有 `ChatModel`（生产用 `OpenAiCompatibleCascade`，
//! 测试注入假实现），本模块不做网络与配置读取。

use serde::Deserialize;

use crate::providers::{ChatMessage, ChatModel, ProviderEndpoint};

use super::dto::{PracticePlanGenerateInput, PracticeQuestion};

/// 题单长度上限（与命令层入参校验一致）。
pub const MAX_PLAN_QUESTIONS: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanParseError {
    ResponseInvalid,
    ResponseEmpty,
}

impl PlanParseError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ResponseInvalid | Self::ResponseEmpty => "PRACTICE_MODEL_RESPONSE_INVALID",
        }
    }
}

/// 题单生成的 LLM 输出结构（宽松解析：缺省字段回落为空）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedPlan {
    #[serde(default)]
    questions: Vec<GeneratedQuestion>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedQuestion {
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    focus: String,
    #[serde(default)]
    expected_points: Vec<String>,
    #[serde(default)]
    followups: Vec<String>,
}

/// 组装题单生成的消息。只依据提供的资料片段出题，要求严格输出 JSON。
pub fn build_plan_messages(
    input: &PracticePlanGenerateInput,
    documents: &[String],
) -> Vec<ChatMessage> {
    let mut prompt = format!(
        "岗位方向：{}\n难度：{}\n面试官风格：{}\n请生成 {} 道面试题",
        input.position, input.difficulty, input.interviewer_style, input.question_count
    );
    if !documents.is_empty() {
        prompt.push_str("\n\n资料片段：\n");
        prompt.push_str(&documents.join("\n\n---\n\n"));
    }
    prompt.push_str(
        "\n\n只依据以上资料出题（没有资料时按岗位方向通用出题），不得编造候选人简历里没有的经历。只输出 JSON："
    );
    prompt.push_str(
        "{\"questions\":[{\"prompt\":\"题目\",\"focus\":\"考察点\",\"expectedPoints\":[\"要点\"],\"followups\":[\"可能追问\"]}]}",
    );
    vec![
        ChatMessage {
            role: "system".into(),
            content: "你是面试出题编辑。只能使用提供的资料，并严格输出 JSON。".into(),
        },
        ChatMessage {
            role: "user".into(),
            content: prompt,
        },
    ]
}

/// 解析并裁剪 LLM 返回的题单 JSON。
/// 每道题的题面必须非空（≤500 字符）；字段缺失回落为空；
/// 超过上限的题目被丢弃；空题单或非法 JSON 都算失败。
pub fn parse_generated_plan(
    response: &str,
    max_questions: usize,
) -> Result<Vec<PracticeQuestion>, PlanParseError> {
    let text = response.trim();
    if text.is_empty() {
        return Err(PlanParseError::ResponseEmpty);
    }
    let parsed: GeneratedPlan =
        serde_json::from_str(text).map_err(|_| PlanParseError::ResponseInvalid)?;
    let mut questions = Vec::new();
    for question in parsed.questions {
        let prompt = question.prompt.trim().to_owned();
        if prompt.is_empty() || prompt.chars().count() > 500 {
            continue;
        }
        questions.push(PracticeQuestion {
            prompt,
            focus: clip_line(&question.focus),
            expected_points: question
                .expected_points
                .iter()
                .map(|point| clip_line(point))
                .filter(|point| !point.is_empty())
                .take(8)
                .collect(),
            followups: question
                .followups
                .iter()
                .map(|followup| clip_line(followup))
                .filter(|followup| !followup.is_empty())
                .take(5)
                .collect(),
        });
        if questions.len() >= max_questions {
            break;
        }
    }
    if questions.is_empty() {
        return Err(PlanParseError::ResponseInvalid);
    }
    Ok(questions)
}

fn clip_line(value: &str) -> String {
    value.trim().chars().take(200).collect()
}

/// 调用现有 LLM 通道生成题单题目。超时/网络失败原样返回 `CascadeError`
/// （命令层映射为可重试错误）；响应非法返回 `PlanParseError`。
pub fn generate_plan_with_model(
    model: &dyn ChatModel,
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
    model_id: &str,
    input: &PracticePlanGenerateInput,
    documents: &[String],
) -> Result<Vec<PracticeQuestion>, PlanGenerationError> {
    let messages = build_plan_messages(input, documents);
    let response = model
        .complete(endpoint, credential, model_id, &messages)
        .map_err(|_| PlanGenerationError::ModelUnavailable)?;
    parse_generated_plan(&response, input.question_count.max(1) as usize)
        .map_err(PlanGenerationError::Parse)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanGenerationError {
    /// LLM 通道不可用（网络、超时、鉴权等；对用户表现为可重试）。
    ModelUnavailable,
    /// 响应不是合法题单 JSON。
    Parse(PlanParseError),
}

impl PlanGenerationError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::ModelUnavailable => "PRACTICE_MODEL_UNAVAILABLE",
            Self::Parse(error) => error.code(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(count: u8) -> PracticePlanGenerateInput {
        PracticePlanGenerateInput {
            position: "后端工程师".into(),
            jd_material_id: None,
            resume_material_id: None,
            interviewer_style: "面试官".into(),
            question_count: count,
            difficulty: "standard".into(),
        }
    }

    #[test]
    fn build_plan_messages_includes_position_and_documents() {
        let messages = build_plan_messages(
            &input(5),
            &[
                "资料：JD\n负责网关开发".to_owned(),
                "资料：简历\n做过支付系统".to_owned(),
            ],
        );
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "user");
        let body = &messages[1].content;
        assert!(body.contains("岗位方向：后端工程师"));
        assert!(body.contains("生成 5 道面试题"));
        assert!(body.contains("负责网关开发"));
        assert!(body.contains("做过支付系统"));
        assert!(body.contains("expectedPoints"));
    }

    #[test]
    fn build_plan_messages_without_documents_still_works() {
        let messages = build_plan_messages(&input(3), &[]);
        assert!(messages[1].content.contains("岗位方向：后端工程师"));
        assert!(!messages[1].content.contains("资料片段"));
    }

    #[test]
    fn parse_plan_accepts_valid_json_and_clips() {
        let response = r#"{"questions":[
            {"prompt":"介绍一个你负责的项目","focus":"项目深度","expectedPoints":["背景","结果"],"followups":["最大困难是什么"]},
            {"prompt":"  ","focus":"空题面应被丢弃"},
            {"prompt":"如何设计限流","focus":"工程判断","expectedPoints":[],"followups":[]}
        ]}"#;
        let questions = parse_generated_plan(response, 12).unwrap();
        assert_eq!(questions.len(), 2);
        assert_eq!(questions[0].prompt, "介绍一个你负责的项目");
        assert_eq!(questions[0].expected_points, vec!["背景", "结果"]);
        assert_eq!(questions[1].prompt, "如何设计限流");
    }

    #[test]
    fn parse_plan_rejects_invalid_and_empty_json() {
        assert_eq!(
            parse_generated_plan("这不是 JSON", 12).unwrap_err(),
            PlanParseError::ResponseInvalid
        );
        assert_eq!(
            parse_generated_plan("", 12).unwrap_err(),
            PlanParseError::ResponseEmpty
        );
        assert_eq!(
            parse_generated_plan(r#"{"questions":[]}"#, 12).unwrap_err(),
            PlanParseError::ResponseInvalid
        );
    }

    #[test]
    fn parse_plan_enforces_question_cap() {
        let question = r#"{"prompt":"题目","focus":"","expectedPoints":[],"followups":[]}"#;
        let response = format!("{{\"questions\":[{},{},{}]}}", question, question, question);
        let questions = parse_generated_plan(&response, 2).unwrap();
        assert_eq!(questions.len(), 2);
    }

    #[test]
    fn parse_plan_tolerates_missing_optional_fields() {
        let response = r#"{"questions":[{"prompt":"只问问题"}]}"#;
        let questions = parse_generated_plan(response, 12).unwrap();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0].focus, "");
        assert!(questions[0].expected_points.is_empty());
    }
}
