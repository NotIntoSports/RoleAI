//! 资料库基准（C33）：分块（chunk_text）不同文档长度、混合检索
//! （search_hybrid）在 1k / 10k 块规模下的查询延迟、SQLite 写入吞吐。
//!
//! 向量用确定性伪随机生成（LCG 哈希替身，不调用 Embedding 服务）；
//! 数据库为 tempfile 内一次性文件，跑完即弃。

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::time::Duration;

use ai_virtual_assistant_desktop_lib::database::Database;
use ai_virtual_assistant_desktop_lib::materials::chunk::{CHUNKER_VERSION, chunk_text};
use ai_virtual_assistant_desktop_lib::materials::hybrid::{
    EmbeddingSpace, index_chunks, search_hybrid,
};
use ai_virtual_assistant_desktop_lib::materials::store::{MaterialStore, NewMaterial};
use ai_virtual_assistant_desktop_lib::providers::{
    EmbeddingError, EmbeddingProbe, ProviderEndpoint,
};

/// 确定性 LCG（Numerical Recipes 参数），跨运行可复现。
struct Lcg(u64);

impl Lcg {
    fn next_usize(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound
    }
}

const PHRASES: &[&str] = &[
    "订单服务的连接池需要按峰值流量扩容",
    "索引缺失导致对账查询在全表扫描",
    "面试系统的语音链路必须保证 32ms 帧延迟",
    "会议助手只在被点名时回答，避免抢话",
    "重连退避采用指数递增并以 30 秒封顶",
    "资料分块按语义段落切分，硬上限 2000 字符",
    "混合检索把关键词与向量召回做倒数排名融合",
    "前端流式字幕需要 done 标记区分中间态与终态",
    "备份恢复期间会话命令会被连接互斥串行化",
    "虚拟声卡安装失败时启动流程必须快速失败",
];

/// 确定性生成约 `target_runes` 字的中文资料文本。
fn synth_document(seed: u64, target_runes: usize) -> String {
    let mut rng = Lcg(seed);
    let mut text = String::with_capacity(target_runes * 3);
    let mut section = 0;
    while text.chars().count() < target_runes {
        if text.chars().count() % 800 < 40 {
            section += 1;
            text.push_str(&format!("\n## 第 {section} 节\n\n"));
        }
        let phrase = PHRASES[rng.next_usize(PHRASES.len())];
        text.push_str(phrase);
        text.push('。');
    }
    text
}

struct BenchCorpus {
    _directory: tempfile::TempDir,
    database: Database,
}

/// 建库并写入 `documents` 篇资料（每篇按 chunk_text 分块）。
fn build_corpus(documents: usize, seed: u64) -> BenchCorpus {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("bench.sqlite3")).expect("open");
    database.migrate().expect("migrate");
    let store = MaterialStore::new(&database);
    for index in 0..documents {
        let text = synth_document(seed + index as u64, 10_000);
        let chunks = chunk_text(&format!("bench-{index}"), &text);
        let id = format!("mat-{index}");
        store
            .insert_text_ready(NewMaterial {
                id: &id,
                file_name: &format!("资料{index}.md"),
                stored_path: &format!("materials/bench-{index}.md"),
                content_sha256: &format!("hash-{index:06}"),
                media_type: "text/markdown",
                byte_size: text.len() as i64,
                parser_version: "bench",
                chunker_version: CHUNKER_VERSION,
                extracted_text: &text,
                chunks: &chunks,
            })
            .expect("insert material");
    }
    BenchCorpus {
        _directory: directory,
        database,
    }
}

/// 确定性向量替身：输入字节 Buller 哈希 → LCG 填充，不访问网络。
struct LcgVectorProbe;

impl EmbeddingProbe for LcgVectorProbe {
    fn embed(
        &self,
        _endpoint: &ProviderEndpoint,
        _credential: Option<&str>,
        _model_id: &str,
        dimensions: u32,
        input: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        let mut h = 0x243F_6A88_85A3_08D3u64;
        for byte in input.as_bytes() {
            h = h.wrapping_mul(0x100_0000_01B3).wrapping_add(*byte as u64);
        }
        let mut vector = Vec::with_capacity(dimensions as usize);
        for i in 0..dimensions {
            h = h.wrapping_mul(0x100_0000_01B3).wrapping_add(i as u64);
            vector.push(((h >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0);
        }
        Ok(vector)
    }
}

fn bench_chunk(c: &mut Criterion) {
    let mut group = c.benchmark_group("chunk");
    for runes in [2_000usize, 20_000, 200_000] {
        let text = synth_document(runes as u64, runes);
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(
            criterion::BenchmarkId::new("chunk_text", format!("{runes}_runes")),
            &text,
            |b, text| b.iter(|| chunk_text(black_box("bench"), black_box(text))),
        );
    }
    group.finish();
}

fn bench_hybrid(c: &mut Criterion) {
    let mut group = c.benchmark_group("hybrid");
    group.sample_size(20);
    let space = EmbeddingSpace {
        provider_id: "bench".into(),
        model_id: "lcg-64".into(),
        dimensions: 64,
        normalized: true,
    };
    let probe = LcgVectorProbe;
    let mut query_rng = Lcg(999);

    for documents in [25usize, 250usize] {
        let corpus = build_corpus(documents, 10_000);
        let chunks = corpus
            .database
            .with_connection(|c| {
                c.query_row("SELECT COUNT(*) FROM material_chunks", [], |r| {
                    r.get::<_, i64>(0)
                })
            })
            .expect("count chunks");
        let label = format!("{}_chunks", chunks);

        // 索引本身也计入基准：探测为本地哈希替身，衡量 SQL + 写路径。
        group.bench_function(format!("index_chunks/{label}"), |b| {
            b.iter(|| {
                index_chunks(
                    black_box(&corpus.database),
                    black_box(&space),
                    black_box(&probe),
                )
                .expect("index")
            })
        });

        let query = "连接池 扩容 语音链路";
        let query_vector = probe
            .embed(
                &ProviderEndpoint {
                    provider_id: "bench".into(),
                    base_url: String::new(),
                },
                None,
                "lcg-64",
                64,
                query,
            )
            .expect("embed query");
        group.throughput(Throughput::Elements(chunks as u64));
        group.bench_function(format!("search_hybrid_vector/{label}"), |b| {
            b.iter(|| {
                search_hybrid(
                    black_box(&corpus.database),
                    black_box(query),
                    black_box(Some(query_vector.as_slice())),
                    black_box(Some(5)),
                )
                .expect("search")
            })
        });
        group.bench_function(format!("search_keyword/{label}"), |b| {
            b.iter(|| {
                search_hybrid(
                    black_box(&corpus.database),
                    black_box(query),
                    black_box(None),
                    black_box(Some(5)),
                )
                .expect("search")
            })
        });
        let _ = query_rng.next_usize(1);
    }
    group.finish();
}

fn bench_sqlite_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("sqlite");
    let corpus = build_corpus(1, 77);
    let store = MaterialStore::new(&corpus.database);
    let text = synth_document(42, 10_000);
    let chunks = chunk_text("bench-write", &text);
    group.throughput(Throughput::Bytes(text.len() as u64));
    let mut counter = 0u64;
    group.bench_function("insert_text_ready_10k_runes", |b| {
        b.iter(|| {
            counter += 1;
            let id = format!("write-{counter}");
            store
                .insert_text_ready(NewMaterial {
                    id: &id,
                    file_name: "write.md",
                    stored_path: "materials/write.md",
                    content_sha256: &format!("write-hash-{counter}"),
                    media_type: "text/markdown",
                    byte_size: text.len() as i64,
                    parser_version: "bench",
                    chunker_version: CHUNKER_VERSION,
                    extracted_text: &text,
                    chunks: &chunks,
                })
                .expect("insert");
        })
    });
    group.finish();
    // 给 LCG 一个消费点避免 dead_code 告警。
    let _ = Lcg(1).next_usize(1);
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(1));
    targets = bench_chunk, bench_hybrid, bench_sqlite_write
);
criterion_main!(benches);
