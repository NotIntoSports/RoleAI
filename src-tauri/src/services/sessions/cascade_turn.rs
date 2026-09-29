//! cascade_turn 子模块：级联单轮的 PCM 推入与诊断句柄。
//! 语音成句 finalize（三阶段锁拆分）见 finalize.rs。
use super::*;

impl<S: PlaybackSink> SessionService<S> {
    pub fn push_pcm(&mut self, pcm: &[u8]) {
        self.capture.push_pcm(pcm);
    }

    /// 诊断：tap 满丢帧累计（实时上行背压观测）。
    pub fn capture_tap_dropped(&self) -> u64 {
        self.capture.tap_dropped()
    }

    /// 无锁热路径句柄（AppState 持有，IPC 推流不再等 sessions 锁）。
    pub fn mic_ingest_handle(&self) -> crate::audio::capture::MicIngestHandle {
        self.capture.mic_ingest_handle()
    }
}
