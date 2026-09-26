//! Silero VAD 包装器：把 16kHz/512 采样窗口映射为语音概率。
use silero_vad_rust::silero_vad::model::OnnxModel;

pub const VAD_WINDOW_SAMPLES: usize = 512; // 32 ms @16 kHz

/// Silero 推理采样率；`forward_chunk` 只接受 8k/16k，采集管线统一降采样到 16k。
const VAD_SAMPLE_RATE: u32 = 16_000;

pub trait VoiceActivityDetector: Send {
    /// 返回窗口内含人声的概率 0.0..=1.0。
    fn process(&mut self, window: &[f32]) -> f32;
}

/// 持有 crate 内置的 Silero ONNX 模型（opset 16，CPU provider）。
pub struct SileroVad {
    model: OnnxModel,
}

impl SileroVad {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let model = silero_vad_rust::load_silero_vad()?;
        Ok(Self { model })
    }
}

impl VoiceActivityDetector for SileroVad {
    fn process(&mut self, window: &[f32]) -> f32 {
        debug_assert_eq!(window.len(), VAD_WINDOW_SAMPLES);
        // 每窗重置循环状态：概率只取决于当前窗口，流式行为确定、可测。
        self.model.reset_states();
        match self.model.forward_chunk(window, VAD_SAMPLE_RATE) {
            Ok(probs) => probs[[0, 0]],
            // 推理失败按无语音处理，不让错误冒泡进实时采集路径。
            Err(error) => {
                tracing::debug!(?error, "silero vad forward_chunk failed");
                0.0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silero_loads_and_silence_scores_low() {
        let mut vad = SileroVad::new().expect("silero model must load");
        let window = vec![0f32; VAD_WINDOW_SAMPLES];
        let prob = vad.process(&window);
        assert!(prob < 0.2, "silence probability must be low, got {prob}");
    }
}

// VAD 驱动的语句分段状态机：512@16k 窗口逐窗判定，语义与 `UtteranceSegmenter` 对齐。
use crate::audio::pcm::downsample_48k_to_16k;
use crate::audio::segmenter::SpeechSegmenter;
use crate::audio::smart_turn::{TurnCompletenessDetector, TURN_COMPLETE_PROB};
use std::collections::VecDeque;

const START_PROB: f32 = 0.5;
const END_PROB: f32 = 0.35;
const START_WINDOWS: usize = 2; // 连续 2 窗 ≥START_PROB（~64ms）
const END_SILENCE_WINDOWS: usize = 22; // 连续 22 窗 <END_PROB（~700ms）
const MIN_SPEECH_WINDOWS: usize = 6; // 累计 ≥6 窗语音（~192ms）
const MAX_UTTERANCE_WINDOWS: usize = 780; // 25s 强制提交
const PRE_ROLL_WINDOWS: usize = 6; // 预卷 ~200ms
const MAX_QUEUED_UTTERANCES: usize = 2;
const WINDOW_BYTES_48K: usize = 3072; // 512@16k ↔ 1536 采样@48k = 3072 字节
const HOLD_MAX_WINDOWS: usize = 78; // 保持态预算：累计停顿 78 窗（~2.5s）后强制提交

pub struct VadSegmenter {
    detector: Box<dyn VoiceActivityDetector>,
    // Smart Turn 完成度分析器：None 时保持纯 VAD 现状行为。
    turn_detector: Option<Box<dyn TurnCompletenessDetector>>,
    pending48: Vec<u8>,
    pending16: Vec<f32>,
    pre_roll: VecDeque<Vec<u8>>,
    utterance: Vec<u8>,
    queued: VecDeque<Vec<u8>>,
    voiced_run: usize,
    speech_windows: usize,
    silence_windows: usize,
    hold_windows: usize, // 保持态已消耗的停顿窗预算（0=非保持）
    active: bool,
    dropped: u32,
}

impl VadSegmenter {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self::with_detector(Box::new(SileroVad::new()?)))
    }

    pub fn with_detector(detector: Box<dyn VoiceActivityDetector>) -> Self {
        Self {
            detector,
            turn_detector: None,
            pending48: Vec::new(),
            pending16: Vec::new(),
            pre_roll: VecDeque::new(),
            utterance: Vec::new(),
            queued: VecDeque::new(),
            voiced_run: 0,
            speech_windows: 0,
            silence_windows: 0,
            hold_windows: 0,
            active: false,
            dropped: 0,
        }
    }

    /// 挂载回合完成度分析器：停顿提交点判“未说完”则不提交、进入保持态继续收集。
    pub fn with_turn_detector(mut self, detector: Box<dyn TurnCompletenessDetector>) -> Self {
        self.turn_detector = Some(detector);
        self
    }

    fn ingest_window(&mut self, raw48: Vec<u8>, prob: f32) {
        let voiced = prob >= START_PROB;
        if !self.active {
            self.pre_roll.push_back(raw48);
            while self.pre_roll.len() > PRE_ROLL_WINDOWS {
                self.pre_roll.pop_front();
            }
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            if self.voiced_run >= START_WINDOWS {
                self.active = true;
                self.utterance = self.pre_roll.drain(..).flatten().collect();
                self.speech_windows = self.voiced_run;
                self.silence_windows = 0;
            }
            return;
        }
        self.utterance.extend_from_slice(&raw48);
        if voiced {
            self.speech_windows += 1;
            self.silence_windows = 0;
        } else {
            if prob >= END_PROB {
                // 0.35..0.5 的过渡带按语音计，避免句尾拖音被截
                self.speech_windows += 1;
            }
            self.silence_windows += 1;
        }
        if self.silence_windows >= END_SILENCE_WINDOWS
            || self.utterance.len() / WINDOW_BYTES_48K >= MAX_UTTERANCE_WINDOWS
        {
            self.finish();
        }
    }

    fn finish(&mut self) {
        // Smart Turn 保持态：只在 VAD 判定语音结束后的语句缓冲上判分
        // （纯静音输入会被模型判“回合已完成”，静音绝不能单独送入）。
        if let Some(detector) = self.turn_detector.as_mut() {
            let samples16: Vec<f32> = {
                let b = downsample_48k_to_16k(&self.utterance);
                b.chunks_exact(2)
                    .map(|x| i16::from_le_bytes([x[0], x[1]]) as f32 / 32768.0)
                    .collect()
            };
            if samples16.len() >= 512 // 过短不判分
                && self.hold_windows < HOLD_MAX_WINDOWS
                && detector.score(&samples16) < TURN_COMPLETE_PROB
            {
                self.hold_windows += self.silence_windows; // 保持预算按停顿窗累计
                self.silence_windows = 0; // 继续等下一句
                self.active = true; // 保持态继续收集
                return;
            }
        }
        // 原提交逻辑不变
        if self.speech_windows >= MIN_SPEECH_WINDOWS {
            if self.queued.len() == MAX_QUEUED_UTTERANCES {
                self.queued.pop_front();
                self.dropped = self.dropped.saturating_add(1);
            }
            self.queued.push_back(std::mem::take(&mut self.utterance));
        }
        self.clear_current();
        self.hold_windows = 0;
    }

    fn clear_current(&mut self) {
        self.pending48.clear();
        self.pending16.clear();
        self.pre_roll.clear();
        self.utterance.clear();
        self.voiced_run = 0;
        self.speech_windows = 0;
        self.silence_windows = 0;
        self.hold_windows = 0;
        self.active = false;
    }
}

impl SpeechSegmenter for VadSegmenter {
    fn ingest(&mut self, pcm: &[u8], suppressed: bool) {
        if suppressed {
            self.clear_current();
            return;
        }
        self.pending48.extend_from_slice(pcm);
        let sixteen_k = downsample_48k_to_16k(pcm);
        self.pending16.extend(
            sixteen_k
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0),
        );
        while self.pending16.len() >= 512 {
            let window: Vec<f32> = self.pending16.drain(..512).collect();
            let raw48: Vec<u8> = {
                let take = self.pending48.len().min(WINDOW_BYTES_48K);
                self.pending48.drain(..take).collect()
            };
            let prob = self.detector.process(&window);
            self.ingest_window(raw48, prob);
        }
    }
    fn take(&mut self) -> Option<Vec<u8>> {
        self.queued.pop_front()
    }
    fn ready(&self) -> bool {
        !self.queued.is_empty()
    }
    fn dropped(&self) -> u32 {
        self.dropped
    }
    fn reset(&mut self) {
        // 取出真检测器再重建，只清状态机、不丢模型（turn 分析器同样保留）
        let detector = std::mem::replace(&mut self.detector, Box::new(sink_detector()));
        let turn_detector = self.turn_detector.take();
        *self = Self::with_detector(detector);
        self.turn_detector = turn_detector;
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// reset 瞬间的占位实现，仅用于 mem::replace 的交换，随即被真检测器顶替。
fn sink_detector() -> impl VoiceActivityDetector {
    struct Sink;
    impl VoiceActivityDetector for Sink {
        fn process(&mut self, _: &[f32]) -> f32 {
            0.0
        }
    }
    Sink
}

// VadSegmenter 状态机测试（FixedVad 逐窗返回脚本化概率，确定性可测）。
// 窗口换算：8 帧 20ms@48k = 5 个 VAD 窗口（512@16k = 32ms）；脚本耗尽后默认 0.0。
#[cfg(test)]
mod segmenter_tests {
    use super::*;
    use crate::audio::segmenter::SpeechSegmenter;

    struct FixedVad {
        probs: std::collections::VecDeque<f32>,
    }
    impl VoiceActivityDetector for FixedVad {
        fn process(&mut self, _window: &[f32]) -> f32 {
            // 脚本耗尽后视为静音
            self.probs.pop_front().unwrap_or(0.0)
        }
    }

    /// 每次判分返回固定得分的 Smart Turn 替身（得分 ≥0.5=回合已完成）。
    struct FixedTurn {
        score: f32,
    }
    impl TurnCompletenessDetector for FixedTurn {
        fn score(&mut self, _: &[f32]) -> f32 {
            self.score
        }
    }

    fn frames48(sample: i16, frames: usize) -> Vec<u8> {
        (0..frames * 960)
            .flat_map(|_| sample.to_le_bytes())
            .collect()
    }

    fn vad(probs: &[f32]) -> VadSegmenter {
        VadSegmenter::with_detector(Box::new(FixedVad {
            probs: probs.iter().copied().collect(),
        }))
    }

    #[test]
    fn silence_is_ignored_and_speech_commits_once_after_pause() {
        // 24 帧静音=15 窗(0.0)；40 帧语音=25 窗(0.9)；40 帧停顿=25 窗(0.0)
        let mut probs = vec![0.0; 18];
        probs.extend(vec![0.9; 22]);
        probs.extend(vec![0.0; 28]);
        let mut s = vad(&probs);
        s.ingest(&frames48(0, 24), false);
        assert!(!s.ready());
        s.ingest(&frames48(2000, 40), false);
        s.ingest(&frames48(0, 40), false); // 停顿超过 700ms（22 窗）
        assert!(s.ready());
        assert!(s.take().is_some());
        assert!(s.take().is_none());
    }

    #[test]
    fn echo_suppression_discards_partial_and_complete_echo() {
        let mut s = vad(&[]);
        s.ingest(&frames48(2000, 40), true); // 抑制期间一律丢弃
        s.ingest(&frames48(0, 40), false);
        assert!(!s.ready());
    }

    #[test]
    fn queue_is_bounded_and_reports_drops() {
        let mut probs = Vec::new();
        for _ in 0..3 {
            probs.extend(vec![0.9; 22]); // 每轮语音脚本 22 窗
            probs.extend(vec![0.0; 28]); // 每轮停顿脚本 28 窗
        }
        let mut s = vad(&probs);
        for _ in 0..3 {
            s.ingest(&frames48(2000, 40), false); // 40 帧 = 25 窗
            s.ingest(&frames48(0, 40), false); // 40 帧 = 25 窗
        }
        assert_eq!(s.dropped(), 1);
        assert!(s.take().is_some());
        assert!(s.take().is_some());
        assert!(s.take().is_none());
    }

    #[test]
    fn preroll_is_included_in_committed_utterance() {
        let mut probs = vec![0.0f32; 11]; // 开口前 ~384ms（6 窗预卷）
        probs.extend(vec![0.9; 22]);
        probs.extend(vec![0.0; 28]);
        let mut s = vad(&probs);
        s.ingest(&frames48(0, 16), false); // 16 帧 = 10 窗静音
        s.ingest(&frames48(2000, 40), false); // 25 窗：1 静音 + 22 语音 + 2 静音
        s.ingest(&frames48(0, 40), false); // 25 窗停顿触发提交
        let utterance = s.take().expect("utterance");
        // 提交长度必须 ≥ 语音段缓冲（22 窗 × 3072 字节@48k）
        assert!(
            utterance.len() >= 22 * 3072,
            "utterance too short: {}",
            utterance.len()
        );
    }

    #[test]
    fn max_utterance_is_force_submitted() {
        let probs = vec![0.9f32; 2000]; // 25 秒级别连续语音
        let mut s = vad(&probs);
        s.ingest(&frames48(2000, 800 * 3), false);
        assert!(s.ready());
    }

    #[test]
    fn incomplete_turn_holds_then_commits_within_budget() {
        // 脚本：语音 15 窗(0.9) → 停顿 5 窗(0.0)（触发判分，得分 0.2=未完）→
        // 继续静默 80 窗（0.0，保持超限 78 窗）→ 强制提交
        let mut audio = vec![0.9f32; 15];
        audio.extend(vec![0.0f32; 5 + 78]);
        let mut s = VadSegmenter::with_detector(Box::new(FixedVad {
            probs: audio.iter().copied().collect(),
        }))
        .with_turn_detector(Box::new(FixedTurn { score: 0.2 }));
        s.ingest(&frames48(2000, 24), false);
        s.ingest(&frames48(0, 249), false); // 15 语音窗 + 83 停顿窗（>78 保持上限）
        assert!(s.ready()); // 保持超限后必须提交
    }

    #[test]
    fn complete_turn_commits_immediately_without_hold() {
        // 得分 0.9=完整 → 停顿 22 窗后照常提交（判分不改变现状时序）
        let mut s = VadSegmenter::with_detector(Box::new(FixedVad {
            probs: std::iter::repeat_n(0.9f32, 30).collect(),
        }))
        .with_turn_detector(Box::new(FixedTurn { score: 0.9 }));
        s.ingest(&frames48(2000, 24), false);
        s.ingest(&frames48(0, 60), false); // 37 窗：15 窗补足 0.9 脚本 + 22 窗静音触发提交
        assert!(s.ready());
    }
}
