//! SessionService 单元测试（纯搬移自 mod.rs 的 tests 模块）。

use super::*;

use crate::{
    app_state::{AppPaths, AppState},
    audio::{RecordingSink, SidecarPoll},
    config::PublicConfig,
    database::Database,
    providers::{
        CascadeError, ChatMessage, ChatModel, EmbeddingError, EmbeddingProbe, ProviderEndpoint,
        RealtimeAudioRequest, RealtimeError, RealtimeModel, RealtimeTextRequest, RealtimeTurn,
        SpeechToText, TextToSpeech,
    },
    runtime::{
        AgentMode, CascadeCredentials, SessionPhase,
        test_support::{ready_e2e_public_config, ready_public_config},
    },
    secrets::MemorySecretStore,
    sessions::{NewSession, SessionStore},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

struct ScriptedAsr {
    text: String,
    calls: AtomicU32,
}

impl ScriptedAsr {
    fn ok(text: &str) -> Self {
        Self {
            text: text.into(),
            calls: AtomicU32::new(0),
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
        Ok(self.text.clone())
    }
}

struct ScriptedLlm {
    reply: String,
    calls: AtomicU32,
    messages: Mutex<Vec<Vec<ChatMessage>>>,
}

impl ScriptedLlm {
    fn ok(reply: &str) -> Self {
        Self {
            reply: reply.into(),
            calls: AtomicU32::new(0),
            messages: Mutex::new(Vec::new()),
        }
    }

    fn seen_messages(&self) -> Vec<Vec<ChatMessage>> {
        self.messages.lock().expect("llm messages").clone()
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
        Ok(self.reply.clone())
    }
}

struct FailingLlm {
    error: CascadeError,
    calls: AtomicU32,
}

impl FailingLlm {
    fn new(error: CascadeError) -> Self {
        Self {
            error,
            calls: AtomicU32::new(0),
        }
    }
}

impl ChatModel for FailingLlm {
    fn complete(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: &[ChatMessage],
    ) -> Result<String, CascadeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.error)
    }
}

struct GateLlm {
    reply: String,
    entered: Arc<std::sync::atomic::AtomicBool>,
    proceed: Arc<std::sync::atomic::AtomicBool>,
    calls: AtomicU32,
}

impl ChatModel for GateLlm {
    fn complete(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: &[ChatMessage],
    ) -> Result<String, CascadeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.store(true, Ordering::SeqCst);
        while !self.proceed.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        Ok(self.reply.clone())
    }
}

struct ScriptedTts {
    pcm: Vec<u8>,
    voices: Mutex<Vec<String>>,
    calls: AtomicU32,
}

impl ScriptedTts {
    fn ok(pcm: &[u8]) -> Self {
        Self {
            pcm: pcm.to_vec(),
            voices: Mutex::new(Vec::new()),
            calls: AtomicU32::new(0),
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
        Ok(self.pcm.clone())
    }
}

struct UnusedRealtime;

impl RealtimeModel for UnusedRealtime {
    fn transcribe_turn(
        &self,
        _: RealtimeAudioRequest<'_>,
        _: &AtomicBool,
    ) -> Result<RealtimeTurn, RealtimeError> {
        panic!("cascaded turn must not call Realtime")
    }
}

struct FakeRealtime {
    user_text: String,
    assistant_text: String,
    tts_pcm: Vec<u8>,
    error: Mutex<Option<RealtimeError>>,
    cancel_after: bool,
    calls: AtomicU32,
    text_calls: AtomicU32,
    pcm: Mutex<Vec<u8>>,
    model_id: Mutex<Option<String>>,
    instructions: Mutex<Option<String>>,
    sample_rate: Mutex<Option<u32>>,
}

impl FakeRealtime {
    fn ok(user_text: &str, assistant_text: &str, pcm: &[u8]) -> Self {
        Self {
            user_text: user_text.into(),
            assistant_text: assistant_text.into(),
            tts_pcm: pcm.to_vec(),
            error: Mutex::new(None),
            cancel_after: false,
            calls: AtomicU32::new(0),
            text_calls: AtomicU32::new(0),
            pcm: Mutex::new(Vec::new()),
            model_id: Mutex::new(None),
            instructions: Mutex::new(None),
            sample_rate: Mutex::new(None),
        }
    }

    fn fail(error: RealtimeError) -> Self {
        Self {
            user_text: String::new(),
            assistant_text: String::new(),
            tts_pcm: Vec::new(),
            error: Mutex::new(Some(error)),
            cancel_after: false,
            calls: AtomicU32::new(0),
            text_calls: AtomicU32::new(0),
            pcm: Mutex::new(Vec::new()),
            model_id: Mutex::new(None),
            instructions: Mutex::new(None),
            sample_rate: Mutex::new(None),
        }
    }

    fn cancel_after_turn(user_text: &str, assistant_text: &str) -> Self {
        let mut fake = Self::ok(user_text, assistant_text, &[0x09]);
        fake.cancel_after = true;
        fake
    }
}

impl RealtimeModel for FakeRealtime {
    fn transcribe_turn(
        &self,
        request: RealtimeAudioRequest<'_>,
        cancel: &AtomicBool,
    ) -> Result<RealtimeTurn, RealtimeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.pcm.lock().expect("pcm") = request.pcm16le.to_vec();
        *self.model_id.lock().expect("model") = Some(request.model_id.to_owned());
        *self.instructions.lock().expect("instructions") = Some(request.instructions.to_owned());
        *self.sample_rate.lock().expect("sample_rate") = Some(request.sample_rate);
        if cancel.load(Ordering::SeqCst) {
            return Err(RealtimeError::Cancelled);
        }
        if let Some(error) = self.error.lock().expect("error").take() {
            return Err(error);
        }
        if self.cancel_after {
            cancel.store(true, Ordering::SeqCst);
        }
        // 模拟 WS 转写增量： hooks 挂了回调就会收到完整前缀快照。
        request.hooks.notify_user(&self.user_text);
        request.hooks.notify_assistant(&self.assistant_text);
        Ok(RealtimeTurn {
            user_text: self.user_text.clone(),
            assistant_text: self.assistant_text.clone(),
            tts_pcm: self.tts_pcm.clone(),
        })
    }

    fn text_turn(
        &self,
        request: RealtimeTextRequest<'_>,
        cancel: &AtomicBool,
    ) -> Result<RealtimeTurn, RealtimeError> {
        self.text_calls.fetch_add(1, Ordering::SeqCst);
        *self.model_id.lock().expect("model") = Some(request.model_id.to_owned());
        *self.instructions.lock().expect("instructions") = Some(request.instructions.to_owned());
        if cancel.load(Ordering::SeqCst) {
            return Err(RealtimeError::Cancelled);
        }
        if let Some(error) = self.error.lock().expect("error").take() {
            return Err(error);
        }
        request.hooks.notify_assistant(&self.assistant_text);
        Ok(RealtimeTurn {
            user_text: self.user_text.clone(),
            assistant_text: self.assistant_text.clone(),
            tts_pcm: self.tts_pcm.clone(),
        })
    }
}

struct UnusedEmbed;

impl EmbeddingProbe for UnusedEmbed {
    fn embed(
        &self,
        _: &ProviderEndpoint,
        _: Option<&str>,
        _: &str,
        _: u32,
        _: &str,
    ) -> Result<Vec<f32>, EmbeddingError> {
        Err(EmbeddingError::RequestFailed)
    }
}

fn opened() -> (tempfile::TempDir, Database) {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
    database.migrate().unwrap();
    (directory, database)
}

fn credentials() -> CascadeCredentials<'static> {
    CascadeCredentials {
        asr: Some("asr"),
        llm: Some("llm"),
        tts: Some("tts"),
        embed: Some("emb"),
        e2e: Some("e2e"),
    }
}

fn start_ready(service: &mut SessionService, database: &Database) -> String {
    match service
        .start(database, &ready_public_config(), true, false)
        .unwrap()
    {
        SessionStartOutcome::Started { session } => session.id,
        SessionStartOutcome::Blocked { issues } => {
            panic!("expected start, blocked {issues:?}")
        }
    }
}

fn start_ready_sink(
    service: &mut SessionService<RecordingSink>,
    database: &Database,
    config: &PublicConfig,
) -> String {
    match service.start(database, config, true, false).unwrap() {
        SessionStartOutcome::Started { session } => session.id,
        SessionStartOutcome::Blocked { issues } => {
            panic!("expected start, blocked {issues:?}")
        }
    }
}

#[test]
fn start_returns_blocked_issues_and_does_not_insert_session() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let empty = crate::runtime::test_support::empty_public_config();

    let outcome = service.start(&database, &empty, false, false).unwrap();
    match outcome {
        SessionStartOutcome::Blocked { issues } => {
            assert!(issues.len() >= 2, "{issues:?}");
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.code == "SESSION_ROUTE_REQUIRED")
            );
        }
        SessionStartOutcome::Started { session, .. } => {
            panic!("started despite preflight {}", session.id)
        }
    }
    assert_eq!(service.phase(), SessionPhase::Idle);
    assert!(SessionStore::new(&database).list().unwrap().is_empty());
}

#[test]
fn start_inserts_session_snapshot_and_reaches_listening() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);

    assert_eq!(service.phase(), SessionPhase::Listening);
    let store = SessionStore::new(&database);
    let row = store.get(&id).unwrap().expect("session");
    assert_eq!(row.status, "listening");
    assert_eq!(row.transport_mode, "direct");
    assert_eq!(row.role_profile_id, "role-1");
    assert_eq!(row.voice_route_id, "route-1");
    assert!(row.started_at.is_some());
    assert!(row.finished_at.is_none());
    let snapshots = store.list_snapshots(&id).unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].voice_route_id, "route-1");
    assert_eq!(snapshots[0].transport_mode, "direct");
    assert!(!snapshots[0].role_hash.is_empty());
    assert!(!snapshots[0].provider_ids.contains("sk-"));
}

#[test]
fn start_with_missing_bridge_exe_fails_closed_without_a_session() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
    database.migrate().unwrap();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let enumerator =
        crate::processes::InjectedProcessEnumerator::new(vec![crate::processes::MeetingProcess {
            pid: 4242,
            name: "zoom.exe".into(),
            title: "Zoom".into(),
        }]);
    let missing = directory.path().join("AudioBridge.exe");
    let error = service
        .start_with_meeting_capture(
            &database,
            &ready_public_config(),
            true,
            super::MeetingCapture {
                exe: &missing,
                pid: 4242,
                enumerator: &enumerator,
            },
            None,
            false,
        )
        .expect_err("missing exe must fail");
    assert_eq!(error.code(), "SESSION_SIDECAR_MISSING");
    assert!(SessionStore::new(&database).list().unwrap().is_empty());
    assert_eq!(service.phase(), SessionPhase::Idle);
}

#[test]
fn second_start_while_active_returns_already_active() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    start_ready(&mut service, &database);

    let error = service
        .start(&database, &ready_public_config(), true, false)
        .expect_err("second start");
    assert_eq!(error.code(), "SESSION_ALREADY_ACTIVE");
    assert!(matches!(error, SessionServiceError::AlreadyActive));
    assert_eq!(SessionStore::new(&database).list().unwrap().len(), 1);
    assert_eq!(service.phase(), SessionPhase::Listening);
}

#[test]
fn finalize_text_persists_turn_and_returns_to_listening() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("助手回复");
    let tts = ScriptedTts::ok(&[0x01, 0x02]);
    let embed = UnusedEmbed;
    let probes = SessionProbes {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };

    let turn = service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &probes,
            credentials(),
            Some("你好"),
        )
        .unwrap()
        .expect("turn");

    assert_eq!(turn.user_text, "你好");
    assert_eq!(turn.assistant_text, "助手回复");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.phase(), SessionPhase::Listening);

    let stored = SessionStore::new(&database).list_turns(&id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].user_text, "你好");
    assert_eq!(stored[0].assistant_text, "助手回复");
    assert!(!stored[0].materials_used);
}

#[test]
fn candidate_suggestion_never_plays_before_user_confirmation() {
    let (_directory, database) = opened();
    let mut config = ready_public_config();
    config.role_profiles[0].id = "personal-candidate".into();
    config.role_profiles[0].scenario = Some(crate::config::RoleScenario::Candidate);
    config.active_role_profile_id = Some("personal-candidate".into());
    let mut service = SessionService::with_sink(RecordingSink::default());
    match service.start(&database, &config, true, false).unwrap() {
        SessionStartOutcome::Started { .. } => {}
        SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
    }
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("建议回答");
    let tts = ScriptedTts::ok(&[1, 2, 3, 4]);
    let embed = UnusedEmbed;
    let probes = SessionProbes {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };
    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("面试问题"))
        .unwrap();
    assert!(
        service.sink().recorded().is_empty(),
        "candidate audio played without confirmation"
    );
    let id = service.session_id().unwrap();
    let meta = SessionStore::new(&database)
        .list_events(id)
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "turn_meta")
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&meta.payload).unwrap();
    assert_eq!(payload["userConfirmed"], false);
    assert_eq!(payload["playbackStatus"], "pending_confirmation");

    let say = service
        .execute_command(
            &database,
            &config,
            &SessionProbes {
                asr: &asr,
                llm: &llm,
                tts: &tts,
                embed: &embed,
                realtime: &UnusedRealtime,
            },
            credentials(),
            parse_cmd(serde_json::json!({
                "v": 1,
                "id": "candidate-say-bypass",
                "action": "say",
                "text": "绕过确认"
            })),
        )
        .unwrap();
    assert!(!say.ok);
    assert!(service.sink().recorded().is_empty());
}

#[test]
fn candidate_confirmation_plays_only_the_current_edited_answer() {
    let (_directory, database) = opened();
    let mut config = ready_public_config();
    config.role_profiles[0].id = "preset-candidate".into();
    config.active_role_profile_id = Some("preset-candidate".into());
    let mut service = SessionService::with_sink(RecordingSink::default());
    match service.start(&database, &config, true, false).unwrap() {
        SessionStartOutcome::Started { .. } => {}
        SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
    }
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("初始建议");
    let tts = ScriptedTts::ok(&[9, 8, 7]);
    let embed = UnusedEmbed;
    let probes = cascaded_probes(&asr, &llm, &tts, &embed);
    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("面试问题"))
        .unwrap();

    let outcome = service
        .execute_command(
            &database,
            &config,
            &probes,
            credentials(),
            parse_cmd(serde_json::json!({
                "v": 1,
                "id": "confirm-current",
                "action": "confirm_candidate",
                "text": "编辑后的回答",
                "expectedRevision": service.revision()
            })),
        )
        .expect("confirm candidate answer");

    assert!(outcome.ok);
    assert_eq!(service.sink().recorded(), [9, 8, 7]);
    let id = service.session_id().unwrap();
    let meta = SessionStore::new(&database)
        .list_events(id)
        .unwrap()
        .into_iter()
        .rev()
        .find(|event| event.kind == "turn_meta")
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&meta.payload).unwrap();
    assert_eq!(payload["userConfirmed"], true);
    assert_eq!(payload["playbackStatus"], "played");
}

#[test]
fn candidate_confirmation_cannot_be_revived_after_takeover_and_resume() {
    let (_directory, database) = opened();
    let mut config = ready_public_config();
    config.role_profiles[0].id = "preset-candidate".into();
    config.active_role_profile_id = Some("preset-candidate".into());
    let mut service = SessionService::with_sink(RecordingSink::default());
    service.start(&database, &config, true, false).unwrap();
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("old suggestion");
    let tts = ScriptedTts::ok(&[9, 8, 7]);
    let embed = UnusedEmbed;
    let probes = cascaded_probes(&asr, &llm, &tts, &embed);
    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
        .unwrap();
    service
        .set_mode(&database, AgentMode::OperatorSpeaking)
        .unwrap();
    service.set_mode(&database, AgentMode::AiActive).unwrap();
    let result = service.execute_command(&database, &config, &probes, credentials(), parse_cmd(serde_json::json!({
            "v": 1, "id": "revive-old", "action": "confirm_candidate", "text": "old answer", "expectedRevision": service.revision()
        }))).unwrap();
    assert!(
        !result.ok,
        "takeover must permanently invalidate the old confirmation"
    );
    assert!(service.sink().recorded().is_empty());
    let meta = latest_turn_meta(&database, service.session_id().unwrap());
    assert_eq!(meta["playbackStatus"], "superseded");
}

fn latest_turn_meta(database: &crate::database::Database, session_id: &str) -> serde_json::Value {
    SessionStore::new(database)
        .list_events(session_id)
        .unwrap()
        .into_iter()
        .rev()
        .find(|event| event.kind == "turn_meta")
        .map(|event| serde_json::from_str(&event.payload).unwrap())
        .unwrap()
}

fn start_candidate(
    service: &mut SessionService<RecordingSink>,
    database: &crate::database::Database,
) -> crate::config::PublicConfig {
    let mut config = ready_public_config();
    config.role_profiles[0].id = "preset-candidate".into();
    config.active_role_profile_id = Some("preset-candidate".into());
    service.start(database, &config, true, false).unwrap();
    config
}

#[test]
fn candidate_confirmation_is_superseded_when_a_new_question_fails() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let config = start_candidate(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("old suggestion");
    let tts = ScriptedTts::ok(&[9, 8, 7]);
    let embed = UnusedEmbed;
    let probes = cascaded_probes(&asr, &llm, &tts, &embed);
    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
        .unwrap();
    let fail = FailingLlm::new(crate::providers::CascadeError::RequestFailed(
        crate::providers::CascadeStage::Llm,
    ));
    let failed = SessionProbes {
        asr: &asr,
        llm: &fail,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };
    assert!(
        service
            .finalize_utterance(&database, &config, &failed, credentials(), Some("next"))
            .is_err()
    );
    let meta = latest_turn_meta(&database, service.session_id().unwrap());
    assert_eq!(meta["playbackStatus"], "superseded");
    let result = service.execute_command(&database, &config, &probes, credentials(), parse_cmd(serde_json::json!({
            "v": 1, "id": "after-fail", "action": "confirm_candidate", "text": "old answer", "expectedRevision": service.revision()
        }))).unwrap();
    assert!(!result.ok);
    assert!(service.sink().recorded().is_empty());
}

#[test]
fn candidate_confirmation_is_superseded_by_pause_mute_and_stop() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let config = start_candidate(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("old suggestion");
    let tts = ScriptedTts::ok(&[9, 8, 7]);
    let embed = UnusedEmbed;
    let probes = cascaded_probes(&asr, &llm, &tts, &embed);
    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
        .unwrap();
    service.set_mode(&database, AgentMode::Paused).unwrap();
    assert_eq!(
        latest_turn_meta(&database, service.session_id().unwrap())["playbackStatus"],
        "superseded"
    );
    let result = service.execute_command(&database, &config, &probes, credentials(), parse_cmd(serde_json::json!({
            "v": 1, "id": "after-pause", "action": "confirm_candidate", "text": "old answer", "expectedRevision": service.revision()
        }))).unwrap();
    assert!(!result.ok);

    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let config = start_candidate(&mut service, &database);
    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
        .unwrap();
    service.set_mode(&database, AgentMode::Muted).unwrap();
    assert_eq!(
        latest_turn_meta(&database, service.session_id().unwrap())["playbackStatus"],
        "superseded"
    );

    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let config = start_candidate(&mut service, &database);
    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("question"))
        .unwrap();
    service.stop(&database).unwrap();
    assert_eq!(
        latest_turn_meta(&database, service.session_id().unwrap())["playbackStatus"],
        "superseded"
    );
}

#[test]
fn candidate_late_response_after_stop_does_not_restore_confirmation() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let config = start_candidate(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let first = ScriptedLlm::ok("old suggestion");
    let tts = ScriptedTts::ok(&[9, 8, 7]);
    let embed = UnusedEmbed;
    service
        .finalize_utterance(
            &database,
            &config,
            &cascaded_probes(&asr, &first, &tts, &embed),
            credentials(),
            Some("question"),
        )
        .unwrap();
    let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let proceed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gated = GateLlm {
        reply: "late".into(),
        entered: std::sync::Arc::clone(&entered),
        proceed: std::sync::Arc::clone(&proceed),
        calls: AtomicU32::new(0),
    };
    let control = service.control();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !entered.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            control.request_stop();
            proceed.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let _ = service.finalize_utterance(
            &database,
            &config,
            &SessionProbes {
                asr: &asr,
                llm: &gated,
                tts: &tts,
                embed: &embed,
                realtime: &UnusedRealtime,
            },
            credentials(),
            Some("next"),
        );
    });
    let status = latest_turn_meta(&database, service.session_id().unwrap())["playbackStatus"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(status, "pending_confirmation");
    let result = service.execute_command(&database, &config, &cascaded_probes(&asr, &first, &tts, &embed), credentials(), parse_cmd(serde_json::json!({
            "v": 1, "id": "late-stop", "action": "confirm_candidate", "text": "late", "expectedRevision": service.revision()
        }))).unwrap();
    assert!(!result.ok);
    assert!(service.sink().recorded().is_empty());
}

#[test]
fn meeting_assistant_only_transcribes_ordinary_meeting_discussion() {
    let (_directory, database) = opened();
    let mut config = ready_public_config();
    config.role_profiles[0].id = "personal-meeting-assistant".into();
    config.role_profiles[0].name = "小助理".into();
    config.role_profiles[0].scenario = Some(crate::config::RoleScenario::MeetingAssistant);
    config.active_role_profile_id = Some("personal-meeting-assistant".into());
    let mut service = SessionService::with_sink(RecordingSink::default());
    match service.start(&database, &config, true, false).unwrap() {
        SessionStartOutcome::Started { .. } => {}
        SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
    }
    service.capture_mut().mark_meeting_bridge_for_tests();
    service.push_pcm(&[1, 0, 2, 0, 3, 0]);
    let asr = ScriptedAsr::ok("今天讨论项目进度");
    let llm = ScriptedLlm::ok("不应生成回复");
    let tts = ScriptedTts::ok(&[1, 2]);
    let embed = UnusedEmbed;

    let turn = service
        .finalize_utterance(
            &database,
            &config,
            &cascaded_probes(&asr, &llm, &tts, &embed),
            credentials(),
            None,
        )
        .unwrap()
        .expect("transcript turn");

    assert_eq!(turn.user_text, "今天讨论项目进度");
    assert_eq!(turn.assistant_text, "");
    assert_eq!(asr.calls.load(Ordering::SeqCst), 1);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
    assert!(service.sink().recorded().is_empty());
}

#[test]
fn meeting_assistant_answers_voice_on_local_microphone() {
    let (_directory, database) = opened();
    let mut config = ready_public_config();
    config.role_profiles[0].id = "personal-meeting-assistant".into();
    config.role_profiles[0].name = "小助理".into();
    config.role_profiles[0].scenario = Some(crate::config::RoleScenario::MeetingAssistant);
    config.active_role_profile_id = Some("personal-meeting-assistant".into());
    let mut service = SessionService::with_sink(RecordingSink::default());
    match service.start(&database, &config, true, false).unwrap() {
        SessionStartOutcome::Started { .. } => {}
        SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
    }
    service.push_pcm(&[1, 0, 2, 0, 3, 0]);
    let asr = ScriptedAsr::ok("反应有点慢");
    let llm = ScriptedLlm::ok("好的，我会加快响应");
    let tts = ScriptedTts::ok(&[3, 4]);
    let embed = UnusedEmbed;

    let turn = service
        .finalize_utterance(
            &database,
            &config,
            &cascaded_probes(&asr, &llm, &tts, &embed),
            credentials(),
            None,
        )
        .unwrap()
        .expect("answer turn");

    assert_eq!(turn.assistant_text, "好的，我会加快响应");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(tts.calls.load(Ordering::SeqCst), 1);
    assert_eq!(service.sink().recorded(), [3, 4]);
}

#[test]
fn meeting_assistant_hotkey_forces_one_answer_and_records_trigger() {
    let (_directory, database) = opened();
    let mut config = ready_public_config();
    config.role_profiles[0].scenario = Some(crate::config::RoleScenario::MeetingAssistant);
    let mut service = SessionService::with_sink(RecordingSink::default());
    start_ready_sink(&mut service, &database, &config);
    service.push_pcm(&[1, 0, 2, 0, 3, 0]);
    let asr = ScriptedAsr::ok("今天讨论项目进度");
    let llm = ScriptedLlm::ok("当前进度正常");
    let tts = ScriptedTts::ok(&[7, 8]);
    let embed = UnusedEmbed;

    let turn = service
        .finalize_utterance_forced(
            &database,
            &config,
            &cascaded_probes(&asr, &llm, &tts, &embed),
            credentials(),
        )
        .unwrap()
        .expect("hotkey turn");

    assert_eq!(turn.assistant_text, "当前进度正常");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    let event = SessionStore::new(&database)
        .list_events(service.session_id().unwrap())
        .unwrap()
        .into_iter()
        .rev()
        .find(|event| event.kind == "turn_meta")
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap();
    assert_eq!(payload["triggerSource"], "hotkey");
}

#[test]
fn meeting_assistant_answers_when_its_configured_name_is_mentioned() {
    let (_directory, database) = opened();
    let mut config = ready_public_config();
    config.role_profiles[0].name = "小助理".into();
    config.role_profiles[0].scenario = Some(crate::config::RoleScenario::MeetingAssistant);
    let mut service = SessionService::with_sink(RecordingSink::default());
    start_ready_sink(&mut service, &database, &config);
    service.push_pcm(&[1, 0, 2, 0, 3, 0]);
    let asr = ScriptedAsr::ok("小助理，请总结刚才的结论");
    let llm = ScriptedLlm::ok("结论是按计划推进");
    let tts = ScriptedTts::ok(&[4, 5, 6]);
    let embed = UnusedEmbed;

    let turn = service
        .finalize_utterance(
            &database,
            &config,
            &cascaded_probes(&asr, &llm, &tts, &embed),
            credentials(),
            None,
        )
        .unwrap()
        .expect("answer turn");

    assert_eq!(turn.assistant_text, "结论是按计划推进");
    assert_eq!(asr.calls.load(Ordering::SeqCst), 1);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(service.sink().recorded(), [4, 5, 6]);
}

#[test]
fn finalize_injected_pcm_uses_asr() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    start_ready(&mut service, &database);
    service.push_pcm(&[0x10, 0x00, 0x20, 0x00, 0x30, 0x00]);
    let asr = ScriptedAsr::ok("从音频来");
    let llm = ScriptedLlm::ok("收到");
    let tts = ScriptedTts::ok(&[0x03, 0x04]);
    let embed = UnusedEmbed;
    let probes = SessionProbes {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };

    let turn = service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &probes,
            credentials(),
            None,
        )
        .unwrap()
        .expect("turn");

    assert_eq!(turn.user_text, "从音频来");
    assert_eq!(asr.calls.load(Ordering::SeqCst), 1);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(service.phase(), SessionPhase::Listening);
}

#[test]
fn takeover_cancels_sink_and_skips_llm() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let id = start_ready_sink(&mut service, &database, &ready_public_config());
    service
        .set_mode(&database, AgentMode::OperatorSpeaking)
        .unwrap();

    assert_eq!(service.mode(), AgentMode::OperatorSpeaking);
    assert!(!service.runtime_can_answer());
    assert!(service.sink().cancelled());

    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("should-not-run");
    let tts = ScriptedTts::ok(&[0x09]);
    let embed = UnusedEmbed;
    let probes = SessionProbes {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };
    let turn = service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &probes,
            credentials(),
            Some("接管后提问"),
        )
        .unwrap();

    assert!(turn.is_none());
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert!(
        SessionStore::new(&database)
            .list_turns(&id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(service.phase(), SessionPhase::Listening);
}

#[test]
fn stop_marks_completed_and_sets_finished_at() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);

    let row = service.stop(&database).unwrap();
    assert_eq!(row.status, "completed");
    assert!(row.finished_at.is_some());
    assert_eq!(service.phase(), SessionPhase::Completed);

    let stored = SessionStore::new(&database).get(&id).unwrap().unwrap();
    assert_eq!(stored.status, "completed");
    assert!(stored.finished_at.is_some());
}

#[test]
fn start_after_stop_opens_a_new_session() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let first = start_ready(&mut service, &database);
    service.stop(&database).unwrap();
    let second = start_ready(&mut service, &database);
    assert_ne!(first, second);
    assert_eq!(service.phase(), SessionPhase::Listening);
    assert_eq!(SessionStore::new(&database).list().unwrap().len(), 2);
}

#[test]
fn empty_voice_id_defaults_to_alloy() {
    let (_directory, database) = opened();
    let mut config = ready_public_config();
    config.speech.voice_routes[0].voice_id = Some(String::new());
    let mut service = SessionService::new();
    match service.start(&database, &config, true, false).unwrap() {
        SessionStartOutcome::Started { .. } => {}
        SessionStartOutcome::Blocked { issues } => panic!("blocked {issues:?}"),
    }
    let asr = ScriptedAsr::ok("hi");
    let llm = ScriptedLlm::ok("ok");
    let tts = ScriptedTts::ok(&[0x11, 0x22]);
    let embed = UnusedEmbed;
    let probes = SessionProbes {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };

    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("hi"))
        .unwrap()
        .expect("turn");
    assert_eq!(
        tts.voices.lock().expect("voices").as_slice(),
        ["alloy".to_string()]
    );
}

#[test]
fn finalize_llm_error_returns_to_listening_and_second_finalize_succeeds() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let fail_llm = FailingLlm::new(CascadeError::RequestFailed(
        crate::providers::CascadeStage::Llm,
    ));
    let tts = ScriptedTts::ok(&[0x01]);
    let embed = UnusedEmbed;
    let fail_probes = SessionProbes {
        asr: &asr,
        llm: &fail_llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };

    let error = service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &fail_probes,
            credentials(),
            Some("第一轮"),
        )
        .expect_err("llm fail");
    assert_eq!(error.code(), "LLM_REQUEST_FAILED");
    assert_eq!(service.last_error_code(), Some("LLM_REQUEST_FAILED"));
    assert_eq!(service.phase(), SessionPhase::Listening);
    let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
    assert_eq!(row.status, "listening");
    assert!(
        SessionStore::new(&database)
            .list_turns(&id)
            .unwrap()
            .is_empty()
    );

    let ok_llm = ScriptedLlm::ok("第二轮回复");
    let ok_probes = SessionProbes {
        asr: &asr,
        llm: &ok_llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };
    let turn = service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &ok_probes,
            credentials(),
            Some("第二轮"),
        )
        .unwrap()
        .expect("second turn");
    assert_eq!(turn.assistant_text, "第二轮回复");
    assert_eq!(service.phase(), SessionPhase::Listening);
    assert_eq!(service.last_error_code(), None);
    assert_eq!(ok_llm.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn finalize_unauthorized_fails_session() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let llm = FailingLlm::new(CascadeError::Unauthorized(
        crate::providers::CascadeStage::Llm,
    ));
    let tts = ScriptedTts::ok(&[0x01]);
    let embed = UnusedEmbed;
    let probes = SessionProbes {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };

    let error = service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &probes,
            credentials(),
            Some("密钥失效"),
        )
        .expect_err("unauthorized");
    assert_eq!(error.code(), "LLM_UNAUTHORIZED");
    assert_eq!(service.last_error_code(), Some("LLM_UNAUTHORIZED"));
    assert_eq!(service.phase(), SessionPhase::Failed);
    let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
    assert_eq!(row.status, "failed");
    assert!(row.finished_at.is_some());
}

#[test]
fn request_cancel_during_slow_llm_skips_tts() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    start_ready(&mut service, &database);
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let proceed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let asr = ScriptedAsr::ok("ignored");
    let llm = GateLlm {
        reply: "不应播报".into(),
        entered: Arc::clone(&entered),
        proceed: Arc::clone(&proceed),
        calls: AtomicU32::new(0),
    };
    let tts = ScriptedTts::ok(&[0x22]);
    let embed = UnusedEmbed;
    let probes = SessionProbes {
        asr: &asr,
        llm: &llm,
        tts: &tts,
        embed: &embed,
        realtime: &UnusedRealtime,
    };
    let control = service.control();
    let waiter = std::thread::spawn(move || {
        while !entered.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        control.request_cancel();
        proceed.store(true, Ordering::SeqCst);
    });

    let error = service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &probes,
            credentials(),
            Some("取消"),
        )
        .expect_err("cancelled");
    waiter.join().expect("cancel thread");
    assert_eq!(error.code(), "SESSION_CANCELLED");
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.phase(), SessionPhase::Listening);
    assert_eq!(service.last_error_code(), Some("SESSION_CANCELLED"));
}

#[test]
fn sidecar_second_crash_fails_session() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);

    service.capture().mark_sidecar_exited();
    assert_eq!(service.poll_sidecar(&database).unwrap(), SidecarPoll::Alive);
    service.capture().mark_sidecar_exited();
    let error = service.poll_sidecar(&database).expect_err("second crash");
    assert_eq!(error.code(), "SESSION_SIDECAR_FAILED");
    assert_eq!(service.phase(), SessionPhase::Failed);
    let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
    assert_eq!(row.status, "failed");
    assert!(row.finished_at.is_some());
}

#[test]
fn stop_after_sidecar_crash_keeps_failed_status() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);

    service.capture().mark_sidecar_exited();
    assert_eq!(service.poll_sidecar(&database).unwrap(), SidecarPoll::Alive);
    service.capture().mark_sidecar_exited();
    let error = service.poll_sidecar(&database).expect_err("second crash");
    assert_eq!(error.code(), "SESSION_SIDECAR_FAILED");
    assert_eq!(service.phase(), SessionPhase::Failed);

    let row = service.stop(&database).unwrap();
    assert_eq!(row.status, "failed");
    assert!(row.finished_at.is_some());
    assert_eq!(service.phase(), SessionPhase::Failed);

    let stored = SessionStore::new(&database).get(&id).unwrap().unwrap();
    assert_eq!(stored.status, "failed");
    assert!(stored.finished_at.is_some());
}

#[test]
fn app_state_open_marks_leftover_sessions_interrupted_without_resume() {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths {
        data_directory: directory.path().join("data"),
        logs_directory: directory.path().join("logs"),
        config_path: directory.path().join("config.json"),
        legacy_search_roots: Vec::new(),
    };
    std::fs::write(&paths.config_path, r#"{"configVersion":1}"#).unwrap();
    std::fs::create_dir_all(&paths.data_directory).unwrap();
    let db_path = paths.data_directory.join("app.sqlite3");
    {
        let database = Database::open(&db_path).unwrap();
        database.migrate().unwrap();
        SessionStore::new(&database)
            .insert_session(NewSession {
                id: "leftover",
                status: "listening",
                role_profile_id: "role-1",
                voice_route_id: "route-1",
                transport_mode: "direct",
            })
            .unwrap();
    }

    let state = AppState::initialize(paths, Arc::new(MemorySecretStore::default())).unwrap();
    let guard = state.database.lock().expect("db");
    let database = guard.as_ref().expect("opened");
    let leftover = SessionStore::new(database)
        .get("leftover")
        .unwrap()
        .expect("row");
    assert_eq!(leftover.status, "interrupted");
    assert!(leftover.finished_at.is_some());

    let service = SessionService::new();
    assert_eq!(service.phase(), SessionPhase::Idle);
    assert_eq!(
        service.capture().poll_sidecar().unwrap(),
        SidecarPoll::Alive
    );
    assert_eq!(service.capture().snapshot_48k().len(), 0);
    assert!(service.session_id().is_none());
}

fn start_e2e(service: &mut SessionService, database: &Database) -> String {
    match service
        .start(database, &ready_e2e_public_config(), true, false)
        .unwrap()
    {
        SessionStartOutcome::Started { session } => session.id,
        SessionStartOutcome::Blocked { issues } => {
            panic!("expected e2e start, blocked {issues:?}")
        }
    }
}

fn e2e_probes<'a>(
    asr: &'a ScriptedAsr,
    llm: &'a ScriptedLlm,
    tts: &'a ScriptedTts,
    embed: &'a UnusedEmbed,
    realtime: &'a FakeRealtime,
) -> SessionProbes<'a> {
    SessionProbes {
        asr,
        llm,
        tts,
        embed,
        realtime,
    }
}

#[test]
fn e2e_start_keeps_direct_transport_and_preflight_passes() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_e2e(&mut service, &database);
    assert_eq!(service.phase(), SessionPhase::Listening);
    let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
    assert_eq!(row.transport_mode, "direct");
    assert_eq!(row.status, "listening");
    let snapshots = SessionStore::new(&database).list_snapshots(&id).unwrap();
    assert_eq!(snapshots[0].transport_mode, "direct");
    assert!(snapshots[0].provider_ids.contains("e2e-1"));
    assert!(snapshots[0].model_ids.contains("gpt-realtime"));
}

#[test]
fn e2e_fake_realtime_turn_persists_and_sets_unused_materials() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_e2e(&mut service, &database);
    service.push_pcm(&[0x10, 0x00, 0x20, 0x00]);
    let asr = ScriptedAsr::ok("should-not-run");
    let llm = ScriptedLlm::ok("should-not-run");
    let tts = ScriptedTts::ok(&[0x99]);
    let embed = UnusedEmbed;
    let realtime = FakeRealtime::ok("你好", "实时回复", &[0x01, 0x02]);
    let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

    let turn = service
        .finalize_utterance(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            None,
        )
        .unwrap()
        .expect("turn");

    assert_eq!(turn.user_text, "你好");
    assert_eq!(turn.assistant_text, "实时回复");
    assert_eq!(turn.tts_pcm, [0x01, 0x02]);
    assert!(!turn.materials_used);
    assert_eq!(realtime.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        realtime.model_id.lock().expect("model").as_deref(),
        Some("gpt-realtime")
    );
    assert_eq!(
        realtime.pcm.lock().expect("pcm").as_slice(),
        service.capture().pcm_for_asr().as_slice()
    );
    assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.phase(), SessionPhase::Listening);
    assert!(service.unused_materials());

    let stored = SessionStore::new(&database).list_turns(&id).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].user_text, "你好");
    assert_eq!(stored[0].assistant_text, "实时回复");
    assert!(!stored[0].materials_used);
}

#[test]
fn e2e_cancel_between_realtime_and_persist_returns_to_listening() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_e2e(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("ignored");
    let tts = ScriptedTts::ok(&[0x01]);
    let embed = UnusedEmbed;
    let realtime = FakeRealtime::cancel_after_turn("取消前", "不应落库");
    let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

    let error = service
        .finalize_utterance(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            None,
        )
        .expect_err("cancelled");
    assert_eq!(error.code(), "SESSION_CANCELLED");
    assert_eq!(service.last_error_code(), Some("SESSION_CANCELLED"));
    assert_eq!(service.phase(), SessionPhase::Listening);
    assert!(
        SessionStore::new(&database)
            .list_turns(&id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(realtime.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn e2e_typed_message_uses_text_instead_of_empty_audio() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_e2e(&mut service, &database);
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::ok("unused");
    let tts = ScriptedTts::ok(&[1]);
    let embed = UnusedEmbed;
    let realtime = FakeRealtime::ok("", "你好", &[1, 2]);
    let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);
    let turn = service
        .finalize_utterance(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            Some("你好啊"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        realtime.calls.load(Ordering::SeqCst),
        0,
        "typed input must not submit empty PCM"
    );
    assert_eq!(realtime.text_calls.load(Ordering::SeqCst), 1);
    assert_eq!(turn.user_text, "你好啊");
    assert_eq!(
        SessionStore::new(&database).list_turns(&id).unwrap().len(),
        1
    );
}

#[test]
fn e2e_connection_failure_can_retry_without_creating_a_phantom_turn() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_e2e(&mut service, &database);
    let asr = ScriptedAsr::ok("unused");
    let llm = ScriptedLlm::ok("unused");
    let tts = ScriptedTts::ok(&[1]);
    let embed = UnusedEmbed;
    let realtime = FakeRealtime::ok("", "你好", &[1, 2]);
    *realtime.error.lock().unwrap() = Some(RealtimeError::ConnectionClosed);
    let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);
    let config = ready_e2e_public_config();
    assert!(
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("你好啊"))
            .is_err()
    );
    assert_eq!(service.phase(), SessionPhase::Listening);
    assert!(
        SessionStore::new(&database)
            .list_turns(&id)
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .finalize_utterance(&database, &config, &probes, credentials(), Some("你好啊"))
            .unwrap()
            .is_some()
    );
    assert_eq!(service.last_error_code(), None);
    assert_eq!(
        SessionStore::new(&database).list_turns(&id).unwrap().len(),
        1
    );
}

#[test]
fn e2e_unauthorized_fails_session() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_e2e(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("ignored");
    let tts = ScriptedTts::ok(&[0x01]);
    let embed = UnusedEmbed;
    let realtime = FakeRealtime::fail(RealtimeError::Unauthorized);
    let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

    let error = service
        .finalize_utterance(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            None,
        )
        .expect_err("unauthorized");
    assert_eq!(error.code(), "REALTIME_UNAUTHORIZED");
    assert_eq!(service.last_error_code(), Some("REALTIME_UNAUTHORIZED"));
    assert_eq!(service.phase(), SessionPhase::Failed);
    let row = SessionStore::new(&database).get(&id).unwrap().unwrap();
    assert_eq!(row.status, "failed");
    assert!(row.finished_at.is_some());
}

#[test]
fn e2e_sends_role_and_materials_before_generate() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
    database.migrate().unwrap();
    let path = directory.path().join("note.txt");
    std::fs::write(&path, "负责订单服务与 Kafka 链路，完整句子用于检索。").unwrap();
    crate::services::MaterialService::new(&database, directory.path())
        .import_file(&path)
        .unwrap();

    let mut service = SessionService::new();
    match service
        .start(&database, &ready_e2e_public_config(), true, false)
        .unwrap()
    {
        SessionStartOutcome::Started { .. } => {}
        SessionStartOutcome::Blocked { issues } => panic!("blocked {issues:?}"),
    }
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("ignored");
    let tts = ScriptedTts::ok(&[0x01]);
    let embed = UnusedEmbed;
    let realtime = FakeRealtime::ok("ignored", "资料回答", &[0x02]);
    let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

    let turn = service
        .finalize_utterance(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            Some("请介绍你做过的订单服务项目"),
        )
        .unwrap()
        .expect("turn");
    assert!(turn.materials_used);
    assert!(!service.unused_materials());
    assert!(
        turn.citations
            .iter()
            .any(|citation| citation.snippet.contains("订单服务"))
    );
    let instructions = realtime
        .instructions
        .lock()
        .expect("instructions")
        .clone()
        .expect("realtime must receive instructions before generate");
    assert!(instructions.contains("UNIQUE_PROMPT_BODY_DO_NOT_SNAPSHOT"));
    assert!(instructions.contains("UNIQUE_STYLE_DO_NOT_SNAPSHOT"));
    assert!(instructions.contains("订单服务"));
    assert_eq!(realtime.text_calls.load(Ordering::SeqCst), 1);
    assert_eq!(realtime.calls.load(Ordering::SeqCst), 0);
    assert!(realtime.sample_rate.lock().expect("sample_rate").is_none());
}

#[test]
fn e2e_voice_only_does_not_mark_materials_used_after_the_fact() {
    let directory = tempfile::tempdir().unwrap();
    let database = Database::open(directory.path().join("app.sqlite3")).unwrap();
    database.migrate().unwrap();
    let path = directory.path().join("note.txt");
    std::fs::write(&path, "负责订单服务与 Kafka 链路，完整句子用于检索。").unwrap();
    crate::services::MaterialService::new(&database, directory.path())
        .import_file(&path)
        .unwrap();

    let mut service = SessionService::new();
    match service
        .start(&database, &ready_e2e_public_config(), true, false)
        .unwrap()
    {
        SessionStartOutcome::Started { .. } => {}
        SessionStartOutcome::Blocked { issues } => panic!("blocked {issues:?}"),
    }
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("ignored");
    let tts = ScriptedTts::ok(&[0x01]);
    let embed = UnusedEmbed;
    let realtime = FakeRealtime::ok("订单服务", "资料回答", &[0x02]);
    let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);

    let turn = service
        .finalize_utterance(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            None,
        )
        .unwrap()
        .expect("turn");
    assert!(!turn.materials_used);
    assert!(turn.citations.is_empty());
    let instructions = realtime
        .instructions
        .lock()
        .expect("instructions")
        .clone()
        .unwrap();
    assert!(instructions.contains("UNIQUE_PROMPT_BODY_DO_NOT_SNAPSHOT"));
    assert!(!instructions.contains("可用资料"));
}

fn cascaded_probes<'a>(
    asr: &'a ScriptedAsr,
    llm: &'a ScriptedLlm,
    tts: &'a ScriptedTts,
    embed: &'a UnusedEmbed,
) -> SessionProbes<'a> {
    SessionProbes {
        asr,
        llm,
        tts,
        embed,
        realtime: &UnusedRealtime,
    }
}

fn parse_cmd(payload: serde_json::Value) -> crate::runtime::AgentCommand {
    crate::runtime::parse_agent_command(&payload).expect("command")
}

#[test]
fn say_speaks_via_tts_and_result_omits_pcm() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    start_ready_sink(&mut service, &database, &ready_public_config());
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("should-not-run");
    let tts = ScriptedTts::ok(&[0x11, 0x22]);
    let embed = UnusedEmbed;
    let probes = cascaded_probes(&asr, &llm, &tts, &embed);

    let outcome = service
        .execute_command(
            &database,
            &ready_public_config(),
            &probes,
            credentials(),
            parse_cmd(serde_json::json!({
                "v": 1,
                "id": "cmd-say",
                "action": "say",
                "text": "请开始"
            })),
        )
        .expect("say");

    assert!(outcome.ok);
    assert_eq!(outcome.command_id, "cmd-say");
    assert_eq!(outcome.action.as_str(), "say");
    assert_eq!(outcome.result["text"], "请开始");
    assert_eq!(outcome.error, "");
    assert_eq!(tts.calls.load(Ordering::SeqCst), 1);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.sink().recorded(), [0x11, 0x22]);
    let json = serde_json::to_string(&outcome.result).unwrap();
    assert!(!json.contains("pcm"));
    assert!(!json.to_ascii_lowercase().contains("sk-"));
}

#[test]
fn retry_replaces_last_assistant_and_revision_mismatch_fails_closed() {
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let first = ScriptedLlm::ok("原回复");
    let tts = ScriptedTts::ok(&[0x01]);
    let embed = UnusedEmbed;
    service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &cascaded_probes(&asr, &first, &tts, &embed),
            credentials(),
            Some("你好"),
        )
        .unwrap()
        .expect("turn");
    assert_eq!(service.revision(), 1);

    let stale = service
        .execute_command(
            &database,
            &ready_public_config(),
            &cascaded_probes(&asr, &first, &tts, &embed),
            credentials(),
            parse_cmd(serde_json::json!({
                "v": 1,
                "id": "cmd-stale",
                "action": "retry",
                "expectedRevision": 0
            })),
        )
        .expect("stale outcome");
    assert!(!stale.ok);
    assert_eq!(stale.error, "SESSION_CHANGED");
    assert_eq!(
        SessionStore::new(&database).list_turns(&id).unwrap()[0].assistant_text,
        "原回复"
    );

    let retry_llm = ScriptedLlm::ok("新回复");
    let outcome = service
        .execute_command(
            &database,
            &ready_public_config(),
            &cascaded_probes(&asr, &retry_llm, &tts, &embed),
            credentials(),
            parse_cmd(serde_json::json!({
                "v": 1,
                "id": "cmd-retry",
                "action": "retry",
                "expectedRevision": 1
            })),
        )
        .expect("retry");
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(outcome.result["question"], "新回复");
    assert_eq!(service.revision(), 2);
    assert_eq!(
        SessionStore::new(&database).list_turns(&id).unwrap()[0].assistant_text,
        "新回复"
    );
}

#[test]
fn correct_replaces_last_assistant_and_speaks_given_text() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let id = start_ready_sink(&mut service, &database, &ready_public_config());
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("原回复");
    let tts = ScriptedTts::ok(&[0x33]);
    let embed = UnusedEmbed;
    service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &cascaded_probes(&asr, &llm, &tts, &embed),
            credentials(),
            Some("你好"),
        )
        .unwrap();

    let outcome = service
        .execute_command(
            &database,
            &ready_public_config(),
            &cascaded_probes(&asr, &llm, &tts, &embed),
            credentials(),
            parse_cmd(serde_json::json!({
                "v": 1,
                "id": "cmd-correct",
                "action": "correct",
                "answer": "改成这句",
                "expectedRevision": 1
            })),
        )
        .expect("correct");
    assert!(outcome.ok);
    assert_eq!(outcome.result["answer"], "改成这句");
    assert_eq!(
        SessionStore::new(&database).list_turns(&id).unwrap()[0].assistant_text,
        "改成这句"
    );
    assert!(service.sink().recorded().ends_with(&[0x33]));
    assert_eq!(service.revision(), 2);
}

#[test]
fn report_returns_summary_without_speaking_or_secrets() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    start_ready_sink(&mut service, &database, &ready_public_config());
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok(
        r#"{"summary":"短纪要","strengths":[],"followUps":[],"limitations":[],"evidence":[]}"#,
    );
    let tts = ScriptedTts::ok(&[0x44]);
    let embed = UnusedEmbed;
    let before = service.sink().recorded().len();
    let outcome = service
        .execute_command(
            &database,
            &ready_public_config(),
            &cascaded_probes(&asr, &llm, &tts, &embed),
            credentials(),
            parse_cmd(serde_json::json!({
                "v": 1,
                "id": "cmd-report",
                "action": "report"
            })),
        )
        .expect("report");
    assert!(outcome.ok);
    assert_eq!(outcome.result["summary"], "短纪要");
    assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.sink().recorded().len(), before);
    let json = serde_json::to_string(&outcome.result).unwrap();
    assert!(!json.contains("pcm"));
    assert!(!json.to_ascii_lowercase().contains("sk-"));
}

#[test]
fn e2e_say_uses_realtime_text_turn() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    start_ready_sink(&mut service, &database, &ready_e2e_public_config());
    let asr = ScriptedAsr::ok("should-not-run");
    let llm = ScriptedLlm::ok("should-not-run");
    let tts = ScriptedTts::ok(&[0x99]);
    let embed = UnusedEmbed;
    let realtime = FakeRealtime::ok("", "请开始", &[0x55, 0x66]);
    let probes = e2e_probes(&asr, &llm, &tts, &embed, &realtime);
    let outcome = service
        .execute_command(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            parse_cmd(serde_json::json!({
                "v": 1,
                "id": "cmd-e2e-say",
                "action": "say",
                "text": "请开始"
            })),
        )
        .expect("e2e say");
    assert!(outcome.ok);
    assert_eq!(realtime.text_calls.load(Ordering::SeqCst), 1);
    assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert_eq!(tts.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.sink().recorded(), [0x55, 0x66]);
}

#[test]
fn e2e_manual_text_with_pump_uses_dedicated_text_turn() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    start_ready_sink(&mut service, &database, &ready_e2e_public_config());
    // 全双工泵在线（真实泵由 commands 层装配；注入共享状态即可命中泵分支）。
    // 手动文本不得等泵轮次（等 500ms 后静默丢弃），必须走独立 text_turn。
    service.realtime_shared = Some(std::sync::Arc::new(
        crate::services::realtime_pump::PumpShared::new(),
    ));
    let asr = ScriptedAsr::ok("voice-should-not-run");
    let llm = ScriptedLlm::ok("should-not-run");
    let tts = ScriptedTts::ok(&[0x99]);
    let realtime = FakeRealtime::ok("", "文字已收到", &[0x31, 0x32]);
    let probes = e2e_probes(&asr, &llm, &tts, &UnusedEmbed, &realtime);
    let turn = service
        .finalize_utterance_with_hooks(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            Some("用打字问的问题"),
            &crate::providers::TurnStreamHooks::none(),
        )
        .unwrap()
        .expect("manual text turn");
    assert_eq!(turn.user_text, "用打字问的问题");
    assert_eq!(turn.assistant_text, "文字已收到");
    assert_eq!(realtime.text_calls.load(Ordering::SeqCst), 1);
    assert_eq!(realtime.calls.load(Ordering::SeqCst), 0);
    assert_eq!(asr.calls.load(Ordering::SeqCst), 0);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.sink().recorded(), [0x31, 0x32]);
}

#[test]
fn e2e_voice_finalize_forwards_stream_snapshots_to_hooks() {
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    start_ready_sink(&mut service, &database, &ready_e2e_public_config());
    let asr = ScriptedAsr::ok("should-not-run");
    let llm = ScriptedLlm::ok("should-not-run");
    let tts = ScriptedTts::ok(&[0x99]);
    let realtime = FakeRealtime::ok("现在几点", "现在是三点", &[0x77]);
    let probes = e2e_probes(&asr, &llm, &tts, &UnusedEmbed, &realtime);
    let user_snapshots = Mutex::new(Vec::<String>::new());
    let assistant_snapshots = Mutex::new(Vec::<String>::new());
    let hooks = crate::providers::TurnStreamHooks {
        user_text: Some(&|text| user_snapshots.lock().unwrap().push(text.to_owned())),
        assistant_text: Some(&|text| assistant_snapshots.lock().unwrap().push(text.to_owned())),
    };

    let turn = service
        .finalize_utterance_with_hooks(
            &database,
            &ready_e2e_public_config(),
            &probes,
            credentials(),
            None,
            &hooks,
        )
        .unwrap()
        .expect("e2e voice turn");

    assert_eq!(turn.user_text, "现在几点");
    assert_eq!(turn.assistant_text, "现在是三点");
    assert_eq!(user_snapshots.lock().unwrap().as_slice(), ["现在几点"]);
    assert_eq!(
        assistant_snapshots.lock().unwrap().as_slice(),
        ["现在是三点"]
    );
}

#[test]
fn stale_barge_flag_before_playback_is_discarded_not_swallowing_the_answer() {
    // 意义：played 后 700ms 尾窗内的真实人声会残留 barge 旗标；播报开始前必须
    // 显式消费，否则下一轮播报闭包入口即取消、整段回答被吞（interrupted）。
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);
    // 直接驱动 finalize 的播报分支：预置 BridgePlayback（play 首行取消检查，
    // 不会触碰不存在的 sidecar 可执行文件），TTS 产物由 ScriptedTts 提供。
    service.configure_playback(Some(crate::audio::playback::BridgePlayback {
        executable: std::path::PathBuf::from("missing-barge-in-playback.exe"),
        endpoint_id: "test-device".into(),
    }));
    service
        .control()
        .set_barge_in_source(Arc::new(AtomicBool::new(true)));
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("不应被吞掉的回复");
    let tts = ScriptedTts::ok(&[0x01, 0x02]);

    service
        .finalize_utterance(
            &database,
            &ready_public_config(),
            &cascaded_probes(&asr, &llm, &tts, &UnusedEmbed),
            credentials(),
            Some("你好"),
        )
        .unwrap()
        .expect("turn");

    let meta = latest_turn_meta(&database, &id);
    // 残留旗标不得构成取消：play 被真实触达（首行取消检查通过），只因夹具
    // 缺少可执行文件而 failed；绝不是 interrupted。
    assert_eq!(meta["playbackStatus"], "failed");
    assert_eq!(service.last_error_code(), Some("SESSION_SIDECAR_MISSING"));
    // 会话仍存活：阶段与库中状态都回到 listening，且残留旗标已被消费。
    assert_eq!(service.phase(), SessionPhase::Listening);
    let row = SessionStore::new(&database).get(&id).unwrap().expect("row");
    assert_eq!(row.status, "listening");
    assert!(!service.control().barge_in_requested());
}

#[test]
fn session_control_reset_clears_barge_in_source() {
    // 意义：停止/重建会话后，旧 capture 的旗标来源不得继续作用于控制端（stop 泄漏）。
    let control = super::SessionControl::new();
    let flag = Arc::new(AtomicBool::new(true));
    control.set_barge_in_source(flag.clone());
    assert!(control.barge_in_requested());

    control.reset();

    assert!(
        !control.barge_in_requested(),
        "reset must drop the barge-in source"
    );
    // 来源已被置 None：旧 Arc 再置位也不影响控制端。
    flag.store(true, Ordering::SeqCst);
    assert!(!control.barge_in_requested());
}

#[test]
fn barge_in_source_wired_only_when_allowed() {
    // 意义：会话级开关接通 capture 旗标与控制端；未启用时旗标不得构成门控。
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    service
        .start_inner(&database, &ready_public_config(), true, None, None, true)
        .unwrap();
    service
        .capture()
        .barge_in_flag()
        .store(true, Ordering::SeqCst);
    assert!(service.control().barge_in_requested());
    assert!(service.control().take_barge_in());
    assert!(
        !service.control().take_barge_in(),
        "flag must reset once taken"
    );

    // 未启用（allow=false，含会议桥走的同一条 else 分支）：旗标置位也不生效。
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    service
        .start_inner(&database, &ready_public_config(), true, None, None, false)
        .unwrap();
    service
        .capture()
        .barge_in_flag()
        .store(true, Ordering::SeqCst);
    assert!(!service.control().barge_in_requested());
}

#[test]
fn stale_summary_job_is_persisted_on_next_finalize() {
    // 意义：后台摘要 job 在下一次 finalize 开头被轮询落库（trim 后写入），
    // 摘要并入同一轮的系统提示，被摘要覆盖的轮次退出原始历史；
    // 失败结果静默丢弃，不落库且回答照常。
    let (_directory, database) = opened();
    let mut service = SessionService::new();
    let id = start_ready(&mut service, &database);
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("最新回答");
    let tts = ScriptedTts::ok(&[0x01]);
    let embed = UnusedEmbed;
    let probes = cascaded_probes(&asr, &llm, &tts, &embed);
    let config = ready_public_config();

    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("第一问"))
        .unwrap()
        .expect("first turn");

    // 预置一个已完成的摘要 job：摘要文本 + 覆盖到第 0 轮（模拟后台压缩已完成）。
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(("这是滚动摘要文本".to_string(), 0))).unwrap();
    drop(tx);
    service.summary_job = Some(rx);

    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("第二问"))
        .unwrap()
        .expect("second turn");

    // 断言一：context_summary 落库。
    assert_eq!(
        SessionStore::new(&database).context_summary(&id).unwrap(),
        Some(("这是滚动摘要文本".to_string(), 0))
    );
    // 断言二：摘要并入本轮系统提示，被摘要的轮次不再作为原始历史发送。
    let latest = llm.seen_messages().pop().expect("second llm call");
    assert!(
        latest[0].content.contains("此前对话摘要：这是滚动摘要文本"),
        "system prompt: {}",
        latest[0].content
    );
    assert!(latest.iter().any(|message| message.content == "第二问"));
    assert!(!latest.iter().any(|message| message.content == "第一问"));
    assert!(service.summary_job.is_none());

    // 断言三：失败路径（摘要生成失败）不落库，且回答照常。
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Err("LLM_RESPONSE_EMPTY".to_string())).unwrap();
    drop(tx);
    service.summary_job = Some(rx);
    service
        .finalize_utterance(&database, &config, &probes, credentials(), Some("第三问"))
        .unwrap()
        .expect("third turn");
    assert_eq!(
        SessionStore::new(&database).context_summary(&id).unwrap(),
        Some(("这是滚动摘要文本".to_string(), 0))
    );
    assert!(service.summary_job.is_none());
    let latest = llm.seen_messages().pop().expect("third llm call");
    assert!(latest.iter().any(|message| message.content == "第三问"));
    let stored = SessionStore::new(&database).list_turns(&id).unwrap();
    assert_eq!(stored.len(), 3);
    assert_eq!(stored[2].assistant_text, "最新回答");
}

#[test]
fn should_compress_uses_threshold_plus_recent_window() {
    use super::NO_CONTEXT_SUMMARY;
    use super::helpers::should_compress;
    // 无摘要（哨兵 -1）：未摘要轮次须超过 12 + 4 才触发。
    assert!(!should_compress(15, NO_CONTEXT_SUMMARY));
    assert!(should_compress(16, NO_CONTEXT_SUMMARY));
    // 已有摘要（upto=3）：按距上次摘要的新增轮次计数。
    assert!(!should_compress(19, 3));
    assert!(should_compress(20, 3));
    assert!(!should_compress(3, 3));
}

#[test]
fn meeting_mention_tolerates_spaces_and_punctuation() {
    use super::meeting_assistant_was_mentioned;
    // 空格 + 中文标点打断角色名：规范化后仍算点名。
    assert!(meeting_assistant_was_mentioned(
        "会议 助手，你好",
        "会议助手"
    ));
    // 英文问号 + 空格分隔的「AI 助手」也算点名。
    assert!(meeting_assistant_was_mentioned("AI 助手?", "会议助手"));
    assert!(meeting_assistant_was_mentioned(
        "嘿，会议助手帮我记一下",
        "会议助手"
    ));
    // 普通讨论不含点名关键词。
    assert!(!meeting_assistant_was_mentioned(
        "你听得见我说话吗",
        "会议助手"
    ));
    assert!(!meeting_assistant_was_mentioned(
        "这个方案大家怎么看",
        "会议助手"
    ));
}

// —— D 线审查修复的回归测试：finalizing 单飞行守卫的归属与复位 ——

use super::finalize::{BeginFinalize, NetworkOutcome, TextPersist, finalize_network};

#[test]
fn failed_turn_persist_releases_the_finalize_guard() {
    // 落库失败（? 传播）曾把收尾守卫卡在“在飞”，该会话后续收尾全部
    // Idle→STATE_INVALID 直到重开会话（D 线审查 P2）。守卫必须在
    // complete_finalize_text 的失败出口复位。
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let config = ready_public_config();
    service.start(&database, &config, true, false).unwrap();
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("answer");
    let tts = ScriptedTts::ok(&[1, 2, 3]);
    let embed = UnusedEmbed;
    let probes = cascaded_probes(&asr, &llm, &tts, &embed);

    let plan = match service
        .begin_finalize(&database, &config, Some("question"), false)
        .unwrap()
    {
        BeginFinalize::Plan(plan) => plan,
        BeginFinalize::Idle => panic!("first finalize must plan"),
    };
    let outcome = finalize_network(
        &plan,
        &database,
        &probes,
        credentials(),
        &TurnStreamHooks::none(),
    );
    // 确定性写失败：先占住本轮 (session_id, turn_index)，insert_turn 撞
    // UNIQUE(session_id, turn_index) 约束，数据库本身保持健康。
    let (session_id, turn_index) = (plan.session_id.clone(), plan.turn_index);
    database
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO session_turns(id, session_id, turn_index, user_text, assistant_text, materials_used, created_at)
                 VALUES ('conflict-row', ?1, ?2, '', '', 0, '2026-01-01T00:00:00Z')",
                rusqlite::params![session_id, turn_index],
            )
        })
        .unwrap();
    assert!(
        service
            .complete_finalize_text(*plan, outcome, &database, credentials())
            .is_err(),
        "unique conflict must fail the persist"
    );

    // 守卫必须已复位：下一次收尾不得返回 Ok(Idle)——Idle 的唯一来源就是
    // 守卫未释放。（persist 失败遗留的 Thinking 相位会让后续尝试在相位
    // 迁移处报 StateInvalid；那是先于本修复存在的独立行为，不在本卡范围。）
    let second = service.begin_finalize(&database, &config, Some("question"), false);
    assert!(
        !matches!(second, Ok(BeginFinalize::Idle)),
        "guard must be released after a failed persist"
    );
}

#[test]
fn dropped_finalize_keeps_a_newer_finalizes_guard() {
    // 旧收尾的阶段三 Dropped 分支曾无条件清守卫：stop→start 后新收尾 B
    // 在飞时，A 的 Dropped 会清掉 B 的旗标，第三个收尾随即能与 B 并发
    // （D 线审查 T03 §2）。守卫按代次归属：A 不得清 B 的。
    let (_directory, database) = opened();
    let mut service = SessionService::with_sink(RecordingSink::default());
    let config = ready_public_config();
    service.start(&database, &config, true, false).unwrap();

    // A：旧会话的收尾进入阶段二。
    let plan_a = match service
        .begin_finalize(&database, &config, Some("q1"), false)
        .unwrap()
    {
        BeginFinalize::Plan(plan) => plan,
        BeginFinalize::Idle => panic!("first finalize must plan"),
    };
    // 更替：stop → start（reset_runtime 清守卫并升代次），新收尾 B 进场。
    service.stop(&database).unwrap();
    service.start(&database, &config, true, false).unwrap();
    let plan_b = match service
        .begin_finalize(&database, &config, Some("q2"), false)
        .unwrap()
    {
        BeginFinalize::Plan(plan) => plan,
        BeginFinalize::Idle => panic!("second finalize must plan"),
    };
    drop(plan_b);
    // A 的阶段三：session_id 失配 → Dropped；不得影响 B 的守卫。
    let dropped = service
        .complete_finalize_text(*plan_a, NetworkOutcome::Idle, &database, credentials())
        .unwrap();
    assert!(matches!(dropped, TextPersist::Dropped));

    // B 仍在飞行：第三个收尾必须被单飞行守卫挡下（Idle）。
    let third = service
        .begin_finalize(&database, &config, Some("q3"), false)
        .unwrap();
    assert!(
        matches!(third, BeginFinalize::Idle),
        "newer finalize B must still own the guard"
    );
}

#[test]
fn cascade_finalize_writes_stage_timeline_into_turn_meta() {
    let (_directory, database) = opened();
    let config = ready_public_config();
    let mut service = SessionService::with_sink(RecordingSink::default());
    match service.start(&database, &config, true, false).unwrap() {
        SessionStartOutcome::Started { .. } => {}
        SessionStartOutcome::Blocked { issues } => panic!("blocked: {issues:?}"),
    }
    let asr = ScriptedAsr::ok("ignored");
    let llm = ScriptedLlm::ok("时间线回答");
    let tts = ScriptedTts::ok(&[1, 2, 3]);
    let embed = UnusedEmbed;
    let probes = cascaded_probes(&asr, &llm, &tts, &embed);
    service
        .finalize_utterance(
            &database,
            &config,
            &probes,
            credentials(),
            Some("时间线问题"),
        )
        .unwrap();

    let meta = latest_turn_meta(&database, service.session_id().unwrap());
    assert_eq!(meta["latencyMode"], "cascade");
    // 手动文本轮无 ASR 调用，但各阶段锚点都应有数值（宽松上界防负载抖动）。
    for stage in ["asrDoneMs", "retrievalDoneMs", "llmDoneMs", "ttsDoneMs"] {
        let value = meta["timeline"][stage].as_u64().expect(stage);
        assert!(value < 5_000, "{stage}={value}");
    }
    assert!(meta["timeline"]["llmFirstTokenMs"].is_u64());
    // 级联「首响」= TTS 完成时刻，透传给既有按线路首响汇总。
    assert_eq!(meta["latencyMsFirstAudio"], meta["timeline"]["ttsDoneMs"]);
}
