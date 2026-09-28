//! 常驻实时会话执行器：一条 WebSocket 服务整场会话，音频持续上行、
//! 服务端事件流式下行，按 `RealtimeCapabilityProfile` 适配各供应商方言。
//! 取代 `OpenAiCompatibleRealtime::transcribe_turn` 的"每轮一连接"路径。
//!
//! 线程模型：监督线程（重连退避）+ 连接线程（读事件轮询 + 命令写出，单线程
//! 持有 socket，tungstenite 同步 WebSocket 不支持拆分读写半）。编排层经
//! `commands` 下行、`recv_event` 上行。
// 阶段 C 接线 SessionService 后移除：部分 API 仅编排层消费。
#![allow(dead_code)]

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;

use super::{
    ProviderEndpoint, RealtimeError,
    openai_realtime::{
        AUDIO_APPEND_CHUNK_BYTES, CONNECT_OPEN_TIMEOUT, InputTranscriptAssembler,
        MAX_TEXT_FRAME_BYTES, RealtimeDialect, RealtimeDialectName, RealtimeTransport,
        SESSION_UPDATED_TIMEOUT, ServerEvent, TungsteniteSocket, complete_handshake, connect_tcp,
        dialect_input_rate, parse_server_event, realtime_dialect, realtime_url, selected_voice,
        send_text, set_tcp_timeouts,
    },
};

/// 单响应硬超时：response.created 后这么久仍未 response.done 视为连接异常，强制重连。
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// 空闲重连：超过该时长未收到任何服务端事件即主动重建。各供应商服务端超时不同
/// 且未完全文档化，取保守值；live 冒烟实测后按画像覆写。
const IDLE_RECONNECT_AFTER: Duration = Duration::from_secs(120);
const RECV_POLL: Duration = Duration::from_millis(20);
const RECONNECT_BASE: Duration = Duration::from_millis(500);
const RECONNECT_CAP: Duration = Duration::from_secs(30);
const HISTORY_REPLAY_TURNS: usize = 8;

/// 重连退避步进：建过会话（网络闪断）重置为基准，连续未建会话的失败指数递增；
/// 等待时长永不突破 RECONNECT_CAP。纯函数以便单测钉住上限语义。
fn next_reconnect_backoff(backoff: Duration, had_session: bool) -> Duration {
    if had_session {
        RECONNECT_BASE
    } else {
        backoff.saturating_mul(2).min(RECONNECT_CAP)
    }
}

/// 轮次检测能力：决定"谁判说完、谁触发应答"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnDetection {
    /// 服务端 VAD（Tier A）：客户端永不 commit。
    ServerVad,
    /// 语义 VAD（Tier A 变体，实验开关）。
    SemanticVad,
    /// 本地驱动（Tier C）：服务端不判句，本地检测器发 CommitTurn。
    Manual,
}

/// 能力画像：每个方言一份，声明协议能力；Actor 与编排层只面向画像编程。
#[derive(Debug, Clone)]
pub struct RealtimeCapabilityProfile {
    pub dialect: RealtimeDialect,
    pub turn_detection: TurnDetection,
    /// 服务端是否下发 input_audio_buffer.speech_started/stopped。
    pub speech_events: bool,
    /// response.cancel 是否有效；false 时打断走连接复位（Tier B）。
    pub response_cancel: bool,
    /// input_audio_buffer.truncate 是否可用。
    pub truncate: bool,
    /// input_audio_buffer.clear 是否可用（打断/回声丢弃后清服务端输入缓冲）。
    pub clear_input: bool,
    /// conversation.item.create 是否可用于上下文回放与文本轮次；false 由 instructions 承载。
    pub item_replay: bool,
    /// input_image_buffer.append 是否可用（Qwen-Omni 系视频/桌面帧上行）。
    /// DashScope 文档限定 Aliyun 方言；GLM/OpenAI 方言静默丢弃，不报错。
    pub video_input: bool,
    /// 连接空闲重连阈值。
    pub idle_reconnect_after: Duration,
}

impl RealtimeCapabilityProfile {
    /// OpenAI Realtime 兼容画像（DashScope qwen-omni 系 / 智谱 GLM / OpenAI 同协议族）。
    /// speech_events / response_cancel 按文档与 OpenAI 语义默认支持；未核实的
    /// 供应商在 live 冒烟后逐项关闭（自动落 Tier B 行为，不改代码路径）。
    pub fn openai_compatible(dialect: RealtimeDialect) -> Self {
        Self {
            dialect,
            // DashScope WS（Aliyun 方言）默认 Manual：服务端 VAD 与客户端
            // commit 并存会竞态（自动提交后手动 commit 撞空缓冲被拒
            // "buffer too small"）。已核实支持 server_vad 的 Qwen-Omni
            // Realtime 模型由 capability_profile 覆写为 Tier A。
            turn_detection: if dialect.name == RealtimeDialectName::Aliyun {
                TurnDetection::Manual
            } else {
                TurnDetection::ServerVad
            },
            speech_events: true,
            response_cancel: true,
            truncate: true,
            clear_input: true,
            item_replay: true,
            // input_image_buffer.append 仅 DashScope Qwen-Omni 系文档化支持。
            video_input: dialect.name == RealtimeDialectName::Aliyun,
            idle_reconnect_after: IDLE_RECONNECT_AFTER,
        }
    }

    pub fn tier(&self) -> &'static str {
        match self.turn_detection {
            TurnDetection::Manual => "C",
            TurnDetection::ServerVad | TurnDetection::SemanticVad => "A",
        }
    }

    /// 实验开关：强制轮次检测方式（`AI_VOICE_REALTIME_VAD=manual|semantic`）。
    fn with_env_overrides(mut self) -> Self {
        match std::env::var("AI_VOICE_REALTIME_VAD").as_deref() {
            Ok("manual") => self.turn_detection = TurnDetection::Manual,
            Ok("semantic") => self.turn_detection = TurnDetection::SemanticVad,
            _ => {}
        }
        self
    }
}

/// 按供应商 base_url 和模型解析画像（方言判定复用 realtime_dialect）。
pub fn capability_profile(
    endpoint: &ProviderEndpoint,
    model_id: &str,
) -> RealtimeCapabilityProfile {
    let mut profile =
        RealtimeCapabilityProfile::openai_compatible(realtime_dialect(&endpoint.base_url));
    if profile.dialect.name == RealtimeDialectName::Aliyun
        && model_id == "qwen3.8-omni-flash-realtime"
    {
        profile.turn_detection = TurnDetection::ServerVad;
    }
    profile.with_env_overrides()
}

/// 会话静态配置：连接建立与每次重连都会使用。
#[derive(Debug, Clone)]
pub struct RealtimeSessionConfig {
    pub endpoint: ProviderEndpoint,
    pub credential: Option<String>,
    pub model_id: String,
    /// 线路配置音色（克隆音色 ID 或官方音色名）；空串回退方言默认。
    pub voice: String,
    pub instructions: String,
    /// 上下文回放（旧→新），连接建立后按序注入；超过 HISTORY_REPLAY_TURNS 截尾。
    pub history: Vec<(String, String)>,
    /// 麦克风会话 true（服务端判完句自动应答）；会议助手点名门控时 false，
    /// 编排层拿到完整转写后自行发 RespondText/CreateResponse。
    pub auto_respond: bool,
    /// 联网搜索：DashScope session.update 顶层 enable_search（仅
    /// Qwen3.8/Qwen3.5-Omni-Realtime 系支持，OpenAI 方言不发送该字段）。
    pub enable_search: bool,
}

impl RealtimeSessionConfig {
    fn history_window(&self) -> &[(String, String)] {
        let skip = self.history.len().saturating_sub(HISTORY_REPLAY_TURNS);
        &self.history[skip..]
    }
}

/// 编排层 → Actor 命令。
#[derive(Debug)]
pub enum ActorCommand {
    /// 追加麦克风音频（任意长度，Actor 按方言 100ms 帧切片）。输入门控期间丢弃。
    AppendAudio(Vec<u8>),
    /// 追加视频帧（摄像头/桌面共享；base64 JPEG）。约 1fps 上行，
    /// Manual 模式下随下一次 commit 一并提交给模型。无视频能力的画像静默丢弃。
    AppendImage(String),
    /// Tier C：本地检测器判完句后提交（Manual 画像下 commit + 必要时 response.create）。
    CommitTurn,
    /// 文本轮次（工作台输入/再说一遍/会议助手点名后）。
    RespondText(String),
    /// 打断：取消在途响应。画像无 response.cancel 时退化为连接复位。
    CancelResponse,
    /// 打断后校正服务端用户 item 边界（播放中被截断的转写）。
    Truncate {
        audio_end_ms: u64,
    },
    /// 清空服务端输入音频缓冲：打断/回声轮丢弃后清残留，
    /// 防止抢跑的回声尾巴混入下一次提交被转写成用户发言。
    ClearInputBuffer,
    /// 暂停/恢复上行（人工接管/静音）。
    GateInput(bool),
    /// 每轮落库后同步回放历史（供重连重建上下文）。
    SetHistory(Vec<(String, String)>),
    Shutdown,
}

/// Actor → 编排层事件。
#[derive(Debug)]
pub enum ActorEvent {
    /// 连接就绪（每次重连成功也会再发）。
    Connected,
    SpeechStarted,
    SpeechStopped,
    UserTranscriptDelta {
        item_id: String,
        text: String,
    },
    UserTranscriptCompleted {
        item_id: String,
        text: String,
    },
    AssistantAudioDelta(Vec<u8>),
    AssistantTextDelta(String),
    /// 服务端开始生成响应（turn.responding 的依据）。
    ResponseStarted,
    /// 一轮响应结束。cancelled=true 表示被打断丢弃。
    ResponseDone {
        cancelled: bool,
    },
    /// 恢复性错误已触发重连（编排层提示"重连中"）。
    Reconnecting(String),
    /// 终局错误（鉴权失败等不重试）。
    Failed(String),
}

pub struct RealtimeSession {
    commands: mpsc::Sender<ActorCommand>,
    events: Arc<Mutex<mpsc::Receiver<ActorEvent>>>,
    shutdown: Arc<AtomicBool>,
}

impl RealtimeSession {
    /// 启动 Actor（监督线程 + 连接线程），立即返回句柄。
    pub fn start(config: RealtimeSessionConfig) -> Result<Self, RealtimeError> {
        Self::start_with_profile(
            capability_profile(&config.endpoint, &config.model_id),
            config,
        )
    }

    /// 画像由调用方给定的启动入口（生产走 `start` 按端点解析；测试直接注入，
    /// 避免环境变量开关在并行测试间互扰）。
    pub fn start_with_profile(
        profile: RealtimeCapabilityProfile,
        config: RealtimeSessionConfig,
    ) -> Result<Self, RealtimeError> {
        // URL 在调用线程先校验，非法立即失败而非监督线程异步报错。
        realtime_url(&config.endpoint.base_url, &config.model_id)?;
        let (commands_tx, commands_rx) = mpsc::channel::<ActorCommand>();
        let (events_tx, events_rx) = mpsc::channel::<ActorEvent>();
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(SharedState {
            profile,
            config: Mutex::new(config),
            commands: Mutex::new(commands_rx),
            events: events_tx,
            shutdown: Arc::clone(&shutdown),
            gated: AtomicBool::new(false),
            assembler: Mutex::new(InputTranscriptAssembler::new()),
            pending_image: Mutex::new(None),
        });
        std::thread::Builder::new()
            .name("realtime-session".into())
            .spawn(move || supervise(state))
            .map_err(|_| RealtimeError::ConnectFailed)?;
        Ok(Self {
            commands: commands_tx,
            events: Arc::new(Mutex::new(events_rx)),
            shutdown,
        })
    }

    pub fn send(&self, command: ActorCommand) {
        let _ = self.commands.send(command);
    }

    /// 编排层轮询事件；timeout 内无事件返回 None。
    pub fn recv_event(&self, timeout: Duration) -> Option<ActorEvent> {
        self.events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .recv_timeout(timeout)
            .ok()
    }
}

impl Drop for RealtimeSession {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

struct SharedState {
    profile: RealtimeCapabilityProfile,
    config: Mutex<RealtimeSessionConfig>,
    commands: Mutex<mpsc::Receiver<ActorCommand>>,
    events: mpsc::Sender<ActorEvent>,
    shutdown: Arc<AtomicBool>,
    gated: AtomicBool,
    assembler: Mutex<InputTranscriptAssembler>,
    /// 最新待发图片；跨重连保留，但发送前必须等当前连接音频先行。
    pending_image: Mutex<Option<String>>,
}

impl SharedState {
    fn config_snapshot(&self) -> RealtimeSessionConfig {
        self.config
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn try_command(&self) -> Option<ActorCommand> {
        self.commands
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .try_recv()
            .ok()
    }
}

enum ConnectionOutcome {
    Shutdown,
    /// 参数：是否成功建立过会话（用于退避复位）+ 原因。
    Reconnect {
        had_session: bool,
        reason: String,
    },
    Terminal(String),
}

fn supervise(state: Arc<SharedState>) {
    let mut backoff = RECONNECT_BASE;
    let mut connections: u64 = 0;
    loop {
        if state.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let config = state.config_snapshot();
        connections += 1;
        eprintln!("[realtime] connection #{connections} opening");
        match run_connection(&state, &config) {
            ConnectionOutcome::Shutdown => {
                eprintln!("[realtime] shutdown after {connections} connection(s)");
                return;
            }
            ConnectionOutcome::Terminal(reason) => {
                eprintln!("[realtime] TERMINAL: {reason}");
                let _ = state.events.send(ActorEvent::Failed(reason));
                return;
            }
            ConnectionOutcome::Reconnect {
                had_session,
                reason,
            } => {
                eprintln!(
                    "[realtime] reconnecting (had_session={had_session}, backoff={}ms): {reason}",
                    backoff.min(RECONNECT_CAP).as_millis()
                );
                let _ = state.events.send(ActorEvent::Reconnecting(reason));
                backoff = next_reconnect_backoff(backoff, had_session);
            }
        }
        // 指数退避等待；Shutdown 立即返回。连续未建会话的失败退避递增，
        // 建过会话（网络闪断）则快速重试。
        let wake = Instant::now() + backoff.min(RECONNECT_CAP);
        while Instant::now() < wake {
            if state.shutdown.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// 连接主体：握手 → session.update → 上下文回放 → 事件/命令轮询循环。
fn run_connection(state: &SharedState, config: &RealtimeSessionConfig) -> ConnectionOutcome {
    let url = match realtime_url(&config.endpoint.base_url, &config.model_id) {
        Ok(url) => url,
        Err(error) => return ConnectionOutcome::Terminal(error.code().to_owned()),
    };
    let mut socket = match connect_socket(&url, config, state) {
        Ok(socket) => socket,
        Err(error) => return classify_connect_error(error),
    };

    // 读用短超时轮询，命令在无服务端帧的间隙写出；单线程持有 socket。
    let mut idle_at = Instant::now() + state.profile.idle_reconnect_after;
    let mut response_opened_at: Option<Instant> = None;
    // Qwen Realtime 图片输入要求当前连接至少 append 过一次音频。桌面共享常在
    // 说话前开启，重连后若图片先到会触发远端错误并形成重连循环；这里只保留
    // 最新帧，等首块音频成功上行后按“音频 → 图片”顺序发送。
    let mut audio_appended = false;
    loop {
        if state.shutdown.load(Ordering::SeqCst) {
            return ConnectionOutcome::Shutdown;
        }
        match socket.recv_text(RECV_POLL) {
            Ok(raw) => {
                idle_at = Instant::now() + state.profile.idle_reconnect_after;
                if let Err(error) = handle_server_event(state, &raw, &mut response_opened_at) {
                    return ConnectionOutcome::Reconnect {
                        had_session: true,
                        reason: error.code().to_owned(),
                    };
                }
            }
            Err(RealtimeError::Timeout) => {}
            Err(error) => {
                return ConnectionOutcome::Reconnect {
                    had_session: true,
                    reason: error.code().to_owned(),
                };
            }
        }
        while let Some(command) = state.try_command() {
            match command {
                ActorCommand::Shutdown => return ConnectionOutcome::Shutdown,
                ActorCommand::AppendAudio(pcm) => {
                    if state.gated.load(Ordering::SeqCst) {
                        continue;
                    }
                    if let Err(error) = append_audio(&state.profile, &mut socket, &pcm) {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                    if !pcm.is_empty() {
                        audio_appended = true;
                    }
                    if audio_appended {
                        let image = state
                            .pending_image
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .clone();
                        if let Some(image) = image {
                            if let Err(error) = append_image(&mut socket, &image) {
                                return ConnectionOutcome::Reconnect {
                                    had_session: true,
                                    reason: error.code().to_owned(),
                                };
                            }
                            *state
                                .pending_image
                                .lock()
                                .unwrap_or_else(|p| p.into_inner()) = None;
                        }
                    }
                }
                ActorCommand::AppendImage(jpeg_b64) => {
                    if !state.profile.video_input {
                        continue;
                    }
                    if audio_appended {
                        if let Err(error) = append_image(&mut socket, &jpeg_b64) {
                            return ConnectionOutcome::Reconnect {
                                had_session: true,
                                reason: error.code().to_owned(),
                            };
                        }
                        *state
                            .pending_image
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = None;
                    } else {
                        *state
                            .pending_image
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = Some(jpeg_b64);
                    }
                }
                ActorCommand::CommitTurn => {
                    if let Err(error) = commit_turn(state, &mut socket, config.auto_respond) {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::RespondText(text) => {
                    if let Err(error) = respond_text(state, &mut socket, &text) {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::CancelResponse => {
                    if let Err(error) = cancel_response(&state.profile, &mut socket) {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::Truncate { audio_end_ms } => {
                    if state.profile.truncate
                        && let Err(error) = send_text(
                            &mut socket,
                            &json!({
                                "type": "input_audio_buffer.truncate",
                                "audio_end_ms": audio_end_ms,
                            })
                            .to_string(),
                        )
                    {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::ClearInputBuffer => {
                    if state.profile.clear_input
                        && let Err(error) =
                            send_text(&mut socket, r#"{"type":"input_audio_buffer.clear"}"#)
                    {
                        return ConnectionOutcome::Reconnect {
                            had_session: true,
                            reason: error.code().to_owned(),
                        };
                    }
                }
                ActorCommand::GateInput(enabled) => {
                    state.gated.store(enabled, Ordering::SeqCst);
                }
                ActorCommand::SetHistory(history) => {
                    state
                        .config
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .history = history;
                }
            }
        }
        if Instant::now() > idle_at {
            return ConnectionOutcome::Reconnect {
                had_session: true,
                reason: "REALTIME_IDLE_RECONNECT".to_owned(),
            };
        }
        if let Some(opened_at) = response_opened_at
            && opened_at.elapsed() > RESPONSE_TIMEOUT
        {
            return ConnectionOutcome::Reconnect {
                had_session: true,
                reason: "REALTIME_RESPONSE_TIMEOUT".to_owned(),
            };
        }
    }
}

fn classify_connect_error(error: RealtimeError) -> ConnectionOutcome {
    match error {
        // 鉴权失败重连无意义，直接终局。
        RealtimeError::Unauthorized => ConnectionOutcome::Terminal("REALTIME_UNAUTHORIZED".into()),
        other => ConnectionOutcome::Reconnect {
            had_session: false,
            reason: other.code().to_owned(),
        },
    }
}

fn connect_socket(
    url: &reqwest::Url,
    config: &RealtimeSessionConfig,
    state: &SharedState,
) -> Result<TungsteniteSocket, RealtimeError> {
    let stream = connect_tcp(url, &state.shutdown)?;
    let _ = stream.set_nodelay(true);
    // 握手与 session.update 等待都要有上限，否则服务端卡住时重连会无限挂起。
    set_tcp_timeouts(&stream, CONNECT_OPEN_TIMEOUT)?;
    let mut builder = tungstenite::ClientRequestBuilder::new(
        url.as_str()
            .parse()
            .map_err(|_| RealtimeError::UrlInvalid)?,
    )
    .with_header("OpenAI-Beta", "realtime=v1");
    if let Some(credential) = config
        .credential
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        builder = builder.with_header("Authorization", format!("Bearer {credential}"));
    }
    let request = builder
        .into_client_request()
        .map_err(|_| RealtimeError::UrlInvalid)?;
    let ws_config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_TEXT_FRAME_BYTES))
        .max_frame_size(Some(MAX_TEXT_FRAME_BYTES));
    let (socket, _) = complete_handshake(tungstenite::client_tls_with_config(
        request,
        stream,
        Some(ws_config),
        None,
    ))?;
    let mut socket = TungsteniteSocket { socket };
    session_setup(state, config, &mut socket)?;
    Ok(socket)
}

/// session.update（画像驱动）→ 等 session.updated → 上下文回放 → Connected。
fn session_setup(
    state: &SharedState,
    config: &RealtimeSessionConfig,
    socket: &mut TungsteniteSocket,
) -> Result<(), RealtimeError> {
    // 每次重连都是新连接：转写装配状态不复用。
    *state.assembler.lock().unwrap_or_else(|p| p.into_inner()) = InputTranscriptAssembler::new();
    let voice = selected_voice(&config.voice, &state.profile.dialect);
    let mut session = json!({
        "modalities": ["text", "audio"],
        "instructions": config.instructions,
        "input_audio_format": state.profile.dialect.audio_format,
        "output_audio_format": state.profile.dialect.output_audio_format,
    });
    session["turn_detection"] = match state.profile.turn_detection {
        TurnDetection::ServerVad => json!({
            "type": "server_vad",
            "threshold": 0.5,
            "prefix_padding_ms": 300,
            "silence_duration_ms": 500,
            // 会议助手点名门控：服务端只提交转写，应答由编排层决定。
            "create_response": config.auto_respond,
        }),
        TurnDetection::SemanticVad => json!({
            "type": "semantic_vad",
            "create_response": config.auto_respond,
        }),
        // 此前 Aliyun Manual 竞态的根源是"服务端自动提交 × 客户端 commit 并存"；
        // Manual 画像下提交权完全归客户端，不再竞态。
        TurnDetection::Manual => Value::Null,
    };
    if !voice.is_empty() {
        session["voice"] = json!(voice);
    }
    // DashScope 联网：session.update 顶层开关（Qwen3.8/Qwen3.5-Omni-Realtime
    // 支持，模型自主判断何时搜索）。OpenAI 方言无此字段，发送会被服务端拒绝。
    if config.enable_search && state.profile.dialect.name == RealtimeDialectName::Aliyun {
        session["enable_search"] = json!(true);
        session["search_options"] = json!({ "enable_source": true });
    }
    send_text(
        socket,
        &json!({"type": "session.update", "session": session}).to_string(),
    )?;
    let deadline = Instant::now() + SESSION_UPDATED_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RealtimeError::SessionUpdateTimeout);
        }
        let raw = socket.recv_text(remaining)?;
        match parse_server_event(&raw)? {
            Some(ServerEvent::SessionUpdated) => break,
            Some(ServerEvent::SessionCreated) | None => {}
            Some(_) => return Err(RealtimeError::SessionUpdateUnexpected),
        }
    }
    replay_history(state, config, socket)?;
    // 重连重置了服务端会话：积压的提交/应答命令属于旧连接的上下文，重放会
    // 对同一句话二次 commit+response.create（重复播报）。只保留音频追加——
    // 那是断连期间用户正在说的话，落进缓冲随下一次 commit 提交。
    while let Some(command) = state.try_command() {
        if matches!(
            command,
            ActorCommand::CommitTurn
                | ActorCommand::RespondText(_)
                | ActorCommand::CancelResponse
                | ActorCommand::Truncate { .. }
                | ActorCommand::ClearInputBuffer
        ) {
            eprintln!("[realtime] dropped stale command after reconnect");
        }
    }
    let _ = state.events.send(ActorEvent::Connected);
    Ok(())
}

/// 上下文回放：旧→新逐轮 conversation.item.create；画像不支持 item_replay 时
/// 上下文由 instructions 承载（调用方拼摘要），此处跳过。
fn replay_history(
    state: &SharedState,
    config: &RealtimeSessionConfig,
    socket: &mut TungsteniteSocket,
) -> Result<(), RealtimeError> {
    if !state.profile.item_replay {
        return Ok(());
    }
    for (user, assistant) in config.history_window() {
        if !user.is_empty() {
            send_text(
                socket,
                &json!({
                    "type": "conversation.item.create",
                    "item": {
                        "type": "message",
                        "role": "user",
                        "content": [{ "type": "input_text", "text": user }],
                    },
                })
                .to_string(),
            )?;
        }
        if !assistant.is_empty() {
            send_text(
                socket,
                &json!({
                    "type": "conversation.item.create",
                    "item": {
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": assistant }],
                    },
                })
                .to_string(),
            )?;
        }
    }
    Ok(())
}

/// 按方言 100ms 帧切片 append（DashScope 单帧限制安全值，其余方言等时长换算）。
/// 采集环是 48kHz，方言输入率不同（DashScope 16k / OpenAI 24k）时必须先降采样：
/// 服务端按 session.update 声明的格式解码，原样发送会得到变调音频、转写失效。
fn append_audio(
    profile: &RealtimeCapabilityProfile,
    socket: &mut TungsteniteSocket,
    pcm: &[u8],
) -> Result<(), RealtimeError> {
    let rate = dialect_input_rate(&profile.dialect);
    let pcm =
        crate::audio::pcm::resample_pcm16_mono(pcm, crate::audio::pcm::CAPTURE_SAMPLE_RATE, rate);
    let chunk_bytes = (rate as usize * 2 / 10).max(AUDIO_APPEND_CHUNK_BYTES / 100);
    for chunk in pcm.chunks(chunk_bytes.max(1)) {
        send_text(
            socket,
            &json!({
                "type": "input_audio_buffer.append",
                "audio": STANDARD.encode(chunk),
            })
            .to_string(),
        )?;
    }
    Ok(())
}

/// 上行一帧 base64 JPEG。调用方必须保证当前连接已发送过音频 append。
fn append_image(socket: &mut TungsteniteSocket, jpeg_b64: &str) -> Result<(), RealtimeError> {
    send_text(
        socket,
        &json!({
            "type": "input_image_buffer.append",
            "image": jpeg_b64,
        })
        .to_string(),
    )
}

/// Manual（Tier C）画像：commit 后由客户端触发应答。服务端 VAD 画像下客户端
/// 永不 commit（避免撞空缓冲竞态），此命令为 no-op。
fn commit_turn(
    state: &SharedState,
    socket: &mut TungsteniteSocket,
    auto_respond: bool,
) -> Result<(), RealtimeError> {
    if state.profile.turn_detection != TurnDetection::Manual {
        return Ok(());
    }
    send_text(socket, r#"{"type":"input_audio_buffer.commit"}"#)?;
    if auto_respond {
        send_text(socket, &response_create_event(true))?;
    }
    Ok(())
}

fn respond_text(
    state: &SharedState,
    socket: &mut TungsteniteSocket,
    text: &str,
) -> Result<(), RealtimeError> {
    if state.profile.item_replay {
        send_text(
            socket,
            &json!({
                "type": "conversation.item.create",
                "item": {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": text }],
                },
            })
            .to_string(),
        )?;
    }
    send_text(socket, &response_create_event(true))?;
    Ok(())
}

/// 打断：画像支持 response.cancel 直接取消；否则连接复位丢弃在途响应
/// （监督线程按"建过会话"快速重连）。
fn cancel_response(
    profile: &RealtimeCapabilityProfile,
    socket: &mut TungsteniteSocket,
) -> Result<(), RealtimeError> {
    if profile.response_cancel {
        send_text(socket, r#"{"type":"response.cancel"}"#)
    } else {
        Err(RealtimeError::ConnectionClosed)
    }
}

fn response_create_event(include_audio: bool) -> String {
    let modalities = if include_audio {
        json!(["text", "audio"])
    } else {
        json!(["text"])
    };
    json!({
        "type": "response.create",
        "response": { "modalities": modalities },
    })
    .to_string()
}

/// 服务端事件 → 编排层事件；返回 Err 表示连接必须重建。
fn handle_server_event(
    state: &SharedState,
    raw: &str,
    response_opened_at: &mut Option<Instant>,
) -> Result<(), RealtimeError> {
    match parse_server_event(raw)? {
        Some(ServerEvent::Audio(pcm)) => {
            let _ = state.events.send(ActorEvent::AssistantAudioDelta(pcm));
        }
        Some(ServerEvent::OutputTranscript(delta) | ServerEvent::OutputText(delta)) => {
            let _ = state.events.send(ActorEvent::AssistantTextDelta(delta));
        }
        Some(ServerEvent::InputTranscriptDelta(payload)) => {
            let item_id = payload["item_id"].as_str().unwrap_or("").to_owned();
            if let Some((_, text, _)) = assembler_update(state, "input_transcript_delta", &payload)
            {
                let _ = state
                    .events
                    .send(ActorEvent::UserTranscriptDelta { item_id, text });
            }
        }
        Some(ServerEvent::InputTranscriptCompleted(payload)) => {
            let item_id = payload["item_id"].as_str().unwrap_or("").to_owned();
            if let Some((_, text, _)) =
                assembler_update(state, "input_transcript_completed", &payload)
            {
                let _ = state
                    .events
                    .send(ActorEvent::UserTranscriptCompleted { item_id, text });
            }
        }
        Some(ServerEvent::SpeechStarted) => {
            if state.profile.speech_events {
                let _ = state.events.send(ActorEvent::SpeechStarted);
            }
        }
        Some(ServerEvent::SpeechStopped) => {
            if state.profile.speech_events {
                let _ = state.events.send(ActorEvent::SpeechStopped);
            }
        }
        Some(ServerEvent::ResponseDone(status)) => {
            *response_opened_at = None;
            let _ = state.events.send(ActorEvent::ResponseDone {
                cancelled: status.as_deref() == Some("cancelled"),
            });
        }
        Some(ServerEvent::ResponseCreated) => {
            *response_opened_at = Some(Instant::now());
            let _ = state.events.send(ActorEvent::ResponseStarted);
        }
        Some(
            ServerEvent::SessionCreated | ServerEvent::SessionUpdated | ServerEvent::InputCommitted,
        )
        | None => {}
    }
    Ok(())
}

fn assembler_update(
    state: &SharedState,
    kind: &str,
    payload: &Value,
) -> Option<(String, String, bool)> {
    let mut assembler = state.assembler.lock().unwrap_or_else(|p| p.into_inner());
    assembler.update(kind, payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};
    use tungstenite::{Message, WebSocket};

    type MockSocket = WebSocket<TcpStream>;

    /// 接受一条连接：读 session.update → 回 created/updated；返回 update 载荷与 socket。
    fn accept_session(listener: &TcpListener) -> (Value, MockSocket) {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let update: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(update["type"], "session.update");
        ws.send(Message::Text(r#"{"type":"session.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(r#"{"type":"session.updated"}"#.into()))
            .unwrap();
        (update, ws)
    }

    fn read_frame(ws: &mut MockSocket) -> Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "mock server timed out waiting for frame"
            );
            ws.get_mut().set_read_timeout(Some(remaining)).unwrap();
            match ws.read().unwrap() {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                Message::Binary(_) => continue,
                Message::Close(_) => panic!("unexpected close"),
            }
        }
    }

    fn spawn_actor(
        port: u16,
        auto_respond: bool,
        history: Vec<(String, String)>,
    ) -> RealtimeSession {
        RealtimeSession::start(RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: "test".into(),
                base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
            },
            credential: None,
            model_id: "qwen3.8-omni-flash-realtime".into(),
            voice: String::new(),
            instructions: "测试指令".into(),
            history,
            auto_respond,
            enable_search: false,
        })
        .unwrap()
    }

    /// 联网搜索：enable_search=true 时 Aliyan 方言 session.update 携带顶层
    /// 开关与来源选项；OpenAI 方言（无此字段）即使开启也不发送。
    #[test]
    fn enable_search_injected_only_for_aliyun_dialect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = RealtimeSession::start(RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: "test".into(),
                base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
            },
            credential: None,
            model_id: "qwen3.8-omni-flash-realtime".into(),
            voice: String::new(),
            instructions: "测试指令".into(),
            history: vec![],
            auto_respond: true,
            enable_search: true,
        })
        .unwrap();
        let (update, _ws) = accept_session(&listener);
        assert_eq!(update["session"]["enable_search"], true);
        assert_eq!(update["session"]["search_options"]["enable_source"], true);
        drop(actor);
    }

    #[test]
    fn enable_search_omitted_for_openai_dialect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = RealtimeSession::start(RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: "test".into(),
                // 非阿里云 base_url → OpenAI 方言。
                base_url: format!("http://127.0.0.1:{port}/v1/realtime"),
            },
            credential: None,
            model_id: "gpt-realtime".into(),
            voice: String::new(),
            instructions: "测试指令".into(),
            history: vec![],
            auto_respond: true,
            enable_search: true,
        })
        .unwrap();
        let (update, _ws) = accept_session(&listener);
        assert!(update["session"].get("enable_search").is_none());
        drop(actor);
    }

    /// Tier C 画像直接注入（不走环境变量开关，避免并行测试互扰）。
    fn spawn_manual_actor(port: u16, auto_respond: bool) -> RealtimeSession {
        let mut profile = RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
            "http://127.0.0.1/api-ws/v1/realtime",
        ));
        profile.turn_detection = TurnDetection::Manual;
        RealtimeSession::start_with_profile(
            profile,
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试指令".into(),
                history: vec![],
                auto_respond,
                enable_search: false,
            },
        )
        .unwrap()
    }

    fn wait_connected(actor: &RealtimeSession) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "Connected event not observed"
            );
            if matches!(
                actor.recv_event(Duration::from_millis(100)),
                Some(ActorEvent::Connected)
            ) {
                return;
            }
        }
    }

    fn assert_no_frame(ws: &mut MockSocket, window: Duration) {
        let deadline = std::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return;
            }
            ws.get_mut().set_read_timeout(Some(remaining)).unwrap();
            match ws.read() {
                Ok(Message::Text(text)) => panic!("unexpected frame: {text}"),
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Binary(_)) => continue,
                Ok(Message::Close(_)) => panic!("unexpected close"),
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => panic!("read error: {error}"),
            }
        }
    }

    #[test]
    fn connects_with_profile_session_update_replays_history_and_streams_audio() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, true, vec![("你好".into(), "你好呀".into())]);
        let (update, mut ws) = accept_session(&listener);

        // 画像：Qwen-Omni Realtime 使用官方 server_vad；服务端判完句并触发响应。
        assert_eq!(update["session"]["turn_detection"]["type"], "server_vad");
        assert_eq!(update["session"]["turn_detection"]["threshold"], 0.5);
        assert_eq!(
            update["session"]["turn_detection"]["prefix_padding_ms"],
            300
        );
        assert_eq!(
            update["session"]["turn_detection"]["silence_duration_ms"],
            500
        );
        assert_eq!(update["session"]["turn_detection"]["create_response"], true);
        assert_eq!(update["session"]["input_audio_format"], "pcm");
        // 上下文回放：旧→新 user/assistant 各一条。
        let item1 = read_frame(&mut ws);
        assert_eq!(item1["type"], "conversation.item.create");
        assert_eq!(item1["item"]["role"], "user");
        assert_eq!(item1["item"]["content"][0]["text"], "你好");
        let item2 = read_frame(&mut ws);
        assert_eq!(item2["item"]["role"], "assistant");
        assert_eq!(item2["item"]["content"][0]["type"], "output_text");
        wait_connected(&actor);

        // 采集环 48kHz：300ms = 28800B 48k → 降采样到方言 16k = 9600B → 3 个 3200B 帧。
        actor.send(ActorCommand::AppendAudio(vec![0x10; 28_800]));
        let mut decoded_total = 0usize;
        for _ in 0..3 {
            let frame = read_frame(&mut ws);
            assert_eq!(frame["type"], "input_audio_buffer.append");
            let decoded = STANDARD.decode(frame["audio"].as_str().unwrap()).unwrap();
            assert_eq!(decoded.len(), AUDIO_APPEND_CHUNK_BYTES);
            decoded_total += decoded.len();
        }
        assert_eq!(
            decoded_total, 9600,
            "48k input must downsample to dialect 16k rate"
        );
    }

    /// Qwen-Omni Realtime 的 server_vad 需要跟随点名门控：会议桥接不自动应答。
    #[test]
    fn qwen_omni_server_vad_create_response_follows_auto_respond() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, false, vec![]);
        let (update, _ws) = accept_session(&listener);
        assert_eq!(update["session"]["turn_detection"]["type"], "server_vad");
        assert_eq!(
            update["session"]["turn_detection"]["create_response"],
            false
        );
        drop(actor);
    }

    /// 服务端 VAD 负责提交和触发响应：本地 CommitTurn 必须是 no-op。
    #[test]
    fn qwen_omni_server_vad_ignores_local_commit_turn() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, true, vec![]);
        let (_, mut ws) = accept_session(&listener);
        wait_connected(&actor);

        actor.send(ActorCommand::AppendAudio(vec![0x30; 9600]));
        actor.send(ActorCommand::CommitTurn);
        actor.send(ActorCommand::RespondText("哨兵".into()));
        let append = read_frame(&mut ws);
        assert_eq!(append["type"], "input_audio_buffer.append");
        let sentinel = read_frame(&mut ws);
        assert_eq!(sentinel["type"], "conversation.item.create");
        assert_eq!(sentinel["item"]["content"][0]["text"], "哨兵");
        drop(actor);
    }

    /// 未核实 server_vad 能力的 DashScope 模型继续走 Manual 兜底。
    #[test]
    fn legacy_dashscope_model_defaults_to_manual_commit_and_create() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = RealtimeSession::start(RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: "test".into(),
                base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
            },
            credential: None,
            model_id: "qwen-audio-3.0-realtime-plus".into(),
            voice: String::new(),
            instructions: "测试指令".into(),
            history: vec![],
            auto_respond: true,
            enable_search: false,
        })
        .unwrap();
        let (update, mut ws) = accept_session(&listener);
        assert_eq!(update["session"]["turn_detection"], Value::Null);
        wait_connected(&actor);
        actor.send(ActorCommand::CommitTurn);
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.commit");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
        drop(actor);
    }

    /// 视频帧：Aliyun 方言满足“音频先行”后原样透传 input_image_buffer.append；
    /// OpenAI 方言无此能力，静默丢弃且不断链。
    #[test]
    fn append_image_forwarded_for_aliyun_and_dropped_for_openai() {
        // Aliyun 方言：帧透传。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, true, vec![]);
        let (_, mut ws) = accept_session(&listener);
        wait_connected(&actor);
        let jpeg = STANDARD.encode([0xAA, 0xBB, 0xCC]);
        actor.send(ActorCommand::AppendAudio(vec![0x30; 9600]));
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.append");
        actor.send(ActorCommand::AppendImage(jpeg.clone()));
        let frame = read_frame(&mut ws);
        assert_eq!(frame["type"], "input_image_buffer.append");
        assert_eq!(frame["image"], Value::String(jpeg));
        drop(actor);

        // OpenAI 方言：无视频能力，帧被静默丢弃（300ms 窗口内无任何帧到达）。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = RealtimeSession::start(RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: "test".into(),
                base_url: format!("http://127.0.0.1:{port}/v1/realtime"),
            },
            credential: None,
            model_id: "gpt-realtime".into(),
            voice: String::new(),
            instructions: "测试指令".into(),
            history: vec![],
            auto_respond: true,
            enable_search: false,
        })
        .unwrap();
        let (_, mut ws) = accept_session(&listener);
        wait_connected(&actor);
        actor.send(ActorCommand::AppendImage("aGk=".into()));
        ws.get_mut()
            .set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        match ws.read() {
            // 同步 tungstenite 的读超时表现为 Io(WouldBlock/TimedOut)：无帧到达。
            Err(tungstenite::Error::Io(io_error))
                if matches!(
                    io_error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            other => panic!("openai dialect must drop video frames, got {other:?}"),
        }
        drop(actor);
    }

    /// 桌面共享可在说话前开启：新连接后的图片先缓存；重连后门控重置，
    /// 仍等下一块音频先行，并且只保留最新一帧。
    #[test]
    fn image_waits_for_audio_across_reconnect_and_sends_latest_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap();
        let port = port.port();
        let actor = spawn_actor(port, true, vec![]);
        let (_, mut ws1) = accept_session(&listener);
        wait_connected(&actor);

        actor.send(ActorCommand::AppendImage("first".into()));
        actor.send(ActorCommand::AppendImage("latest".into()));
        assert_no_frame(&mut ws1, Duration::from_millis(300));

        ws1.send(Message::Close(None)).unwrap();
        ws1.get_mut().shutdown(std::net::Shutdown::Both).ok();
        let (_, mut ws2) = accept_session(&listener);
        wait_connected(&actor);

        actor.send(ActorCommand::AppendImage("after-reconnect".into()));
        assert_no_frame(&mut ws2, Duration::from_millis(300));
        actor.send(ActorCommand::AppendAudio(vec![0x30; 9600]));
        assert_eq!(read_frame(&mut ws2)["type"], "input_audio_buffer.append");
        let image = read_frame(&mut ws2);
        assert_eq!(image["type"], "input_image_buffer.append");
        assert_eq!(image["image"], "after-reconnect");
        drop(actor);
    }

    #[test]
    fn server_events_are_forwarded_to_orchestrator() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, true, vec![]);
        let (_, mut ws) = accept_session(&listener);
        wait_connected(&actor);

        for event in [
            r#"{"type":"input_audio_buffer.speech_started"}"#.to_string(),
            r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"i1","text":"十一"}"#.to_string(),
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"十一点嘛"}"#.to_string(),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([7u8, 8, 9])
            ),
            r#"{"type":"response.audio_transcript.delta","delta":"十一点了"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }

        let mut seen: Vec<ActorEvent> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while seen.len() < 6 && std::time::Instant::now() < deadline {
            if let Some(event) = actor.recv_event(Duration::from_millis(200)) {
                seen.push(event);
            }
        }
        assert_eq!(seen.len(), 6, "expected all six events, got {seen:?}");
        assert!(matches!(seen.remove(0), ActorEvent::SpeechStarted));
        assert!(matches!(
            seen.remove(0),
            ActorEvent::UserTranscriptDelta { ref text, .. } if text == "十一"
        ));
        assert!(matches!(
            seen.remove(0),
            ActorEvent::UserTranscriptCompleted { ref text, .. } if text == "十一点嘛"
        ));
        assert!(matches!(
            seen.remove(0),
            ActorEvent::AssistantAudioDelta(ref pcm) if pcm == &[7u8, 8, 9]
        ));
        assert!(matches!(
            seen.remove(0),
            ActorEvent::AssistantTextDelta(ref text) if text == "十一点了"
        ));
        assert!(matches!(
            seen.remove(0),
            ActorEvent::ResponseDone { cancelled: false }
        ));
    }

    #[test]
    fn cancel_sends_response_cancel_and_done_reports_cancelled() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, true, vec![]);
        let (_, mut ws) = accept_session(&listener);
        wait_connected(&actor);

        actor.send(ActorCommand::CancelResponse);
        assert_eq!(read_frame(&mut ws)["type"], "response.cancel");

        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"cancelled"}}"#.into(),
        ))
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            assert!(std::time::Instant::now() < deadline, "no ResponseDone");
            match actor.recv_event(Duration::from_millis(200)) {
                Some(ActorEvent::ResponseDone { cancelled }) => {
                    assert!(cancelled);
                    return;
                }
                Some(_) | None => continue,
            }
        }
    }

    #[test]
    fn gated_input_drops_audio_until_reenabled() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, true, vec![]);
        let (_, mut ws) = accept_session(&listener);
        wait_connected(&actor);

        actor.send(ActorCommand::GateInput(true));
        actor.send(ActorCommand::AppendAudio(vec![0x20; 3200]));
        // 门控期间的音频必须被丢弃：下一个到达服务端的帧是文本轮次（哨兵）。
        actor.send(ActorCommand::GateInput(false));
        actor.send(ActorCommand::RespondText("接管一句".into()));
        let sentinel = read_frame(&mut ws);
        assert_eq!(sentinel["type"], "conversation.item.create");
        assert_eq!(sentinel["item"]["content"][0]["text"], "接管一句");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
    }

    #[test]
    fn manual_profile_disables_server_vad_and_commit_creates_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_manual_actor(port, true);
        let (update, mut ws) = accept_session(&listener);
        assert!(update["session"]["turn_detection"].is_null());
        wait_connected(&actor);

        // Tier C：客户端 commit + response.create（auto_respond=true）。
        // 9600B@48k → 降采样 3200B@16k，恰一个 100ms append 帧。
        actor.send(ActorCommand::AppendAudio(vec![0x30; 9600]));
        actor.send(ActorCommand::CommitTurn);
        let append = read_frame(&mut ws);
        assert_eq!(append["type"], "input_audio_buffer.append");
        assert_eq!(
            STANDARD
                .decode(append["audio"].as_str().unwrap())
                .unwrap()
                .len(),
            3200
        );
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.commit");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
    }

    #[test]
    fn manual_profile_without_auto_respond_commits_without_create() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_manual_actor(port, false);
        let (_, mut ws) = accept_session(&listener);
        wait_connected(&actor);

        actor.send(ActorCommand::CommitTurn);
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.commit");
        // 无 response.create：编排层按点名门控自行触发。
    }

    #[test]
    fn reconnects_after_server_close_and_replays_context_again() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, true, vec![("上轮".into(), "上答".into())]);
        let (_, mut ws1) = accept_session(&listener);
        wait_connected(&actor);
        ws1.send(Message::Close(None)).unwrap();
        ws1.get_mut().shutdown(std::net::Shutdown::Both).ok();

        // 第二次连接：上下文再次回放（服务端会话无历史）。
        let (update2, mut ws2) = accept_session(&listener);
        assert_eq!(update2["session"]["turn_detection"]["type"], "server_vad");
        let replayed = read_frame(&mut ws2);
        assert_eq!(replayed["type"], "conversation.item.create");
        assert_eq!(replayed["item"]["content"][0]["text"], "上轮");
        wait_connected(&actor);
    }

    /// DashScope 流式首响 live 冒烟（Tier A 全链路）：常驻连接 + 持续上行真实语音 +
    /// server_vad 自动提交 + 首个 response.audio.delta 时间戳。
    /// 运行：REALTIME_SMOKE_CONFIG=<config> REALTIME_SMOKE_ROUTE=<route id>
    /// REALTIME_SMOKE_WAV=<16k mono wav（必须是人声，静音不会触发服务端 VAD）>
    /// cargo test --lib live_streaming_first_audio_latency -- --ignored --nocapture
    #[test]
    #[ignore = "Uses the saved Windows credential and consumes provider quota; set REALTIME_SMOKE_CONFIG and REALTIME_SMOKE_ROUTE explicitly"]
    #[cfg(windows)]
    fn live_streaming_first_audio_latency() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;

        let path = std::env::var("REALTIME_SMOKE_CONFIG").expect("explicit config path required");
        let config: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let route_id = std::env::var("REALTIME_SMOKE_ROUTE").expect("route id required");
        let route = config["speech"]["voiceRoutes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"].as_str() == Some(route_id.as_str()))
            .expect("selected route must exist")
            .clone();
        let provider = config["models"]["providers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == route["e2eProviderId"])
            .unwrap()
            .clone();
        let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
        let credential = secrets
            .read(provider["credential"]["reference"].as_str().unwrap())
            .unwrap()
            .expect("saved credential must exist");
        let voice = route["voiceId"].as_str().unwrap_or("").to_owned();

        // 读取 wav data 块作为 16k mono PCM（必须是人声样本）。
        let wav_path = std::env::var("REALTIME_SMOKE_WAV").expect("speech wav required");
        let bytes = std::fs::read(&wav_path).unwrap();
        let pcm = {
            assert!(bytes.len() >= 12 && &bytes[0..4] == b"RIFF", "expect wav");
            let mut offset = 12usize;
            loop {
                assert!(offset + 8 <= bytes.len(), "no data chunk");
                let id = &bytes[offset..offset + 4];
                let size =
                    u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
                if id == b"data" {
                    break bytes[offset + 8..(offset + 8 + size).min(bytes.len())].to_vec();
                }
                offset += 8 + size + (size & 1);
            }
        };
        let pcm = match std::env::var("REALTIME_SMOKE_AUDIO_MS")
            .ok()
            .and_then(|ms| ms.parse::<usize>().ok())
        {
            Some(ms) => {
                let capped = &pcm[..pcm.len().min(ms * 32)]; // 16k×2B/ms
                println!("live streaming: pcm capped to {ms}ms ({}B)", capped.len());
                capped.to_vec()
            }
            None => pcm,
        };
        // ActorCommand::AppendAudio 的契约是采集环 48kHz；live 样本文件是 16kHz。
        // 先补齐到采集率，让 Actor 内部再降回 DashScope 16kHz，复刻真实链路。
        let pcm = crate::audio::pcm::resample_pcm16_mono(
            &pcm,
            16_000,
            crate::audio::pcm::CAPTURE_SAMPLE_RATE,
        );
        println!("live streaming: pcm_bytes={}", pcm.len());

        let actor = RealtimeSession::start(RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: provider["id"].as_str().unwrap().to_owned(),
                base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
            },
            credential: Some(credential.as_str().to_owned()),
            model_id: route["e2eModelId"].as_str().unwrap_or("").to_owned(),
            voice,
            instructions: "你是会议助手，请用中文简短回答。".into(),
            history: vec![],
            auto_respond: true,
            enable_search: false,
        })
        .unwrap();

        let t_start = std::time::Instant::now();
        let mut connected_at: Option<Duration> = None;
        let mut first_delta_at: Option<Duration> = None;
        let mut speech_stopped_at: Option<Duration> = None;
        let mut audio_sent = 0usize;
        let mut user_text = String::new();
        let mut assistant_text = String::new();
        let mut audio_bytes = 0usize;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        // 模拟持续上行：真实说话约每 100ms 一帧；整句送完后停止上行。
        let mut last_push = std::time::Instant::now();
        while std::time::Instant::now() < deadline {
            // 每帧 100ms 语音按墙钟节奏上行（复刻真实麦克风节奏）。
            // 语音按墙钟节奏上行；语音送完后继续持续送静音，让服务端 VAD 判定
            // 句尾并生成响应（真实麦克风持续在线，静音帧一直存在）。
            if last_push.elapsed() >= std::time::Duration::from_millis(100) {
                let chunk = if audio_sent < pcm.len() {
                    let end = (audio_sent + 3200).min(pcm.len());
                    pcm[audio_sent..end].to_vec()
                } else {
                    vec![0u8; 3200]
                };
                audio_sent += chunk.len();
                actor.send(ActorCommand::AppendAudio(chunk));
                last_push = std::time::Instant::now();
            }
            match actor.recv_event(std::time::Duration::from_millis(50)) {
                Some(ActorEvent::Connected) => {
                    connected_at = Some(t_start.elapsed());
                    println!("live streaming: connected at {:?}", connected_at.unwrap());
                }
                Some(ActorEvent::SpeechStopped) => {
                    speech_stopped_at = Some(t_start.elapsed()); // 最近一次说完
                }
                Some(ActorEvent::UserTranscriptDelta { text, .. }) => user_text = text,
                Some(ActorEvent::UserTranscriptCompleted { text, .. }) => user_text = text,
                Some(ActorEvent::AssistantAudioDelta(pcm)) => {
                    if first_delta_at.is_none() {
                        first_delta_at = Some(t_start.elapsed());
                        println!(
                            "live streaming: FIRST AUDIO DELTA at {:?} (speech_stopped at {speech_stopped_at:?})",
                            first_delta_at.unwrap()
                        );
                    }
                    audio_bytes += pcm.len();
                }
                Some(ActorEvent::AssistantTextDelta(delta)) => assistant_text.push_str(&delta),
                Some(ActorEvent::ResponseDone { cancelled }) => {
                    println!(
                        "live streaming: done at {:?} cancelled={cancelled} user=\\\"{user_text}\\\" assistant=\\\"{assistant_text}\\\" audio_bytes={audio_bytes}",
                        t_start.elapsed()
                    );
                    // 多段语音样本会先触发一次 barge-in cancelled；继续等最终
                    // completed，避免把“被打断的一轮”误报成验收成功。
                    if cancelled {
                        continue;
                    }
                    if let (Some(connected), Some(first)) = (connected_at, first_delta_at) {
                        println!(
                            "live streaming: connect={connected:?} first_delta_since_start={first:?}"
                        );
                        if let Some(stopped) = speech_stopped_at {
                            println!(
                                "live streaming: first_audio_after_speech_stopped={:?}",
                                first.saturating_sub(stopped)
                            );
                        }
                    }
                    return;
                }
                Some(ActorEvent::Reconnecting(reason)) => {
                    println!("live streaming: reconnecting ({reason})");
                }
                Some(ActorEvent::Failed(reason)) => panic!("session failed: {reason}"),
                _ => {}
            }
        }
        panic!(
            "live streaming smoke timed out; user=\\\"{user_text}\\\" assistant=\\\"{assistant_text}\\\""
        );
    }

    /// 凭证识别冒烟：打印三个已存凭证的前缀特征（脱敏），判断对应供应商。
    /// 运行：cargo test --lib live_identify_provider_credentials -- --ignored --nocapture
    #[test]
    #[ignore = "Reads saved Windows credentials; prints only masked prefixes"]
    #[cfg(windows)]
    fn live_identify_provider_credentials() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;
        let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
        for id in [
            "3ebf81df-78de-46d3-adb3-2f050db54d6f",
            "c3925e07-1a33-4869-8f66-c93c3ec864f9",
            "38a37317-725f-4db1-97c3-111aa1e5d168",
        ] {
            let reference = format!("providers/{id}/api-key");
            let masked = match secrets.read(&reference) {
                Ok(Some(secret)) => {
                    let value = secret.as_str();
                    let prefix: String = value.chars().take(8).collect();
                    format!(
                        "len={} prefix={prefix}… has_dot={}",
                        value.chars().count(),
                        value.contains('.')
                    )
                }
                Ok(None) => "missing".to_owned(),
                Err(_) => "read-error".to_owned(),
            };
            println!("credential {id}: {masked}");
        }
    }

    /// GLM 模型清单探测：用已存 key 拉取可用模型列表（定位 realtime 模型名）。
    /// 运行：cargo test --lib live_glm_list_models -- --ignored --nocapture
    #[test]
    #[ignore = "Reads saved Windows credentials and calls the provider HTTP API"]
    #[cfg(windows)]
    fn live_glm_list_models() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;
        let secrets = SecretService::new("default", Arc::new(WindowsSecretStore::new())).unwrap();
        for id in [
            "c3925e07-1a33-4869-8f66-c93c3ec864f9",
            "38a37317-725f-4db1-97c3-111aa1e5d168",
        ] {
            let Ok(Some(secret)) = secrets.read(&format!("providers/{id}/api-key")) else {
                println!("{id}: key missing");
                continue;
            };
            let client = reqwest::blocking::Client::new();
            let response = client
                .get("https://open.bigmodel.cn/api/paas/v4/models")
                .bearer_auth(secret.as_str())
                .timeout(std::time::Duration::from_secs(15))
                .send();
            match response {
                Ok(response) => {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                    let models: Vec<String> = parsed["data"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item["id"].as_str().map(str::to_owned))
                                .filter(|id| {
                                    id.contains("realtime")
                                        || id.contains("voice")
                                        || id.contains("audio")
                                        || id.contains("omni")
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    println!("{id}: HTTP {status} realtime_like={}", models.join(", "));
                }
                Err(error) => println!("{id}: request failed: {error}"),
            }
        }
    }

    /// 重连退避：未建会话指数递增且永不突破 RECONNECT_CAP；建过会话重置基准。
    #[test]
    fn reconnect_backoff_doubles_but_never_exceeds_cap() {
        let mut backoff = RECONNECT_BASE;
        let mut steps = Vec::new();
        for _ in 0..16 {
            steps.push(backoff);
            backoff = next_reconnect_backoff(backoff, false);
        }
        assert_eq!(steps[0], Duration::from_millis(500));
        assert_eq!(steps[5], Duration::from_secs(16));
        assert!(
            steps.iter().all(|step| *step <= RECONNECT_CAP),
            "退避不得突破上限: {steps:?}"
        );
        // 触顶后保持上限（32s→30s 钳制），不会无限翻倍。
        assert_eq!(*steps.last().unwrap(), RECONNECT_CAP);
    }

    #[test]
    fn reconnect_backoff_resets_after_established_session() {
        let capped = next_reconnect_backoff(RECONNECT_CAP, false);
        assert_eq!(next_reconnect_backoff(capped, true), RECONNECT_BASE);
    }

    /// session.update 被服务端 error 事件拒绝：实现没有逐字段回退——
    /// 错误原因转发给编排层（Reconnecting 事件），整条连接拆掉重连并按
    /// 原画像重放 session.update（连接仍可用）。
    #[test]
    fn session_update_error_rejects_connection_and_forwards_reason_to_orchestrator() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let actor = spawn_actor(port, true, vec![]);

        // 首连：读走 session.update 后直接回 error 拒绝（不给 created/updated）。
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let update: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(update["type"], "session.update");
        ws.send(Message::Text(
            r#"{"type":"error","error":{"code":"SESSION_FIELD_REJECTED","message":"turn_detection not supported"}}"#
                .into(),
        ))
        .unwrap();

        // 错误原因必须转发给编排层，并携带服务端返回的 code。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "Reconnecting reason not forwarded"
            );
            match actor.recv_event(Duration::from_millis(100)) {
                Some(ActorEvent::Reconnecting(reason)) => {
                    assert!(
                        reason.contains("SESSION_FIELD_REJECTED"),
                        "reason={reason}"
                    );
                    break;
                }
                Some(_) | None => continue,
            }
        }

        // 拒绝后连接仍可用：重连成功，画像原样重放（无逐字段回退）。
        let (update2, _ws2) = accept_session(&listener);
        assert_eq!(update2["session"]["turn_detection"]["type"], "server_vad");
        wait_connected(&actor);
    }
}
