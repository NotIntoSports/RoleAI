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
    /// 回放是否包含 assistant 轮。Qwen-Omni Realtime 的 item.create 只接受
    /// user message：assistant item 服务端不回执且直接断连（真机逐条回执
    /// 实测，2026-09-29），Aliyun 方言必须跳过，否则每次重连即死循环。
    pub replay_assistant: bool,
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
            replay_assistant: dialect.name != RealtimeDialectName::Aliyun,
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

// 子模块：纯搬移拆分（C24）。跨子模块私有项升 pub(super)，父模块私有 use 引入
//（子模块经 glob 共享同名绑定），调用点路径不变。
mod connection;
mod session_flow;

use self::connection::{SharedState, supervise};
use self::session_flow::{
    append_audio, append_image, cancel_response, commit_turn, handle_server_event, respond_text,
    session_setup,
};

#[cfg(test)]
mod tests;
