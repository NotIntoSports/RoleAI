//! 级联编排单元测试（纯搬移自 cascade.rs 的 tests 模块）。

use super::{
    CascadeCredentials, CascadeTurnDeps, CascadeTurnRequest, HistoryTurn, build_messages,
    run_cascade_turn,
};
use crate::providers::TurnStreamHooks;
use crate::{
    config::PublicConfig,
    database::Database,
    materials::hybrid::{EmbeddingSpace, index_chunks},
    providers::{
        CascadeError, CascadeStage, ChatMessage, ChatModel, EmbeddingError, EmbeddingProbe,
        ProviderEndpoint, SpeechToText, TextToSpeech,
    },
    runtime::{SessionRuntime, active_role_profile, test_support::ready_public_config},
    services::MaterialService,
};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

struct ScriptedAsr {
    text: String,
    calls: AtomicU32,
    errors: Mutex<Vec<CascadeError>>,
}

impl ScriptedAsr {
    fn ok(text: &str) -> Self {
        Self {
            text: text.into(),
            calls: AtomicU32::new(0),
            errors: Mutex::new(Vec::new()),
        }
    }
}

impl SpeechToText for ScriptedAsr {
    fn transcribe(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: &[u8],
        _: u32,
    ) -> Result<String, CascadeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut errors = self.errors.lock().expect("asr errors");
        if let Some(error) = errors.first().cloned() {
            errors.remove(0);
            return Err(error);
        }
        Ok(self.text.clone())
    }
}

struct ScriptedLlm {
    reply: String,
    calls: AtomicU32,
    messages: Mutex<Vec<Vec<ChatMessage>>>,
    errors: Mutex<Vec<CascadeError>>,
    cancel: Option<std::sync::Arc<AtomicBool>>,
}

impl ScriptedLlm {
    fn ok(reply: &str) -> Self {
        Self {
            reply: reply.into(),
            calls: AtomicU32::new(0),
            messages: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            cancel: None,
        }
    }

    fn fail(errors: Vec<CascadeError>) -> Self {
        let mut llm = Self::ok("should-not-run");
        llm.errors = Mutex::new(errors);
        llm
    }

    fn cancel_after(reply: &str, cancel: std::sync::Arc<AtomicBool>) -> Self {
        let mut llm = Self::ok(reply);
        llm.cancel = Some(cancel);
        llm
    }
}

impl ChatModel for ScriptedLlm {
    fn complete(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        messages: &[ChatMessage],
    ) -> Result<String, CascadeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.messages
            .lock()
            .expect("llm messages")
            .push(messages.to_vec());
        let mut errors = self.errors.lock().expect("llm errors");
        if let Some(error) = errors.first().cloned() {
            errors.remove(0);
            return Err(error);
        }
        if let Some(flag) = &self.cancel {
            flag.store(true, Ordering::SeqCst);
        }
        Ok(self.reply.clone())
    }
}

struct ScriptedTts {
    pcm: Result<Vec<u8>, CascadeError>,
    calls: AtomicU32,
    voices: Mutex<Vec<String>>,
}

impl ScriptedTts {
    fn ok(pcm: &[u8]) -> Self {
        Self {
            pcm: Ok(pcm.to_vec()),
            calls: AtomicU32::new(0),
            voices: Mutex::new(Vec::new()),
        }
    }

    fn fail(error: CascadeError) -> Self {
        Self {
            pcm: Err(error),
            calls: AtomicU32::new(0),
            voices: Mutex::new(Vec::new()),
        }
    }
}

impl TextToSpeech for ScriptedTts {
    fn synthesize(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        voice_id: &str,
        _: &str,
    ) -> Result<Vec<u8>, CascadeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.voices
            .lock()
            .expect("tts voices")
            .push(voice_id.to_owned());
        match &self.pcm {
            Ok(bytes) => Ok(bytes.clone()),
            Err(error) => Err(*error),
        }
    }
}

struct EmbedCall {
    base_url: String,
    credential: Option<String>,
    model_id: String,
    dimensions: u32,
    input: String,
}

struct ScriptedEmbed {
    vector: Result<Vec<f32>, EmbeddingError>,
    calls: AtomicU32,
    seen: Mutex<Vec<EmbedCall>>,
}

impl ScriptedEmbed {
    fn ok(vector: Vec<f32>) -> Self {
        Self {
            vector: Ok(vector),
            calls: AtomicU32::new(0),
            seen: Mutex::new(Vec::new()),
        }
    }

    fn fail(error: EmbeddingError) -> Self {
        Self {
            vector: Err(error),
            calls: AtomicU32::new(0),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl EmbeddingProbe for ScriptedEmbed {
    fn embed(
        &self,
        endpoint: &ProviderEndpoint,
        credential: Option<&str>,
        model_id: &str,
        dimensions: u32,
        input: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().expect("embed seen").push(EmbedCall {
            base_url: endpoint.base_url.clone(),
            credential: credential.map(ToOwned::to_owned),
            model_id: model_id.to_owned(),
            dimensions,
            input: input.to_owned(),
        });
        self.vector.clone()
    }
}

struct IndexEmbed;

impl EmbeddingProbe for IndexEmbed {
    fn embed(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        dimensions: u32,
        input: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        let mut vector = vec![0.0; dimensions as usize];
        if input.contains("向量甲") {
            vector[0] = 1.0;
        } else if input.contains("向量乙") {
            vector[1] = 1.0;
        } else if input.contains("向量丙") {
            vector[0] = 0.3;
            if dimensions > 2 {
                vector[2] = 0.7;
            }
        } else if dimensions > 0 {
            vector[dimensions as usize - 1] = 1.0;
        }
        Ok(vector)
    }
}

fn opened(directory: &tempfile::TempDir) -> Database {
    let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
    database.migrate().unwrap();
    database
}

fn cherry_config() -> PublicConfig {
    let mut config = ready_public_config();
    config.speech.voice_routes[0].voice_id = Some("Cherry".into());
    config.knowledge.embedding_configs[0].dimensions = 4;
    config
}

fn role_profile() -> crate::config::RoleProfileConfig {
    let config = ready_public_config();
    active_role_profile(&config).expect("role profile").clone()
}

fn import(directory: &tempfile::TempDir, database: &Database, name: &str, body: &str) -> String {
    let path = directory.path().join(name);
    std::fs::write(&path, body).unwrap();
    MaterialService::new(database, directory.path())
        .import_file(&path)
        .unwrap()
        .id
}

fn run_turn(
    deps: &CascadeTurnDeps<'_>,
    config: &PublicConfig,
    user_text: &str,
    history: &[HistoryTurn],
    cancel: &AtomicBool,
) -> Result<super::CascadeTurn, CascadeError> {
    run_cascade_turn(
        deps,
        CascadeTurnRequest {
            config,
            credentials: CascadeCredentials {
                asr: Some("asr-secret"),
                llm: Some("llm-secret"),
                tts: Some("tts-secret"),
                embed: Some("embed-secret"),
                e2e: None,
            },
            pcm: None,
            sample_rate: 16_000,
            user_text: Some(user_text),
            history,
            context_summary: None,
        },
        cancel,
        &TurnStreamHooks::none(),
    )
}

#[test]
fn rrf_path_uses_vector_winner_and_real_embed_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let fts_winner = import(
        &directory,
        &database,
        "fts.txt",
        "订单服务订单服务订单服务订单服务。向量乙标记。",
    );
    let vec_winner = import(
        &directory,
        &database,
        "vec.txt",
        "订单服务订单服务。向量甲标记。",
    );
    import(&directory, &database, "mid.txt", "订单服务。向量丙标记。");
    index_chunks(
        &database,
        &EmbeddingSpace {
            provider_id: "emb-1".into(),
            model_id: "bge".into(),
            dimensions: 4,
            normalized: true,
        },
        &IndexEmbed,
    )
    .unwrap();

    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::ok("融合回答");
    let tts = ScriptedTts::ok(&[0x11, 0x22]);
    let embed = ScriptedEmbed::ok(vec![1.0, 0.0, 0.0, 0.0]);
    let runtime = SessionRuntime::new();
    let sleeps = Mutex::new(Vec::<Duration>::new());
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|delay| sleeps.lock().unwrap().push(delay),
    };

    let turn = run_turn(&deps, &config, "订单服务", &[], &AtomicBool::new(false)).unwrap();

    assert!(turn.materials_used);
    assert_eq!(turn.citations[0].material_id, vec_winner);
    assert!(
        turn.citations
            .iter()
            .any(|item| item.material_id == fts_winner)
    );
    assert!(
        turn.citations
            .iter()
            .all(|item| item.snippet.chars().count() <= 160)
    );
    assert_eq!(turn.assistant_text, "融合回答");
    assert_eq!(turn.tts_pcm, [0x11, 0x22]);
    assert_eq!(tts.voices.lock().unwrap().as_slice(), ["Cherry"]);
    let seen = embed.seen.lock().unwrap();
    assert_eq!(seen[0].base_url, "https://emb.example.test/v1");
    assert_eq!(seen[0].credential.as_deref(), Some("embed-secret"));
    assert_eq!(seen[0].model_id, "bge");
    assert_eq!(seen[0].dimensions, 4);
    assert_eq!(seen[0].input, "订单服务");
    let messages = llm.messages.lock().unwrap();
    assert!(
        messages[0][0]
            .content
            .contains("UNIQUE_PROMPT_BODY_DO_NOT_SNAPSHOT")
    );
    assert!(
        messages[0][0]
            .content
            .contains("UNIQUE_STYLE_DO_NOT_SNAPSHOT")
    );
    assert!(messages[0].last().unwrap().content.contains("订单服务"));
    assert!(messages[0].last().unwrap().content.contains("向量甲"));
    assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn embed_failure_falls_back_to_fts_only() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let material_id = import(
        &directory,
        &database,
        "note.txt",
        "负责订单服务与 Kafka 链路，完整句子用于检索。",
    );
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::ok("全文回答");
    let tts = ScriptedTts::ok(&[0x01, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::RequestFailed);
    let runtime = SessionRuntime::new();
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|_| {},
    };

    let turn = run_turn(&deps, &config, "订单服务", &[], &AtomicBool::new(false)).unwrap();
    assert!(turn.materials_used);
    assert_eq!(turn.citations[0].material_id, material_id);
    assert!(turn.citations[0].snippet.contains("订单服务"));
    let seen = embed.seen.lock().unwrap();
    assert_eq!(seen[0].base_url, "https://emb.example.test/v1");
    assert_eq!(seen[0].credential.as_deref(), Some("embed-secret"));
}

#[test]
fn unused_materials_when_fts_and_vectors_are_empty() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::ok("无资料回答");
    let tts = ScriptedTts::ok(&[0x02, 0x00]);
    let embed = ScriptedEmbed::ok(vec![1.0, 0.0, 0.0, 0.0]);
    let runtime = SessionRuntime::new();
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|_| {},
    };

    let turn = run_turn(
        &deps,
        &config,
        "没有任何匹配的查询词zzzz",
        &[],
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!turn.materials_used);
    assert!(turn.citations.is_empty());
    assert_eq!(turn.assistant_text, "无资料回答");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn retrieval_error_continues_without_materials() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    import(
        &directory,
        &database,
        "note.txt",
        "负责订单服务与 Kafka 链路，完整句子用于检索。",
    );
    database
        .with_connection(|connection| connection.execute_batch("DROP TABLE material_chunks_fts"))
        .unwrap();
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::ok("检索失败仍回答");
    let tts = ScriptedTts::ok(&[0x03, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::Timeout);
    let runtime = SessionRuntime::new();
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|_| {},
    };

    let turn = run_turn(&deps, &config, "订单服务", &[], &AtomicBool::new(false)).unwrap();
    assert!(!turn.materials_used);
    assert!(turn.citations.is_empty());
    assert_eq!(turn.assistant_text, "检索失败仍回答");
}

#[test]
fn cancel_between_llm_and_tts_does_not_start_tts() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::cancel_after("已生成", std::sync::Arc::clone(&cancel));
    let tts = ScriptedTts::ok(&[0x04, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::RequestFailed);
    let runtime = SessionRuntime::new();
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|_| {},
    };

    let error = run_turn(&deps, &config, "你好", &[], &cancel).unwrap_err();
    assert_eq!(error, CascadeError::Cancelled);
    assert_eq!(error.code(), "SESSION_CANCELLED");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn tts_failure_keeps_assistant_text_and_empty_pcm() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::ok("文本仍在");
    let tts = ScriptedTts::fail(CascadeError::RequestFailed(CascadeStage::Tts));
    let embed = ScriptedEmbed::fail(EmbeddingError::RequestFailed);
    let runtime = SessionRuntime::new();
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|_| {},
    };

    let turn = run_turn(&deps, &config, "你好", &[], &AtomicBool::new(false)).unwrap();
    assert_eq!(turn.assistant_text, "文本仍在");
    assert!(turn.tts_pcm.is_empty());
    assert_eq!(turn.error_code, Some("TTS_FAILED"));
    assert_eq!(turn.user_text, "你好");
}

#[test]
fn unauthorized_is_not_retried() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::fail(vec![CascadeError::Unauthorized(CascadeStage::Llm)]);
    let tts = ScriptedTts::ok(&[0x05, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::Unauthorized);
    let runtime = SessionRuntime::new();
    let sleeps = Mutex::new(Vec::<Duration>::new());
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|delay| sleeps.lock().unwrap().push(delay),
    };

    let error = run_turn(&deps, &config, "你好", &[], &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error, CascadeError::Unauthorized(CascadeStage::Llm));
    assert_eq!(error.code(), "LLM_UNAUTHORIZED");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert!(sleeps.lock().unwrap().is_empty());
    assert_eq!(embed.calls.load(Ordering::SeqCst), 1);
    assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn timeout_retries_three_times_with_exponential_backoff() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::fail(vec![
        CascadeError::Timeout(CascadeStage::Llm),
        CascadeError::ConnectionReset(CascadeStage::Llm),
        CascadeError::Timeout(CascadeStage::Llm),
    ]);
    let tts = ScriptedTts::ok(&[0x06, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::RequestFailed);
    let runtime = SessionRuntime::new();
    let sleeps = Mutex::new(Vec::<Duration>::new());
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|delay| sleeps.lock().unwrap().push(delay),
    };

    let error = run_turn(&deps, &config, "你好", &[], &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error, CascadeError::Timeout(CascadeStage::Llm));
    assert_eq!(llm.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        sleeps.lock().unwrap().as_slice(),
        [Duration::from_millis(200), Duration::from_millis(400)]
    );
}

#[test]
fn rate_limited_honors_retry_after_once() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::fail(vec![
        CascadeError::RateLimited {
            stage: CascadeStage::Llm,
            retry_after_secs: Some(7),
        },
        CascadeError::RateLimited {
            stage: CascadeStage::Llm,
            retry_after_secs: Some(9),
        },
    ]);
    let tts = ScriptedTts::ok(&[0x07, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::RequestFailed);
    let runtime = SessionRuntime::new();
    let sleeps = Mutex::new(Vec::<Duration>::new());
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|delay| sleeps.lock().unwrap().push(delay),
    };

    let error = run_turn(&deps, &config, "你好", &[], &AtomicBool::new(false)).unwrap_err();
    assert_eq!(
        error,
        CascadeError::RateLimited {
            stage: CascadeStage::Llm,
            retry_after_secs: Some(9),
        }
    );
    assert_eq!(llm.calls.load(Ordering::SeqCst), 2);
    assert_eq!(sleeps.lock().unwrap().as_slice(), [Duration::from_secs(7)]);
}

#[test]
fn server_error_is_tried_twice() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::fail(vec![
        CascadeError::ServerError(CascadeStage::Llm),
        CascadeError::ServerError(CascadeStage::Llm),
    ]);
    let tts = ScriptedTts::ok(&[0x08, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::RequestFailed);
    let runtime = SessionRuntime::new();
    let sleeps = Mutex::new(Vec::<Duration>::new());
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|delay| sleeps.lock().unwrap().push(delay),
    };

    let error = run_turn(&deps, &config, "你好", &[], &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error, CascadeError::ServerError(CascadeStage::Llm));
    assert_eq!(llm.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        sleeps.lock().unwrap().as_slice(),
        [Duration::from_millis(200)]
    );
}

#[test]
fn takeover_does_not_call_llm() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let asr = ScriptedAsr::ok("转写");
    let llm = ScriptedLlm::ok("不该出现");
    let tts = ScriptedTts::ok(&[0x09, 0x00]);
    let embed = ScriptedEmbed::ok(vec![1.0, 0.0, 0.0, 0.0]);
    let mut runtime = SessionRuntime::new();
    runtime.takeover();
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|_| {},
    };

    let error = run_cascade_turn(
        &deps,
        CascadeTurnRequest {
            config: &config,
            credentials: CascadeCredentials::default(),
            pcm: Some(&[0x00, 0x01]),
            sample_rate: 16_000,
            user_text: Some("忽略"),
            history: &[],
            context_summary: None,
        },
        &AtomicBool::new(false),
        &TurnStreamHooks::none(),
    )
    .unwrap_err();
    assert_eq!(error, CascadeError::AnswerBlocked);
    assert_eq!(error.code(), "SESSION_ANSWER_BLOCKED");
    assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert_eq!(embed.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn pcm_uses_asr_and_history_keeps_last_twenty_turns() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let asr = ScriptedAsr::ok("现场转写");
    let llm = ScriptedLlm::ok("带历史");
    let tts = ScriptedTts::ok(&[0x0A, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::RequestFailed);
    let runtime = SessionRuntime::new();
    let config = cherry_config();
    let history: Vec<HistoryTurn> = (0..21)
        .map(|index| HistoryTurn {
            user_text: format!("u{index}"),
            assistant_text: format!("a{index}"),
        })
        .collect();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|_| {},
    };

    let turn = run_cascade_turn(
        &deps,
        CascadeTurnRequest {
            config: &config,
            credentials: CascadeCredentials::default(),
            pcm: Some(&[0x10, 0x20]),
            sample_rate: 16_000,
            user_text: Some("应被忽略"),
            history: &history,
            context_summary: None,
        },
        &AtomicBool::new(false),
        &TurnStreamHooks::none(),
    )
    .unwrap();
    assert_eq!(turn.user_text, "现场转写");
    assert_eq!(asr.calls.load(Ordering::SeqCst), 1);
    let messages = llm.messages.lock().unwrap()[0].clone();
    let user_turns: Vec<_> = messages
        .iter()
        .filter(|message| message.role == "user")
        .map(|message| message.content.clone())
        .collect();
    assert!(!user_turns.iter().any(|text| text == "u0"));
    assert_eq!(user_turns.first().map(String::as_str), Some("u1"));
    assert_eq!(user_turns.last().map(String::as_str), Some("现场转写"));
    assert_eq!(user_turns.len(), 21);
}

#[test]
fn context_summary_is_merged_into_system_message() {
    let role = role_profile(); // 既有测试助手
    let history = vec![HistoryTurn {
        user_text: "旧问题".into(),
        assistant_text: "旧回答".into(),
    }];
    let messages = build_messages(&role, Some("此前讨论了时间"), &history, "新问题", &[]);
    // history 按既有方式追加 user/assistant 成对消息：system + 2 + 1 = 4。
    assert_eq!(messages.len(), 4);
    assert!(messages[0].content.contains("此前对话摘要：此前讨论了时间"));
    assert_eq!(messages[1].content, "旧问题");
    assert_eq!(messages[2].content, "旧回答");
    assert_eq!(messages[3].content, "新问题");
}

/// 只实现 complete_streaming 的假 LLM：模拟边生成边吐前缀快照。
struct StreamingLlm {
    text: String,
    complete_calls: AtomicU32,
}

impl ChatModel for StreamingLlm {
    fn complete(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: &[ChatMessage],
    ) -> Result<String, CascadeError> {
        panic!("streaming turns must not fall back to complete()");
    }

    fn complete_streaming(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: &[ChatMessage],
        on_snapshot: &dyn Fn(&str),
    ) -> Result<String, CascadeError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        let prefix = self.text.chars().take(2).collect::<String>();
        on_snapshot(&prefix);
        on_snapshot(&self.text);
        Ok(self.text.clone())
    }
}

#[test]
fn streaming_llm_snapshots_reach_turn_hooks() {
    let directory = tempfile::tempdir().unwrap();
    let database = opened(&directory);
    let llm = StreamingLlm {
        text: "流式全文回答".into(),
        complete_calls: AtomicU32::new(0),
    };
    let asr = ScriptedAsr::ok("unused");
    let tts = ScriptedTts::ok(&[0x21, 0x00]);
    let embed = ScriptedEmbed::fail(EmbeddingError::RequestFailed);
    let runtime = SessionRuntime::new();
    let config = cherry_config();
    let deps = CascadeTurnDeps {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        database: &database,
        runtime: &runtime,
        sleep: &|_| {},
    };
    let snapshots = Mutex::new(Vec::<String>::new());
    let hooks = crate::providers::TurnStreamHooks {
        user_text: None,
        assistant_text: Some(&|snapshot| snapshots.lock().unwrap().push(snapshot.to_owned())),
    };

    let turn = run_cascade_turn(
        &deps,
        CascadeTurnRequest {
            config: &config,
            credentials: CascadeCredentials::default(),
            pcm: None,
            sample_rate: 16_000,
            user_text: Some("流式问题"),
            history: &[],
            context_summary: None,
        },
        &AtomicBool::new(false),
        &hooks,
    )
    .unwrap();

    assert_eq!(turn.assistant_text, "流式全文回答");
    assert_eq!(
        snapshots.lock().unwrap().as_slice(),
        ["流式".to_owned(), "流式全文回答".to_owned()]
    );
    assert_eq!(llm.complete_calls.load(Ordering::SeqCst), 1);
}
