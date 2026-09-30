//! practice 域：模拟面试训练（题单、训练推进、客观指标、报告）。
//! 本模块不承载 Tauri 命令（命令在 crate::commands::practice），
//! 纯计算与数据结构放在这里以便完全单测。

pub mod director;
pub mod dto;
pub mod metrics;
pub mod plan;
pub mod report;
pub mod store;

pub use self::dto::{
    PracticeDimensions, PracticePlan, PracticePlanGenerateInput, PracticePlanSummary,
    PracticeProgress, PracticeQuestion, PracticeQuestionReview, PracticeReport,
    PracticeReportSummary, PracticeSessionStartInput,
};
