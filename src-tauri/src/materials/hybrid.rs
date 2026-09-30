use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, params};

use crate::{
    database::{Database, DatabaseError},
    materials::store::{MaterialSearchHit, MaterialStore},
    providers::{EmbeddingProbe, ProviderEndpoint},
};

const DEFAULT_TOP_K: u32 = 20;
const MAX_TOP_K: u32 = 20;
const CANDIDATE_K: u32 = 20;
const RRF_K: f64 = 60.0;
const SNIPPET_CHARS: usize = 160;
const VEC_TABLE: &str = "material_chunk_vectors";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingSpace {
    pub provider_id: String,
    pub model_id: String,
    pub dimensions: u32,
    pub normalized: bool,
}

pub fn space_fingerprint(space: &EmbeddingSpace) -> String {
    format!(
        "{}|{}|{}|cosine|{}",
        space.provider_id,
        space.model_id,
        space.dimensions,
        if space.normalized { "true" } else { "false" }
    )
}

pub fn index_chunks(
    database: &Database,
    space: &EmbeddingSpace,
    probe: &dyn EmbeddingProbe,
) -> Result<(), DatabaseError> {
    index_chunks_at(
        database,
        space,
        probe,
        &ProviderEndpoint {
            provider_id: space.provider_id.clone(),
            base_url: String::new(),
        },
        None,
    )
}

pub fn index_chunks_at(
    database: &Database,
    space: &EmbeddingSpace,
    probe: &dyn EmbeddingProbe,
    endpoint: &ProviderEndpoint,
    credential: Option<&str>,
) -> Result<(), DatabaseError> {
    if !(1..=65_536).contains(&space.dimensions) {
        return Err(DatabaseError::Operation);
    }
    let fingerprint = space_fingerprint(space);
    let space_id = upsert_space(database, space, &fingerprint)?;
    let table_plan = prepare_vector_table(database, space.dimensions)?;

    let chunks = load_indexable_chunks(database)?;
    let mut outcomes = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let outcome = match probe.embed(
            endpoint,
            credential,
            &space.model_id,
            space.dimensions,
            &chunk.content,
        ) {
            Ok(vector) if usable_vector(&vector, space.dimensions) => {
                ChunkOutcome::Ready(maybe_normalize(vector, space.normalized))
            }
            _ => ChunkOutcome::Failed,
        };
        outcomes.push((chunk, outcome));
    }

    database.with_transaction(|transaction| {
        let ready_count = outcomes
            .iter()
            .filter(|(_, outcome)| matches!(outcome, ChunkOutcome::Ready(_)))
            .count();

        if ready_count > 0 {
            match table_plan {
                VecTablePlan::DimensionChange => {
                    transaction.execute(
                        "UPDATE material_chunks
                         SET embedding_status = 'stale'
                         WHERE embedding_status = 'ready'",
                        [],
                    )?;
                    transaction.execute("DROP TABLE IF EXISTS material_chunk_vectors", [])?;
                    transaction.execute_batch(&create_vec_table_sql(space.dimensions))?;
                }
                VecTablePlan::Missing => {
                    transaction.execute_batch(&create_vec_table_sql(space.dimensions))?;
                }
                VecTablePlan::SameDimension => {
                    transaction.execute("DELETE FROM material_chunk_vectors", [])?;
                }
            }
            transaction.execute(
                "UPDATE material_chunks
                 SET embedding_status = 'stale'
                 WHERE embedding_status = 'ready'
                   AND (embedding_space_id IS NULL OR embedding_space_id != ?1)",
                params![space_id],
            )?;
        }

        for (chunk, outcome) in &outcomes {
            match outcome {
                ChunkOutcome::Ready(vector) => {
                    let embedding =
                        serde_json::to_string(vector).map_err(|_| rusqlite::Error::InvalidQuery)?;
                    transaction.execute(
                        "INSERT INTO material_chunk_vectors(chunk_id, embedding) VALUES (?1, ?2)",
                        params![chunk.id, embedding],
                    )?;
                    transaction.execute(
                        "UPDATE material_chunks
                         SET embedding_status = 'ready', embedding_space_id = ?2
                         WHERE id = ?1",
                        params![chunk.id, space_id],
                    )?;
                }
                ChunkOutcome::Failed => {
                    if ready_count > 0 {
                        transaction.execute(
                            "UPDATE material_chunks
                             SET embedding_status = 'failed', embedding_space_id = NULL
                             WHERE id = ?1",
                            params![chunk.id],
                        )?;
                    } else {
                        transaction.execute(
                            "UPDATE material_chunks
                             SET embedding_status = 'failed', embedding_space_id = NULL
                             WHERE id = ?1
                               AND embedding_status != 'ready'",
                            params![chunk.id],
                        )?;
                    }
                }
            }
        }

        let mut failed_materials = HashSet::new();
        let mut touched_materials = HashSet::new();
        for (chunk, outcome) in &outcomes {
            touched_materials.insert(chunk.material_id.clone());
            if matches!(outcome, ChunkOutcome::Failed) {
                failed_materials.insert(chunk.material_id.clone());
            }
        }
        let now = chrono::Utc::now().to_rfc3339();
        for material_id in touched_materials {
            if failed_materials.contains(&material_id) {
                transaction.execute(
                    "UPDATE materials
                     SET status = 'text_ready', embedding_space_id = NULL, updated_at = ?2
                     WHERE id = ?1",
                    params![material_id, now],
                )?;
            } else {
                transaction.execute(
                    "UPDATE materials
                     SET status = 'vector_ready', embedding_space_id = ?2, updated_at = ?3
                     WHERE id = ?1",
                    params![material_id, space_id, now],
                )?;
            }
        }

        if ready_count > 0 {
            transaction.execute("UPDATE embedding_spaces SET active = 0", [])?;
            transaction.execute(
                "UPDATE embedding_spaces SET active = 1 WHERE id = ?1",
                params![space_id],
            )?;
        }
        Ok(())
    })
}

pub fn search_hybrid(
    database: &Database,
    query: &str,
    query_vector: Option<&[f32]>,
    top_k: Option<u32>,
) -> Result<Vec<MaterialSearchHit>, DatabaseError> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let top_k = top_k.unwrap_or(DEFAULT_TOP_K).clamp(1, MAX_TOP_K);
    let store = MaterialStore::new(database);
    let Some(query_vector) = query_vector else {
        return store.search_text(query, top_k);
    };
    let Some(sql) = vec_table_sql(database)? else {
        return store.search_text(query, top_k);
    };
    let Some(dimensions) = parse_vec_dimensions(&sql) else {
        return store.search_text(query, top_k);
    };
    if query_vector.len() as u32 != dimensions {
        return store.search_text(query, top_k);
    }

    let vectors = search_vectors(database, query_vector)?;
    if vectors.is_empty() {
        return store.search_text(query, top_k);
    }
    let fts = store.search_text(query, CANDIDATE_K)?;
    let fused = reciprocal_rank_fusion(fts, vectors);
    let mut merged = merge_adjacent(fused);
    merged.truncate(top_k as usize);
    Ok(merged)
}

fn upsert_space(
    database: &Database,
    space: &EmbeddingSpace,
    fingerprint: &str,
) -> Result<String, DatabaseError> {
    database.with_transaction(|transaction| {
        if let Some(id) = transaction
            .query_row(
                "SELECT id FROM embedding_spaces WHERE fingerprint = ?1",
                params![fingerprint],
                |row| row.get(0),
            )
            .optional()?
        {
            return Ok(id);
        }
        let id = uuid::Uuid::new_v4().to_string();
        transaction.execute(
            "INSERT INTO embedding_spaces(
                id, provider_id, model_id, dimensions, distance, normalized,
                fingerprint, active, created_at
             ) VALUES (?1, ?2, ?3, ?4, 'cosine', ?5, ?6, 0, ?7)",
            params![
                id,
                space.provider_id,
                space.model_id,
                space.dimensions as i64,
                i64::from(space.normalized),
                fingerprint,
                chrono::Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(id)
    })
}

fn prepare_vector_table(
    database: &Database,
    dimensions: u32,
) -> Result<VecTablePlan, DatabaseError> {
    database.with_connection(|connection| {
        let existing = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name = ?1",
                params![VEC_TABLE],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(match existing.as_deref().and_then(parse_vec_dimensions) {
            Some(current) if current == dimensions => VecTablePlan::SameDimension,
            Some(_) => VecTablePlan::DimensionChange,
            None => VecTablePlan::Missing,
        })
    })
}

fn create_vec_table_sql(dimensions: u32) -> String {
    format!(
        "CREATE VIRTUAL TABLE material_chunk_vectors USING vec0(
            chunk_id TEXT PRIMARY KEY,
            embedding float[{dimensions}] distance_metric=cosine
        )"
    )
}

fn load_indexable_chunks(database: &Database) -> Result<Vec<IndexChunk>, DatabaseError> {
    database.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT material_chunks.id, material_chunks.material_id, material_chunks.content
             FROM material_chunks
             JOIN materials ON materials.id = material_chunks.material_id
             WHERE materials.retrieval_blocked = 0
               AND materials.status IN ('text_ready', 'vector_ready')
             ORDER BY material_chunks.material_id, material_chunks.chunk_index",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(IndexChunk {
                id: row.get(0)?,
                material_id: row.get(1)?,
                content: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

fn vec_table_sql(database: &Database) -> Result<Option<String>, DatabaseError> {
    database.with_connection(|connection| {
        connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name = ?1",
                params![VEC_TABLE],
                |row| row.get(0),
            )
            .optional()
    })
}

fn parse_vec_dimensions(sql: &str) -> Option<u32> {
    let start = sql.find("float[")?;
    let rest = &sql[start + "float[".len()..];
    let end = rest.find(']')?;
    rest[..end].parse().ok()
}

fn search_vectors(
    database: &Database,
    query_vector: &[f32],
) -> Result<Vec<MaterialSearchHit>, DatabaseError> {
    let embedding = serde_json::to_string(query_vector).map_err(|_| DatabaseError::Operation)?;
    database.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT materials.id, knn.chunk_id, materials.file_name,
                    material_chunks.section, material_chunks.content
             FROM (
                SELECT chunk_id, distance
                FROM material_chunk_vectors
                WHERE embedding MATCH ?1
                  AND k = 20
             ) AS knn
             JOIN material_chunks ON material_chunks.id = knn.chunk_id
             JOIN materials ON materials.id = material_chunks.material_id
             WHERE materials.retrieval_blocked = 0
               AND materials.status IN ('text_ready', 'vector_ready')
               AND material_chunks.embedding_status = 'ready'
               AND material_chunks.embedding_space_id = (
                    SELECT id FROM embedding_spaces WHERE active = 1 LIMIT 1
               )
             ORDER BY knn.distance ASC",
        )?;
        let rows = statement.query_map(params![embedding], |row| {
            let content: String = row.get(4)?;
            Ok(MaterialSearchHit {
                material_id: row.get(0)?,
                chunk_id: row.get(1)?,
                file_name: row.get(2)?,
                section: row.get(3)?,
                snippet: content.chars().take(SNIPPET_CHARS).collect(),
                rank: 0.0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
    })
}

fn reciprocal_rank_fusion(
    fts: Vec<MaterialSearchHit>,
    vectors: Vec<MaterialSearchHit>,
) -> Vec<MaterialSearchHit> {
    let mut scores = HashMap::<String, f64>::new();
    let mut hits = HashMap::<String, MaterialSearchHit>::new();
    for (rank, hit) in fts.into_iter().enumerate() {
        *scores.entry(hit.chunk_id.clone()).or_insert(0.0) += rrf_score(rank);
        hits.entry(hit.chunk_id.clone()).or_insert(hit);
    }
    for (rank, hit) in vectors.into_iter().enumerate() {
        *scores.entry(hit.chunk_id.clone()).or_insert(0.0) += rrf_score(rank);
        hits.entry(hit.chunk_id.clone()).or_insert(hit);
    }
    let mut fused: Vec<MaterialSearchHit> = hits
        .into_iter()
        .map(|(chunk_id, mut hit)| {
            hit.rank = scores[&chunk_id];
            hit
        })
        .collect();
    fused.sort_by(|left, right| {
        right
            .rank
            .partial_cmp(&left.rank)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    fused
}

fn rrf_score(rank: usize) -> f64 {
    1.0 / (RRF_K + rank as f64)
}

fn merge_adjacent(hits: Vec<MaterialSearchHit>) -> Vec<MaterialSearchHit> {
    let mut ordered = hits;
    ordered.sort_by(|left, right| {
        left.material_id.cmp(&right.material_id).then_with(|| {
            chunk_index(&left.chunk_id, &left.material_id)
                .cmp(&chunk_index(&right.chunk_id, &right.material_id))
        })
    });
    struct Run {
        hit: MaterialSearchHit,
        last_index: i64,
    }
    let mut runs: Vec<Run> = Vec::new();
    for hit in ordered {
        let index = chunk_index(&hit.chunk_id, &hit.material_id);
        if let Some(run) = runs.last_mut()
            && run.hit.material_id == hit.material_id
            && index == run.last_index + 1
        {
            run.last_index = index;
            run.hit.rank = run.hit.rank.max(hit.rank);
            run.hit.snippet = merge_snippets(&run.hit.snippet, &hit.snippet);
            continue;
        }
        runs.push(Run {
            last_index: index,
            hit,
        });
    }
    let mut merged: Vec<MaterialSearchHit> = runs.into_iter().map(|run| run.hit).collect();
    merged.sort_by(|left, right| {
        right
            .rank
            .partial_cmp(&left.rank)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.chunk_id.cmp(&right.chunk_id))
    });
    merged
}

fn merge_snippets(left: &str, right: &str) -> String {
    let mut combined = String::from(left);
    if !right.is_empty() {
        if !combined.is_empty() {
            combined.push(' ');
        }
        combined.push_str(right);
    }
    combined.chars().take(SNIPPET_CHARS).collect()
}

fn chunk_index(chunk_id: &str, material_id: &str) -> i64 {
    chunk_id
        .strip_prefix(material_id)
        .and_then(|rest| rest.strip_prefix(':'))
        .and_then(|index| index.parse().ok())
        .unwrap_or(0)
}

fn usable_vector(vector: &[f32], dimensions: u32) -> bool {
    vector.len() as u32 == dimensions && vector.iter().all(|value| value.is_finite())
}

fn maybe_normalize(mut vector: Vec<f32>, normalized: bool) -> Vec<f32> {
    if !normalized {
        return vector;
    }
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }
    vector
}

struct IndexChunk {
    id: String,
    material_id: String,
    content: String,
}

enum ChunkOutcome {
    Ready(Vec<f32>),
    Failed,
}

#[derive(Clone, Copy)]
enum VecTablePlan {
    SameDimension,
    DimensionChange,
    Missing,
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    /// 维度解析必须从 `float[N]` 提取尺寸，解析不了时宁可降级为 None。
    #[test]
    fn parse_vec_dimensions_extracts_size_or_fails_closed() {
        assert_eq!(
            parse_vec_dimensions(
                "CREATE VIRTUAL TABLE material_chunk_vectors USING vec0(
                    chunk_id TEXT PRIMARY KEY,
                    embedding float[4] distance_metric=cosine
                )"
            ),
            Some(4)
        );
        assert_eq!(parse_vec_dimensions("float[1024]"), Some(1024));
        assert_eq!(parse_vec_dimensions("embedding float[abc]"), None);
        assert_eq!(parse_vec_dimensions("no vector column here"), None);
        assert_eq!(parse_vec_dimensions("truncated float[128"), None);
    }

    /// chunk 序号解析：`<material_id>:<n>` 才有序号；缺段、前缀不符、
    /// 非数字一律回退 0，保证合并排序对脏数据仍然稳定。
    #[test]
    fn chunk_index_falls_back_to_zero_for_untagged_ids() {
        assert_eq!(chunk_index("mat-1:3", "mat-1"), 3);
        assert_eq!(chunk_index("mat-1", "mat-1"), 0, "缺冒号索引段");
        assert_eq!(chunk_index("other:3", "mat-1"), 0, "material 前缀不符");
        assert_eq!(chunk_index("mat-1:x", "mat-1"), 0, "非数字索引");
        assert_eq!(chunk_index("mat-1:", "mat-1"), 0, "空索引段");
    }

    /// 向量可用性 = 长度精确匹配且全部有限；NaN/Inf 必须判不可用。
    #[test]
    fn usable_vector_requires_exact_length_and_finite_values() {
        assert!(usable_vector(&[1.0, 2.0], 2));
        assert!(!usable_vector(&[1.0], 2), "长度不符");
        assert!(!usable_vector(&[1.0, 2.0, 3.0], 2), "长度不符（偏长）");
        assert!(!usable_vector(&[1.0, f32::NAN], 2), "NaN 不可用");
        assert!(!usable_vector(&[1.0, f32::INFINITY], 2), "Inf 不可用");
    }

    /// 相邻片段合并：右片段为空时只保留左边；合并结果截到 snippet 上限。
    #[test]
    fn merge_snippets_appends_with_space_and_caps_length() {
        assert_eq!(merge_snippets("前半", "后半"), "前半 后半");
        assert_eq!(merge_snippets("只有左边", ""), "只有左边");
        assert_eq!(merge_snippets("", "只有右边"), "只有右边");
        let long = "字".repeat(SNIPPET_CHARS + 10);
        assert_eq!(
            merge_snippets("", &long).chars().count(),
            SNIPPET_CHARS,
            "合并结果必须截到 snippet 上限"
        );
    }
}
