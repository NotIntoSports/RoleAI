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

pub struct VadSegmenter {
    detector: Box<dyn VoiceActivityDetector>,
    pending48: Vec<u8>,
    pending16: Vec<f32>,
    pre_roll: VecDeque<Vec<u8>>,
    utterance: Vec<u8>,
    queued: VecDeque<Vec<u8>>,
    voiced_run: usize,
    speech_windows: usize,
    silence_windows: usize,
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
            pending48: Vec::new(),
            pending16: Vec::new(),
            pre_roll: VecDeque::new(),
            utterance: Vec::new(),
            queued: VecDeque::new(),
            voiced_run: 0,
            speech_windows: 0,
            silence_windows: 0,
            active: false,
            dropped: 0,
        }
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
        if self.speech_windows >= MIN_SPEECH_WINDOWS {
            if self.queued.len() == MAX_QUEUED_UTTERANCES {
                self.queued.pop_front();
                self.dropped = self.dropped.saturating_add(1);
            }
            self.queued.push_back(std::mem::take(&mut self.utterance));
        }
        self.clear_current();
    }

    fn clear_current(&mut self) {
        self.pending48.clear();
        self.pending16.clear();
        self.pre_roll.clear();
        self.utterance.clear();
        self.voiced_run = 0;
        self.speech_windows = 0;
        self.silence_windows = 0;
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
        // 取出真检测器再重建，只清状态机、不丢模型
        let detector = std::mem::replace(&mut self.detector, Box::new(sink_detector()));
        *self = Self::with_detector(detector);
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
}
