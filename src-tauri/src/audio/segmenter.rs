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
/// `Any` 超trait 提供向下转型能力，供工厂语义测试与调用方按需区分实现。
pub trait SpeechSegmenter: Send + std::any::Any {
    fn ingest(&mut self, pcm: &[u8], suppressed: bool);
    fn take(&mut self) -> Option<Vec<u8>>;
    fn ready(&self) -> bool;
    fn dropped(&self) -> u32;
    fn reset(&mut self);
    fn as_any(&self) -> &dyn std::any::Any;
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
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// 工厂：`AI_VOICE_VAD=off` 强制能量门限实现；否则优先 Silero VAD，
/// 模型加载失败（缺模型/初始化异常）降级回能量门限，保证采集路径永不断流。
// 测试注意：任何依赖 Silero 行为（或能量行为）的测试必须持 `factory_test_support`
// 的 AI_VOICE_VAD 测试锁；无锁读者与 set_var 并发在部分平台是数据竞争。
pub fn default_segmenter() -> Box<dyn SpeechSegmenter> {
    if std::env::var("AI_VOICE_VAD").as_deref() == Ok("off") {
        tracing::warn!("AI_VOICE_VAD=off：Silero VAD 被关闭，已降级为能量门限分段");
        return Box::new(UtteranceSegmenter::default());
    }
    match crate::audio::vad::VadSegmenter::new() {
        Ok(vad) => Box::new(vad),
        Err(error) => {
            tracing::warn!(%error, "Silero VAD 不可用（模型加载失败），已降级为能量门限分段");
            Box::new(UtteranceSegmenter::default())
        }
    }
}

/// 测试辅助：工厂实现选择依赖进程级环境变量，跨模块用例共用一把锁串行执行，
/// 避免并行测试互扰。仅 `cfg(test)` 编译，不进产物。
#[cfg(test)]
pub(crate) mod factory_test_support {
    use std::sync::{Mutex, MutexGuard};

    static FACTORY_ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        FACTORY_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// RAII 守卫：构造时保存 AI_VOICE_VAD 原值并置为 off，Drop 时恢复原值（无原值则删除）。
    /// 闭包内断言失败 panic 展开时同样恢复，off 开关不会泄漏到进程内其余用例。
    struct VadOffGuard {
        previous: Option<String>,
    }

    impl VadOffGuard {
        fn set() -> Self {
            let previous = std::env::var("AI_VOICE_VAD").ok();
            // 单测进程内一次性环境变量设置（Rust 2024 要求 unsafe）
            unsafe { std::env::set_var("AI_VOICE_VAD", "off") };
            Self { previous }
        }
    }

    impl Drop for VadOffGuard {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => {
                    // 单测进程内一次性环境变量恢复（Rust 2024 要求 unsafe）
                    unsafe { std::env::set_var("AI_VOICE_VAD", value) };
                }
                None => {
                    // 单测进程内一次性环境变量清理（Rust 2024 要求 unsafe）
                    unsafe { std::env::remove_var("AI_VOICE_VAD") };
                }
            }
        }
    }

    /// 持锁期间以 `AI_VOICE_VAD=off` 强制能量实现运行闭包，正常返回与 panic 展开均恢复原值。
    pub(crate) fn with_vad_off<T>(f: impl FnOnce() -> T) -> T {
        let _guard = lock();
        let _off = VadOffGuard::set();
        f()
    }
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

    // 工厂语义由进程级环境变量决定，两个工厂用例与跨模块管线用例共用
    // `factory_test_support` 的锁串行执行，避免并行互扰。
    #[test]
    fn factory_off_switch_returns_energy_implementation() {
        // 复用 with_vad_off：持锁、保存并恢复原值、panic 展开亦不泄漏 off 开关
        let s = factory_test_support::with_vad_off(super::default_segmenter);
        assert!(s.as_any().downcast_ref::<UtteranceSegmenter>().is_some());
    }

    #[test]
    fn factory_default_prefers_silero_vad() {
        let _guard = factory_test_support::lock();
        // 单测进程内一次性环境变量清理（确保开关未残留）
        unsafe { std::env::remove_var("AI_VOICE_VAD") };
        let s = super::default_segmenter();
        assert!(
            s.as_any()
                .downcast_ref::<crate::audio::vad::VadSegmenter>()
                .is_some(),
            "默认必须优先 Silero VAD 实现"
        );
    }
}
