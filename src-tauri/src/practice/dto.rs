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
