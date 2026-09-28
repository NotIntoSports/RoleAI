use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use ts_rs::TS;

use crate::database::{Database, DatabaseError};

const DEFAULT_TOP_K: u32 = 20;
const MAX_TOP_K: u32 = 20;
const SNIPPET_CHARS: usize = 160;

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MaterialSearchHit {
    pub material_id: String,
    pub chunk_id: String,
    pub file_name: String,
    pub section: String,
    pub snippet: String,
    pub rank: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialRecord {
    pub id: String,
    pub file_name: String,
    pub stored_path: String,
    pub content_sha256: String,
    pub media_type: String,
    pub byte_size: i64,
    pub status: String,
    pub retrieval_blocked: bool,
    pub chunk_count: i64,
}

pub struct NewMaterial<'a> {
    pub id: &'a str,
    pub file_name: &'a str,
    pub stored_path: &'a str,
    pub content_sha256: &'a str,
    pub media_type: &'a str,
    pub byte_size: i64,
    pub parser_version: &'a str,
    pub chunker_version: &'a str,
    pub extracted_text: &'a str,
    pub chunks: &'a [crate::materials::MaterialChunk],
}

pub struct MaterialStore<'a> {
    database: &'a Database,
}

impl<'a> MaterialStore<'a> {
    pub fn new(database: &'a Database) -> Self {
        Self { database }
    }

    pub fn find_by_hash(
        &self,
        content_sha256: &str,
    ) -> Result<Option<MaterialRecord>, DatabaseError> {
        self.get_where("content_sha256 = ?1", params![content_sha256])
    }

    pub fn get(&self, id: &str) -> Result<Option<MaterialRecord>, DatabaseError> {
        self.get_where("id = ?1", params![id])
    }

    pub fn insert_text_ready(&self, material: NewMaterial<'_>) -> Result<(), DatabaseError> {
        let now = chrono::Utc::now().to_rfc3339();
        self.database.with_transaction(|transaction| {
            transaction.execute(
                "INSERT INTO materials(
                    id, file_name, stored_path, content_sha256, media_type, byte_size,
                    status, retrieval_blocked, parser_version, chunker_version,
                    embedding_space_id, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'text_ready', 0, ?7, ?8, NULL, ?9, ?9)",
                params![
                    material.id,
                    material.file_name,
                    material.stored_path,
                    material.content_sha256,
                    material.media_type,
                    material.byte_size,
                    material.parser_version,
                    material.chunker_version,
                    now,
                ],
            )?;
            transaction.execute(
                "INSERT INTO material_documents(material_id, extracted_text, extracted_at)
                 VALUES (?1, ?2, ?3)",
                params![material.id, material.extracted_text, now],
            )?;
            for chunk in material.chunks {
                let chunk_id = format!("{}:{}", material.id, chunk.index);
                let content_sha256 = sha256_hex(chunk.content.as_bytes());
                transaction.execute(
                    "INSERT INTO material_chunks(
                        id, material_id, chunk_index, content, content_sha256, size_estimate,
                        start_char, end_char, section, embedding_status, embedding_space_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'skipped', NULL)",
                    params![
                        chunk_id,
                        material.id,
                        chunk.index,
                        chunk.content,
                        content_sha256,
                        chunk.size_estimate,
                        chunk.start_char as i64,
                        chunk.end_char as i64,
                        chunk.section,
                    ],
                )?;
                transaction.execute(
                    "INSERT INTO material_chunks_fts(content, material_id, chunk_id)
                     VALUES (?1, ?2, ?3)",
                    params![chunk.content, material.id, chunk_id],
                )?;
            }
            Ok(())
        })
    }

    pub fn block_retrieval(&self, id: &str) -> Result<Option<String>, DatabaseError> {
        self.database.with_transaction(|transaction| {
            let stored_path: Option<String> = transaction
                .query_row(
                    "SELECT stored_path FROM materials WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?;
            if stored_path.is_some() {
                transaction.execute(
                    "UPDATE materials SET retrieval_blocked = 1, status = 'deleting', updated_at = ?2
                     WHERE id = ?1",
                    params![id, chrono::Utc::now().to_rfc3339()],
                )?;
            }
            Ok(stored_path)
        })
    }

    pub fn delete_indexed_rows(&self, id: &str) -> Result<(), DatabaseError> {
        self.database.with_transaction(|transaction| {
            let has_vectors: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'material_chunk_vectors')",
                [],
                |row| row.get(0),
            )?;
            if has_vectors {
                transaction.execute(
                    "DELETE FROM material_chunk_vectors
                     WHERE chunk_id IN (SELECT id FROM material_chunks WHERE material_id = ?1)",
                    params![id],
                )?;
            }
            transaction.execute(
                "DELETE FROM material_chunks_fts WHERE material_id = ?1",
                params![id],
            )?;
            transaction.execute(
                "DELETE FROM material_chunks WHERE material_id = ?1",
                params![id],
            )?;
            transaction.execute(
                "DELETE FROM material_documents WHERE material_id = ?1",
                params![id],
            )?;
            transaction.execute("DELETE FROM materials WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn enqueue_cleanup(
        &self,
        stored_path: &str,
        error_code: &str,
    ) -> Result<(), DatabaseError> {
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO material_file_cleanup(stored_path, failed_at, last_error_code)
                 VALUES (?1, ?2, ?3)",
                params![stored_path, chrono::Utc::now().to_rfc3339(), error_code],
            )?;
            Ok(())
        })
    }

    pub fn cleanup_paths(&self) -> Result<Vec<String>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT stored_path FROM material_file_cleanup ORDER BY id")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn remove_cleanup_path(&self, stored_path: &str) -> Result<(), DatabaseError> {
        self.database.with_connection(|connection| {
            connection.execute(
                "DELETE FROM material_file_cleanup WHERE stored_path = ?1",
                params![stored_path],
            )?;
            Ok(())
        })
    }

    pub fn searchable_chunk_count(&self, query: &str) -> Result<i64, DatabaseError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(0);
        }
        Ok(self.search_text(query, DEFAULT_TOP_K)?.len() as i64)
    }

    pub fn search_text(
        &self,
        query: &str,
        top_k: u32,
    ) -> Result<Vec<MaterialSearchHit>, DatabaseError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let top_k = top_k.clamp(1, MAX_TOP_K);
        if let Some(inner) = exact_quoted_inner(query) {
            return self.search_like(inner, top_k);
        }
        if query.chars().count() >= 3 {
            self.search_match(&fts_match_query(query), top_k)
        } else {
            self.search_like(query, top_k)
        }
    }

    pub fn list(&self) -> Result<Vec<MaterialRecord>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT materials.id, file_name, stored_path, content_sha256, media_type, byte_size,
                        status, retrieval_blocked,
                        (SELECT COUNT(*) FROM material_chunks WHERE material_chunks.material_id = materials.id)
                 FROM materials
                 WHERE status != 'deleting'
                 ORDER BY created_at DESC, id ASC",
            )?;
            let rows = statement.query_map([], map_material_record)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    fn search_match(
        &self,
        query: &str,
        top_k: u32,
    ) -> Result<Vec<MaterialSearchHit>, DatabaseError> {
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT materials.id, fts.chunk_id, materials.file_name,
                        material_chunks.section, material_chunks.content,
                        bm25(material_chunks_fts)
                 FROM material_chunks_fts AS fts
                 JOIN materials ON materials.id = fts.material_id
                 JOIN material_chunks ON material_chunks.id = fts.chunk_id
                 WHERE material_chunks_fts MATCH ?1
                   AND materials.retrieval_blocked = 0
                   AND materials.status IN ('text_ready', 'vector_ready')
                 ORDER BY bm25(material_chunks_fts) ASC
                 LIMIT ?2",
            )?;
            let rows = statement.query_map(params![query, top_k], map_search_hit)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    fn search_like(
        &self,
        query: &str,
        top_k: u32,
    ) -> Result<Vec<MaterialSearchHit>, DatabaseError> {
        let pattern = format!("%{}%", escape_like(query));
        self.database.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT materials.id, material_chunks.id, materials.file_name,
                        material_chunks.section, material_chunks.content, 0.0
                 FROM material_chunks
                 JOIN materials ON materials.id = material_chunks.material_id
                 WHERE material_chunks.content LIKE ?1 ESCAPE '\\'
                   AND materials.retrieval_blocked = 0
                   AND materials.status IN ('text_ready', 'vector_ready')
                 LIMIT ?2",
            )?;
            let rows = statement.query_map(params![pattern, top_k], map_search_hit)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    fn get_where(
        &self,
        predicate: &str,
        params: impl rusqlite::Params,
    ) -> Result<Option<MaterialRecord>, DatabaseError> {
        let sql = format!(
            "SELECT materials.id, file_name, stored_path, content_sha256, media_type, byte_size,
                    status, retrieval_blocked,
                    (SELECT COUNT(*) FROM material_chunks WHERE material_chunks.material_id = materials.id)
             FROM materials
             WHERE {predicate}"
        );
        self.database.with_connection(|connection| {
            connection
                .query_row(&sql, params, map_material_record)
                .optional()
        })
    }
}

fn map_material_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MaterialRecord> {
    Ok(MaterialRecord {
        id: row.get(0)?,
        file_name: row.get(1)?,
        stored_path: row.get(2)?,
        content_sha256: row.get(3)?,
        media_type: row.get(4)?,
        byte_size: row.get(5)?,
        status: row.get(6)?,
        retrieval_blocked: row.get::<_, i64>(7)? == 1,
        chunk_count: row.get(8)?,
    })
}

fn exact_quoted_inner(query: &str) -> Option<&str> {
    let query = query.trim();
    let inner = query.strip_prefix('"')?.strip_suffix('"')?;
    if inner.is_empty() || inner.contains('"') {
        return None;
    }
    Some(inner)
}

fn fts_match_query(query: &str) -> String {
    let tokens = extract_search_tokens(query);
    if tokens.is_empty() {
        return fts_phrase(query);
    }
    tokens
        .iter()
        .map(|token| fts_phrase(token))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn fts_phrase(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

fn extract_search_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_cjk = false;

    for ch in query.chars() {
        if is_cjk(ch) {
            if !current.is_empty() && !current_cjk {
                push_ascii_token(&current, &mut tokens);
                current.clear();
            }
            current.push(ch);
            current_cjk = true;
        } else if ch.is_ascii_alphanumeric() {
            if !current.is_empty() && current_cjk {
                push_cjk_tokens(&current, &mut tokens);
                current.clear();
            }
            current.push(ch);
            current_cjk = false;
        } else if !current.is_empty() {
            if current_cjk {
                push_cjk_tokens(&current, &mut tokens);
            } else {
                push_ascii_token(&current, &mut tokens);
            }
            current.clear();
        }
    }
    if !current.is_empty() {
        if current_cjk {
            push_cjk_tokens(&current, &mut tokens);
        } else {
            push_ascii_token(&current, &mut tokens);
        }
    }
    tokens
}

fn push_ascii_token(token: &str, tokens: &mut Vec<String>) {
    if token.chars().count() < 2 {
        return;
    }
    if FTS_OPERATORS
        .iter()
        .any(|operator| token.eq_ignore_ascii_case(operator))
    {
        return;
    }
    tokens.push(token.to_string());
}

fn push_cjk_tokens(block: &str, tokens: &mut Vec<String>) {
    let mut text = block.to_string();
    for stopword in CJK_STOPWORDS {
        text = text.replace(stopword, "\u{1e}");
    }
    for part in text.split('\u{1e}') {
        if part.chars().count() >= 2 {
            tokens.push(part.to_string());
        }
    }
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
    )
}

const CJK_STOPWORDS: &[&str] = &[
    "有没有",
    "做过的",
    "请介绍",
    "介绍",
    "做过",
    "什么",
    "哪些",
    "怎么",
    "如何",
    "是否",
    "一下",
    "项目",
    "请",
    "你",
    "您",
    "我",
    "的",
    "了",
    "吗",
    "呢",
    "吧",
    "啊",
    "是",
    "在",
    "和",
    "与",
    "及",
    "等",
    "做",
    "过",
];

const FTS_OPERATORS: &[&str] = &["AND", "OR", "NOT", "NEAR"];

fn map_search_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<MaterialSearchHit> {
    let content: String = row.get(4)?;
    Ok(MaterialSearchHit {
        material_id: row.get(0)?,
        chunk_id: row.get(1)?,
        file_name: row.get(2)?,
        section: row.get(3)?,
        snippet: snippet(&content),
        rank: row.get(5)?,
    })
}

fn snippet(content: &str) -> String {
    content.chars().take(SNIPPET_CHARS).collect()
}

fn escape_like(query: &str) -> String {
    let mut escaped = String::new();
    for ch in query.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::{exact_quoted_inner, extract_search_tokens, fts_match_query};

    #[test]
    fn extracts_keywords_from_natural_chinese_questions() {
        assert_eq!(
            extract_search_tokens("请介绍你做过的订单服务项目"),
            vec!["订单服务".to_string()]
        );
        assert_eq!(
            extract_search_tokens("订单服务 Kafka"),
            vec!["订单服务".to_string(), "Kafka".to_string()]
        );
        assert_eq!(
            extract_search_tokens("订单服务"),
            vec!["订单服务".to_string()]
        );
        assert_eq!(
            fts_match_query("订单服务 Kafka"),
            "\"订单服务\" AND \"Kafka\""
        );
        assert_eq!(
            exact_quoted_inner("\"请介绍你做过的订单服务项目\""),
            Some("请介绍你做过的订单服务项目")
        );
    }
}
