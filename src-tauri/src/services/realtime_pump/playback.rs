//! 播放流抽象与播放模式枚举（纯搬移自 realtime_pump.rs）。

use super::*;

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
