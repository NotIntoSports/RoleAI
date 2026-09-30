//! 会话数据持久化（与 services/sessions 运行时层区分）：store 是 SQLite 表 CRUD
//! （`SessionStore`），export 生成 Markdown/JSON 导出并附带引用出处。

pub mod export;
pub mod store;

pub use export::{SessionExportError, SessionExportFormat, export_practice_report, export_session};
pub use store::{
    NewCitation, NewSession, NewSnapshot, NewTurn, RuntimeSnapshot, SessionCitation, SessionEvent,
    SessionRecord, SessionStore, SessionTurn,
};
