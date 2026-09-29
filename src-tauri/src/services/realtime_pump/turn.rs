//! 轮次累积器与在途轮冲刷（纯搬移自 realtime_pump.rs）。

use super::*;

pub(super) struct TurnAccumulator {
    pub(super) timeline: TurnTimeline,
    pub(super) user_text: String,
    pub(super) assistant_text: String,
    pub(super) speech_stopped_at: Option<Instant>,
    pub(super) first_audio_at: Option<Instant>,
    pub(super) responding: bool,
    /// response.done 已收到：其后到达的迟到尾包 delta 一律丢弃（防重复播报），
    /// 直到下一个 response.created 复位。delta 早于 created 的历史行为不受影响。
    pub(super) response_closed: bool,
    pub(super) played_audio: bool,
    pub(super) audio_bytes: usize,
    pub(super) audio_delta_count: usize,
    pub(super) audio_delta_max_gap_ms: Option<u64>,
    pub(super) audio_delta_gaps_over_150_ms: usize,
    pub(super) audio_delta_gaps_over_500_ms: usize,
    pub(super) aec_residual_correlation: Option<f64>,
    pub(super) aec_residual_max: Option<f64>,
    pub(super) aec_residual_over_threshold: usize,
    pub(super) last_audio_delta_at: Option<Instant>,
    pub(super) playback_generation_at_start: Option<u64>,
    pub(super) playback_restarts_at_start: Option<u64>,
    pub(super) playback_write_failed: bool,
    pub(super) interrupted: bool,
    pub(super) transcript_only: bool,
    /// 本轮应答由 ForceRespond 触发（写入 CompletedTurn.forced 供 turn_meta 标注）。
    pub(super) forced: bool,
    /// 本轮被文本回声过滤判定为回声（AI 播报声被麦克风回收）：done 到达时
    /// 不成轮不落库，否则自问自答循环会被固化进历史。
    pub(super) echo_dropped: bool,
    /// 正在应答的那句话的转写 item_id（点名=定稿 item；ForceRespond=在途
    /// delta item；文本来自 last_transcript_only 记 None）。回答期间据以
    /// 区分「同一句话的定稿」与「新的一句话」。
    pub(super) answer_item_id: Option<String>,
    /// 最近一次用户转写 delta 的 item_id（ForceRespond 判定回答素材归属）。
    pub(super) last_user_item_id: Option<String>,
    /// 回答开始看门狗：RespondText 发出时刻；收到 ResponseStarted 即清空。
    pub(super) respond_requested_at: Option<Instant>,
    /// 看门狗重试次数：0=未重试（首次超时重发），1=已重试（再超时放弃）。
    pub(super) respond_attempts: u8,
}

impl TurnAccumulator {
    pub(super) fn new() -> Self {
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

    pub(super) fn first_audio_ms(&self) -> Option<u64> {
        Some(
            self.first_audio_at?
                .duration_since(self.speech_stopped_at.unwrap_or_else(Instant::now))
                .as_millis() as u64,
        )
    }

    pub(super) fn has_pending_content(&self) -> bool {
        self.responding || !self.user_text.is_empty() || !self.assistant_text.is_empty()
    }

    pub(super) fn record_audio_delta_at(&mut self, now: Instant) {
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

    pub(super) fn record_aec_residual(&mut self, value: f64) {
        self.aec_residual_correlation = Some(value);
        self.aec_residual_max = Some(self.aec_residual_max.map_or(value, |old| old.max(value)));
        if value >= 0.9 {
            self.aec_residual_over_threshold += 1;
        }
    }
}

/// 连接重置前冲刷在途轮：已收到的内容立即成轮（打断语义），状态归零。
/// 已扣住待确认的音频（候选模式）不受影响，仍等 FlushHeld/DiscardHeld。
pub(super) fn flush_in_flight_turn(
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
