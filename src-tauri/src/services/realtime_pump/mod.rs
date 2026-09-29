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

// 子模块：纯搬移拆分（C21），各项经 use 自举，pub 项保持原路径可用。
mod echo;
mod playback;
mod shared;
mod turn;

use self::echo::{AecResidualMonitor, current_turn_echo, is_current_or_recent_echo};
pub use self::playback::{PlaybackControl, RealtimePlaybackMode, SilentPlayback};
pub use self::shared::{CompletedTurn, PumpCommand, PumpLive, PumpShared, TurnTimeline};
use self::turn::{TurnAccumulator, flush_in_flight_turn};

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
mod tests;
