//! Deterministic PCM16 VAD/utterance segmentation with bounded memory.
use std::collections::VecDeque;

const FRAME_SAMPLES: usize = 960; // 20 ms at 48 kHz
const FRAME_BYTES: usize = FRAME_SAMPLES * 2;
const START_FRAMES: usize = 3;
const END_SILENCE_FRAMES: usize = 35; // 700 ms
const MIN_SPEECH_FRAMES: usize = 10; // 200 ms
const MAX_UTTERANCE_FRAMES: usize = 1_250; // 25 seconds
const PRE_ROLL_FRAMES: usize = 10;
const MAX_QUEUED_UTTERANCES: usize = 2;
const VOICE_PEAK: i16 = 500;

#[derive(Debug, Default)]
pub struct UtteranceSegmenter {
    pending: Vec<u8>,
    pre_roll: VecDeque<Vec<u8>>,
    utterance: Vec<u8>,
    queued: VecDeque<Vec<u8>>,
    voiced_run: usize,
    speech_frames: usize,
    silence_frames: usize,
    active: bool,
    dropped: u32,
}

impl UtteranceSegmenter {
    pub fn ingest(&mut self, pcm: &[u8], suppressed: bool) {
        if suppressed {
            self.clear_current();
            return;
        }
        self.pending.extend_from_slice(pcm);
        let complete = self.pending.len() / FRAME_BYTES * FRAME_BYTES;
        let frames: Vec<Vec<u8>> = self.pending[..complete]
            .chunks_exact(FRAME_BYTES)
            .map(<[u8]>::to_vec)
            .collect();
        self.pending.drain(..complete);
        for frame in frames {
            self.ingest_frame(frame);
        }
    }

    pub fn take(&mut self) -> Option<Vec<u8>> {
        self.queued.pop_front()
    }
    pub fn ready(&self) -> bool {
        !self.queued.is_empty()
    }
    pub fn dropped(&self) -> u32 {
        self.dropped
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn ingest_frame(&mut self, frame: Vec<u8>) {
        let voiced = frame
            .chunks_exact(2)
            .any(|s| i16::from_le_bytes([s[0], s[1]]).unsigned_abs() >= VOICE_PEAK as u16);
        if !self.active {
            self.pre_roll.push_back(frame.clone());
            while self.pre_roll.len() > PRE_ROLL_FRAMES {
                self.pre_roll.pop_front();
            }
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            if self.voiced_run >= START_FRAMES {
                self.active = true;
                self.utterance = self.pre_roll.drain(..).flatten().collect();
                self.speech_frames = self.voiced_run;
                self.silence_frames = 0;
            }
            return;
        }
        self.utterance.extend_from_slice(&frame);
        if voiced {
            self.speech_frames += 1;
            self.silence_frames = 0;
        } else {
            self.silence_frames += 1;
        }
        if self.silence_frames >= END_SILENCE_FRAMES
            || self.utterance.len() / FRAME_BYTES >= MAX_UTTERANCE_FRAMES
        {
            self.finish();
        }
    }

    fn finish(&mut self) {
        if self.speech_frames >= MIN_SPEECH_FRAMES {
            if self.queued.len() == MAX_QUEUED_UTTERANCES {
                self.queued.pop_front();
                self.dropped = self.dropped.saturating_add(1);
            }
            self.queued.push_back(std::mem::take(&mut self.utterance));
        }
        self.clear_current();
    }

    fn clear_current(&mut self) {
        self.pending.clear();
        self.pre_roll.clear();
        self.utterance.clear();
        self.voiced_run = 0;
        self.speech_frames = 0;
        self.silence_frames = 0;
        self.active = false;
    }
}

/// 语句分段器统一接口：捕获路径不感知具体实现（能量门限 / Silero VAD）。
pub trait SpeechSegmenter: Send {
    fn ingest(&mut self, pcm: &[u8], suppressed: bool);
    fn take(&mut self) -> Option<Vec<u8>>;
    fn ready(&self) -> bool;
    fn dropped(&self) -> u32;
    fn reset(&mut self);
}

impl SpeechSegmenter for UtteranceSegmenter {
    fn ingest(&mut self, pcm: &[u8], suppressed: bool) {
        self.ingest(pcm, suppressed);
    }
    fn take(&mut self) -> Option<Vec<u8>> {
        self.take()
    }
    fn ready(&self) -> bool {
        self.ready()
    }
    fn dropped(&self) -> u32 {
        self.dropped()
    }
    fn reset(&mut self) {
        self.reset()
    }
}

/// 工厂：Task 4 接入 Silero 选择逻辑，本任务先固定能量实现。
pub fn default_segmenter() -> Box<dyn SpeechSegmenter> {
    Box::new(UtteranceSegmenter::default())
}

/// `CaptureState`/`AudioCapture` 派生 `Debug` 需要；分段器内部状态不进调试输出。
impl std::fmt::Debug for Box<dyn SpeechSegmenter> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeechSegmenter").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(sample: i16) -> Vec<u8> {
        std::iter::repeat_n(sample.to_le_bytes(), FRAME_SAMPLES)
            .flatten()
            .collect()
    }
    #[test]
    fn silence_is_ignored_and_speech_commits_once_after_pause() {
        let mut vad = UtteranceSegmenter::default();
        for _ in 0..50 {
            vad.ingest(&frame(0), false);
        }
        assert!(!vad.ready());
        for _ in 0..12 {
            vad.ingest(&frame(2000), false);
        }
        for _ in 0..35 {
            vad.ingest(&frame(0), false);
        }
        assert!(vad.take().is_some());
        assert!(vad.take().is_none());
    }
    #[test]
    fn echo_suppression_discards_partial_and_complete_echo() {
        let mut vad = UtteranceSegmenter::default();
        for _ in 0..20 {
            vad.ingest(&frame(2000), true);
        }
        for _ in 0..40 {
            vad.ingest(&frame(0), false);
        }
        assert!(!vad.ready());
    }
    #[test]
    fn queue_is_bounded_and_reports_drops() {
        let mut vad = UtteranceSegmenter::default();
        for _ in 0..3 {
            for _ in 0..10 {
                vad.ingest(&frame(2000), false);
            }
            for _ in 0..35 {
                vad.ingest(&frame(0), false);
            }
        }
        assert_eq!(vad.dropped(), 1);
        assert!(vad.take().is_some());
        assert!(vad.take().is_some());
        assert!(vad.take().is_none());
    }
}
