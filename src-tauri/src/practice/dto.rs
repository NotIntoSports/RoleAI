//! 模拟面试训练的公共 DTO（serde + ts_rs 导出到 src/generated/bindings.ts）。
//! 命令层在 crate::commands::practice；纯计算在 metrics / plan / director。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 题单里的单道题目。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct PracticeQuestion {
    pub prompt: String,
    #[serde(default)]
    pub focus: String,
    #[serde(default)]
    pub expected_points: Vec<String>,
    #[serde(default)]
    pub followups: Vec<String>,
}

/// 一份可编辑的题单。保存/编辑/删除走 practice_plan_* 命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct PracticePlan {
    pub id: String,
    pub title: String,
    pub position: String,
    pub interviewer_style: String,
    pub difficulty: String,
    pub questions: Vec<PracticeQuestion>,
    pub created_at: String,
    pub updated_at: String,
}

impl PracticePlan {
    pub fn question_count(&self) -> usize {
        self.questions.len()
    }
}

/// 题单列表条目（不含题面明细）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PracticePlanSummary {
    pub id: String,
    pub title: String,
    pub position: String,
    pub interviewer_style: String,
    pub difficulty: String,
    #[ts(type = "number")]
    pub question_count: usize,
    pub created_at: String,
    pub updated_at: String,
}

/// 题单生成入参：JD / 简历资料 id 均可选，至少要有岗位方向。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct PracticePlanGenerateInput {
    pub position: String,
    #[serde(default)]
    #[ts(optional)]
    pub jd_material_id: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub resume_material_id: Option<String>,
    pub interviewer_style: String,
    pub question_count: u8,
    pub difficulty: String,
}

/// 开始训练：注入题单 overlay 并启动一次普通会话。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase")]
pub struct PracticeSessionStartInput {
    pub plan_id: String,
    #[serde(default)]
    #[ts(optional)]
    pub role_profile_id: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub voice_route_id: Option<String>,
}

/// 训练进度（E07 信息条与报告页共用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PracticeProgress {
    pub plan_id: String,
    #[ts(type = "number")]
    pub question_index: usize,
    #[ts(type = "number")]
    pub total_questions: usize,
    #[ts(type = "number")]
    pub followups_used: u32,
    #[ts(type = "number")]
    pub followup_limit: u32,
    pub finished: bool,
}

/// 报告的四个维度评分（1～5；LLM 不可用时为 0 表示未评）。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PracticeDimensions {
    pub content_depth: f64,
    pub structure_clarity: f64,
    pub fluency: f64,
    pub job_fit: f64,
}

impl Default for PracticeDimensions {
    fn default() -> Self {
        Self {
            content_depth: 0.0,
            structure_clarity: 0.0,
            fluency: 0.0,
            job_fit: 0.0,
        }
    }
}

/// 单题点评（题目与回答由本地转写补全；评分与点评来自 LLM）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PracticeQuestionReview {
    #[ts(type = "number")]
    pub index: usize,
    pub question: String,
    pub answer: String,
    pub score: f64,
    pub strengths: Vec<String>,
    pub issues: Vec<String>,
    pub model_answer: String,
}

/// 一份完整的训练报告（落库与前端展示共用同一结构）。
/// `llm_available` 为 false 时定性部分为空，前端显示"定性点评生成失败，可重试"。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PracticeReport {
    pub session_id: String,
    pub plan_id: String,
    pub position: String,
    pub interviewer_style: String,
    pub llm_available: bool,
    pub total_score: f64,
    pub dimensions: PracticeDimensions,
    pub per_question: Vec<PracticeQuestionReview>,
    pub top_suggestions: Vec<String>,
    pub objective: crate::practice::metrics::PracticeMetrics,
    pub created_at: String,
}

/// 报告列表条目（按 created_at 倒序，用于成长曲线与训练历史）。
/// dimensions / duration 供 E09 历史列表与成长曲线直接使用，避免逐条拉全量报告。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct PracticeReportSummary {
    pub session_id: String,
    pub plan_id: String,
    pub position: String,
    pub interviewer_style: String,
    pub total_score: f64,
    #[serde(default)]
    pub dimensions: PracticeDimensions,
    /// 报告内客观指标的总回答时长；时间信息不全时为 None。
    #[serde(default)]
    pub duration_seconds: Option<f64>,
    pub created_at: String,
}
