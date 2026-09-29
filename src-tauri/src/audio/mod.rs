//! 音频采集、播放与热路径处理：PCM 重采样与格式转换（pcm）、Silero VAD 分段（segmenter）、
//! 打断检测（barge_in）与播放/采集设备抽象（playback/capture）。
//! 采样率常量见 `ASR_SAMPLE_RATE`；所有热路径以 48kHz 单帧为基本单位。

pub mod barge_in;
pub mod capture;
pub mod monitor;
pub mod pcm;
pub mod playback;
pub mod segmenter;
pub mod smart_turn;
pub mod vad;

pub use capture::{
    AudioCapture, AudioError, NoopSink, PlaybackSink, RecordingSink, SidecarPoll,
    bridge_command_args, parse_level_peak,
};
pub use pcm::{
    ASR_SAMPLE_RATE, CAPTURE_SAMPLE_RATE, PcmRing, RING_CAPACITY_BYTES, downsample_48k_to_16k,
    resample_pcm16_mono,
};
