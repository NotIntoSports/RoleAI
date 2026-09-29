//! 资料库子系统流水线：parse（PDF/DOCX/TXT 抽文本）→ chunk（分块）→
//! hybrid（关键词 + 向量混合检索）→ store（SQLite 持久化），backup 负责导入导出。

pub mod backup;
pub mod chunk;
pub mod hybrid;
pub mod parse;
pub mod store;

pub use backup::{BackupError, BackupService};
pub use chunk::{CHUNKER_VERSION, MaterialChunk, chunk_text};
pub use hybrid::EmbeddingSpace;
pub use parse::{ParseError, extract_text, parser_version};
pub use store::{MaterialSearchHit, MaterialStore, NewMaterial};
