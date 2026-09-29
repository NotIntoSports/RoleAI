//! 播报期间的打断监听：连续人声窗口达到阈值即触发，缓冲从开口起的原文。
use crate::audio::pcm::downsample_48k_to_16k;
use crate::audio::vad::VoiceActivityDetector;

const START_PROB: f32 = 0.5;
const TRIGGER_WINDOWS: usize = 8; // 连续 ~256ms 人声即触发（插话要跟手）
const WINDOW_BYTES_48K: usize = 3072;

pub struct BargeInMonitor {
    detector: Box<dyn VoiceActivityDetector>,
    pending48: Vec<u8>,
    pending16: Vec<f32>,
    buffer: Vec<u8>,
    voiced_run: usize,
    staged: Option<Vec<u8>>,
}

// 检测器不可 Debug；CaptureState 派生 Debug 时只暴露可观测状态。
impl std::fmt::Debug for BargeInMonitor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BargeInMonitor")
            .field("buffered_windows", &(self.buffer.len() / WINDOW_BYTES_48K))
            .field("voiced_run", &self.voiced_run)
            .field("staged", &self.staged.is_some())
            .finish()
    }
}

impl BargeInMonitor {
    pub fn with_detector(detector: Box<dyn VoiceActivityDetector>) -> Self {
        Self {
            detector,
            pending48: Vec::new(),
            pending16: Vec::new(),
            buffer: Vec::new(),
            voiced_run: 0,
            staged: None,
        }
    }

    pub fn ingest(&mut self, pcm: &[u8]) {
        if self.staged.is_some() {
            return; // 已触发未取走，停止收集
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
            let voiced = self.detector.process(&window) >= START_PROB;
            self.buffer.extend_from_slice(&raw48);
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            if self.voiced_run >= TRIGGER_WINDOWS {
                self.staged = Some(std::mem::take(&mut self.buffer));
                self.voiced_run = 0;
                return;
            }
            while self.buffer.len() > 40 * WINDOW_BYTES_48K {
                // 缓冲上界 ~20s：只保留最近 40 窗，防止整段播报期间无限积累
                let drop = self.buffer.len() - 40 * WINDOW_BYTES_48K;
                self.buffer.drain(..drop);
            }
        }
    }

    pub fn triggered(&self) -> bool {
        self.staged.is_some()
    }

    pub fn take_staged(&mut self) -> Option<Vec<u8>> {
        self.staged.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::vad::VoiceActivityDetector;

    struct FixedVad {
        probs: std::collections::VecDeque<f32>,
    }
    impl VoiceActivityDetector for FixedVad {
        fn process(&mut self, _: &[f32]) -> f32 {
            self.probs.pop_front().unwrap_or(0.0)
        }
    }

    fn monitor(probs: &[f32]) -> BargeInMonitor {
        BargeInMonitor::with_detector(Box::new(FixedVad {
            probs: probs.iter().copied().collect(),
        }))
    }

    fn frames48(sample: i16, frames: usize) -> Vec<u8> {
        (0..frames * 960)
            .flat_map(|_| sample.to_le_bytes())
            .collect()
    }

    #[test]
    fn sustained_voice_triggers_and_stages_buffered_speech() {
        let mut probs = vec![0.9f32; 40]; // 连续语音 ≥8 窗触发
        probs.extend(vec![0.0; 40]);
        let mut m = monitor(&probs);
        m.ingest(&frames48(2000, 60)); // 60 帧 ≈ 37 窗语音 → 触发
        assert!(m.triggered());
        let utterance = m.take_staged().expect("staged");
        assert!(utterance.len() >= 8 * 3072);
        assert!(m.take_staged().is_none()); // 已取走
    }

    #[test]
    fn sporadic_voice_below_threshold_does_not_trigger() {
        let mut probs = Vec::new();
        for _ in 0..10 {
            probs.push(0.9);
            probs.push(0.0);
        } // 交替 = 无连续
        probs.extend(vec![0.0; 40]);
        let mut m = monitor(&probs);
        m.ingest(&frames48(2000, 120));
        assert!(m.take_staged().is_none());
    }

    #[test]
    fn silence_never_triggers() {
        let mut m = monitor(&[]);
        m.ingest(&frames48(0, 200));
        assert!(m.take_staged().is_none());
    }
}
