//! AEC 残留监测与当前轮/历史轮回声判定（纯搬移自 realtime_pump.rs）。

use super::*;

/// 播放参考与麦克风电平包络的相关度监测。这里不是第二个 AEC，而是评估
/// WebView AEC 是否残留强回声；连续高残留时切换原生可打断兜底。
#[derive(Default)]
pub(super) struct AecResidualMonitor {
    playback_envelopes: VecDeque<f32>,
    microphone_envelopes: VecDeque<f32>,
}

impl AecResidualMonitor {
    pub(super) fn observe_playback(&mut self, pcm: &[u8]) {
        append_envelopes(&mut self.playback_envelopes, pcm, 24_000);
    }

    pub(super) fn observe_microphone(&mut self, pcm: &[u8]) -> Option<f64> {
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

pub(super) fn append_envelopes(target: &mut VecDeque<f32>, pcm: &[u8], sample_rate: u32) {
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

pub(super) fn pearson(left: &[f32], right: &[f32]) -> f64 {
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

/// 当前轮回声判定所需的播报状态。单独成结构，避免判断函数参数超过 clippy 上限。
pub(super) struct CurrentTurnEcho<'a> {
    pub(super) assistant: &'a str,
    pub(super) responding: bool,
    pub(super) played_audio: bool,
    pub(super) first_audio_at: Option<Instant>,
    pub(super) post_playback_until: Option<Instant>,
    /// 原生播放闭锁期间才启用当前轮短片段回声闸；WebAudio 全双工不靠它挡打断。
    pub(super) guard: bool,
}

/// 历史轮回声沿用通用相似度；当前轮已实际出声时，短片段也按回声处理。
/// 播报期间麦克风闸门关闭，此时服务端转写出的当前回答片段大概率是设备
/// 尾音/回声，而不是已经上行的新用户语音。
pub(super) fn is_current_or_recent_echo(
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

pub(super) fn current_turn_echo<'a>(
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

pub(super) fn common_suffix_length(left: &str, right: &str) -> usize {
    left.chars()
        .rev()
        .zip(right.chars().rev())
        .take_while(|(left, right)| left == right)
        .count()
}
