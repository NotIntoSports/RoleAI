//! 实时会话泵：消费 `RealtimeSession`（常驻 WS Actor）事件，驱动流式播放、
//! 服务端打断、点名门控与候选闸门；完成的轮次经共享队列交给 SessionService
//! 落库（泵不持有 sessions 互斥锁，也不触碰数据库）。
//!
//! 音频热路径：采集线程 tap → 泵（AppendAudio）→ 服务端；响应 delta → 泵 →
//! StreamPlayback（常驻 AudioBridge）。任何一环都不经过业务锁。

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use super::super::providers::realtime_session::{ActorCommand, ActorEvent, RealtimeSession};

use crate::audio::playback::PlaybackDiagnostics;

use super::echo_guard::{
    GATE_DRAIN_FALLBACK_TAIL, gate_drain_deadline_from_bytes, gate_timers_expired,
};

/// 回答开始看门狗：`RespondText` 发出后迟迟收不到 `response.created` 时，
/// 第一次超时清服务端输入缓冲并重发一次；再超时放弃本轮（response_failed
/// 成轮）。实测 Qwen-Omni 偶发吞掉 item.create+response.create 后沉默 60-90s，
/// 服务端 `RESPONSE_TIMEOUT` 只从 response.created 起算，罩不住这一段。
pub const RESPOND_START_TIMEOUT: Duration = Duration::from_secs(8);

/// 流式播放抽象：生产实现是常驻 AudioBridge `StreamPlayback`；测试用内存替身。
pub trait PlaybackStream: Send + Sync {
    fn write(&self, pcm: &[u8]) -> Result<(), &'static str>;
    fn clear(&self) -> Result<(), &'static str>;
    /// 播净当前缓冲后由 sidecar 回执（回声闸门据此重开上行）。无缓冲时立即回执。
    fn drain(&self) -> Result<(), &'static str> {
        Ok(())
    }

    /// 原生播放保活/健康探测；测试替身和无设备播放默认无副作用。
    fn ping(&self) -> Result<(), &'static str> {
        Ok(())
    }
    /// 取走「已播净」标记（消费型）。替身无设备缓冲，恒为已播净。
    fn take_drained(&self) -> bool {
        true
    }
    /// sidecar 是否存活；死亡时闸门不得永久关闭。
    fn is_alive(&self) -> bool {
        true
    }

    fn diagnostics(&self) -> PlaybackDiagnostics {
        PlaybackDiagnostics::default()
    }
}

impl PlaybackStream for crate::audio::playback::StreamPlayback {
    fn write(&self, pcm: &[u8]) -> Result<(), &'static str> {
        crate::audio::playback::StreamPlayback::write(self, pcm)
    }
    fn clear(&self) -> Result<(), &'static str> {
        crate::audio::playback::StreamPlayback::clear(self)
    }
    fn drain(&self) -> Result<(), &'static str> {
        crate::audio::playback::StreamPlayback::drain(self)
    }
    fn ping(&self) -> Result<(), &'static str> {
        crate::audio::playback::StreamPlayback::ping(self)
    }
    fn take_drained(&self) -> bool {
        crate::audio::playback::StreamPlayback::take_stream_drained(self)
    }
    fn is_alive(&self) -> bool {
        crate::audio::playback::StreamPlayback::is_alive(self)
    }

    fn diagnostics(&self) -> PlaybackDiagnostics {
        crate::audio::playback::StreamPlayback::diagnostics(self)
    }
}

/// 无渲染设备/纯文字会话的空播放实现。
pub struct SilentPlayback;

impl PlaybackStream for SilentPlayback {
    fn write(&self, _: &[u8]) -> Result<(), &'static str> {
        Ok(())
    }
    fn clear(&self) -> Result<(), &'static str> {
        Ok(())
    }
}

/// 泵完成后交给 SessionService 落库的一轮。
/// 逐轮关键事件相对泵启动（≈会话开始）的毫秒数，写入 turn_meta 定位慢在哪一段。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTimeline {
    pub speech_started_ms: Option<u64>,
    pub speech_stopped_ms: Option<u64>,
    pub transcript_done_ms: Option<u64>,
    pub response_created_ms: Option<u64>,
    pub first_audio_ms: Option<u64>,
    pub response_done_ms: Option<u64>,
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

    fn push_turn(&self, turn: CompletedTurn) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackControl {
    Clear,
}

/// 本机麦克风走 WebView 全双工；会议桥接/兜底走原生 AudioBridge。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealtimePlaybackMode {
    WebAudio,
    Native,
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

pub struct RealtimePump {
    pub shared: Arc<PumpShared>,
    commands_tx: mpsc::Sender<PumpCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// 泵的静态行为配置。
pub struct PumpConfig {
    /// 会议助手点名门控：转写完成后按点名决定是否应答。
    pub auto_respond: bool,
    pub role_name: String,
    /// 候选模式：生成照常但音频扣住不播，确认后放行。
    pub hold_playback: bool,
    pub playback_mode: RealtimePlaybackMode,
    /// 播报回声抑制续窗：每次音频落设备后以「delta 时长 + 尾窗」调用；
    /// None 表示无麦克风抑制需求（纯文字会话）。
    pub suppress_echo: Option<Box<dyn Fn(std::time::Duration) + Send + Sync>>,
    /// 回答开始看门狗超时；生产默认 `RESPOND_START_TIMEOUT`，测试调短。
    pub respond_start_timeout: Duration,
}

impl RealtimePump {
    /// 启动泵线程。`mic_tap` 由采集侧订阅（tap 通道），`playback` 为常驻播放句柄。
    pub fn start(
        session: RealtimeSession,
        mic_tap: mpsc::Receiver<Vec<u8>>,
        playback: Arc<dyn PlaybackStream>,
        config: PumpConfig,
        live_sink: Option<Box<dyn Fn(PumpLive) + Send + Sync>>,
    ) -> Self {
        let shared = Arc::new(PumpShared::new());
        let (commands_tx, commands_rx) = mpsc::channel::<PumpCommand>();
        let shared_for_thread = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("realtime-pump".into())
            .spawn(move || {
                run_pump(
                    session,
                    mic_tap,
                    playback,
                    config,
                    shared_for_thread,
                    commands_rx,
                    live_sink,
                );
            })
            .ok();
        Self {
            shared,
            commands_tx,
            thread,
        }
    }

    pub fn send(&self, command: PumpCommand) {
        let _ = self.commands_tx.send(command);
    }

    /// 命令发送端克隆：采集线程信号转发桥使用（与泵生命周期解耦，
    /// 泵 Drop 后通道断开，转发线程随 send 失败退出）。
    pub fn command_sink(&self) -> mpsc::Sender<PumpCommand> {
        self.commands_tx.clone()
    }
}

impl Drop for RealtimePump {
    fn drop(&mut self) {
        let _ = self.commands_tx.send(PumpCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 播放参考与麦克风电平包络的相关度监测。这里不是第二个 AEC，而是评估
/// WebView AEC 是否残留强回声；连续高残留时切换原生可打断兜底。
#[derive(Default)]
struct AecResidualMonitor {
    playback_envelopes: VecDeque<f32>,
    microphone_envelopes: VecDeque<f32>,
}

impl AecResidualMonitor {
    fn observe_playback(&mut self, pcm: &[u8]) {
        append_envelopes(&mut self.playback_envelopes, pcm, 24_000);
    }

    fn observe_microphone(&mut self, pcm: &[u8]) -> Option<f64> {
        append_envelopes(&mut self.microphone_envelopes, pcm, 48_000);
        if self.playback_envelopes.len() < 8 || self.microphone_envelopes.len() < 8 {
            return None;
        }
        let mut best = f64::MIN;
        let max_lag = self.microphone_envelopes.len().saturating_sub(8);
        for lag in 0..=max_lag.min(100) {
            let comparison_len = self.playback_envelopes.len().min(24);
            let microphone = self
                .microphone_envelopes
                .iter()
                .skip(lag)
                .take(comparison_len)
                .copied()
                .collect::<Vec<_>>();
            let mut playback = self
                .playback_envelopes
                .iter()
                .rev()
                .take(comparison_len)
                .copied()
                .collect::<Vec<_>>();
            playback.reverse();
            best = best.max(pearson(&playback, &microphone));
        }
        (best > 0.0).then_some(best)
    }
}

fn append_envelopes(target: &mut VecDeque<f32>, pcm: &[u8], sample_rate: u32) {
    let step_samples = ((sample_rate / 100).max(1) as usize).max(1);
    for block in pcm.chunks(step_samples * 2) {
        let peak = block
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs() as f32)
            .fold(0.0, f32::max);
        target.push_back(peak);
    }
    while target.len() > 300 {
        target.pop_front();
    }
}

fn pearson(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let count = left.len() as f64;
    let left_sum: f64 = left.iter().map(|value| f64::from(*value)).sum();
    let right_sum: f64 = right.iter().map(|value| f64::from(*value)).sum();
    let left_mean = left_sum / count;
    let right_mean = right_sum / count;
    let mut numerator = 0.0;
    let mut left_energy = 0.0;
    let mut right_energy = 0.0;
    for (left, right) in left.iter().zip(right) {
        let left = f64::from(*left) - left_mean;
        let right = f64::from(*right) - right_mean;
        numerator += left * right;
        left_energy += left * left;
        right_energy += right * right;
    }
    if left_energy == 0.0 || right_energy == 0.0 {
        return 0.0;
    }
    numerator / (left_energy * right_energy).sqrt()
}

struct TurnAccumulator {
    timeline: TurnTimeline,
    user_text: String,
    assistant_text: String,
    speech_stopped_at: Option<Instant>,
    first_audio_at: Option<Instant>,
    responding: bool,
    /// response.done 已收到：其后到达的迟到尾包 delta 一律丢弃（防重复播报），
    /// 直到下一个 response.created 复位。delta 早于 created 的历史行为不受影响。
    response_closed: bool,
    played_audio: bool,
    audio_bytes: usize,
    audio_delta_count: usize,
    audio_delta_max_gap_ms: Option<u64>,
    audio_delta_gaps_over_150_ms: usize,
    audio_delta_gaps_over_500_ms: usize,
    aec_residual_correlation: Option<f64>,
    aec_residual_max: Option<f64>,
    aec_residual_over_threshold: usize,
    last_audio_delta_at: Option<Instant>,
    playback_generation_at_start: Option<u64>,
    playback_restarts_at_start: Option<u64>,
    playback_write_failed: bool,
    interrupted: bool,
    transcript_only: bool,
    /// 本轮应答由 ForceRespond 触发（写入 CompletedTurn.forced 供 turn_meta 标注）。
    forced: bool,
    /// 本轮被文本回声过滤判定为回声（AI 播报声被麦克风回收）：done 到达时
    /// 不成轮不落库，否则自问自答循环会被固化进历史。
    echo_dropped: bool,
    /// 正在应答的那句话的转写 item_id（点名=定稿 item；ForceRespond=在途
    /// delta item；文本来自 last_transcript_only 记 None）。回答期间据以
    /// 区分「同一句话的定稿」与「新的一句话」。
    answer_item_id: Option<String>,
    /// 最近一次用户转写 delta 的 item_id（ForceRespond 判定回答素材归属）。
    last_user_item_id: Option<String>,
    /// 回答开始看门狗：RespondText 发出时刻；收到 ResponseStarted 即清空。
    respond_requested_at: Option<Instant>,
    /// 看门狗重试次数：0=未重试（首次超时重发），1=已重试（再超时放弃）。
    respond_attempts: u8,
}

impl TurnAccumulator {
    fn new() -> Self {
        Self {
            timeline: TurnTimeline::default(),
            user_text: String::new(),
            assistant_text: String::new(),
            speech_stopped_at: None,
            first_audio_at: None,
            responding: false,
            response_closed: false,
            played_audio: false,
            audio_bytes: 0,
            audio_delta_count: 0,
            audio_delta_max_gap_ms: None,
            audio_delta_gaps_over_150_ms: 0,
            audio_delta_gaps_over_500_ms: 0,
            aec_residual_correlation: None,
            aec_residual_max: None,
            aec_residual_over_threshold: 0,
            last_audio_delta_at: None,
            playback_generation_at_start: None,
            playback_restarts_at_start: None,
            playback_write_failed: false,
            interrupted: false,
            transcript_only: false,
            forced: false,
            echo_dropped: false,
            answer_item_id: None,
            last_user_item_id: None,
            respond_requested_at: None,
            respond_attempts: 0,
        }
    }

    fn first_audio_ms(&self) -> Option<u64> {
        Some(
            self.first_audio_at?
                .duration_since(self.speech_stopped_at.unwrap_or_else(Instant::now))
                .as_millis() as u64,
        )
    }

    fn has_pending_content(&self) -> bool {
        self.responding || !self.user_text.is_empty() || !self.assistant_text.is_empty()
    }

    fn record_audio_delta_at(&mut self, now: Instant) {
        if let Some(last) = self.last_audio_delta_at {
            let gap = now.duration_since(last);
            let gap_ms = gap.as_millis() as u64;
            self.audio_delta_max_gap_ms = Some(
                self.audio_delta_max_gap_ms
                    .map_or(gap_ms, |previous| previous.max(gap_ms)),
            );
            if gap_ms > 150 {
                self.audio_delta_gaps_over_150_ms += 1;
            }
            if gap_ms > 500 {
                self.audio_delta_gaps_over_500_ms += 1;
            }
        }
        self.last_audio_delta_at = Some(now);
        self.audio_delta_count += 1;
    }

    fn record_aec_residual(&mut self, value: f64) {
        self.aec_residual_correlation = Some(value);
        self.aec_residual_max = Some(self.aec_residual_max.map_or(value, |old| old.max(value)));
        if value >= 0.9 {
            self.aec_residual_over_threshold += 1;
        }
    }
}

/// 连接重置前冲刷在途轮：已收到的内容立即成轮（打断语义），状态归零。
/// 已扣住待确认的音频（候选模式）不受影响，仍等 FlushHeld/DiscardHeld。
fn flush_in_flight_turn(
    turn: &mut TurnAccumulator,
    shared: &Arc<PumpShared>,
    playback: &Arc<dyn PlaybackStream>,
    playback_mode: RealtimePlaybackMode,
    emit: &dyn Fn(&PumpLive),
) {
    if turn.has_pending_content() && !turn.echo_dropped {
        let responding = turn.responding;
        let diagnostics = playback.diagnostics();
        let playback_restarts = diagnostics.restarts;
        let playback_restarted_during_turn = turn
            .playback_restarts_at_start
            .is_some_and(|started| playback_restarts > started);
        shared.push_turn(CompletedTurn {
            user_text: std::mem::take(&mut turn.user_text),
            assistant_text: std::mem::take(&mut turn.assistant_text),
            first_audio_ms: turn.first_audio_ms(),
            interrupted: responding || turn.interrupted,
            transcript_only: !responding,
            held: false,
            forced: turn.forced,
            audio_bytes: turn.audio_bytes,
            playback_write_failed: turn.playback_write_failed,
            playback_alive: playback.is_alive(),
            audio_delta_count: turn.audio_delta_count,
            audio_delta_max_gap_ms: turn.audio_delta_max_gap_ms,
            audio_delta_gaps_over_150_ms: turn.audio_delta_gaps_over_150_ms,
            audio_delta_gaps_over_500_ms: turn.audio_delta_gaps_over_500_ms,
            aec_residual_correlation: turn.aec_residual_correlation,
            aec_residual_max: turn.aec_residual_max,
            aec_residual_over_threshold: turn.aec_residual_over_threshold,
            playback_mode,
            playback_generation: turn
                .playback_generation_at_start
                .or(Some(diagnostics.generation)),
            playback_restarts: Some(playback_restarts),
            playback_restarted_during_turn,
            playback_last_event: diagnostics.last_event,
            echo_dropped: turn.echo_dropped,
            echo_dropped_total: shared.echo_dropped_total(),
            response_failed: false,
            timeline: turn.timeline.clone(),
            completed_at: Instant::now(),
        });
    }
    *turn = TurnAccumulator::new();
    shared.speaking.store(false, Ordering::SeqCst);
    emit(&PumpLive::Speaking(false));
}

/// 当前轮回声判定所需的播报状态。单独成结构，避免判断函数参数超过 clippy 上限。
struct CurrentTurnEcho<'a> {
    assistant: &'a str,
    responding: bool,
    played_audio: bool,
    first_audio_at: Option<Instant>,
    post_playback_until: Option<Instant>,
    /// 原生播放闭锁期间才启用当前轮短片段回声闸；WebAudio 全双工不靠它挡打断。
    guard: bool,
}

/// 历史轮回声沿用通用相似度；当前轮已实际出声时，短片段也按回声处理。
/// 播报期间麦克风闸门关闭，此时服务端转写出的当前回答片段大概率是设备
/// 尾音/回声，而不是已经上行的新用户语音。
fn is_current_or_recent_echo(
    user_text: &str,
    recent_assistant: &[String],
    current: &CurrentTurnEcho<'_>,
) -> bool {
    if crate::services::echo_guard::is_echo(user_text, recent_assistant) {
        return true;
    }
    if !current.responding || !current.played_audio {
        return false;
    }
    // WebAudio 全双工上行常开：播报中的当轮回声转写对不上历史缓存（当轮
    // 文本尚未入缓存），必须直接比对在途 assistant 文本——模型正念到的话
    // 被麦克风回收，是「复述自己」症状的主源头。不启用 Native 的
    // 「短句一律按回声」兜底：上行常开时真实插话会从中途到达，不能按
    // 时长一刀切；且 echo_guard 对 <6 字的短句本就不判，真人确认词安全。
    if !current.guard {
        return crate::services::echo_guard::is_echo(user_text, &[current.assistant.to_string()]);
    }
    let user = crate::services::echo_guard::normalize(user_text);
    let spoken = crate::services::echo_guard::normalize(current.assistant);
    let user_len = user.chars().count();
    if let Some(until) = current.post_playback_until
        && Instant::now() < until
        && let Some(previous) = recent_assistant.last()
    {
        let previous = crate::services::echo_guard::normalize(previous);
        let suffix = common_suffix_length(&user, &previous);
        if (2..=12).contains(&user_len) && suffix >= 2 && suffix * 2 >= user_len {
            return true;
        }
    }
    if (2..=12).contains(&user_len) && !spoken.is_empty() && spoken.contains(&user) {
        return true;
    }

    // 文本 transcript delta 可能远落后于音频。设备已出声一段时间后，
    // 麦克风闸门本就关闭；此时到达的短用户转写优先按当前轮尾音/回声处理。
    user_len <= 12
        && current
            .first_audio_at
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(400))
}

fn current_turn_echo<'a>(
    turn: &'a TurnAccumulator,
    post_playback_until: Option<Instant>,
    playback_mode: RealtimePlaybackMode,
) -> CurrentTurnEcho<'a> {
    CurrentTurnEcho {
        assistant: &turn.assistant_text,
        responding: turn.responding,
        played_audio: turn.played_audio,
        first_audio_at: turn.first_audio_at,
        post_playback_until,
        guard: playback_mode == RealtimePlaybackMode::Native,
    }
}

fn common_suffix_length(left: &str, right: &str) -> usize {
    left.chars()
        .rev()
        .zip(right.chars().rev())
        .take_while(|(left, right)| left == right)
        .count()
}

fn run_pump(
    session: RealtimeSession,
    mic_tap: mpsc::Receiver<Vec<u8>>,
    playback: Arc<dyn PlaybackStream>,
    config: PumpConfig,
    shared: Arc<PumpShared>,
    commands: mpsc::Receiver<PumpCommand>,
    live_sink: Option<Box<dyn Fn(PumpLive) + Send + Sync>>,
) {
    let pump_started = Instant::now();
    let since_start = || Some(pump_started.elapsed().as_millis() as u64);
    let playback_mode = config.playback_mode;
    let mut aec_monitor = AecResidualMonitor::default();
    let mut post_playback_echo_until: Option<Instant> = None;
    let mut last_playback_ping = Instant::now();
    let emit = |event: &PumpLive| {
        if let Some(sink) = live_sink.as_ref() {
            sink(event.clone());
        }
    };
    let mut turn = TurnAccumulator::new();
    // 重连前已定稿、但响应尚未开始的用户文本；Connected 后用文本轮恢复应答。
    let mut pending_recovered_text: Option<String> = None;
    // 门控模式回答在途期间收到的新点名：当前回答结束后自动发起
    // （最多保留最新 1 条，新点名覆盖旧点名）。
    let mut pending_mention: Option<String> = None;
    let respond_start_timeout = config.respond_start_timeout;
    let mut held_audio: Vec<u8> = Vec::new();
    const HELD_CAP_BYTES: usize = 60 * 48_000; // 60s@24k 上限，溢出即弃并停止扣留
    // 回声闸门（仅 Native/会议桥接路径）：播报/播净期间关闭——麦克风不认证上行
    // （回声不得进服务端缓冲），并持续续本地回声抑制窗（分段器不产段）。
    // WebAudio 模式不关门：WebView AEC 同上下文消除回声，上行保持全双工开放，
    // 模型服务端 VAD 随时听得见用户（官方同款打断/判句）；渗入的回声转写由
    // echo_guard 文本过滤兜底。
    let mut uplink_open = true;
    let mut gate_closed_at: Option<Instant> = None;
    // 播净回执丢失时的短兜底：按已写入音频时长续期，避免设备事件偶发
    // 丢失后麦克风被 20 秒安全阀闭锁，后续用户语音整句丢失。
    let mut gate_drain_deadline: Option<Instant> = None;
    /// 本地打断后的上行重开尾窗：clear 生效、设备缓冲排空与混响衰减需要时间，
    /// 立即开门会让回声尾巴上行进服务端缓冲。1200ms 覆盖扬声器余音与
    /// 小房间混响的常见衰减时长，代价只是打断后 ~1s 内不上行。
    const BARGE_REOPEN_TAIL: Duration = Duration::from_millis(1200);
    /// 最近播报文本缓存容量（回声比对窗口）。
    const ECHO_RECENT_TURNS: usize = 3;
    // 回声比对缓存：每轮播过的 assistant 文本（归一化比对在 echo_guard 内做）。
    let mut recent_assistant: VecDeque<String> = VecDeque::new();
    // 本地打断后的延迟重开时刻；None 表示无挂起重开。
    let mut uplink_reopen_at: Option<Instant> = None;

    loop {
        if playback_mode == RealtimePlaybackMode::WebAudio
            && last_playback_ping.elapsed() >= Duration::from_millis(500)
        {
            // 原生兜底进程保活；StreamPlayback 内部会在死亡时自动重建。
            // 只在真正发出 ping 后重置计时。忙循环每圈都刷新会让间隔永远到不了 500ms。
            let _ = playback.ping();
            last_playback_ping = Instant::now();
        }
        // 1) 麦克风 tap → 持续上行（Native 闸门关闭期间只排空通道不发送；
        // WebAudio 全双工常开，回声转写由 echo_guard 文本过滤兜底）。
        while let Ok(pcm) = mic_tap.try_recv() {
            let residual = aec_monitor.observe_microphone(&pcm);
            if playback_mode == RealtimePlaybackMode::WebAudio
                && turn.responding
                && let Some(residual) = residual
            {
                // 包络相关度只做诊断，不据此切换播放：两段语音的包络本身就可能高相关
                // （无回声的假麦克风也会触发），且默认输出下原生兜底是 SilentPlayback，
                // 切换会让回答整段无声并卡住成轮。回声兜底交给文本 echo_guard。
                turn.record_aec_residual(residual);
            }
            if uplink_open {
                session.send(ActorCommand::AppendAudio(pcm));
            }
        }
        if !uplink_open {
            if let Some(suppress) = config.suppress_echo.as_ref() {
                suppress(Duration::from_secs(1));
            }
            if uplink_reopen_at.is_some_and(|at| Instant::now() >= at)
                || playback.take_drained()
                || !playback.is_alive()
                || gate_timers_expired(gate_drain_deadline, gate_closed_at, Instant::now())
            {
                // 打断/播净重开瞬间清一次服务端输入缓冲：关门期与重开尾窗渗入的
                // 回声残渣（早于/晚于打断的零星帧）不得混进下一次提交。
                session.send(ActorCommand::ClearInputBuffer);
                uplink_open = true;
                gate_closed_at = None;
                gate_drain_deadline = None;
                uplink_reopen_at = None;
            }
        }
        // 2) 控制命令。
        match commands.try_recv() {
            Ok(PumpCommand::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                session.send(ActorCommand::Shutdown);
                return;
            }
            Ok(PumpCommand::FlushHeld) => {
                let held = std::mem::take(&mut held_audio);
                let _ = playback.write(&held);
                shared.speaking.store(true, Ordering::SeqCst);
                if !held.is_empty() {
                    // 冲刷出声：同样关闸 + 排水，等播净回执（与正常播报同路）。
                    uplink_reopen_at = None;
                    uplink_open = false;
                    gate_closed_at.get_or_insert_with(Instant::now);
                    gate_drain_deadline =
                        Some(gate_drain_deadline_from_bytes(Instant::now(), held.len()));
                    // 关门即作废遗留播净回执（初始值/上轮残留），见 AssistantAudioDelta。
                    let _ = playback.take_drained();
                    let _ = playback.drain();
                }
            }
            Ok(PumpCommand::DiscardHeld) => {
                held_audio.clear();
            }
            Ok(PumpCommand::CommitTurn) => {
                // Native 闸门关闭期间提交没有意义：新语音根本没上行过，缓冲里只
                // 可能是抢跑的回声残渣。WebAudio 全双工上行常开，提交总是有效。
                if uplink_open {
                    session.send(ActorCommand::CommitTurn);
                }
            }
            Ok(PumpCommand::LocalBargeIn) => {
                tracing::debug!(
                    target: "realtime_pump",
                    responding = turn.responding,
                    uplink_open,
                    ?playback_mode,
                    "本地 barge_in（采集侧打断监听触发）"
                );
                // 本地打断：语义同服务端 speech_started——响应在途才清空取消。
                if turn.responding {
                    let _ = playback.clear();
                    session.send(ActorCommand::CancelResponse);
                    emit(&PumpLive::PlaybackControl(PlaybackControl::Clear));
                    turn.interrupted = true;
                    shared.speaking.store(false, Ordering::SeqCst);
                    emit(&PumpLive::Speaking(false));
                    shared.interrupted_total.fetch_add(1, Ordering::Relaxed);
                }
                if !uplink_open {
                    // Native 闸门关闭期的打断：服务端缓冲里只可能是关门前的抢跑
                    // 回声，清空防止混入下一次提交；上行延迟到尾窗后再开（混响衰减）。
                    session.send(ActorCommand::ClearInputBuffer);
                    uplink_reopen_at = Some(Instant::now() + BARGE_REOPEN_TAIL);
                }
                // WebAudio（闸门常开）不动服务端缓冲：用户语音已在其中，正是
                // 打断后模型要听的下一句；clear 会把它抹掉。
            }
            Ok(PumpCommand::AppendImage(jpeg_b64)) => {
                // 视频帧直通实时会话：不经麦克风 tap/回声闸门（画面无回声语义），
                // Manual 模式下由下一次 CommitTurn 一并提交给模型。
                session.send(ActorCommand::AppendImage(jpeg_b64));
            }
            Ok(PumpCommand::SetHistory(history)) => {
                session.send(ActorCommand::SetHistory(history));
            }
            Ok(PumpCommand::ForceRespond) => {
                // 热键/按钮强制回答：正在回答时忽略；否则取当前在途的用户
                // 转写，没有就取最近一条仅转写发言，以文本轮发起应答（与
                // 重连恢复的文本应答同路）。语义上等于用户点名。
                if turn.responding {
                    tracing::debug!(
                        target: "realtime_pump",
                        "强制回答被忽略：响应已在途"
                    );
                } else {
                    let text = (!turn.user_text.is_empty())
                        .then(|| turn.user_text.clone())
                        .or_else(|| {
                            shared
                                .last_transcript_only
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .clone()
                        });
                    if let Some(text) = text {
                        tracing::debug!(
                            target: "realtime_pump",
                            text = %text,
                            "强制回答：对仅转写发言发起应答"
                        );
                        shared
                            .last_transcript_only
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .take();
                        // 文本取自在途转写 delta 时记录其 item_id：同一句话的
                        // 定稿 completed 晚到时据以识别，不被当作新发言；取自
                        // last_transcript_only 时记 None（原 item 已成轮）。
                        let answer_item_id = if turn.user_text.is_empty() {
                            None
                        } else {
                            turn.last_user_item_id.clone()
                        };
                        turn.user_text = text.clone();
                        turn.answer_item_id = answer_item_id;
                        turn.forced = true;
                        turn.responding = true;
                        turn.response_closed = false;
                        turn.echo_dropped = false;
                        turn.respond_requested_at = Some(Instant::now());
                        turn.respond_attempts = 0;
                        emit(&PumpLive::User(text));
                        session.send(ActorCommand::RespondText(turn.user_text.clone()));
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        // 3) Actor 事件。
        match session.recv_event(std::time::Duration::from_millis(25)) {
            Some(ActorEvent::Connected) => {
                shared.reconnecting.store(false, Ordering::SeqCst);
                // 图片前置错误等瞬时重连发生在转写完成、响应未开始时，不能把
                // 用户这句话固化成 text_only；新连接上下文重放后补一次文本应答。
                if let Some(text) = pending_recovered_text.take() {
                    turn.user_text = text.clone();
                    turn.responding = true;
                    turn.response_closed = false;
                    turn.respond_requested_at = Some(Instant::now());
                    turn.respond_attempts = 0;
                    emit(&PumpLive::User(text.clone()));
                    session.send(ActorCommand::RespondText(text));
                }
            }
            Some(ActorEvent::Reconnecting(_)) => {
                shared.reconnecting.store(true, Ordering::SeqCst);
                // 连接即将重置：在途轮次的 responding/interrupted 状态若不冲刷，
                // 会残留到重连后——turn 2 音频被 interrupted 分支静默丢弃、
                // speaking 卡 true 令前端 phase 停在「回复中」、finalize 停摆。
                // 断连前已收到的用户语音立即成轮落库（打断语义），不静默丢失。
                let recoverable_text = config.auto_respond
                    && !turn.responding
                    && !turn.echo_dropped
                    && !turn.user_text.is_empty();
                if recoverable_text {
                    pending_recovered_text = Some(std::mem::take(&mut turn.user_text));
                    turn = TurnAccumulator::new();
                    shared.speaking.store(false, Ordering::SeqCst);
                    emit(&PumpLive::Speaking(false));
                } else {
                    flush_in_flight_turn(&mut turn, &shared, &playback, playback_mode, &emit);
                }
                // 断连的半截响应不再续播：清空设备缓冲，掐断回声源并立即恢复上行。
                let _ = playback.clear();
                uplink_open = true;
                gate_closed_at = None;
                gate_drain_deadline = None;
            }
            Some(ActorEvent::Failed(reason)) => {
                *shared.failed.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason);
                shared.reconnecting.store(false, Ordering::SeqCst);
            }
            Some(ActorEvent::SpeechStarted) => {
                turn.timeline
                    .speech_started_ms
                    .get_or_insert_with(|| since_start().unwrap_or(0));
                tracing::debug!(
                    target: "realtime_pump",
                    responding = turn.responding,
                    played_audio = turn.played_audio,
                    speech_started_ms = turn.timeline.speech_started_ms,
                    "服务端 speech_started（服务端 VAD 听到用户开口）"
                );
                // 服务端 VAD 听见用户开口：若一轮响应在途，立即清空播放 + 取消生成
                // （生成期插话服务端也会自动取消，客户端取消是确定性兜底）。
                if turn.responding {
                    if !config.auto_respond && !turn.played_audio {
                        // 门控模式（会议助手 + 会议桥接）回答还没出声：别人插话、
                        // 环境噪声或用户自己接着说都会触发 speech_started，此时
                        // 取消会让点名/强制回答永远出不了声。只记日志不打断。
                        tracing::debug!(
                            target: "realtime_pump",
                            "门控模式：回答未出声，忽略服务端 speech_started"
                        );
                    } else {
                        let _ = playback.clear();
                        session.send(ActorCommand::CancelResponse);
                        emit(&PumpLive::PlaybackControl(PlaybackControl::Clear));
                        turn.interrupted = true;
                        shared.speaking.store(false, Ordering::SeqCst);
                        emit(&PumpLive::Speaking(false));
                        shared.interrupted_total.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Some(ActorEvent::SpeechStopped) => {
                // 句中停顿也会触发 speech_stopped：始终取最近一次作为说完锚点，
                // 否则首响延迟会被句中停顿虚增。
                turn.speech_stopped_at = Some(Instant::now());
                turn.timeline.speech_stopped_ms = since_start();
            }
            Some(ActorEvent::ResponseStarted) => {
                turn.timeline.response_created_ms = since_start();
                turn.responding = true;
                turn.response_closed = false;
                // 服务端已确认响应开始：回答开始看门狗解除。
                turn.respond_requested_at = None;
                // 真实响应开始：强制回答的候选素材视为已消费，避免响应失败
                // 前素材残留导致重复追答同一句。
                shared
                    .last_transcript_only
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take();
            }
            Some(ActorEvent::UserTranscriptDelta { item_id, text }) => {
                // 门控模式回答在途：另一句话的 partial 只上屏，不得覆盖正在
                // 回答的用户文本（其定稿 completed 会按新发言单独成轮）。
                let same_answer = turn.answer_item_id.as_deref() == Some(item_id.as_str());
                if !config.auto_respond && turn.responding && !same_answer {
                    if !is_current_or_recent_echo(
                        &text,
                        recent_assistant.make_contiguous(),
                        &current_turn_echo(&turn, post_playback_echo_until, playback_mode),
                    ) {
                        emit(&PumpLive::User(text));
                    } else {
                        tracing::debug!(
                            target: "realtime_pump",
                            text = %text,
                            "partial 用户转写判定为回声，不上屏"
                        );
                    }
                } else {
                    turn.last_user_item_id = Some(item_id);
                    turn.user_text = text;
                    // 回声转写的 partial 快照也不上屏：定稿判定回声时屏幕不应闪现过。
                    if !is_current_or_recent_echo(
                        &turn.user_text,
                        recent_assistant.make_contiguous(),
                        &current_turn_echo(&turn, post_playback_echo_until, playback_mode),
                    ) {
                        emit(&PumpLive::User(turn.user_text.clone()));
                    } else {
                        tracing::debug!(
                            target: "realtime_pump",
                            text = %turn.user_text,
                            "partial 用户转写判定为回声，不上屏"
                        );
                    }
                }
            }
            Some(ActorEvent::UserTranscriptCompleted { item_id, text }) => {
                turn.timeline.transcript_done_ms = since_start();
                // 新一条用户发言定稿：先清掉上一轮回声丢弃的残留标记。
                turn.echo_dropped = false;
                if !turn.responding {
                    // 无在途响应时顺带清 delta 屏蔽（上一轮回声丢弃的遗留）。
                    turn.interrupted = false;
                }
                if is_current_or_recent_echo(
                    &text,
                    recent_assistant.make_contiguous(),
                    &current_turn_echo(&turn, post_playback_echo_until, playback_mode),
                ) {
                    tracing::debug!(
                        target: "realtime_pump",
                        responding = turn.responding,
                        text = %text,
                        "用户转写定稿判定为回声：整轮丢弃并清服务端缓冲"
                    );
                    // 回声轮：AI 播报声被麦克风回收成「用户发言」。取消在途响应、
                    // 清服务端缓冲，整轮丢弃——绝不 emit/成轮/落库，否则
                    // 「自己回答自己」的循环被固化为正式对话历史。
                    if turn.responding {
                        let _ = playback.clear();
                        session.send(ActorCommand::CancelResponse);
                        shared.speaking.store(false, Ordering::SeqCst);
                        emit(&PumpLive::Speaking(false));
                    }
                    session.send(ActorCommand::ClearInputBuffer);
                    turn.user_text.clear();
                    turn.echo_dropped = true;
                    shared.echo_dropped_total.fetch_add(1, Ordering::SeqCst);
                    // cancel 生效前在途的响应 delta 一律丢弃（无响应在途时
                    // 该屏蔽随下一个真实转写定稿复位）。
                    turn.interrupted = true;
                    continue;
                }
                // 门控模式回答在途：区分「同一句话的定稿」与「新的一句话」，
                // 后者不得覆盖正在回答的用户文本（等待期吞话的根源）。
                if !config.auto_respond && turn.responding {
                    if turn.answer_item_id.as_deref() == Some(item_id.as_str()) {
                        // 同一句话的定稿：只用完整文本更新在途回答。
                        turn.user_text = text;
                        tracing::debug!(
                            target: "realtime_pump",
                            item_id = %item_id,
                            text = %turn.user_text,
                            "回答中的同句转写定稿：更新在途回答文本"
                        );
                        emit(&PumpLive::User(turn.user_text.clone()));
                    } else {
                        // 新的一句话：立即成仅转写轮并留作追答素材；若点名则
                        // 排队，当前回答结束后自动发起（保留最新 1 条）。
                        tracing::debug!(
                            target: "realtime_pump",
                            item_id = %item_id,
                            text = %text,
                            "回答在途收到新发言：仅转写成轮，不覆盖在途回答"
                        );
                        emit(&PumpLive::User(text.clone()));
                        let mentioned = crate::services::sessions::meeting_assistant_was_mentioned(
                            &text,
                            &config.role_name,
                        );
                        if mentioned {
                            pending_mention = Some(text.clone());
                        }
                        let diagnostics = playback.diagnostics();
                        let transcript_timeline = TurnTimeline {
                            transcript_done_ms: turn.timeline.transcript_done_ms,
                            ..TurnTimeline::default()
                        };
                        let new_text = text.clone();
                        shared.push_turn(CompletedTurn {
                            user_text: text,
                            assistant_text: String::new(),
                            first_audio_ms: None,
                            interrupted: false,
                            transcript_only: true,
                            held: false,
                            forced: false,
                            response_failed: false,
                            audio_bytes: 0,
                            playback_write_failed: false,
                            playback_alive: playback.is_alive(),
                            audio_delta_count: 0,
                            audio_delta_max_gap_ms: None,
                            audio_delta_gaps_over_150_ms: 0,
                            audio_delta_gaps_over_500_ms: 0,
                            aec_residual_correlation: None,
                            aec_residual_max: None,
                            aec_residual_over_threshold: 0,
                            playback_mode,
                            playback_generation: Some(diagnostics.generation),
                            playback_restarts: Some(diagnostics.restarts),
                            playback_restarted_during_turn: false,
                            playback_last_event: diagnostics.last_event,
                            echo_dropped: false,
                            echo_dropped_total: shared.echo_dropped_total(),
                            timeline: transcript_timeline,
                            completed_at: Instant::now(),
                        });
                        *shared
                            .last_transcript_only
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = Some(new_text);
                    }
                    continue;
                }
                turn.user_text = text;
                tracing::debug!(
                    target: "realtime_pump",
                    responding = turn.responding,
                    text = %turn.user_text,
                    "用户转写定稿（非回声，进入对话流）"
                );
                emit(&PumpLive::User(turn.user_text.clone()));
                if !config.auto_respond && !turn.responding {
                    // 会议助手点名门控：服务端 create_response=false，应答由泵决定。
                    // 响应在途时（点名/强制回答的文本轮已发出）不再重复触发，
                    // 也不得把在途响应替换成仅转写轮——该 completed 只是同一句
                    // 话的转写定稿，晚于 ForceRespond 到达。
                    let mentioned = crate::services::sessions::meeting_assistant_was_mentioned(
                        &turn.user_text,
                        &config.role_name,
                    );
                    if mentioned {
                        shared
                            .last_transcript_only
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .take();
                        let prompt = turn.user_text.clone();
                        turn.answer_item_id = Some(item_id);
                        turn.respond_requested_at = Some(Instant::now());
                        turn.respond_attempts = 0;
                        session.send(ActorCommand::RespondText(prompt));
                        turn.responding = true;
                    } else {
                        // 未点名：立即成轮（create_response=false 不会有 response.done），
                        // 文本留给热键强制回答。
                        *shared
                            .last_transcript_only
                            .lock()
                            .unwrap_or_else(|p| p.into_inner()) = Some(turn.user_text.clone());
                        turn.transcript_only = true;
                        let diagnostics = playback.diagnostics();
                        let completed = CompletedTurn {
                            user_text: std::mem::take(&mut turn.user_text),
                            assistant_text: String::new(),
                            first_audio_ms: None,
                            interrupted: false,
                            transcript_only: true,
                            held: false,
                            forced: false,
                            audio_bytes: 0,
                            playback_write_failed: false,
                            playback_alive: playback.is_alive(),
                            audio_delta_count: 0,
                            audio_delta_max_gap_ms: None,
                            audio_delta_gaps_over_150_ms: 0,
                            audio_delta_gaps_over_500_ms: 0,
                            aec_residual_correlation: None,
                            aec_residual_max: None,
                            aec_residual_over_threshold: 0,
                            playback_mode,
                            playback_generation: Some(diagnostics.generation),
                            playback_restarts: Some(diagnostics.restarts),
                            playback_restarted_during_turn: false,
                            playback_last_event: diagnostics.last_event,
                            echo_dropped: false,
                            echo_dropped_total: shared.echo_dropped_total(),
                            response_failed: false,
                            timeline: turn.timeline.clone(),
                            completed_at: Instant::now(),
                        };
                        shared.push_turn(completed);
                        turn = TurnAccumulator::new();
                    }
                }
            }
            Some(ActorEvent::AssistantTextDelta(delta)) => {
                turn.assistant_text.push_str(&delta);
                emit(&PumpLive::Assistant(turn.assistant_text.clone()));
            }
            Some(ActorEvent::AssistantAudioDelta(pcm)) => {
                let now = Instant::now();
                aec_monitor.observe_playback(&pcm);
                if turn.playback_generation_at_start.is_none() {
                    let diagnostics = playback.diagnostics();
                    turn.playback_generation_at_start = Some(diagnostics.generation);
                    turn.playback_restarts_at_start = Some(diagnostics.restarts);
                }
                turn.first_audio_at.get_or_insert(now);
                turn.timeline
                    .first_audio_ms
                    .get_or_insert_with(|| since_start().unwrap_or(0));
                turn.record_audio_delta_at(now);
                turn.audio_bytes += pcm.len();
                if turn.interrupted {
                    // 打断后的在途 delta：服务端 cancel 生效前仍会到达，全部丢弃。
                } else if turn.response_closed {
                    // response.done 之后的迟到尾包（取消生效前/重复流）：
                    // 无门控直写会造成重复播报，一律丢弃。
                } else if config.hold_playback {
                    if held_audio.len() + pcm.len() <= HELD_CAP_BYTES {
                        held_audio.extend_from_slice(&pcm);
                    }
                } else {
                    if playback_mode == RealtimePlaybackMode::WebAudio {
                        emit(&PumpLive::AssistantAudio(pcm.clone()));
                        // WebView AEC 负责消除同上下文播放参考；本地 suppress
                        // 只用于保持 BargeInMonitor 活跃，不关闭上行。
                        if let Some(suppress) = config.suppress_echo.as_ref() {
                            suppress(Duration::from_millis(
                                (pcm.len() as u64 * 1000) / (24_000 * 2) + 700,
                            ));
                        }
                        if !turn.played_audio {
                            emit(&PumpLive::Speaking(true));
                        }
                    } else {
                        if playback.write(&pcm).is_err() {
                            turn.playback_write_failed = true;
                        }
                        // 流式续窗：barge 监听在播报期间只认真人插话（24k*2B PCM）。
                        if let Some(suppress) = config.suppress_echo.as_ref() {
                            suppress(Duration::from_millis(
                                (pcm.len() as u64 * 1000) / (24_000 * 2) + 700,
                            ));
                        }
                        if !turn.played_audio {
                            emit(&PumpLive::Speaking(true));
                            // Native 专属：关门即作废遗留的播净回执（StreamPlayback
                            // 初始标记为「已播净」，上轮回执会残留，不消费会被陈旧
                            // 回执立刻重开闸门）。WebAudio 不关门，无此语义。
                            let _ = playback.take_drained();
                            uplink_reopen_at = None;
                            uplink_open = false;
                            gate_closed_at.get_or_insert_with(Instant::now);
                        }
                    }
                    turn.played_audio = true;
                    shared.speaking.store(true, Ordering::SeqCst);
                    if playback_mode == RealtimePlaybackMode::Native {
                        gate_drain_deadline =
                            Some(gate_drain_deadline_from_bytes(Instant::now(), pcm.len()));
                    }
                }
            }
            Some(ActorEvent::ResponseDone { cancelled }) => {
                turn.timeline.response_done_ms = since_start();
                if cancelled {
                    turn.interrupted = true;
                }
                let was_playing = turn.played_audio;
                // 定稿级回声兜底：转写 completed 缺席（部分方言只有 delta）或
                // 判定时机更晚时，这是落库前最后一道闸。比对只针对此前轮次的
                // 播报——本条 assistant_text 还未入缓存，模型复述用户原话不会误伤。
                let user_echo = !turn.echo_dropped
                    && !turn.user_text.is_empty()
                    && is_current_or_recent_echo(
                        &turn.user_text,
                        recent_assistant.make_contiguous(),
                        &current_turn_echo(&turn, post_playback_echo_until, playback_mode),
                    );
                tracing::debug!(
                    target: "realtime_pump",
                    cancelled,
                    was_playing,
                    responding = turn.responding,
                    user_echo,
                    echo_dropped = turn.echo_dropped,
                    interrupted = turn.interrupted,
                    user_text = %turn.user_text,
                    assistant_chars = turn.assistant_text.chars().count(),
                    speech_started_ms = turn.timeline.speech_started_ms,
                    first_audio_ms = turn.timeline.first_audio_ms,
                    response_done_ms = turn.timeline.response_done_ms,
                    aec_residual_correlation = ?turn.aec_residual_correlation,
                    aec_residual_max = ?turn.aec_residual_max,
                    ?playback_mode,
                    "响应结束（轮次定稿诊断）"
                );
                // 先截获本轮播报文本：下面 mem::take 进 CompletedTurn 后就取不到了。
                let assistant_spoken = turn.assistant_text.clone();
                let playback_diagnostics = playback.diagnostics();
                let playback_restarts = playback_diagnostics.restarts;
                let playback_restarted_during_turn = turn
                    .playback_restarts_at_start
                    .is_some_and(|started| playback_restarts > started);
                if turn.has_pending_content() && !turn.echo_dropped && !user_echo {
                    let completed = CompletedTurn {
                        user_text: std::mem::take(&mut turn.user_text),
                        assistant_text: std::mem::take(&mut turn.assistant_text),
                        first_audio_ms: turn.first_audio_ms(),
                        interrupted: turn.interrupted,
                        transcript_only: turn.transcript_only,
                        held: config.hold_playback && !held_audio.is_empty(),
                        forced: turn.forced,
                        audio_bytes: turn.audio_bytes,
                        playback_write_failed: turn.playback_write_failed,
                        playback_alive: playback.is_alive(),
                        audio_delta_count: turn.audio_delta_count,
                        audio_delta_max_gap_ms: turn.audio_delta_max_gap_ms,
                        audio_delta_gaps_over_150_ms: turn.audio_delta_gaps_over_150_ms,
                        audio_delta_gaps_over_500_ms: turn.audio_delta_gaps_over_500_ms,
                        aec_residual_correlation: turn.aec_residual_correlation,
                        aec_residual_max: turn.aec_residual_max,
                        aec_residual_over_threshold: turn.aec_residual_over_threshold,
                        playback_mode,
                        playback_generation: turn
                            .playback_generation_at_start
                            .or(Some(playback_diagnostics.generation)),
                        playback_restarts: Some(playback_restarts),
                        playback_restarted_during_turn,
                        playback_last_event: playback_diagnostics.last_event,
                        echo_dropped: turn.echo_dropped,
                        echo_dropped_total: shared.echo_dropped_total(),
                        response_failed: false,
                        timeline: turn.timeline.clone(),
                        completed_at: Instant::now(),
                    };
                    shared.push_turn(completed);
                }
                // 本轮播过的内容进回声比对缓存——无论该轮是否被丢弃，
                // 声音已经出了扬声器，其回声都可能回来。
                if !assistant_spoken.is_empty() {
                    recent_assistant.push_back(assistant_spoken);
                    while recent_assistant.len() > ECHO_RECENT_TURNS {
                        recent_assistant.pop_front();
                    }
                }
                shared.speaking.store(false, Ordering::SeqCst);
                emit(&PumpLive::Speaking(false));
                turn = TurnAccumulator::new();
                // done 之后、下一个 created 之前：迟到尾包 delta 全部丢弃。
                turn.response_closed = true;
                if config.hold_playback {
                    // 候选模式音频仍被扣住未出声，无排水语义。
                } else if playback_mode == RealtimePlaybackMode::WebAudio {
                    // 前端 500ms jitter buffer 会让最后一个 delta 晚于 done 出声。
                    // 上行保持全双工开放；文本短片段兜底窗口覆盖尾音。
                    post_playback_echo_until = Some(Instant::now() + Duration::from_millis(6_000));
                } else if was_playing {
                    // Rust 侧写入完成 ≠ 出声结束（sidecar 有播放缓冲）：
                    // 发 drain 并保持闸门关闭，等「已播净」回执后由循环头开门。
                    let _ = playback.take_drained();
                    let _ = playback.drain();
                    gate_drain_deadline = Some(Instant::now() + GATE_DRAIN_FALLBACK_TAIL);
                } else {
                    uplink_open = true;
                    gate_closed_at = None;
                }
                // 当前回答结束：发起回答期间排队的新点名（若有）。
                if let Some(text) = pending_mention.take() {
                    tracing::debug!(
                        target: "realtime_pump",
                        text = %text,
                        "回答结束：发起排队中的点名应答"
                    );
                    turn.user_text = text.clone();
                    turn.responding = true;
                    turn.echo_dropped = false;
                    turn.respond_requested_at = Some(Instant::now());
                    turn.respond_attempts = 0;
                    emit(&PumpLive::User(text));
                    session.send(ActorCommand::RespondText(turn.user_text.clone()));
                }
            }
            None => {}
        }
        // 4) 回答开始看门狗：RespondText 已发出但迟迟没有 response.created
        // （实测 Qwen-Omni 偶发吞掉请求后沉默至断链）。第一次超时清服务端
        // 输入缓冲并重发一次；再超时放弃本轮——落 response_failed 轮、文本
        // 还给「让助手回答」候选，绝不让泵永久卡在「回答中」。
        if let Some(requested_at) = turn.respond_requested_at
            && requested_at.elapsed() >= respond_start_timeout
        {
            if turn.respond_attempts == 0 {
                tracing::warn!(
                    target: "realtime_pump",
                    user_chars = turn.user_text.chars().count(),
                    responding = turn.responding,
                    waited_ms = requested_at.elapsed().as_millis() as u64,
                    "回答开始超时：清服务端输入缓冲后重发一次应答请求"
                );
                session.send(ActorCommand::ClearInputBuffer);
                session.send(ActorCommand::RespondText(turn.user_text.clone()));
                turn.respond_attempts += 1;
                turn.respond_requested_at = Some(Instant::now());
            } else {
                tracing::warn!(
                    target: "realtime_pump",
                    user_chars = turn.user_text.chars().count(),
                    forced = turn.forced,
                    waited_ms = requested_at.elapsed().as_millis() as u64,
                    "回答开始二次超时：放弃本轮，保留用户文本供再次追答"
                );
                let user_text = std::mem::take(&mut turn.user_text);
                let diagnostics = playback.diagnostics();
                shared.push_turn(CompletedTurn {
                    user_text: user_text.clone(),
                    assistant_text: std::mem::take(&mut turn.assistant_text),
                    first_audio_ms: None,
                    interrupted: false,
                    transcript_only: false,
                    held: false,
                    forced: turn.forced,
                    response_failed: true,
                    audio_bytes: 0,
                    playback_write_failed: turn.playback_write_failed,
                    playback_alive: playback.is_alive(),
                    audio_delta_count: 0,
                    audio_delta_max_gap_ms: None,
                    audio_delta_gaps_over_150_ms: 0,
                    audio_delta_gaps_over_500_ms: 0,
                    aec_residual_correlation: None,
                    aec_residual_max: None,
                    aec_residual_over_threshold: 0,
                    playback_mode,
                    playback_generation: Some(diagnostics.generation),
                    playback_restarts: Some(diagnostics.restarts),
                    playback_restarted_during_turn: false,
                    playback_last_event: diagnostics.last_event,
                    echo_dropped: false,
                    echo_dropped_total: shared.echo_dropped_total(),
                    timeline: std::mem::take(&mut turn.timeline),
                    completed_at: Instant::now(),
                });
                // 响应从未真正开始（已开始则看门狗早已解除）：掐掉服务端可能
                // 仍在途的幽灵响应，防止它晚到后以空用户文本成轮播放。
                session.send(ActorCommand::CancelResponse);
                turn = TurnAccumulator::new();
                shared.speaking.store(false, Ordering::SeqCst);
                emit(&PumpLive::Speaking(false));
                // 文本还给「让助手回答」候选：用户可直接再点一次按钮。
                *shared
                    .last_transcript_only
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = Some(user_text);
            }
        }
    }
}

#[cfg(test)]
mod pump_tests {
    use super::*;
    use crate::providers::ProviderEndpoint;
    use crate::providers::realtime_dialect;
    use crate::providers::realtime_session::{
        RealtimeCapabilityProfile, RealtimeSession, RealtimeSessionConfig,
    };
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;
    use tungstenite::{Message, WebSocket};

    #[derive(Default)]
    struct MemSink {
        written: Mutex<Vec<u8>>,
        clear_count: std::sync::atomic::AtomicUsize,
    }

    impl MemSink {
        fn written(&self) -> Vec<u8> {
            self.written
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
        }
        fn clears(&self) -> usize {
            self.clear_count.load(Ordering::SeqCst)
        }
    }

    impl PlaybackStream for MemSink {
        fn write(&self, pcm: &[u8]) -> Result<(), &'static str> {
            self.written
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .extend_from_slice(pcm);
            Ok(())
        }
        fn clear(&self) -> Result<(), &'static str> {
            self.clear_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn accept_session(listener: &TcpListener) -> (serde_json::Value, WebSocket<TcpStream>) {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let update: serde_json::Value =
            serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
        ws.send(Message::Text(r#"{"type":"session.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(r#"{"type":"session.updated"}"#.into()))
            .unwrap();
        (update, ws)
    }

    fn read_frame(ws: &mut WebSocket<TcpStream>) -> serde_json::Value {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!remaining.is_zero(), "mock timeout");
            ws.get_mut().set_read_timeout(Some(remaining)).unwrap();
            match ws.read().unwrap() {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
                Message::Binary(_) => continue,
                Message::Close(_) => panic!("unexpected close"),
            }
        }
    }

    /// 短窗非阻塞探测：断言窗口内没有新帧（回声闸门关闭期间上行应静默）。
    fn assert_no_frame(ws: &mut WebSocket<TcpStream>, window: Duration) {
        let deadline = std::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return;
            }
            ws.get_mut().set_read_timeout(Some(remaining)).unwrap();
            match ws.read() {
                Ok(Message::Text(_)) => panic!("unexpected frame while gate closed"),
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Ok(Message::Binary(_)) => continue,
                Ok(Message::Close(_)) => panic!("unexpected close"),
                Err(tungstenite::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => panic!("read error: {error}"),
            }
        }
    }

    /// 消费闸门重开瞬间的 input_audio_buffer.clear 帧（回声残渣清理）。
    fn expect_gate_reopen_clear(ws: &mut WebSocket<TcpStream>) {
        let frame = read_frame(ws);
        assert_eq!(
            frame["type"], "input_audio_buffer.clear",
            "gate reopen must clear the server input buffer first"
        );
    }

    /// 短窗读取一帧；窗口内无帧返回 None（用于竞态友好的探针重试）。
    fn try_read_frame(
        ws: &mut WebSocket<TcpStream>,
        window: Duration,
    ) -> Option<serde_json::Value> {
        let deadline = std::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            ws.get_mut().set_read_timeout(Some(remaining)).unwrap();
            match ws.read() {
                Ok(Message::Text(text)) => return serde_json::from_str(&text).ok(),
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Ok(Message::Binary(_)) => continue,
                Ok(Message::Close(_)) => panic!("unexpected close"),
                Err(tungstenite::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut =>
                {
                    return None;
                }
                Err(error) => panic!("read error: {error}"),
            }
        }
    }

    struct PumpFixture {
        pump: RealtimePump,
        tap_tx: mpsc::SyncSender<Vec<u8>>,
        sink: Arc<MemSink>,
    }

    fn spawn_pump(
        listener_port: u16,
        auto_respond: bool,
        role_name: &str,
        hold_playback: bool,
    ) -> PumpFixture {
        spawn_pump_with_live(listener_port, auto_respond, role_name, hold_playback, None)
    }

    fn spawn_pump_with_live(
        listener_port: u16,
        auto_respond: bool,
        role_name: &str,
        hold_playback: bool,
        live_sink: Option<Box<dyn Fn(PumpLive) + Send + Sync>>,
    ) -> PumpFixture {
        spawn_pump_with(
            listener_port,
            auto_respond,
            role_name,
            hold_playback,
            RESPOND_START_TIMEOUT,
            live_sink,
        )
    }

    /// 看门狗测试用：可指定回答开始超时（调短到几百毫秒，避免测试变慢）。
    fn spawn_pump_with(
        listener_port: u16,
        auto_respond: bool,
        role_name: &str,
        hold_playback: bool,
        respond_start_timeout: Duration,
        live_sink: Option<Box<dyn Fn(PumpLive) + Send + Sync>>,
    ) -> PumpFixture {
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{listener_port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond,
                enable_search: false,
            },
        )
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink: Arc<MemSink> = Arc::new(MemSink::default());
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond,
                role_name: role_name.into(),
                hold_playback,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout,
            },
            live_sink,
        );
        PumpFixture { pump, tap_tx, sink }
    }

    fn wait_for(predicate: impl Fn() -> bool, what: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if predicate() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("condition not met in time: {what}");
    }

    /// 全链路：麦克风 tap 上行 → 转写 → 音频 delta 流式写播放 → 成轮。
    #[test]
    fn streams_audio_uplink_and_response_delta_into_playback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        // 采集环 48kHz：9600B（300ms）→ append_audio 降采样到方言 16k = 3200B 一帧。
        fixture.tap_tx.send(vec![1u8; 9600]).unwrap();
        let frame = read_frame(&mut ws);
        assert_eq!(frame["type"], "input_audio_buffer.append");
        assert_eq!(
            STANDARD.decode(frame["audio"].as_str().unwrap()).unwrap(),
            vec![1u8; 3200]
        );

        for event in [
            r#"{"type":"input_audio_buffer.speech_started"}"#.to_string(),
            r#"{"type":"input_audio_buffer.speech_stopped"}"#.to_string(),
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"十一点嘛"}"#.to_string(),
            r#"{"type":"response.created"}"#.to_string(),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            ),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([3u8, 4])
            ),
            r#"{"type":"response.audio_transcript.delta","delta":"十一点了"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }

        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "turn completion",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert_eq!(turn.user_text, "十一点嘛");
        assert_eq!(turn.assistant_text, "十一点了");
        assert!(turn.first_audio_ms.is_some(), "首响必须计时");
        assert!(!turn.interrupted && !turn.transcript_only && !turn.held);
        assert_eq!(fixture.sink.written(), [1u8, 2, 3, 4]);
    }

    /// 打断：播报中 speech_started → 清空播放 + response.cancel + 在途 delta 丢弃。
    #[test]
    fn barge_in_clears_playback_and_cancels_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([9u8; 800])
            )
            .into(),
        ))
        .unwrap();
        // 播放已在出声时用户开口 → 客户端清空 + 服务端取消。
        ws.send(Message::Text(
            r#"{"type":"input_audio_buffer.speech_started"}"#.into(),
        ))
        .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([7u8; 800])
            )
            .into(),
        ))
        .unwrap();
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"cancelled"}}"#.into(),
        ))
        .unwrap();

        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "interrupted turn",
        );
        assert_eq!(fixture.sink.clears(), 1, "必须清空播放");
        // 打断时序：speech_started → response.cancel（打断瞬间）；关门后 MemSink
        // 视为已播净会立即重开并 clear。两帧都到达，顺序以实际为准。
        let first = read_frame(&mut ws)["type"].clone();
        let second = read_frame(&mut ws)["type"].clone();
        assert!(
            (first == "response.cancel" && second == "input_audio_buffer.clear")
                || (first == "input_audio_buffer.clear" && second == "response.cancel"),
            "expected cancel + clear in either order, got {first} then {second}"
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert!(turn.interrupted);
        assert_eq!(
            fixture.sink.written(),
            [9u8; 800],
            "打断后的在途 delta 必须丢弃"
        );
    }

    /// 会议助手点名门控：未点名只落转写（立即成轮），点名才触发应答。
    #[test]
    fn mention_gating_routes_transcript_only_or_respond() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, false, "会议助手", false);
        let (_, mut ws) = accept_session(&listener);

        // 未点名 → 只成转写轮，服务端不应收到 response.create。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"这个方案大家怎么看"}"#
                .into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "transcript-only turn",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert!(turn.transcript_only);
        assert_eq!(turn.user_text, "这个方案大家怎么看");

        // 点名 → RespondText（item.create + response.create）。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"会议助手，帮我记一下时间"}"#
                .into(),
        ))
        .unwrap();
        let item = read_frame(&mut ws);
        assert_eq!(item["type"], "conversation.item.create");
        assert_eq!(
            item["item"]["content"][0]["text"],
            "会议助手，帮我记一下时间"
        );
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
    }

    /// 强制回答：仅转写轮之后收到 ForceRespond → 以该句文本发起应答，
    /// 完成轮 assistant_text 非空且 forced=true；候选素材随之清空。
    #[test]
    fn forced_respond_answers_latest_transcript_only_turn() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, false, "会议助手", false);
        let (_, mut ws) = accept_session(&listener);

        // 未点名发言：只落转写轮。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"你听得见我说话吗"}"#
                .into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "transcript-only turn",
        );
        let transcript_turn = fixture.pump.shared.take_completed().unwrap();
        assert!(transcript_turn.transcript_only && !transcript_turn.forced);
        assert!(
            fixture
                .pump
                .shared
                .last_transcript_only
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some(),
            "transcript-only text must be kept as force-respond candidate"
        );

        // 热键强制回答 → item.create（原文）+ response.create。
        fixture.pump.send(PumpCommand::ForceRespond);
        let item = read_frame(&mut ws);
        assert_eq!(item["type"], "conversation.item.create");
        assert_eq!(item["item"]["content"][0]["text"], "你听得见我说话吗");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");

        // 服务端完成应答：成轮 assistant_text 非空、forced=true。
        for event in [
            r#"{"type":"response.created"}"#.to_string(),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            ),
            r#"{"type":"response.audio_transcript.delta","delta":"听得见"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "forced answer turn",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert_eq!(turn.user_text, "你听得见我说话吗");
        assert_eq!(turn.assistant_text, "听得见");
        assert!(turn.forced && !turn.transcript_only);
        assert!(
            fixture
                .pump
                .shared
                .last_transcript_only
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_none(),
            "candidate must be consumed once the forced answer starts"
        );
        drop(fixture.pump);
    }

    /// 强制回答去重：回答在途时收到 ForceRespond 必须忽略——不得向服务端
    /// 重复发送 item.create/response.create。
    #[test]
    fn forced_respond_is_ignored_while_responding() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, false, "会议助手", false);
        let (_, mut ws) = accept_session(&listener);

        // 点名触发正常回答。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"会议助手，你好"}"#
                .into(),
        ))
        .unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");

        // 响应在途（created 已到）：ForceRespond 必须被忽略。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        fixture.pump.send(PumpCommand::ForceRespond);
        assert_no_frame(&mut ws, Duration::from_millis(400));

        // 正常完成：forced 不得误标。
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "mentioned turn",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert!(!turn.forced);
        drop(fixture.pump);
    }

    /// 回答开始看门狗：点名后服务端对 RespondText 不回任何响应 → 第一次超时
    /// 清服务端输入缓冲并重发一次；再超时放弃本轮（response_failed 成轮、
    /// 用户文本回到「让助手回答」候选），之后 ForceRespond 能重新发起请求。
    #[test]
    fn respond_timeout_retries_then_marks_turn_failed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump_with(
            port,
            false,
            "会议助手",
            false,
            Duration::from_millis(300),
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        // 点名 → item.create + response.create 发出，服务端全程沉默。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"会议助手，帮我记一下时间"}"#
                .into(),
        ))
        .unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");

        // 第一次超时：清服务端输入缓冲 + 重发同一次应答请求。
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.clear");
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");

        // 第二次超时：放弃（掐掉幽灵响应）→ response_failed 完成轮 + 候选恢复。
        assert_eq!(read_frame(&mut ws)["type"], "response.cancel");
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "response-failed turn",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert!(
            turn.response_failed,
            "看门狗放弃的轮必须标记 response_failed"
        );
        assert!(!turn.transcript_only);
        assert!(turn.assistant_text.is_empty());
        assert_eq!(turn.user_text, "会议助手，帮我记一下时间");
        assert_eq!(
            fixture
                .pump
                .shared
                .last_transcript_only
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_deref(),
            Some("会议助手，帮我记一下时间"),
            "失败文本必须回到「让助手回答」候选"
        );

        // 之后 ForceRespond 能再次发起请求（泵没有卡在「回答中」）。
        fixture.pump.send(PumpCommand::ForceRespond);
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
        drop(fixture.pump);
    }

    /// 看门狗放弃后 responding 不再永久为 true：下一句未点名的话能正常
    /// 生成仅转写轮（点名的回答请求卡死不再吞掉后续发言）。
    #[test]
    fn respond_timeout_recovers_gating_for_next_utterance() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump_with(
            port,
            false,
            "会议助手",
            false,
            Duration::from_millis(300),
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        // 点名触发回答，服务端沉默到看门狗放弃。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"会议助手，你好"}"#
                .into(),
        ))
        .unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.clear");
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.cancel");
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "response-failed turn",
        );
        assert!(
            fixture
                .pump
                .shared
                .take_completed()
                .unwrap()
                .response_failed
        );

        // 下一句未点名发言：正常生成仅转写轮。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"这个方案大家怎么看"}"#
                .into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "transcript-only turn after watchdog give-up",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert!(turn.transcript_only);
        assert_eq!(turn.user_text, "这个方案大家怎么看");
        drop(fixture.pump);
    }

    /// 门控模式：回答还没出声（未收到音频）时服务端 VAD 触发 speech_started，
    /// 不得取消回答；回答照常完成。auto_respond 路径由
    /// `barge_in_clears_playback_and_cancels_response` 覆盖，行为不变。
    #[test]
    fn gated_speech_started_before_audio_keeps_response_alive() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, false, "会议助手", false);
        let (_, mut ws) = accept_session(&listener);

        // 点名 → 应答请求发出（created 尚未到达，更没有音频）。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"会议助手，今天几号"}"#
                .into(),
        ))
        .unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");

        // 别人插话/环境噪声触发服务端 VAD：门控模式不得 response.cancel。
        ws.send(Message::Text(
            r#"{"type":"input_audio_buffer.speech_started"}"#.into(),
        ))
        .unwrap();
        assert_no_frame(&mut ws, Duration::from_millis(400));

        // 回答照常完成，未被取消。
        for event in [
            r#"{"type":"response.created"}"#.to_string(),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            ),
            r#"{"type":"response.audio_transcript.delta","delta":"今天是十月一号"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "surviving answer turn",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert!(!turn.interrupted, "回答不得被出声前的 VAD 打断标记");
        assert_eq!(turn.assistant_text, "今天是十月一号");
        assert_eq!(turn.user_text, "会议助手，今天几号");
        drop(fixture.pump);
    }

    /// 等待回答期间另一人发言（不同 item_id、未点名）：立即成仅转写轮，
    /// 不覆盖正在回答的用户文本；回答完成后原句照常落库。
    #[test]
    fn concurrent_utterance_becomes_transcript_only_while_responding() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, false, "会议助手", false);
        let (_, mut ws) = accept_session(&listener);

        // 点名 → 应答在途。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"会议助手，今天几号"}"#
                .into(),
        ))
        .unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();

        // 另一人说了一句未点名的话（不同 item_id）：立即成仅转写轮。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"顺便说一下下午三点开会"}"#
                .into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "concurrent transcript-only turn",
        );
        let transcript_turn = fixture.pump.shared.take_completed().unwrap();
        assert!(transcript_turn.transcript_only);
        assert_eq!(transcript_turn.user_text, "顺便说一下下午三点开会");
        assert_eq!(
            fixture
                .pump
                .shared
                .last_transcript_only
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_deref(),
            Some("顺便说一下下午三点开会"),
            "并发发言必须成为可追答素材"
        );

        // 回答完成：user_text 保持原句，未被并发发言覆盖。
        for event in [
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            ),
            r#"{"type":"response.audio_transcript.delta","delta":"今天是十月一号"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(|| fixture.pump.shared.completed_count() == 1, "answer turn");
        let answer_turn = fixture.pump.shared.take_completed().unwrap();
        assert!(!answer_turn.transcript_only);
        assert_eq!(answer_turn.user_text, "会议助手，今天几号");
        assert_eq!(answer_turn.assistant_text, "今天是十月一号");
        drop(fixture.pump);
    }

    /// 等待回答期间点名（不同 item_id）：先成仅转写轮并排队，当前回答结束
    /// 后自动发起排队中的点名应答，无需用户再点按钮。
    #[test]
    fn mention_during_response_is_queued_until_done() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, false, "会议助手", false);
        let (_, mut ws) = accept_session(&listener);

        // 第一句点名 → 应答在途。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"会议助手，今天几号"}"#
                .into(),
        ))
        .unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();

        // 回答进行中又一句点名：立即成仅转写轮，请求排队不并发。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"b","transcript":"会议助手，再帮我记一下时间"}"#
                .into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "queued mention transcript-only turn",
        );
        let transcript_turn = fixture.pump.shared.take_completed().unwrap();
        assert!(transcript_turn.transcript_only);
        assert_eq!(transcript_turn.user_text, "会议助手，再帮我记一下时间");
        assert_no_frame(&mut ws, Duration::from_millis(300));

        // 当前回答结束：第一轮回答先成轮，排队中的点名自动发起。
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "first answer turn",
        );
        let first_answer = fixture.pump.shared.take_completed().unwrap();
        assert_eq!(first_answer.user_text, "会议助手，今天几号");
        assert_eq!(read_frame(&mut ws)["type"], "conversation.item.create");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");

        // 排队应答正常完成成轮。
        for event in [
            r#"{"type":"response.created"}"#.to_string(),
            r#"{"type":"response.audio_transcript.delta","delta":"记好了"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "queued answer turn",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert!(!turn.transcript_only);
        assert_eq!(turn.user_text, "会议助手，再帮我记一下时间");
        assert_eq!(turn.assistant_text, "记好了");
        drop(fixture.pump);
    }

    /// 真实供应商强制回答冒烟（默认 #[ignore]，消耗配额）：以 create_response=false
    /// 连真实端到端线路，上行人声触发仅转写轮，再 ForceRespond 验证
    /// conversation.item.create + response.create 被服务端接受并返回音频回答——
    /// 这是「让助手回答」链路在 Qwen-Omni 上的关键未验证段。
    /// REALTIME_SMOKE_CONFIG=<导出的 config json>
    /// REALTIME_SMOKE_ROUTE=<e2e 线路 id>
    /// REALTIME_SMOKE_WAV=<16k mono wav（必须是人声，静音不会触发转写）>
    /// cargo test --lib live_forced_respond_transcript_only -- --ignored --nocapture
    #[test]
    #[ignore = "Uses the saved Windows credential and consumes provider quota; set REALTIME_SMOKE_* explicitly"]
    #[cfg(windows)]
    fn live_forced_respond_transcript_only() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;

        let path = std::env::var("REALTIME_SMOKE_CONFIG").expect("explicit config path required");
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let route_id = std::env::var("REALTIME_SMOKE_ROUTE").expect("route id required");
        let route = config["speech"]["voiceRoutes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"].as_str() == Some(route_id.as_str()))
            .expect("selected route must exist")
            .clone();
        assert_eq!(
            route["mode"].as_str(),
            Some("e2e"),
            "forced-respond live smoke only applies to the e2e route"
        );
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

        // 读取 wav data 块作为 16k mono PCM（必须是人声，例如「你听得见我说话吗」）。
        let wav_path = std::env::var("REALTIME_SMOKE_WAV").expect("speech wav required");
        let bytes = std::fs::read(&wav_path).unwrap();
        let pcm16k = {
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
        // AppendAudio 契约是采集环 48kHz；先补齐到采集率让 Actor 降回 16k，复刻真实链路。
        let pcm = crate::audio::pcm::resample_pcm16_mono(
            &pcm16k,
            16_000,
            crate::audio::pcm::CAPTURE_SAMPLE_RATE,
        );
        println!("live forced: pcm_bytes={} (16k source)", pcm.len());

        let session = RealtimeSession::start(RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: provider["id"].as_str().unwrap().to_owned(),
                base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
            },
            credential: Some(credential.as_str().to_owned()),
            model_id: route["e2eModelId"].as_str().unwrap_or("").to_owned(),
            voice,
            instructions: "你是会议助手。普通讨论只听不答；被点名或被要求回答时用中文简短回答。"
                .into(),
            history: vec![],
            auto_respond: false, // 会议助手点名门控：create_response=false
            enable_search: false,
        })
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink: Arc<MemSink> = Arc::new(MemSink::default());
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: false,
                role_name: "会议助手".into(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );

        let started = Instant::now();
        // 上行语音（100ms/帧按墙钟节奏）→ 静音尾窗 → CommitTurn（采集侧分段检测的应用同款时序）。
        // 默认 commit 后停止推流（已验证可用）；REALTIME_SMOKE_TRAILING=1 复刻
        // 应用真实状态——采集不停、静音持续上行，用于复现 response.create 沉默。
        let trailing = std::env::var("REALTIME_SMOKE_TRAILING").is_ok();
        let mut sent = 0usize;
        let mut silence_frames = 0usize;
        let mut last_push = Instant::now();
        let mut committed = false;
        let transcript_deadline = Instant::now() + Duration::from_secs(60);
        let mut transcript_turn = None;
        while Instant::now() < transcript_deadline {
            if (trailing || !committed) && last_push.elapsed() >= Duration::from_millis(100) {
                let chunk = if sent < pcm.len() {
                    let end = (sent + 3200).min(pcm.len());
                    pcm[sent..end].to_vec()
                } else {
                    silence_frames += 1;
                    // 语音送完后补 700ms 静音再提交一次（真实分段器在句尾静音后判停）。
                    if silence_frames == 7 {
                        pump.send(PumpCommand::CommitTurn);
                        committed = true;
                        println!("live forced: commit at {:?}", started.elapsed());
                        continue;
                    }
                    vec![0u8; 3200]
                };
                sent += chunk.len();
                let _ = tap_tx.try_send(chunk);
                last_push = Instant::now();
            }
            if let Some(completed) = pump.shared.take_completed() {
                transcript_turn = Some(completed);
                break;
            }
        }
        let transcript_turn = transcript_turn
            .unwrap_or_else(|| panic!("live forced: transcript-only turn never completed"));
        println!(
            "live forced: transcript_only turn at {:?} user=\"{}\"",
            started.elapsed(),
            transcript_turn.user_text
        );
        assert!(
            transcript_turn.transcript_only && !transcript_turn.forced,
            "unmentioned speech must complete as transcript-only"
        );

        // 强制回答：期望 item.create + response.create 被真实服务端接受并成轮。
        pump.send(PumpCommand::ForceRespond);
        println!("live forced: ForceRespond sent at {:?}", started.elapsed());
        let answer_deadline = Instant::now() + Duration::from_secs(90);
        let answer = loop {
            if Instant::now() > answer_deadline {
                panic!("live forced: forced answer never completed");
            }
            if let Some(completed) = pump.shared.take_completed() {
                break completed;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        println!(
            "live forced: answer at {:?} user=\"{}\" assistant=\"{}\" audio_bytes={} forced={} transcript_only={}",
            started.elapsed(),
            answer.user_text,
            answer.assistant_text,
            answer.audio_bytes,
            answer.forced,
            answer.transcript_only
        );
        assert!(answer.forced, "turn must be marked forced");
        assert!(!answer.transcript_only);
        assert!(!answer.assistant_text.is_empty(), "assistant text required");
        assert!(answer.audio_bytes > 0, "assistant audio required");
        drop(pump);
    }

    /// 点名应答 + 采集持续推流（应用真实时序）冒烟（默认 #[ignore]）：commit 后
    /// 采集不停、静音持续上行（真实麦克风/会议桥就是这么跑的），验证点名触发的
    /// RespondText 在「服务端缓冲有未提交尾流」时是否仍能拿到回答。
    /// REALTIME_SMOKE_CONFIG / REALTIME_SMOKE_ROUTE / REALTIME_SMOKE_WAV 同上。
    #[test]
    #[ignore = "Uses the saved Windows credential and consumes provider quota; set REALTIME_SMOKE_* explicitly"]
    #[cfg(windows)]
    fn live_mention_respond_with_trailing_capture() {
        use crate::secrets::{SecretService, WindowsSecretStore};
        use std::sync::Arc;

        let path = std::env::var("REALTIME_SMOKE_CONFIG").expect("explicit config path required");
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
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

        let wav_path = std::env::var("REALTIME_SMOKE_WAV").expect("speech wav required");
        let bytes = std::fs::read(&wav_path).unwrap();
        let pcm16k = {
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
        let pcm = crate::audio::pcm::resample_pcm16_mono(
            &pcm16k,
            16_000,
            crate::audio::pcm::CAPTURE_SAMPLE_RATE,
        );

        let session = RealtimeSession::start(RealtimeSessionConfig {
            endpoint: ProviderEndpoint {
                provider_id: provider["id"].as_str().unwrap().to_owned(),
                base_url: provider["baseUrl"].as_str().unwrap().to_owned(),
            },
            credential: Some(credential.as_str().to_owned()),
            model_id: route["e2eModelId"].as_str().unwrap_or("").to_owned(),
            voice: route["voiceId"].as_str().unwrap_or("").to_owned(),
            instructions: "你是会议助手。普通讨论只听不答；被点名或被要求回答时用中文简短回答。"
                .into(),
            history: vec![],
            auto_respond: false,
            enable_search: false,
        })
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink: Arc<MemSink> = Arc::new(MemSink::default());
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: false,
                role_name: "会议助手".into(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );

        let started = Instant::now();
        // 应用真实时序：语音 → 700ms 静音 → CommitTurn → 采集不停，静音一直上行。
        let mut sent = 0usize;
        let mut silence_frames = 0usize;
        let mut last_push = Instant::now();
        let mut committed = false;
        let deadline = Instant::now() + Duration::from_secs(75);
        let mut turn = None;
        while Instant::now() < deadline {
            if last_push.elapsed() >= Duration::from_millis(100) {
                let chunk = if sent < pcm.len() {
                    let end = (sent + 3200).min(pcm.len());
                    pcm[sent..end].to_vec()
                } else {
                    silence_frames += 1;
                    if silence_frames == 7 && !committed {
                        pump.send(PumpCommand::CommitTurn);
                        committed = true;
                        println!("mention trailing: commit at {:?}", started.elapsed());
                    }
                    vec![0u8; 3200]
                };
                sent += chunk.len();
                let _ = tap_tx.try_send(chunk);
                last_push = Instant::now();
            }
            if let Some(completed) = pump.shared.take_completed() {
                turn = Some(completed);
                break;
            }
        }
        let turn = turn.unwrap_or_else(|| panic!("mention trailing: turn never completed"));
        println!(
            "mention trailing: turn at {:?} user=\"{}\" assistant=\"{}\" audio_bytes={} forced={} transcript_only={}",
            started.elapsed(),
            turn.user_text,
            turn.assistant_text,
            turn.audio_bytes,
            turn.forced,
            turn.transcript_only
        );
        if turn.assistant_text.is_empty() {
            println!(
                "mention trailing: NO ANSWER — RespondText stalled with trailing capture (root cause reproduces for the mention path)"
            );
        } else {
            println!("mention trailing: answered while trailing capture was flowing");
        }
        drop(pump);
    }

    /// 候选模式：delta 扣住不播，确认后放行。
    #[test]
    fn candidate_hold_defers_playback_until_flush() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", true);
        let (_, mut ws) = accept_session(&listener);

        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([5u8; 320])
            )
            .into(),
        ))
        .unwrap();
        std::thread::sleep(Duration::from_millis(120));
        assert!(fixture.sink.written().is_empty(), "候选模式不得直接出声");

        fixture.pump.send(PumpCommand::FlushHeld);
        wait_for(|| fixture.sink.written().len() == 320, "flushed audio");
        assert_eq!(fixture.sink.written(), [5u8; 320]);
    }
    /// 字幕事件：用户转写与助手回复的快照经 live_sink 发出（partial 帧）。
    #[test]
    fn live_sink_receives_transcript_and_reply_snapshots() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (_tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(MemSink::default());
        let live_seen: Arc<Mutex<Vec<PumpLive>>> = Arc::new(Mutex::new(Vec::new()));
        let live_for_sink = Arc::clone(&live_seen);
        let live_for_closure = Arc::clone(&live_for_sink);
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            Some(Box::new(move |event| {
                let live_for_sink = live_for_closure.clone();
                live_for_sink
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(event);
            })),
        );
        let (_, mut ws) = accept_session(&listener);
        for event in [
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"十一点嘛"}"#.to_string(),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            ),
            r#"{"type":"response.audio_transcript.delta","delta":"十一点了"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }

        wait_for(
            || {
                live_seen
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .len()
                    >= 3
            },
            "three live events",
        );
        let seen = live_seen
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(
            seen.iter()
                .any(|event| matches!(event, PumpLive::User(text) if text == "十一点嘛"))
        );
        assert!(
            seen.iter()
                .any(|event| matches!(event, PumpLive::Assistant(text) if text == "十一点了"))
        );
        assert!(
            seen.iter()
                .any(|event| matches!(event, PumpLive::Speaking(true)))
        );
        drop(pump);
    }

    /// Manual 模式（DashScope）下本地 SmartTurn 判定说完 → 泵向服务端
    /// 提交缓冲；auto_respond 时附带 response.create 触发应答。
    #[test]
    fn commit_turn_command_submits_buffer_and_creates_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        fixture.pump.send(PumpCommand::CommitTurn);
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.commit");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
        drop(fixture.pump);
    }

    /// Manual 模式无服务端 speech_started：本地打断命令应清空播放并取消
    /// 在途响应；响应不在途时只静默（不打断计数）。
    #[test]
    fn local_barge_in_clears_playback_and_cancels_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        // 响应在途：response.created → 音频 delta（写播放）。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([7u8, 8])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !fixture.sink.written().is_empty(), "audio played");

        fixture.pump.send(PumpCommand::LocalBargeIn);
        // MemSink 无缓冲即视为已播净：关门后下一圈重开先发 clear（回声残渣
        // 清理），随后才是打断取消。真实设备由播净回执驱动，语义一致。
        expect_gate_reopen_clear(&mut ws);
        assert_eq!(read_frame(&mut ws)["type"], "response.cancel");
        wait_for(|| fixture.sink.clears() >= 1, "playback cleared");
        drop(fixture.pump);
    }

    /// 连接重置冲刷在途轮：response 在途 + 已收转写时触发重连 →
    /// 立即成轮（interrupted=true）落队、speaking 复位；重连后 turn 2 从
    /// 干净状态开始，正常 response.done 成轮（不残留 interrupted 丢音频）。
    #[test]
    fn reconnecting_flushes_in_flight_turn_and_resets_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);

        // turn 1：响应在途 + 用户转写 + 音频已播。
        let (_, mut ws) = accept_session(&listener);
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"第一句"}"#
                .into(),
        ))
        .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !fixture.sink.written().is_empty(), "turn1 audio played");
        // 服务端拆连 → supervise 退避后重连，mock 监听器接第二条连接。
        drop(ws);
        let (_, mut ws_reconnected) = accept_session(&listener);

        // 冲刷轮次落队（finalize 会取走）+ speaking 复位。
        wait_for(
            || fixture.pump.shared.completed_count() >= 1,
            "turn1 flushed",
        );
        let flushed = fixture.pump.shared.take_completed().unwrap();
        assert_eq!(flushed.user_text, "第一句");
        assert!(
            flushed.interrupted,
            "in-flight response must flush as interrupted"
        );
        assert!(!fixture.pump.shared.speaking.load(Ordering::SeqCst));

        // turn 2 在新连接上正常完成：无 interrupted 残留，音频不被丢弃。
        ws_reconnected
            .send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws_reconnected
            .send(Message::Text(
                format!(
                    r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                    STANDARD.encode([3u8, 4])
                )
                .into(),
            ))
            .unwrap();
        wait_for(
            || fixture.sink.written().ends_with(&[3u8, 4]),
            "turn2 audio not dropped",
        );
        ws_reconnected
            .send(Message::Text(
                r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
            ))
            .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() >= 1,
            "turn2 completed",
        );
        let turn2 = fixture.pump.shared.take_completed().unwrap();
        assert!(!turn2.interrupted);
        drop(fixture.pump);
    }

    /// 回声闸门：音频落设备后上行应静默（回声不得进服务端缓冲），直到
    /// response.done → drain → 「已播净」回执才恢复上行。
    struct GatedSink {
        written: Mutex<Vec<u8>>,
        drained: std::sync::atomic::AtomicBool,
        stall_drain: std::sync::atomic::AtomicBool,
        fail_writes: std::sync::atomic::AtomicBool,
        // 观察用计数：drained 回执会被泵循环头微秒级消费掉，跨线程轮询
        // 该标志必然错过；drain 调用数才是稳定的观察点。
        drain_calls: std::sync::atomic::AtomicU32,
        ping_calls: std::sync::atomic::AtomicU32,
        clear_calls: std::sync::atomic::AtomicU32,
        // sidecar 死亡模拟：首写成功后即宣告死亡，用于钉住
        // 「死亡时闸门不得永久关闭」的重开路径。
        dead: std::sync::atomic::AtomicBool,
        die_after_first_write: std::sync::atomic::AtomicBool,
    }

    impl Default for GatedSink {
        fn default() -> Self {
            Self {
                written: Mutex::new(Vec::new()),
                drained: std::sync::atomic::AtomicBool::new(false),
                stall_drain: std::sync::atomic::AtomicBool::new(false),
                fail_writes: std::sync::atomic::AtomicBool::new(false),
                drain_calls: std::sync::atomic::AtomicU32::new(0),
                ping_calls: std::sync::atomic::AtomicU32::new(0),
                clear_calls: std::sync::atomic::AtomicU32::new(0),
                dead: std::sync::atomic::AtomicBool::new(false),
                die_after_first_write: std::sync::atomic::AtomicBool::new(false),
            }
        }
    }

    impl PlaybackStream for GatedSink {
        fn write(&self, pcm: &[u8]) -> Result<(), &'static str> {
            if self.fail_writes.load(Ordering::SeqCst) {
                return Err("PLAYBACK_START_FAILED");
            }
            self.written
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .extend_from_slice(pcm);
            if self.die_after_first_write.load(Ordering::SeqCst) {
                self.dead.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
        fn clear(&self) -> Result<(), &'static str> {
            self.clear_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn drain(&self) -> Result<(), &'static str> {
            // 测试替身：无设备缓冲，drain 即播净。
            self.drain_calls.fetch_add(1, Ordering::SeqCst);
            if !self.stall_drain.load(Ordering::SeqCst) {
                self.drained.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
        fn ping(&self) -> Result<(), &'static str> {
            self.ping_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn take_drained(&self) -> bool {
            self.drained.swap(false, Ordering::SeqCst)
        }
        fn is_alive(&self) -> bool {
            !self.dead.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn playback_gates_mic_uplink_until_drained() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        // 开门状态：tap 帧正常上行。
        tap_tx.send(vec![1u8; 9600]).unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.append");

        // 播报：created → delta 落设备 → 闸门应关闭。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([7u8, 8])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !sink.written.lock().unwrap().is_empty(), "audio written");
        tap_tx.send(vec![2u8; 9600]).unwrap();
        assert_no_frame(&mut ws, Duration::from_millis(600));

        // done → drain → 替身立即回执 → 闸门重开。tap 帧与开门存在竞态
        // （早发的帧在关门期间被丢弃），用探针重试直到上行恢复。
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        expect_gate_reopen_clear(&mut ws);
        let mut frame = None;
        for _ in 0..20 {
            tap_tx.send(vec![3u8; 9600]).unwrap();
            if let Some(recovered) = try_read_frame(&mut ws, Duration::from_millis(150)) {
                frame = Some(recovered);
                break;
            }
        }
        let frame = frame.expect("uplink must resume after drained receipt");
        assert_eq!(frame["type"], "input_audio_buffer.append");
        assert_eq!(
            STANDARD.decode(frame["audio"].as_str().unwrap()).unwrap(),
            vec![3u8; 3200]
        );
        drop(pump);
    }

    /// 闸门重开瞬间必须清一次服务端输入缓冲：关门期与重开尾窗渗入的
    /// 回声残渣不得混进下一次提交。重开后第一帧应为 clear,随后 append 恢复。
    #[test]
    fn gate_reopen_clears_server_input_buffer_before_resuming_uplink() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        // 播报：created → delta 落设备 → 闸门关闭。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([7u8, 8])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !sink.written.lock().unwrap().is_empty(), "audio written");

        // done → drain → 替身立即回执「已播净」→ 闸门重开。
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        wait_for(
            || sink.drain_calls.load(Ordering::SeqCst) >= 1,
            "drain command",
        );

        // 重开后第一帧必须是 clear(清回声残渣),其后 append 恢复上行。
        let frame = read_frame(&mut ws);
        assert_eq!(
            frame["type"], "input_audio_buffer.clear",
            "gate reopen must clear server input buffer first"
        );
        let mut resumed = None;
        for _ in 0..20 {
            tap_tx.send(vec![3u8; 9600]).unwrap();
            if let Some(recovered) = try_read_frame(&mut ws, Duration::from_millis(150)) {
                resumed = Some(recovered);
                break;
            }
        }
        assert_eq!(
            resumed.expect("uplink must resume after clear")["type"],
            "input_audio_buffer.append"
        );
        drop(pump);
    }

    /// 播净回执丢失时不能等 20 秒安全阀：短兜底后麦克风必须恢复上行。
    #[test]
    fn playback_gate_reopens_after_drain_receipt_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        sink.stall_drain.store(true, Ordering::SeqCst);
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        tap_tx.send(vec![1u8; 9600]).unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.append");
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([7u8, 8])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !sink.written.lock().unwrap().is_empty(), "audio written");
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        wait_for(
            || sink.drain_calls.load(Ordering::SeqCst) >= 1,
            "drain command",
        );

        tap_tx.send(vec![2u8; 9600]).unwrap();
        assert_no_frame(&mut ws, Duration::from_millis(300));
        std::thread::sleep(GATE_DRAIN_FALLBACK_TAIL);
        expect_gate_reopen_clear(&mut ws);
        let mut frame = None;
        for _ in 0..20 {
            tap_tx.send(vec![3u8; 9600]).unwrap();
            if let Some(recovered) = try_read_frame(&mut ws, Duration::from_millis(150)) {
                frame = Some(recovered);
                break;
            }
        }
        assert_eq!(
            frame.expect("fallback must reopen uplink")["type"],
            "input_audio_buffer.append"
        );
        drop(pump);
    }

    /// 播放写入失败不能被误标为已出声：保留音频字节数与写入失败诊断。
    #[test]
    fn playback_write_failure_is_reported_in_completed_turn() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (_tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        sink.fail_writes.store(true, Ordering::SeqCst);
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, mut ws) = accept_session(&listener);
        for event in [
            r#"{"type":"response.created"}"#.to_string(),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2, 3, 4])
            ),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(
            || pump.shared.completed_count() == 1,
            "failed playback turn",
        );
        let turn = pump.shared.take_completed().unwrap();
        assert_eq!(turn.audio_bytes, 4);
        assert!(turn.playback_write_failed);
        assert!(turn.playback_alive);
        drop(pump);
    }

    /// response.done 之后的迟到音频 delta 必须被丢弃（防重复播报），且空轮不入队。
    #[test]
    fn late_deltas_after_done_are_dropped_and_empty_turn_skipped() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !fixture.sink.written().is_empty(), "audio played");
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() >= 1,
            "turn completed",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert!(
            !turn.user_text.is_empty()
                || !turn.assistant_text.is_empty()
                || turn.first_audio_ms.is_some()
                || turn.interrupted
        );

        // done → drain → 播净重开：先消费重开 clear 帧，再做迟到 delta 断言。
        expect_gate_reopen_clear(&mut ws);

        // done 后迟到 delta：不得再写播放，也不得再成轮。
        let written_after_done = fixture.sink.written().len();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([9u8, 9])
            )
            .into(),
        ))
        .unwrap();
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        assert_no_frame(&mut ws, Duration::from_millis(400));
        assert_eq!(fixture.sink.written().len(), written_after_done);
        assert_eq!(
            fixture.pump.shared.completed_count(),
            0,
            "empty duplicate turn must not enqueue"
        );
        drop(fixture.pump);
    }

    /// 回声回归测试专用替身：drained 初始为 true（对齐 StreamPlayback 的
    /// 生产语义：无缓冲即已播净），drain 帧后置位。
    struct StaleReceiptSink {
        written: Mutex<Vec<u8>>,
        drained: std::sync::atomic::AtomicBool,
    }

    impl Default for StaleReceiptSink {
        fn default() -> Self {
            Self {
                written: Mutex::new(Vec::new()),
                drained: std::sync::atomic::AtomicBool::new(true),
            }
        }
    }

    impl PlaybackStream for StaleReceiptSink {
        fn write(&self, pcm: &[u8]) -> Result<(), &'static str> {
            self.written
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .extend_from_slice(pcm);
            Ok(())
        }
        fn clear(&self) -> Result<(), &'static str> {
            Ok(())
        }
        fn drain(&self) -> Result<(), &'static str> {
            self.drained.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn take_drained(&self) -> bool {
            self.drained.swap(false, Ordering::SeqCst)
        }
    }

    /// 根因回归：闸门关闭前残留的「已播净」回执（初始 true / 上轮 drain
    /// 回执）不得在关门后把闸门重新打开——否则播报期回声全程上行，被转写成
    /// 「用户发言」触发自问自答循环。关门时必须先作废遗留回执。
    #[test]
    fn stale_drained_receipt_disarmed_at_gate_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(StaleReceiptSink::default());
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        tap_tx.send(vec![1u8; 9600]).unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.append");

        // 播报落设备 → 闸门关闭；陈旧 drained 回执必须失效，tap 不得上行。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([7u8, 8])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !sink.written.lock().unwrap().is_empty(), "audio written");
        tap_tx.send(vec![2u8; 9600]).unwrap();
        assert_no_frame(&mut ws, Duration::from_millis(600));

        // done → drain → 本次真实回执到达后闸门才重开。
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        expect_gate_reopen_clear(&mut ws);
        let mut frame = None;
        for _ in 0..20 {
            tap_tx.send(vec![3u8; 9600]).unwrap();
            if let Some(recovered) = try_read_frame(&mut ws, Duration::from_millis(150)) {
                frame = Some(recovered);
                break;
            }
        }
        assert_eq!(
            frame.expect("uplink must resume on fresh drained receipt")["type"],
            "input_audio_buffer.append"
        );
        drop(pump);
    }

    /// 闸门关闭（播报中）到达的 CommitTurn 必须丢弃：新语音根本没上行，
    /// 提交的只可能是回声残渣或空缓冲；播净回执后提交恢复放行。
    #[test]
    fn commit_turn_is_dropped_while_gate_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (_tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        // 播报中：闸门关闭，CommitTurn 不得到达服务端。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([7u8, 8])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !sink.written.lock().unwrap().is_empty(), "audio written");
        pump.send(PumpCommand::CommitTurn);
        assert_no_frame(&mut ws, Duration::from_millis(400));

        // done → drain → 播净回执 → 闸门重开（先 clear 清残渣），提交恢复放行。
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        wait_for(
            || sink.drain_calls.load(Ordering::SeqCst) >= 1,
            "drain receipt",
        );
        expect_gate_reopen_clear(&mut ws);
        pump.send(PumpCommand::CommitTurn);
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.commit");
        assert_eq!(read_frame(&mut ws)["type"], "response.create");
        drop(pump);
    }

    /// 视频帧（摄像头/桌面共享）：PumpCommand 直通实时会话，
    /// 服务端收到 input_image_buffer.append（Aliyun 方言）。
    #[test]
    fn append_image_is_forwarded_to_session() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);
        // 图片输入的官方前置条件是当前连接至少 append 过一次音频。
        fixture.tap_tx.send(vec![1u8; 9600]).unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.append");
        fixture.pump.send(PumpCommand::AppendImage("aGk=".into()));
        let frame = read_frame(&mut ws);
        assert_eq!(frame["type"], "input_image_buffer.append");
        assert_eq!(frame["image"], "aGk=");
        drop(fixture.pump);
    }

    /// 瞬时重连发生在“转写完成但响应未开始”时：不落 text_only，
    /// 新连接就绪后用文本轮恢复应答并正常完成。
    #[test]
    fn reconnect_recovers_completed_transcript_without_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"第一句"}"#
                .into(),
        ))
        .unwrap();
        // 给泵一个完整循环消费转写事件，避免关闭连接和事件消费竞态。
        std::thread::sleep(Duration::from_millis(80));
        drop(ws);

        let (_, mut ws_reconnected) = accept_session(&listener);
        wait_for(
            || fixture.pump.shared.completed_count() == 0,
            "no text_only turn",
        );
        let item = read_frame(&mut ws_reconnected);
        assert_eq!(item["type"], "conversation.item.create");
        assert_eq!(item["item"]["content"][0]["text"], "第一句");
        assert_eq!(read_frame(&mut ws_reconnected)["type"], "response.create");

        for event in [
            r#"{"type":"response.created"}"#.to_string(),
            r#"{"type":"response.audio_transcript.delta","delta":"恢复回答"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws_reconnected.send(Message::Text(event.into())).unwrap();
        }
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "recovered turn completion",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert_eq!(turn.user_text, "第一句");
        assert_eq!(turn.assistant_text, "恢复回答");
        assert!(!turn.transcript_only);
        drop(fixture.pump);
    }

    /// 会议桥接点名门控不因重连而绕过：auto_respond=false 仍只转写。
    #[test]
    fn reconnect_keeps_meeting_gate_transcript_only() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, false, "会议助手", false);
        let (_, mut ws) = accept_session(&listener);
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"普通讨论"}"#
                .into(),
        ))
        .unwrap();
        std::thread::sleep(Duration::from_millis(80));
        drop(ws);

        let (_, mut ws_reconnected) = accept_session(&listener);
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "transcript-only flush",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert_eq!(turn.user_text, "普通讨论");
        assert!(turn.transcript_only && turn.assistant_text.is_empty());
        assert_no_frame(&mut ws_reconnected, Duration::from_millis(300));
        drop(fixture.pump);
    }

    /// 回声轮整轮丢弃：定稿转写与上一轮播报文本归一化命中（中文数字差异
    /// 也要命中）→ 清服务端输入缓冲、不成轮、不入历史。
    #[test]
    fn echo_transcript_is_dropped_and_input_buffer_cleared() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        // 第 1 轮正常完成，播报文本进回声比对缓存。
        for event in [
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"现在是什么时间"}"#.to_string(),
            r#"{"type":"response.created"}"#.to_string(),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            ),
            r#"{"type":"response.audio_transcript.delta","delta":"现在是2026年9月28日，星期一。"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(|| fixture.pump.shared.completed_count() == 1, "turn 1");
        fixture.pump.shared.take_completed().unwrap();
        // turn 1 done → drain → 播净重开：先消费重开 clear。
        expect_gate_reopen_clear(&mut ws);

        // 第 2 轮转写 = 第 1 轮播报的回声（ASR 把数字转成中文数字）。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i2","transcript":"现在是二零二六年九月二十八日星期一。"}"#
                .into(),
        ))
        .unwrap();
        let frame = read_frame(&mut ws);
        assert_eq!(
            frame["type"], "input_audio_buffer.clear",
            "必须清服务端输入缓冲"
        );
        assert_no_frame(&mut ws, Duration::from_millis(400));
        assert_eq!(
            fixture.pump.shared.completed_count(),
            0,
            "echo turn must not enqueue"
        );
        drop(fixture.pump);
    }

    /// 响应在途时收到回声转写：取消响应 + 清输入缓冲，done 到达后不成轮。
    #[test]
    fn echo_transcript_while_responding_cancels_and_drops() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        // 第 1 轮正常完成，喂入回声比对缓存。
        for event in [
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"现在是什么时间"}"#.to_string(),
            r#"{"type":"response.created"}"#.to_string(),
            r#"{"type":"response.audio_transcript.delta","delta":"现在是2026年9月28日，星期一。"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(|| fixture.pump.shared.completed_count() == 1, "turn 1");
        fixture.pump.shared.take_completed().unwrap();

        // 第 2 轮响应在途（音频播放中）时回声转写到达。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([3u8, 4])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !fixture.sink.written().is_empty(), "turn 2 audio");
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i2","transcript":"现在是二零二六年九月二十八日星期一。"}"#
                .into(),
        ))
        .unwrap();
        // MemSink 无缓冲即已播净：关门后重开 clear 可能先于 cancel 到达，两帧都断言。
        let first = read_frame(&mut ws)["type"].clone();
        let second = read_frame(&mut ws)["type"].clone();
        assert!(
            (first == "response.cancel" && second == "input_audio_buffer.clear")
                || (first == "input_audio_buffer.clear" && second == "response.cancel"),
            "expected cancel + clear in either order, got {first} then {second}"
        );
        wait_for(|| fixture.sink.clears() >= 1, "playback cleared");

        // 取消的响应收尾：echo_dropped 轮不得成轮。
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"cancelled"}}"#.into(),
        ))
        .unwrap();
        // done(cancelled) 后 was_playing → drain → 播净重开 clear,消费后再断言静默。
        expect_gate_reopen_clear(&mut ws);
        assert_no_frame(&mut ws, Duration::from_millis(300));
        assert_eq!(fixture.pump.shared.completed_count(), 0);
        drop(fixture.pump);
    }

    /// 当前回答正在出声时，ASR 只回收到其中短片段（如“美元”）也必须按
    /// 回声处理：取消响应、清输入缓冲、不成轮，防止继续自问自答。
    #[test]
    fn current_turn_short_echo_fragment_is_dropped() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        for event in [
            r#"{"type":"response.created"}"#.to_string(),
            r#"{"type":"response.audio_transcript.delta","delta":"国际现货黄金大约是4285美元每盎司"}"#.to_string(),
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([5u8, 6])
            ),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(
            || !fixture.sink.written().is_empty(),
            "current audio playing",
        );
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"echo","transcript":"美元"}"#
                .into(),
        ))
        .unwrap();

        // MemSink 无缓冲即已播净：关门后重开 clear 可能先于 cancel 到达，两帧都断言。
        let first = read_frame(&mut ws)["type"].clone();
        let second = read_frame(&mut ws)["type"].clone();
        assert!(
            (first == "response.cancel" && second == "input_audio_buffer.clear")
                || (first == "input_audio_buffer.clear" && second == "response.cancel"),
            "expected cancel + clear in either order, got {first} then {second}"
        );
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"cancelled"}}"#.into(),
        ))
        .unwrap();
        wait_for(|| fixture.sink.clears() >= 1, "playback cleared");
        assert_eq!(fixture.pump.shared.completed_count(), 0);
        drop(fixture.pump);
    }

    /// 正常新话题不会被“当前回答短片段回声”规则误吞。
    fn test_echo<'a>(
        assistant: &'a str,
        responding: bool,
        played_audio: bool,
        first_audio_at: Option<Instant>,
    ) -> CurrentTurnEcho<'a> {
        CurrentTurnEcho {
            assistant,
            responding,
            played_audio,
            first_audio_at,
            post_playback_until: None,
            guard: true,
        }
    }

    #[test]
    fn current_turn_echo_rule_does_not_swallow_new_topic() {
        assert!(is_current_or_recent_echo(
            "美元",
            &[],
            &test_echo("国际现货黄金大约是4285美元每盎司", true, true, None),
        ));
        assert!(!is_current_or_recent_echo(
            "换一个话题",
            &[],
            &test_echo(
                "国际现货黄金大约是4285美元每盎司",
                true,
                true,
                Some(Instant::now()),
            ),
        ));
        assert!(!is_current_or_recent_echo(
            "美元",
            &[],
            &test_echo(
                "国际现货黄金大约是4285美元每盎司",
                false,
                true,
                Some(Instant::now() - Duration::from_secs(2)),
            ),
        ));
        // 文本 transcript lag 时，播报已持续足够久的短转写仍按回声兜底。
        assert!(is_current_or_recent_echo(
            "拒绝",
            &[],
            &test_echo(
                "",
                true,
                true,
                Some(Instant::now() - Duration::from_millis(500)),
            ),
        ));
    }

    /// WebAudio 播报中的当轮回声：上行常开，在途回答被麦克风回收转写后
    /// 必须判回声（历史缓存里还没有当轮文本，只能对在途 assistant 比对）。
    #[test]
    fn webaudio_current_turn_echo_is_detected_against_inflight_answer() {
        fn webaudio(assistant: &str) -> CurrentTurnEcho<'_> {
            CurrentTurnEcho {
                assistant,
                responding: true,
                played_audio: true,
                first_audio_at: Some(Instant::now()),
                post_playback_until: None,
                guard: false,
            }
        }
        // 当轮播报被整体回收转写（同形/中文数字变体）。
        assert!(is_current_or_recent_echo(
            "国际现货黄金大约是四二八五美元每盎司",
            &[],
            &webaudio("国际现货黄金大约是4285美元每盎司，短线波动加剧。"),
        ));
        // 播报的截断片段（只回收半句）。
        assert!(is_current_or_recent_echo(
            "四二八五美元每盎司",
            &[],
            &webaudio("国际现货黄金大约是4285美元每盎司，短线波动加剧。"),
        ));
        // 播报中的真实插话：与在途回答无相似度，不得吞。
        assert!(!is_current_or_recent_echo(
            "等一下，先停一下",
            &[],
            &webaudio("国际现货黄金大约是4285美元每盎司，短线波动加剧。"),
        ));
        // 未出声（无播放）时不判：播报前到达的转写是真人语音。
        assert!(!is_current_or_recent_echo(
            "国际现货黄金大约是四二八五美元每盎司",
            &[],
            &CurrentTurnEcho {
                assistant: "国际现货黄金大约是4285美元每盎司，短线波动加剧。",
                responding: true,
                played_audio: false,
                first_audio_at: None,
                post_playback_until: None,
                guard: false,
            },
        ));
    }

    /// audio delta 间隔写入完成轮，用于区分网络抖动与播放设备问题。
    #[test]
    fn audio_delta_gap_metrics_are_reported() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            )
            .into(),
        ))
        .unwrap();
        std::thread::sleep(Duration::from_millis(170));
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([3u8, 4])
            )
            .into(),
        ))
        .unwrap();
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        wait_for(
            || fixture.pump.shared.completed_count() == 1,
            "gap metric turn",
        );
        let turn = fixture.pump.shared.take_completed().unwrap();
        assert_eq!(turn.audio_delta_count, 2);
        let max_gap = turn.audio_delta_max_gap_ms.expect("two deltas have a gap");
        assert!(max_gap >= 150, "actual gap was {max_gap}ms");
        assert_eq!(turn.audio_delta_gaps_over_150_ms, 1);
        assert_eq!(turn.audio_delta_gaps_over_500_ms, 0);
        assert!(!turn.echo_dropped);
        drop(fixture.pump);
    }

    #[test]
    fn webaudio_mode_keeps_mic_uplink_open_while_streaming_audio() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        let audio = Arc::new(Mutex::new(Vec::<u8>::new()));
        let seen_audio = Arc::clone(&audio);
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::WebAudio,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            Some(Box::new(move |event| {
                if let PumpLive::AssistantAudio(pcm) = event {
                    seen_audio.lock().unwrap().extend_from_slice(&pcm);
                }
            })),
        );
        let (_, mut ws) = accept_session(&listener);

        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2, 3, 4])
            )
            .into(),
        ))
        .unwrap();
        wait_for(
            || audio.lock().unwrap().len() == 4,
            "assistant audio forwarded",
        );

        // WebAudio 全双工：AI 播放不关闭麦克风上行。
        tap_tx.send(vec![5u8; 9600]).unwrap();
        assert_eq!(read_frame(&mut ws)["type"], "input_audio_buffer.append");
        assert!(sink.written.lock().unwrap().is_empty());
        drop(pump);
    }

    #[test]
    fn webaudio_barge_in_clears_frontend_playback_and_cancels_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (_tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        let controls = Arc::new(Mutex::new(Vec::<PlaybackControl>::new()));
        let seen_controls = Arc::clone(&controls);
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::WebAudio,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            Some(Box::new(move |event| {
                if let PumpLive::PlaybackControl(control) = event {
                    seen_controls.lock().unwrap().push(control);
                }
            })),
        );
        let (_, mut ws) = accept_session(&listener);
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            r#"{"type":"input_audio_buffer.speech_started"}"#.into(),
        ))
        .unwrap();

        assert_eq!(read_frame(&mut ws)["type"], "response.cancel");
        wait_for(
            || {
                controls
                    .lock()
                    .unwrap()
                    .last()
                    .is_some_and(|control| matches!(control, PlaybackControl::Clear))
            },
            "frontend playback cleared",
        );
        drop(pump);
    }

    /// WebAudio 期间原生播放进程空闲，必须按约 500ms 发保活。
    /// 忙循环本身不得刷新计时，否则 ping 永远发不出去。
    #[test]
    fn webaudio_mode_pings_native_playback_about_every_500ms() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (_tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::WebAudio,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, _ws) = accept_session(&listener);

        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            sink.ping_calls.load(Ordering::SeqCst),
            0,
            "busy loop must not emit a keepalive before 500ms"
        );
        wait_for(
            || sink.ping_calls.load(Ordering::SeqCst) >= 1,
            "first keepalive ping",
        );
        let first_seen = Instant::now();
        wait_for(
            || sink.ping_calls.load(Ordering::SeqCst) >= 2,
            "second keepalive ping",
        );
        let gap = first_seen.elapsed();
        assert!(
            gap >= Duration::from_millis(350),
            "keepalive interval too short: {gap:?}"
        );
        assert!(
            gap < Duration::from_millis(1500),
            "keepalive interval too long: {gap:?}"
        );
        drop(pump);
    }

    #[test]
    fn aec_residual_monitor_detects_matching_envelope() {
        let mut monitor = AecResidualMonitor::default();
        let reference = alternating_envelope_pcm(24_000);
        monitor.observe_playback(&reference);
        let microphone = alternating_envelope_pcm(48_000);
        let residual = monitor
            .observe_microphone(&microphone)
            .expect("matching envelope should produce a score");
        assert!(residual >= 0.9, "residual was {residual}");
    }

    fn alternating_envelope_pcm(rate: u32) -> Vec<u8> {
        let blocks = 24usize;
        let block_samples = rate as usize / 100; // 10ms
        let mut pcm = Vec::new();
        for block in 0..blocks {
            let sample: i16 = if block % 2 == 0 { 12_000 } else { 100 };
            for _ in 0..block_samples {
                pcm.extend_from_slice(&sample.to_le_bytes());
            }
        }
        pcm
    }

    /// 转写只有 delta 没有 completed 的方言：定稿级兜底——done 时 user_text
    /// 与此前播报命中回声，同样不成轮。
    #[test]
    fn echo_user_text_without_completed_event_dropped_at_response_done() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", false);
        let (_, mut ws) = accept_session(&listener);

        for event in [
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","transcript":"现在是什么时间"}"#.to_string(),
            r#"{"type":"response.created"}"#.to_string(),
            r#"{"type":"response.audio_transcript.delta","delta":"现在是2026年9月28日，星期一。"}"#.to_string(),
            r#"{"type":"response.done","response":{"status":"completed"}}"#.to_string(),
        ] {
            ws.send(Message::Text(event.into())).unwrap();
        }
        wait_for(|| fixture.pump.shared.completed_count() == 1, "turn 1");
        fixture.pump.shared.take_completed().unwrap();

        // 第 2 轮：只有 partial delta 携带回声文本，无 completed 事件。
        ws.send(Message::Text(
            r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"i2","text":"现在是二零二六年九月二十八日星期一。"}"#
                .into(),
        ))
        .unwrap();
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            r#"{"type":"response.audio_transcript.delta","delta":"没错，就是2026年9月28日，星期一。"}"#
                .into(),
        ))
        .unwrap();
        ws.send(Message::Text(
            r#"{"type":"response.done","response":{"status":"completed"}}"#.into(),
        ))
        .unwrap();
        assert_no_frame(&mut ws, Duration::from_millis(300));
        assert_eq!(
            fixture.pump.shared.completed_count(),
            0,
            "echo user text must not persist at done"
        );
        drop(fixture.pump);
    }

    /// 重连边界：断连处理必须清空设备缓冲（掐断回声源）并立即恢复上行，
    /// 不得等播净回执或 1.5s 兜底尾窗（播净回执在 stall_drain 下永不到达）。
    #[test]
    fn reconnecting_clears_playback_and_reopens_uplink() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        sink.stall_drain.store(true, Ordering::SeqCst);
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        // 播报中闸门关闭：tap 帧不得上行。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !sink.written.lock().unwrap().is_empty(), "turn1 audio");
        tap_tx.send(vec![2u8; 9600]).unwrap();
        assert_no_frame(&mut ws, Duration::from_millis(300));

        // 服务端拆连 → 重连：清空播放 + 立即开门。
        drop(ws);
        let (_, mut ws_reconnected) = accept_session(&listener);
        wait_for(|| pump.shared.completed_count() >= 1, "turn1 flushed");
        assert!(
            sink.clear_calls.load(Ordering::SeqCst) >= 1,
            "重连必须清空设备缓冲"
        );

        // 早发的探针帧可能落在关门瞬间被丢弃（允许的竞态），重试直到上行恢复。
        let mut reopened = false;
        for _ in 0..6 {
            tap_tx.send(vec![3u8; 9600]).unwrap();
            if let Some(frame) = try_read_frame(&mut ws_reconnected, Duration::from_millis(120))
                && frame["type"] == "input_audio_buffer.append"
            {
                reopened = true;
                break;
            }
        }
        assert!(reopened, "重连后上行必须立即恢复，不得等播净回执");
        drop(pump);
    }

    /// 候选模式扣留上限：恰好触顶的 delta 全部扣留，超限 delta 被丢弃
    /// （不扩大扣留缓冲），冲刷时只有上限内的部分落设备。
    #[test]
    fn candidate_hold_drops_deltas_beyond_rolling_cap() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fixture = spawn_pump(port, true, "", true);
        let (_, mut ws) = accept_session(&listener);

        // 与泵内 HELD_CAP_BYTES 一致：60s@24k（48_000 B/s）。
        // 单帧不得超过会话侧 MAX_TEXT_FRAME_BYTES（1MB，base64 后），
        // 故用 5×576_000 字节的 delta 累计到上限。
        const HELD_CAP_BYTES: usize = 60 * 48_000;
        const DELTA_BYTES: usize = 576_000;
        for _ in 0..(HELD_CAP_BYTES / DELTA_BYTES) {
            ws.send(Message::Text(
                format!(
                    r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                    STANDARD.encode(vec![5u8; DELTA_BYTES])
                )
                .into(),
            ))
            .unwrap();
        }
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode(vec![6u8; 32])
            )
            .into(),
        ))
        .unwrap();
        // 等六个 delta 都被泵处理完，避免 FlushHeld 抢在超限 delta 之前。
        std::thread::sleep(Duration::from_millis(400));
        assert!(fixture.sink.written().is_empty(), "候选模式不得直接出声");

        fixture.pump.send(PumpCommand::FlushHeld);
        wait_for(
            || fixture.sink.written().len() == HELD_CAP_BYTES,
            "flushed held audio",
        );
        assert_eq!(
            fixture.sink.written(),
            vec![5u8; HELD_CAP_BYTES],
            "超限 delta 必须被丢弃，只有上限内的部分落设备"
        );
        assert_eq!(
            fixture.sink.clears(),
            0,
            "超限丢弃是静默的：不得清空设备缓冲"
        );
        drop(fixture.pump);
    }

    /// sidecar 死亡时闸门不得永久关闭：不等播净回执（stall_drain 下永不
    /// 到达）、不等 1.5s 兜底截止，下一圈循环立即重开上行。
    #[test]
    fn dead_sidecar_playback_reopens_gate_without_drained_receipt() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let session = RealtimeSession::start_with_profile(
            RealtimeCapabilityProfile::openai_compatible(realtime_dialect(
                "http://127.0.0.1/api-ws/v1/realtime",
            )),
            RealtimeSessionConfig {
                endpoint: ProviderEndpoint {
                    provider_id: "test".into(),
                    base_url: format!("http://127.0.0.1:{port}/api-ws/v1/realtime"),
                },
                credential: None,
                model_id: "qwen3.8-omni-flash-realtime".into(),
                voice: String::new(),
                instructions: "测试".into(),
                history: vec![],
                auto_respond: true,
                enable_search: false,
            },
        )
        .unwrap();
        let (tap_tx, tap_rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let sink = Arc::new(GatedSink::default());
        sink.stall_drain.store(true, Ordering::SeqCst);
        sink.die_after_first_write.store(true, Ordering::SeqCst);
        let pump = RealtimePump::start(
            session,
            tap_rx,
            Arc::clone(&sink) as Arc<dyn PlaybackStream>,
            PumpConfig {
                auto_respond: true,
                role_name: String::new(),
                hold_playback: false,
                playback_mode: RealtimePlaybackMode::Native,
                suppress_echo: None,
                respond_start_timeout: RESPOND_START_TIMEOUT,
            },
            None,
        );
        let (_, mut ws) = accept_session(&listener);

        // 首次写播放后 sidecar 宣告死亡，闸门随写关闭。
        ws.send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .unwrap();
        ws.send(Message::Text(
            format!(
                r#"{{"type":"response.audio.delta","delta":"{}"}}"#,
                STANDARD.encode([1u8, 2])
            )
            .into(),
        ))
        .unwrap();
        wait_for(|| !sink.written.lock().unwrap().is_empty(), "audio written");

        // 探针预算（7×150ms ≈ 1.05s）必须短于 1.5s 兜底截止，
        // 保证重开只能来自 is_alive 路径。
        let mut reopened = false;
        for _ in 0..7 {
            tap_tx.send(vec![3u8; 9600]).unwrap();
            if let Some(frame) = try_read_frame(&mut ws, Duration::from_millis(150))
                && frame["type"] == "input_audio_buffer.append"
            {
                reopened = true;
                break;
            }
        }
        assert!(
            reopened,
            "sidecar 死亡后闸门必须立即重开，不得等播净回执或兜底截止"
        );
        drop(pump);
    }
}
