//! 泵与 SessionService 共享的轮次数据与控制命令（纯搬移自 realtime_pump.rs）。

use super::*;

/// 泵完成后交给 SessionService 落库的一轮。
/// 逐轮关键事件相对泵启动（≈会话开始）的毫秒数，写入 turn_meta 定位慢在哪一段。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct TurnTimeline {
    #[ts(type = "number | null")]
    pub speech_started_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub speech_stopped_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub transcript_done_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub response_created_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub first_audio_ms: Option<u64>,
    #[ts(type = "number | null")]
    pub response_done_ms: Option<u64>,
    // —— 以下为级联（cascade）/端到端非泵路径的分阶段打点；Realtime 泵路径
    // 不产生这些阶段，序列化为 null（读侧按「缺失即无该阶段」处理）。 ——
    /// ASR 转写完成（级联；相对轮次起点）。
    #[ts(type = "number | null")]
    pub asr_done_ms: Option<u64>,
    /// RAG 资料检索完成（级联与 e2e 文本轮）。
    #[ts(type = "number | null")]
    pub retrieval_done_ms: Option<u64>,
    /// LLM 首个增量快照（级联与 e2e 非泵轮的首 token）。
    #[ts(type = "number | null")]
    pub llm_first_token_ms: Option<u64>,
    /// LLM 补全结束。
    #[ts(type = "number | null")]
    pub llm_done_ms: Option<u64>,
    /// TTS 合成完成（级联中即「首包可播音频就绪」）。
    #[ts(type = "number | null")]
    pub tts_done_ms: Option<u64>,
    /// 播放开始（级联 finalize 播放阶段；预留，泵路径由前端 WebAudio 掌握）。
    #[ts(type = "number | null")]
    pub playback_started_ms: Option<u64>,
    /// 播放结束或被打断（预留，同上）。
    #[ts(type = "number | null")]
    pub playback_done_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletedTurn {
    pub timeline: TurnTimeline,
    /// 泵成轮时刻；SessionService 落库时据此算前端 finalize 滞后。
    pub completed_at: Instant,
    pub user_text: String,
    pub assistant_text: String,
    /// speech_stopped → 首个音频 delta 的毫秒数（首响延迟核心指标）。
    pub first_audio_ms: Option<u64>,
    /// 播报中被用户语音打断（服务端 VAD barge-in）。
    pub interrupted: bool,
    /// 会议助手未点名：只落转写，无应答。
    pub transcript_only: bool,
    /// 候选模式：音频被闸门扣住（pending_confirmation 由 SessionService 标注）。
    pub held: bool,
    /// 本轮收到的助手 PCM 字节数；区分服务端没给音频与播放链路无声。
    pub audio_bytes: usize,
    /// 本轮回答由热键/按钮强制触发（未点名的仅转写发言被追答）。
    pub forced: bool,
    /// 播放写入失败（sidecar 死亡/管道断开）时保留诊断信号。
    pub playback_write_failed: bool,
    /// response.done 时流式播放 sidecar 是否仍存活。
    pub playback_alive: bool,
    /// 本轮收到的 assistant audio delta 数量。
    pub audio_delta_count: usize,
    /// 相邻 audio delta 的最大间隔；None 表示少于两个 delta。
    pub audio_delta_max_gap_ms: Option<u64>,
    pub audio_delta_gaps_over_150_ms: usize,
    pub audio_delta_gaps_over_500_ms: usize,
    /// 最近一次播报参考与麦克风包络的归一化相关度。
    pub aec_residual_correlation: Option<f64>,
    pub aec_residual_max: Option<f64>,
    pub aec_residual_over_threshold: usize,
    pub playback_mode: RealtimePlaybackMode,
    pub playback_generation: Option<u64>,
    pub playback_restarts: Option<u64>,
    pub playback_restarted_during_turn: bool,
    pub playback_last_event: String,
    /// 本轮被当前/历史播报文本回声过滤丢弃。
    pub echo_dropped: bool,
    /// 截至本轮完成时，泵累计丢弃的回声轮数。
    pub echo_dropped_total: u64,
    /// 回答请求发出后始终未开始（看门狗放弃）：用户文本保留、回答为空。
    pub response_failed: bool,
}

/// 泵与 SessionService 之间的共享状态。
#[derive(Default)]
pub struct PumpShared {
    completed: Mutex<VecDeque<CompletedTurn>>,
    /// 最近一条未点名的仅转写文本：热键/按钮强制回答（ForceRespond）的候选
    /// 素材。生成仅转写轮时写入，任一真实响应开始时清空（素材已被消费）。
    pub last_transcript_only: Mutex<Option<String>>,
    /// AI 播报中（UI 阶段显示用）。
    pub speaking: AtomicBool,
    /// 连接重连中（UI 提示用）。
    pub reconnecting: AtomicBool,
    /// 最近一次终局错误（Failed 事件）。
    pub failed: Mutex<Option<String>>,
    /// 累计指标。
    pub turns_total: AtomicU64,
    pub interrupted_total: AtomicU64,
    /// 文本/短片段回声兜底累计丢弃数。
    pub echo_dropped_total: AtomicU64,
    /// tap 满丢帧累计（Stage D 诊断页消费）。
    #[allow(dead_code)]
    pub ingress_dropped_total: AtomicU64,
}

impl PumpShared {
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn push_turn(&self, turn: CompletedTurn) {
        self.completed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back(turn);
        self.turns_total.fetch_add(1, Ordering::Relaxed);
    }

    /// 取走最旧的一轮（前端 finalize 的就绪信号即此非空）。
    pub fn take_completed(&self) -> Option<CompletedTurn> {
        self.completed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
    }

    pub fn completed_count(&self) -> usize {
        self.completed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .len()
    }

    pub fn echo_dropped_total(&self) -> u64 {
        self.echo_dropped_total.load(Ordering::SeqCst)
    }
}

/// 泵 → UI 的实时字幕/状态事件（经 commands 层闭包转发为 Tauri 事件）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PumpLive {
    /// 用户转写快照（累积全文）。
    User(String),
    /// 助手回复快照（累积全文）。
    Assistant(String),
    /// 播报状态变化。
    Speaking(bool),
    /// WebAudio 模式：assistant PCM delta（24kHz mono s16le）转发给前端。
    AssistantAudio(Vec<u8>),
    /// WebAudio 播放队列控制。
    PlaybackControl(PlaybackControl),
}

/// SessionService → 泵的控制命令。
#[derive(Debug)]
pub enum PumpCommand {
    /// 候选确认：放行扣住的音频到设备。
    FlushHeld,
    /// 候选拒绝：丢弃扣住的音频。
    DiscardHeld,
    /// 本地 SmartTurn 判定一句话说完：向服务端提交音频缓冲。
    /// Manual 模式（DashScope）服务端 VAD 已禁用，这是唯一的提交路径。
    CommitTurn,
    /// 本地打断监听触发：清空播放并取消在途响应。
    /// Manual 模式没有服务端 speech_started，播报打断只能本地触发。
    LocalBargeIn,
    /// 视频帧（摄像头/桌面共享，base64 JPEG）：直通实时会话，约 1fps。
    AppendImage(String),
    /// 停止泵（会话结束）。
    Shutdown,
    /// 更新上下文回放历史（每轮落库后同步，供重连回放）。
    SetHistory(Vec<(String, String)>),
    /// 热键/按钮强制回答：对当前在途用户转写、或最近一条未点名的仅转写
    /// 发言发起应答。回答进行中收到时忽略。
    ForceRespond,
}
