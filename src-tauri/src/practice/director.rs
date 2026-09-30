//! 训练推进状态机：题号、追问上限、跳过，以及面试官提示词 overlay。
//! 纯计算，不持有数据库与锁；进度持久化走 `practice_meta` 会话事件，
//! 可随时用 `from_events` 重建（未注册的动作/损坏 JSON 一律忽略）。

use serde::Deserialize;

use super::dto::{PracticePlan, PracticeProgress, PracticeQuestion};

/// 每题默认追问上限（需求默认 2，可配置）。
pub const DEFAULT_FOLLOWUP_LIMIT: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectorError {
    PlanEmpty,
}

impl DirectorError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::PlanEmpty => "PRACTICE_PLAN_INVALID",
        }
    }
}

/// 记录一次回答后的推进结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PracticeAdvance {
    /// 仍在当前题：继续追问或在下一轮给收束评价。
    FollowUp,
    /// 本题完成，进入下一题（或已到最后一题之后为 finished）。
    Advanced,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PracticeDirector {
    plan_id: String,
    questions: Vec<PracticeQuestion>,
    question_index: usize,
    followups_used: u32,
    followup_limit: u32,
    finished: bool,
}

impl PracticeDirector {
    pub fn from_plan(plan: &PracticePlan, followup_limit: u32) -> Result<Self, DirectorError> {
        if plan.questions.is_empty() {
            return Err(DirectorError::PlanEmpty);
        }
        Ok(Self {
            plan_id: plan.id.clone(),
            questions: plan.questions.clone(),
            question_index: 0,
            followups_used: 0,
            followup_limit,
            finished: false,
        })
    }

    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }

    pub fn followup_limit(&self) -> u32 {
        self.followup_limit
    }

    pub fn progress(&self) -> PracticeProgress {
        PracticeProgress {
            plan_id: self.plan_id.clone(),
            question_index: self.question_index,
            total_questions: self.questions.len(),
            followups_used: self.followups_used,
            followup_limit: self.followup_limit,
            finished: self.finished,
        }
    }

    pub fn current_question(&self) -> Option<&PracticeQuestion> {
        self.questions.get(self.question_index)
    }

    /// 候选人回答了一次。在追问上限内保持当前题；达到上限自动推进。
    pub fn record_answer(&mut self) -> PracticeAdvance {
        if self.finished {
            return PracticeAdvance::Advanced;
        }
        // 每题回答数 = 1（首答）+ 追问数。回答数超过 1 + 上限时推进。
        if self.followups_used >= self.followup_limit {
            return self.advance();
        }
        self.followups_used += 1;
        PracticeAdvance::FollowUp
    }

    /// 跳过当前题（“下一题”）。已结束返回 false。
    pub fn skip(&mut self) -> bool {
        if self.finished {
            return false;
        }
        self.advance();
        true
    }

    fn advance(&mut self) -> PracticeAdvance {
        self.followups_used = 0;
        if self.question_index + 1 < self.questions.len() {
            self.question_index += 1;
            PracticeAdvance::Advanced
        } else {
            self.finished = true;
            PracticeAdvance::Advanced
        }
    }

    /// 当前题号（1 基；结束后为总题数）。
    pub fn question_number(&self) -> usize {
        if self.finished {
            self.questions.len()
        } else {
            self.question_index + 1
        }
    }

    /// 注入角色 style_instructions 的训练规则（沿用现有角色提示词机制）。
    /// 覆盖整个题单与追问规则，面试官在会话全程按此推进。
    pub fn overlay(&self) -> String {
        let mut overlay = String::new();
        overlay.push_str(
            "你现在正在进行模拟面试训练。严格按以下题单顺序提问，每题开头标注【第X题】：\n",
        );
        for (index, question) in self.questions.iter().enumerate() {
            overlay.push_str(&format!("{}. {}\n", index + 1, question.prompt));
            if !question.focus.trim().is_empty() {
                overlay.push_str(&format!("   （考察点：{}", question.focus.trim()));
                if !question.expected_points.is_empty() {
                    overlay.push_str(&format!(
                        "；期望要点：{}",
                        question.expected_points.join("、")
                    ));
                }
                overlay.push_str("）\n");
            }
        }
        overlay.push_str(&format!(
            "规则：候选人回答后，可针对当前题追问，每题追问最多 {} 次；达到上限或候选人说“下一题”时，先给一句简短收束评价，再进入下一题。全部题目结束后，做 3 句以内的总评并宣布训练结束。不要考察题单以外的内容。\n",
            self.followup_limit
        ));
        overlay
    }

    /// 生成 `practice_meta` 事件 payload（SessionStore::append_event 用）。
    pub fn event_payload(&self, action: &str) -> String {
        serde_json::json!({
            "action": action,
            "planId": self.plan_id,
            "questionIndex": self.question_index,
            "followupsUsed": self.followups_used,
            "followupLimit": self.followup_limit,
            "finished": self.finished,
        })
        .to_string()
    }
}

/// `practice_meta` 事件 payload 的读取视图。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PracticeMetaEvent {
    #[serde(default)]
    plan_id: String,
    #[serde(default)]
    question_index: usize,
    #[serde(default)]
    followups_used: u32,
    #[serde(default)]
    finished: bool,
}

/// 从事件流重建推进器：取最后一条合法 `practice_meta`；
/// 没有（或全部损坏）时从第 0 题开始。忽略题号越界的事件。
pub fn from_events(
    plan: &PracticePlan,
    followup_limit: u32,
    events: &[(&str, &str)],
) -> Result<PracticeDirector, DirectorError> {
    let mut director = PracticeDirector::from_plan(plan, followup_limit)?;
    for (kind, payload) in events {
        if *kind != "practice_meta" {
            continue;
        }
        let Ok(meta) = serde_json::from_str::<PracticeMetaEvent>(payload) else {
            continue;
        };
        if meta.plan_id != plan.id {
            continue;
        }
        if meta.question_index < director.questions.len() {
            director.question_index = meta.question_index;
        }
        director.followups_used = meta.followups_used;
        director.finished = meta.finished && meta.question_index + 1 >= director.questions.len();
    }
    Ok(director)
}

/// 从转写文本提取面试官最近标注的题号（assistant_text 中的【第X题】）。
/// 事件流缺失时的兜底推导（例如会话在别的机器上续跑）。
pub fn question_number_from_transcript(turns: &[String]) -> Option<usize> {
    let mut last: Option<usize> = None;
    for text in turns {
        for candidate in transcript_markers(text) {
            last = Some(candidate);
        }
    }
    last
}

fn transcript_markers(text: &str) -> Vec<usize> {
    let mut numbers = Vec::new();
    // 按字符边界扫描，避免在多字节汉字中间切分。
    let mut offsets: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
    offsets.push(text.len());
    for &index in &offsets {
        if !text[index..].starts_with("【第") {
            continue;
        }
        let rest = &text[index + "【第".len()..];
        if let Some(close) = rest.find("题】")
            && let Ok(number) = rest[..close].trim().parse::<usize>()
        {
            numbers.push(number);
        }
    }
    numbers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(questions: usize) -> PracticePlan {
        PracticePlan {
            id: "plan-1".into(),
            title: "后端一面".into(),
            position: "后端工程师".into(),
            interviewer_style: "严苛面试官".into(),
            difficulty: "standard".into(),
            questions: (0..questions)
                .map(|index| PracticeQuestion {
                    prompt: format!("第{}题的题面", index + 1),
                    focus: "考察点".into(),
                    expected_points: vec!["要点一".into()],
                    followups: Vec::new(),
                })
                .collect(),
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: "2026-10-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn empty_plan_is_rejected() {
        assert_eq!(
            PracticeDirector::from_plan(&plan(0), DEFAULT_FOLLOWUP_LIMIT).unwrap_err(),
            DirectorError::PlanEmpty
        );
    }

    #[test]
    fn answers_follow_up_until_cap_then_advance() {
        let mut director = PracticeDirector::from_plan(&plan(3), 2).unwrap();
        assert_eq!(director.question_number(), 1);

        // 首答 → 追问一次 → 追问第二次（到达上限内）
        assert_eq!(director.record_answer(), PracticeAdvance::FollowUp);
        assert_eq!(director.record_answer(), PracticeAdvance::FollowUp);
        assert_eq!(director.progress().followups_used, 2);
        assert_eq!(director.question_number(), 1);

        // 第 3 次回答超过上限：自动推进到第 2 题
        assert_eq!(director.record_answer(), PracticeAdvance::Advanced);
        assert_eq!(director.question_number(), 2);
        assert_eq!(director.progress().followups_used, 0);
    }

    #[test]
    fn zero_followup_limit_advances_after_first_answer() {
        let mut director = PracticeDirector::from_plan(&plan(2), 0).unwrap();
        assert_eq!(director.record_answer(), PracticeAdvance::Advanced);
        assert_eq!(director.question_number(), 2);
    }

    #[test]
    fn skip_moves_to_next_question_and_reports_finish() {
        let mut director = PracticeDirector::from_plan(&plan(2), 2).unwrap();
        assert!(director.skip());
        assert_eq!(director.question_number(), 2);
        assert!(director.skip());
        assert!(director.progress().finished);
        // 结束后再跳过返回 false
        assert!(!director.skip());
        // 结束后回答不再推进
        assert_eq!(director.record_answer(), PracticeAdvance::Advanced);
        assert!(director.progress().finished);
    }

    #[test]
    fn overlay_lists_all_questions_and_rules() {
        let director = PracticeDirector::from_plan(&plan(2), 2).unwrap();
        let overlay = director.overlay();
        assert!(overlay.contains("1. 第1题的题面"));
        assert!(overlay.contains("2. 第2题的题面"));
        assert!(overlay.contains("【第X题】"));
        assert!(overlay.contains("追问最多 2 次"));
        assert!(overlay.contains("考察点"));
    }

    #[test]
    fn events_roundtrip_and_ignore_corrupt_or_foreign_payloads() {
        let mut director = PracticeDirector::from_plan(&plan(3), 2).unwrap();
        director.record_answer();
        let first = (
            String::from("practice_meta"),
            director.event_payload("answer"),
        );
        director.skip();
        let second = (
            String::from("practice_meta"),
            director.event_payload("skip"),
        );

        let plan = plan(3);
        let rebuilt = from_events(
            &plan,
            2,
            &[
                ("practice_meta", "{broken json"),
                ("turn_meta", "{\"questionIndex\":99}"),
                (
                    "practice_meta",
                    "{\"planId\":\"other-plan\",\"questionIndex\":2}",
                ),
                (&first.0, &first.1),
                (&second.0, &second.1),
            ],
        )
        .unwrap();
        assert_eq!(rebuilt.question_number(), 2);
        assert_eq!(rebuilt.progress().followups_used, 0);
        assert!(!rebuilt.progress().finished);

        // 事件流为空时从头开始。
        let fresh = from_events(&plan, 2, &[]).unwrap();
        assert_eq!(fresh.question_number(), 1);
    }

    #[test]
    fn finish_event_only_finishes_on_last_question() {
        let plan = plan(3);
        let payload = serde_json::json!({
            "action": "advance",
            "planId": "plan-1",
            "questionIndex": 2,
            "followupsUsed": 0,
            "followupLimit": 2,
            "finished": true,
        })
        .to_string();
        let event = ("practice_meta", payload.as_str());
        let rebuilt = from_events(&plan, 2, &[event]).unwrap();
        assert_eq!(rebuilt.question_number(), 3);
        assert!(rebuilt.progress().finished);
    }

    #[test]
    fn transcript_marker_fallback_parses_last_marker() {
        assert_eq!(
            question_number_from_transcript(&[
                "【第1题】请介绍".to_owned(),
                "【第2题】追问内容".to_owned()
            ]),
            Some(2)
        );
        assert_eq!(
            question_number_from_transcript(&["没有标注".to_owned()]),
            None
        );
        assert_eq!(question_number_from_transcript(&[]), None);
        // 非法题号忽略
        assert_eq!(
            question_number_from_transcript(&[
                "【第x题】坏标注".to_owned(),
                "【第3题】好的".to_owned()
            ]),
            Some(3)
        );
    }
}
