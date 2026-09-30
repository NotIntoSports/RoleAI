//! 训练报告的定性点评：提示词构建、LLM 响应解析（含围栏/杂文兜底）、
//! 两次尝试的生成策略，以及把会话转写按题号分组。
//! 客观指标（metrics.rs）永不失败；LLM 两次都失败时由命令层落"仅客观指标"的兜底报告。

use serde::Deserialize;

use crate::providers::{ChatMessage, ChatModel, ProviderEndpoint};
use crate::sessions::SessionTurn;

use super::dto::{PracticeDimensions, PracticeQuestionReview};
use super::plan::{extract_json_object, strip_code_fence};

/// LLM 两次尝试都失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewUnavailable;

/// 把会话轮次按题号分组：面试官 assistant_text 中的【第X题】标注宣布当前题，
/// 其后到达的用户回答归入该题；第一个标注之前的开场白不归入任何题。
/// `plan_questions` 提供题面（缺失时回落到标注原文）。
/// 返回（1 基题号, 题面, 该题全部回答拼接）。
pub fn group_answers_by_question(
    turns: &[SessionTurn],
    plan_questions: Option<&[super::dto::PracticeQuestion]>,
) -> Vec<(usize, String, String)> {
    let mut grouped: Vec<(usize, Vec<String>)> = Vec::new();
    let mut labels: std::collections::BTreeMap<usize, String> = Default::default();
    let mut current_index = 0usize;
    let mut question_announced = false;
    for turn in turns {
        // 先归档本轮回答（它属于此前已宣布的题），再处理本轮面试官消息里的
        // 【第X题】标注（它引出的是下一轮回答）。
        if question_announced && !turn.user_text.trim().is_empty() {
            match grouped
                .iter_mut()
                .find(|(index, _)| *index == current_index)
            {
                Some(entry) => entry.1.push(turn.user_text.clone()),
                None => grouped.push((current_index, vec![turn.user_text.clone()])),
            }
        }
        if let Some(number) = super::director::question_number_from_transcript(
            std::slice::from_ref(&turn.assistant_text),
        ) {
            current_index = number.saturating_sub(1);
            let label = plan_questions
                .and_then(|questions| questions.get(current_index))
                .map(|question| question.prompt.clone())
                .unwrap_or_else(|| extract_marker_line(&turn.assistant_text));
            labels.insert(current_index, label);
            question_announced = true;
        }
    }
    grouped
        .into_iter()
        .map(|(index, answers)| {
            let question = labels
                .get(&index)
                .cloned()
                .unwrap_or_else(|| "（题面缺失）".to_owned());
            (index + 1, question, answers.join("\n\n"))
        })
        .collect()
}

fn extract_marker_line(text: &str) -> String {
    match text.find("【第") {
        Some(start) => {
            let rest = &text[start..];
            match rest.find('】') {
                Some(close) => {
                    let line_end = rest[close..]
                        .find('\n')
                        .map(|offset| close + offset)
                        .unwrap_or(rest.len());
                    rest[..line_end].trim().to_owned()
                }
                None => rest.trim().to_owned(),
            }
        }
        None => text.chars().take(80).collect(),
    }
}

/// 组装定性点评的消息（严格 JSON 输出）。
pub fn build_review_messages(
    position: &str,
    interviewer_style: &str,
    grouped: &[(usize, String, String)],
) -> Vec<ChatMessage> {
    let mut transcript = String::new();
    for (number, question, answer) in grouped {
        transcript.push_str(&format!(
            "【第{number}题】{question}\n候选人回答：{answer}\n\n"
        ));
    }
    let user = format!(
        "岗位：{position}\n面试官风格：{interviewer_style}\n\n逐题转写（题目/候选人回答）：\n{transcript}\
         请逐题点评并给出总体维度评分。分数为 1～5（可为 0.5 步进）。只输出 JSON：\
         {{\"perQuestion\":[{{\"index\":1,\"score\":4,\"strengths\":[\"优点\"],\"issues\":[\"问题\"],\"modelAnswer\":\"改进示范\"}}],\
         \"dimensions\":{{\"contentDepth\":4,\"structureClarity\":4,\"fluency\":4,\"jobFit\":4}},\
         \"totalScore\":4,\"topSuggestions\":[\"建议1\",\"建议2\",\"建议3\"]}}"
    );
    vec![
        ChatMessage {
            role: "system".into(),
            content: "你是面试教练。只依据转写文本点评，不编造候选人未说的内容。评分只作为练习参考。严格输出 JSON。".into(),
        },
        ChatMessage {
            role: "user".into(),
            content: user,
        },
    ]
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedReview {
    #[serde(default)]
    per_question: Vec<GeneratedQuestionReview>,
    #[serde(default)]
    dimensions: GeneratedDimensions,
    #[serde(default)]
    total_score: f64,
    #[serde(default)]
    top_suggestions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeneratedQuestionReview {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    score: f64,
    #[serde(default)]
    strengths: Vec<String>,
    #[serde(default)]
    issues: Vec<String>,
    #[serde(default)]
    model_answer: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct GeneratedDimensions {
    #[serde(default)]
    content_depth: f64,
    #[serde(default)]
    structure_clarity: f64,
    #[serde(default)]
    fluency: f64,
    #[serde(default)]
    job_fit: f64,
}

fn clamp_score(value: f64) -> f64 {
    value.clamp(0.0, 5.0)
}

/// 解析点评 JSON（直接解析 → 剥围栏 → 首尾大括号提取，三层兜底），
/// 分数裁剪到 1..=5、建议最多 5 条、单条文本截断到 300 字。
pub fn parse_generated_review(
    response: &str,
) -> Result<
    (
        Vec<PracticeQuestionReview>,
        PracticeDimensions,
        f64,
        Vec<String>,
    ),
    ReviewUnavailable,
> {
    let text = response.trim();
    if text.is_empty() {
        return Err(ReviewUnavailable);
    }
    let parsed: GeneratedReview = serde_json::from_str(text)
        .or_else(|_| serde_json::from_str(&strip_code_fence(text)))
        .or_else(|_| serde_json::from_str(&extract_json_object(text)))
        .map_err(|_| ReviewUnavailable)?;
    let per_question = parsed
        .per_question
        .into_iter()
        .take(12)
        .map(|review| PracticeQuestionReview {
            index: review.index.max(1),
            question: String::new(),
            answer: String::new(),
            score: clamp_score(review.score),
            strengths: clip_list(review.strengths, 5),
            issues: clip_list(review.issues, 5),
            model_answer: clip_line(&review.model_answer),
        })
        .collect();
    let dimensions = PracticeDimensions {
        content_depth: clamp_score(parsed.dimensions.content_depth),
        structure_clarity: clamp_score(parsed.dimensions.structure_clarity),
        fluency: clamp_score(parsed.dimensions.fluency),
        job_fit: clamp_score(parsed.dimensions.job_fit),
    };
    let total_score = clamp_score(parsed.total_score);
    let top_suggestions = clip_list(parsed.top_suggestions, 3);
    Ok((per_question, dimensions, total_score, top_suggestions))
}

fn clip_line(value: &str) -> String {
    value.trim().chars().take(300).collect()
}

fn clip_list(values: Vec<String>, limit: usize) -> Vec<String> {
    values
        .into_iter()
        .map(|value| clip_line(&value))
        .filter(|value| !value.is_empty())
        .take(limit)
        .collect()
}

/// 调用现有 LLM 通道生成点评；最多两次尝试（非法 JSON 或通道失败都重试一次）。
/// 两次都失败返回 `ReviewUnavailable`（命令层落仅客观指标的兜底报告）。
pub fn generate_review_with_model(
    model: &dyn ChatModel,
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
    model_id: &str,
    messages: &[ChatMessage],
) -> Result<
    (
        Vec<PracticeQuestionReview>,
        PracticeDimensions,
        f64,
        Vec<String>,
    ),
    ReviewUnavailable,
> {
    let mut last_error = ReviewUnavailable;
    for _ in 0..2 {
        match model.complete(endpoint, credential, model_id, messages) {
            Ok(response) => match parse_generated_review(&response) {
                Ok(review) => return Ok(review),
                Err(error) => last_error = error,
            },
            Err(_) => last_error = ReviewUnavailable,
        }
    }
    Err(last_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(user: &str, assistant: &str, index: i64) -> SessionTurn {
        SessionTurn {
            id: format!("turn-{index}"),
            session_id: "sess".into(),
            turn_index: index,
            user_text: user.into(),
            assistant_text: assistant.into(),
            materials_used: false,
            created_at: "2026-10-01T00:00:0Z".into(),
        }
    }

    #[test]
    fn grouping_uses_markers_and_plan_prompts() {
        let plan_questions = vec![
            super::super::dto::PracticeQuestion {
                prompt: "介绍一个你负责的项目".into(),
                focus: String::new(),
                expected_points: Vec::new(),
                followups: Vec::new(),
            },
            super::super::dto::PracticeQuestion {
                prompt: "如何设计限流".into(),
                focus: String::new(),
                expected_points: Vec::new(),
                followups: Vec::new(),
            },
        ];
        let turns = vec![
            turn("你好", "【第1题】介绍一个你负责的项目", 0),
            turn("我做过支付网关", "追问：最大的困难？", 1),
            turn("并发削峰", "【第2题】如何设计限流？", 2),
            turn("令牌桶加熔断", "", 3),
        ];
        let grouped = group_answers_by_question(&turns, Some(&plan_questions));
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, 1);
        assert_eq!(grouped[0].1, "介绍一个你负责的项目");
        assert!(grouped[0].2.contains("我做过支付网关"));
        assert!(grouped[0].2.contains("并发削峰"));
        assert_eq!(grouped[1].0, 2);
        assert_eq!(grouped[1].1, "如何设计限流");
        assert_eq!(grouped[1].2, "令牌桶加熔断");
    }

    #[test]
    fn grouping_without_plan_falls_back_to_marker_line() {
        let turns = vec![
            turn("你好", "【第3题】分布式事务怎么保证一致性？", 0),
            turn("两阶段加补偿", "", 1),
        ];
        let grouped = group_answers_by_question(&turns, None);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].0, 3);
        assert!(grouped[0].1.contains("【第3题】"));
        assert_eq!(grouped[0].2, "两阶段加补偿");
    }

    #[test]
    fn grouping_drops_answers_before_any_question_was_announced() {
        let turns = vec![turn("随便聊聊", "今天状态不错", 0)];
        assert!(group_answers_by_question(&turns, None).is_empty());
    }

    #[test]
    fn review_messages_carry_transcript_and_json_contract() {
        let grouped = vec![(1usize, "题目一".to_owned(), "回答一".to_owned())];
        let messages = build_review_messages("后端", "严苛面试官", &grouped);
        assert_eq!(messages[0].role, "system");
        let body = &messages[1].content;
        assert!(body.contains("岗位：后端"));
        assert!(body.contains("【第1题】题目一"));
        assert!(body.contains("回答一"));
        assert!(body.contains("contentDepth"));
        assert!(body.contains("topSuggestions"));
    }

    #[test]
    fn parse_review_accepts_valid_json_and_clamps() {
        let response = r#"{"perQuestion":[
            {"index":1,"score":4.5,"strengths":["结构清晰"],"issues":[],"modelAnswer":"可以补充量化结果"},
            {"index":2,"score":9,"strengths":[],"issues":["偏题"],"modelAnswer":""}
        ],"dimensions":{"contentDepth":4,"structureClarity":3,"fluency":6,"jobFit":2},
        "totalScore":3.5,"topSuggestions":["A","B","C","D"]}"#;
        let (per_question, dimensions, total, suggestions) =
            parse_generated_review(response).unwrap();
        assert_eq!(per_question.len(), 2);
        assert!((per_question[0].score - 4.5).abs() < f64::EPSILON);
        assert!((per_question[1].score - 5.0).abs() < f64::EPSILON); // 9 → clamp 5
        assert!((dimensions.fluency - 5.0).abs() < f64::EPSILON); // 6 → clamp 5
        assert!((total - 3.5).abs() < f64::EPSILON);
        assert_eq!(suggestions.len(), 3); // cap 3
    }

    #[test]
    fn parse_review_falls_back_to_fence_and_prose() {
        let fenced = "```json\n{\"perQuestion\":[{\"index\":1,\"score\":3}],\"totalScore\":3}\n```";
        assert!(parse_generated_review(fenced).is_ok());
        let prose = "点评如下：{\"dimensions\":{\"fluency\":4},\"totalScore\":4} 完";
        let (_, dimensions, total, _) = parse_generated_review(prose).unwrap();
        assert!((dimensions.fluency - 4.0).abs() < f64::EPSILON);
        assert!((total - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_review_rejects_garbage_and_empty() {
        assert_eq!(
            parse_generated_review("不是 JSON").unwrap_err(),
            ReviewUnavailable
        );
        assert_eq!(parse_generated_review("").unwrap_err(), ReviewUnavailable);
    }

    /// 题目标记行提取的退化输入：只有【第 没有右括号时取到行尾，
    /// 完全没有标记时截取前 80 字作题面占位。
    #[test]
    fn marker_line_extraction_degrades_gracefully() {
        assert_eq!(extract_marker_line("【第3题"), "【第3题");
        assert_eq!(
            extract_marker_line("【第3题 没有右括号"),
            "【第3题 没有右括号"
        );
        let long = "这是一段完全没有题目标记的回答文本。".repeat(10);
        let extracted = extract_marker_line(&long);
        assert_eq!(extracted.chars().count(), 80);
        assert_eq!(extract_marker_line("短文本"), "短文本");
    }

    /// 点评通道失败必须以 ReviewUnavailable 收尾，且恰好重试两次；
    /// 第一次失败、第二次成功的恢复路径也要接得住。
    use crate::providers::{CascadeError, CascadeStage};

    struct BrokenModel {
        failures: std::sync::atomic::AtomicUsize,
        fail_times: usize,
        payload: &'static str,
    }

    impl ChatModel for BrokenModel {
        fn complete(
            &self,
            _: &ProviderEndpoint,
            _: Option<&str>,
            _: &str,
            _: &[ChatMessage],
        ) -> Result<String, CascadeError> {
            if self.failures.load(std::sync::atomic::Ordering::SeqCst) < self.fail_times {
                self.failures
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return Err(CascadeError::RequestFailed(CascadeStage::Llm));
            }
            Ok(self.payload.to_owned())
        }
    }

    #[test]
    fn review_generation_fails_closed_after_two_attempts() {
        let model = BrokenModel {
            failures: std::sync::atomic::AtomicUsize::new(0),
            fail_times: 2,
            payload: "",
        };
        let messages = build_review_messages("后端", "严苛面试官", &[]);
        let result = generate_review_with_model(
            &model,
            &ProviderEndpoint {
                provider_id: "fake".into(),
                base_url: String::new(),
            },
            None,
            "model-x",
            &messages,
        );
        assert_eq!(result.unwrap_err(), ReviewUnavailable);
        assert_eq!(
            model.failures.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "必须恰好尝试两次"
        );
    }

    #[test]
    fn review_generation_retries_after_transport_failure() {
        let model = BrokenModel {
            failures: std::sync::atomic::AtomicUsize::new(0),
            fail_times: 1,
            payload: r#"{"perQuestion":[{"index":1,"score":3}],"totalScore":3}"#,
        };
        let messages = build_review_messages("后端", "严苛面试官", &[]);
        let result = generate_review_with_model(
            &model,
            &ProviderEndpoint {
                provider_id: "fake".into(),
                base_url: String::new(),
            },
            None,
            "model-x",
            &messages,
        );
        let (_, _, total, _) = result.unwrap();
        assert!((total - 3.0).abs() < f64::EPSILON);
        assert_eq!(
            model.failures.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "第二次尝试必须成功"
        );
    }
}
