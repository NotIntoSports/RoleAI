//! 音频管线热路径基准。输入全部为确定性生成的信号（正弦/白噪声/静音），
//! 不依赖外部音频文件；Silero/Smart Turn 模型走仓库自带资源。
//!
//! 运行：`cargo bench --manifest-path src-tauri/Cargo.toml`（或 scripts/run-benchmarks.ps1）。

use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

use ai_virtual_assistant_desktop_lib::audio::pcm::resample_pcm16_mono;

/// 确定性正弦 PCM16LE 单声道（48 kHz）。
fn sine_pcm48(samples: usize, freq_hz: f32, amplitude: f32) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(samples * 2);
    for n in 0..samples {
        let t = n as f32 / 48_000.0;
        let v = (amplitude * (2.0 * std::f32::consts::PI * freq_hz * t).sin()).clamp(-1.0, 1.0);
        let sample = (v * 32767.0) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

fn bench_resample(c: &mut Criterion) {
    // 1 秒 48 kHz 正弦 → 16 kHz：实时管线里每个采集块的常规转换量级。
    let input = sine_pcm48(48_000, 220.0, 0.5);
    c.bench_function("pcm/resample_48k_to_16k_1s", |b| {
        b.iter(|| resample_pcm16_mono(black_box(&input), 48_000, 16_000))
    });
}

criterion_group!(benches, bench_resample);
criterion_main!(benches);
