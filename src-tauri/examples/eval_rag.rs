//! RAG 检索离线评测（lane-C C43）。
//!
//! 语料：`tests/fixtures/rag/documents.json`（虚构产品资料 10 篇）；
//! 题集：`tests/fixtures/rag/questions.jsonl`（50 题，每题标注金标资料与金标段落片段）。
//!
//! 三种检索方式对比：
//! 1. keyword：FTS5 关键词检索（`search_hybrid(q, None, k)`）；
//! 2. vector：确定性本地嵌入替身——字符 bigram 哈希向量（非真实 Embedding，只评机制）；
//! 3. hybrid：关键词 + 向量替身的混合检索。
//!
//! 指标：Recall@1/3/5 与 MRR，分「块级」（命中块必须包含金标片段）与
//! 「文档级」（命中块来自金标资料）两种口径。
//!
//! 运行：`cargo run --manifest-path src-tauri/Cargo.toml --release --example eval_rag`
//! 结果写入 `target/rag-results.json`。在线模式（真实 Embedding Key）默认不跑。

use std::fs;
use std::path::PathBuf;

use ai_virtual_assistant_desktop_lib::database::Database;
use ai_virtual_assistant_desktop_lib::materials::chunk::{CHUNKER_VERSION, chunk_text};
use ai_virtual_assistant_desktop_lib::materials::hybrid::{EmbeddingSpace, search_hybrid};
use ai_virtual_assistant_desktop_lib::materials::store::{MaterialStore, NewMaterial};
use ai_virtual_assistant_desktop_lib::providers::{
    EmbeddingError, EmbeddingProbe, ProviderEndpoint,
};

const DIMENSIONS: u32 = 128;

/// 字符 bigram 哈希嵌入替身：对输入的全部相邻字符对做 FNV 混合，
/// 投影到固定维度的 LCG 填充向量。确定性、无网络；不代表真实语义质量。
struct BigramHashProbe;

impl EmbeddingProbe for BigramHashProbe {
    fn embed(
        &self,
        _endpoint: &ProviderEndpoint,
        _credential: Option<&str>,
        _model_id: &str,
        dimensions: u32,
        input: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        let chars: Vec<char> = input.chars().collect();
        let mut vector = vec![0f32; dimensions as usize];
        if chars.is_empty() {
            return Ok(vector);
        }
        for window in chars.windows(2) {
            let mut h = 0x8422_4353_4D4C_B3A1u64;
            for ch in window {
                h = (h ^ *ch as u64).wrapping_mul(0x0000_0100_0000_01B3);
            }
            let slot = (h >> 11) as usize % vector.len();
            let sign = if h & 1 == 0 { 1.0 } else { -1.0 };
            vector[slot] += sign;
        }
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
        for value in &mut vector {
            *value /= norm;
        }
        Ok(vector)
    }
}

fn corpus_documents() -> serde_json::Value {
    let path = fixture_path("documents.json");
    serde_json::from_str(&fs::read_to_string(path).expect("read documents.json")).expect("parse")
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/rag/{name}"))
}

fn build_corpus() -> (tempfile::TempDir, Database, Vec<(String, String)>) {
    let documents = corpus_documents();
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("rag-eval.sqlite3")).expect("open");
    database.migrate().expect("migrate");
    let store = MaterialStore::new(&database);
    let mut docs = Vec::new();
    for entry in documents["documents"].as_array().expect("documents array") {
        let id = entry["id"].as_str().expect("id").to_owned();
        let text = entry["text"].as_str().expect("text").to_owned();
        let chunks = chunk_text(&id, &text);
        store
            .insert_text_ready(NewMaterial {
                id: &id,
                file_name: &format!("{id}.md"),
                stored_path: &format!("fixtures/{id}.md"),
                content_sha256: &format!("{id}-hash"),
                media_type: "text/markdown",
                byte_size: text.len() as i64,
                parser_version: "eval",
                chunker_version: CHUNKER_VERSION,
                extracted_text: &text,
                chunks: &chunks,
            })
            .expect("insert");
        docs.push((id, text));
    }
    (directory, database, docs)
}

fn normalize_text(text: &str) -> String {
    // 与生产 echo_guard 同源的轻量规一化思路：小写 + 去空白差异。
    text.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("")
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let questions_path = fixture_path("questions.jsonl");
    let raw = fs::read_to_string(&questions_path)?;
    let questions: Vec<serde_json::Value> = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;

    let space = EmbeddingSpace {
        provider_id: "eval".into(),
        model_id: "bigram-hash-128".into(),
        dimensions: DIMENSIONS,
        normalized: true,
    };
    let probe = BigramHashProbe;
    let endpoint = ProviderEndpoint {
        provider_id: "eval".into(),
        base_url: String::new(),
    };

    let (_directory, database, docs) = build_corpus();

    // 预建向量索引（替身向量）。
    index_for_eval(&database, &space, &probe)?;

    let modes = ["keyword", "vector", "hybrid"];
    let mut results = serde_json::Map::new();
    for mode in modes {
        let mut chunk_hits_at = [0usize; 3];
        let mut doc_hits_at = [0usize; 3];
        let mut mrr_chunk = 0f64;
        let mut mrr_doc = 0f64;
        let mut total = 0usize;

        for question in &questions {
            let text = question["question"].as_str().expect("question");
            let gold_doc = question["docId"].as_str().expect("docId");
            let gold_snippet = normalize_text(question["goldSnippet"].as_str().expect("snippet"));
            let query_vector = if mode == "keyword" {
                None
            } else {
                Some(probe.embed(&endpoint, None, "bigram-hash-128", DIMENSIONS, text)?)
            };
            let hits = search_hybrid(&database, text, query_vector.as_deref(), Some(5))?;
            total += 1;
            let mut chunk_rank = 0usize;
            let mut doc_rank = 0usize;
            for (index, hit) in hits.iter().enumerate() {
                let hit_doc = hit.material_id.as_str();
                if chunk_rank == 0
                    && hit_doc == gold_doc
                    && chunk_has_snippet(&database, &hit.chunk_id, &gold_snippet)
                {
                    chunk_rank = index + 1;
                }
                if doc_rank == 0 && hit_doc == gold_doc {
                    doc_rank = index + 1;
                }
            }
            for (k_index, k) in [1usize, 3, 5].iter().enumerate() {
                if chunk_rank != 0 && chunk_rank <= *k {
                    chunk_hits_at[k_index] += 1;
                }
                if doc_rank != 0 && doc_rank <= *k {
                    doc_hits_at[k_index] += 1;
                }
            }
            if chunk_rank != 0 {
                mrr_chunk += 1.0 / chunk_rank as f64;
            }
            if doc_rank != 0 {
                mrr_doc += 1.0 / doc_rank as f64;
            }
        }

        results.insert(
            mode.to_owned(),
            serde_json::json!({
                "chunkLevel": {
                    "recallAt1": chunk_hits_at[0] as f64 / total as f64,
                    "recallAt3": chunk_hits_at[1] as f64 / total as f64,
                    "recallAt5": chunk_hits_at[2] as f64 / total as f64,
                    "mrr": mrr_chunk / total as f64,
                },
                "documentLevel": {
                    "recallAt1": doc_hits_at[0] as f64 / total as f64,
                    "recallAt3": doc_hits_at[1] as f64 / total as f64,
                    "recallAt5": doc_hits_at[2] as f64 / total as f64,
                    "mrr": mrr_doc / total as f64,
                },
                "questions": total,
            }),
        );
    }

    let output = serde_json::json!({
        "corpus": { "documents": docs.len(), "questions": questions.len() },
        "modes": results,
        "note": "vector/hybrid 使用字符 bigram 哈希嵌入替身，只评检索机制，不代表真实 Embedding 语义质量",
    });
    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/rag-results.json");
    fs::write(&out_path, serde_json::to_string_pretty(&output)?)?;
    println!("{}", serde_json::to_string_pretty(&output)?);
    println!("written to {}", out_path.display());
    Ok(())
}

fn index_for_eval(
    database: &Database,
    space: &EmbeddingSpace,
    probe: &dyn EmbeddingProbe,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ai_virtual_assistant_desktop_lib::materials::hybrid::index_chunks(database, space, probe)
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:?}").into() })
}

fn chunk_has_snippet(database: &Database, chunk_id: &str, gold_snippet: &str) -> bool {
    database
        .with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT content FROM material_chunks WHERE id = ?1")?;
            let content: Option<String> =
                match statement.query_row(rusqlite::params![chunk_id], |row| row.get(0)) {
                    Ok(value) => Some(value),
                    Err(rusqlite::Error::QueryReturnedNoRows) => None,
                    Err(error) => return Err(error),
                };
            Ok(content)
        })
        .ok()
        .flatten()
        .map(|content| normalize_text(&content).contains(gold_snippet))
        .unwrap_or(false)
}
