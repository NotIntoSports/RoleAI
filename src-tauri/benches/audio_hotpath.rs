//! 音频管线热路径基准（C32）。输入全部为确定性生成的信号（正弦/白噪声/静音），
//! 不依赖外部音频文件；Silero/Smart Turn 模型走仓库自带资源（tests 同款加载方式）。
//!
//! 运行：`cargo bench --manifest-path src-tauri/Cargo.toml`（或 scripts/run-benchmarks.ps1）。
//! 口径：除非另注，单次迭代 = 一个实时采集块（3072 字节 = 32ms@48kHz 16-bit 单声道）
//! 或一次模型推理。

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::Path;
use std::time::Duration;

use ai_virtual_assistant_desktop_lib::audio::barge_in::BargeInMonitor;
use ai_virtual_assistant_desktop_lib::audio::pcm::{
    PcmRing, RING_CAPACITY_BYTES, resample_pcm16_mono,
};
use ai_virtual_assistant_desktop_lib::audio::segmenter::{SpeechSegmenter, UtteranceSegmenter};
use ai_virtual_assistant_desktop_lib::audio::smart_turn::{
    SmartTurnAnalyzer, TurnCompletenessDetector,
};
use ai_virtual_assistant_desktop_lib::audio::vad::{
    SileroVad, VadSegmenter, VoiceActivityDetector,
};
use ai_virtual_assistant_desktop_lib::services::bench_support::{
    gate_drain_deadline_from_bytes, gate_timers_expired, is_echo, normalize,
};

/// 采集块字节数：1536 采样 @48 kHz 16-bit 单声道（= 512 @16 kHz，一个 VAD 窗）。
const CHUNK_BYTES: usize = 3072;

/// 确定性 LCG 白噪声（避免依赖 rand crate；跨平台/跨运行可复现）。
struct Lcg(u64);

impl Lcg {
    fn next_f32(&mut self) -> f32 {
        // 数值分析教材级参数（Numerical Recipes）。
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as f32 / (u32::MAX >> 1) as f32) - 1.0
    }
}

/// 确定性正弦 + 可选白噪声 PCM16LE 单声道（48 kHz）。
fn synth_pcm48(samples: usize, freq_hz: f32, amplitude: f32, noise: f32, seed: u64) -> Vec<u8> {
    let mut rng = Lcg(seed);
    let mut pcm = Vec::with_capacity(samples * 2);
    for n in 0..samples {
        let t = n as f32 / 48_000.0;
        let sine = amplitude * (2.0 * std::f32::consts::PI * freq_hz * t).sin();
        let mut v = sine + noise * rng.next_f32();
        v = v.clamp(-1.0, 1.0);
        let sample = (v * 32767.0) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
    }
    pcm
}

/// 16 kHz f32 采样（VAD/Smart Turn 输入口径）。
fn synth_f32_16k(samples: usize, freq_hz: f32, amplitude: f32, seed: u64) -> Vec<f32> {
    let mut rng = Lcg(seed);
    (0..samples)
        .map(|n| {
            let t = n as f32 / 16_000.0;
            (amplitude * (2.0 * std::f32::consts::PI * freq_hz * t).sin() + 0.02 * rng.next_f32())
                .clamp(-1.0, 1.0)
        })
        .collect()
}

fn bench_pcm(c: &mut Criterion) {
    let mut group = c.benchmark_group("pcm");
    let one_second = synth_pcm48(48_000, 220.0, 0.5, 0.0, 7);
    group.throughput(Throughput::Bytes(one_second.len() as u64));
    group.bench_function("resample_48k_to_16k_1s", |b| {
        b.iter(|| resample_pcm16_mono(black_box(&one_second), 48_000, 16_000))
    });

    // 环形缓冲：先灌满 3 秒容量，再测稳态写入（含覆盖旧数据的分配路径）。
    let mut ring = PcmRing::new();
    let chunk = synth_pcm48(CHUNK_BYTES / 2, 220.0, 0.5, 0.0, 1);
    for _ in 0..(RING_CAPACITY_BYTES / chunk.len() + 2) {
        ring.push(&chunk);
    }
    group.throughput(Throughput::Bytes(chunk.len() as u64));
    group.bench_function("ring_push_1536_samples_steady_state", |b| {
        b.iter(|| ring.push(black_box(&chunk)))
    });
    group.finish();
}

fn bench_vad(c: &mut Criterion) {
    let mut group = c.benchmark_group("vad");
    let mut silero = SileroVad::new().expect("silero model must load");
    let silence = vec![0f32; 512];
    let speech = synth_f32_16k(512, 200.0, 0.4, 11);
    group.bench_function("silero_single_frame_silence", |b| {
        b.iter(|| silero.process(black_box(&silence)))
    });
    group.bench_function("silero_single_frame_speech", |b| {
        b.iter(|| silero.process(black_box(&speech)))
    });
    group.finish();
}

fn bench_smart_turn(c: &mut Criterion) {
    let mut group = c.benchmark_group("smart_turn");
    let model_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/models/smart-turn-v3.2-cpu.onnx");
    let mut analyzer = SmartTurnAnalyzer::new(&model_path).expect("load smart turn model");
    // 4 秒 16 kHz 语音段（与单元测试同量级）：一次完整"特征 + 推理"。
    let turn = synth_f32_16k(64_000, 180.0, 0.35, 23);
    group.throughput(Throughput::Bytes((turn.len() * 4) as u64));
    group.bench_function("score_4s_turn", |b| {
        b.iter(|| analyzer.score(black_box(&turn)))
    });
    group.finish();
}

fn bench_segmenter(c: &mut Criterion) {
    let mut group = c.benchmark_group("segmenter");
    let chunk = synth_pcm48(CHUNK_BYTES / 2, 220.0, 0.4, 0.01, 31);

    // 能量分段器：连续语音块稳态路径。
    let mut energy = UtteranceSegmenter::default();
    group.throughput(Throughput::Bytes(chunk.len() as u64));
    group.bench_function("utterance_energy_ingest_32ms", |b| {
        b.iter(|| energy.ingest(black_box(&chunk), false))
    });

    // VAD 分段器（含 Silero 推理 + Smart Turn 完整性判定挂载）。
    let mut vad_segmenter = VadSegmenter::new().expect("vad segmenter");
    group.bench_function("vad_ingest_32ms_with_silero", |b| {
        b.iter(|| vad_segmenter.ingest(black_box(&chunk), false))
    });

    // 打断检测：同样喂 32ms 块（含下行重采样 + RMS + 底噪追踪）。
    let mut barge = BargeInMonitor::with_detector(Box::new(SileroVad::new().expect("silero")));
    group.bench_function("barge_in_ingest_32ms_with_silero", |b| {
        b.iter(|| barge.ingest(black_box(&chunk)))
    });
    group.finish();
}

fn bench_echo_guard(c: &mut Criterion) {
    let mut group = c.benchmark_group("echo_guard");
    let recent = vec![
        normalize("我们这次主要讨论数据库的连接池配置，需要把最大连接数上调。"),
        normalize("好的，那么下一轮我会针对索引和查询计划给出具体建议。"),
    ];
    group.bench_function("is_echo_typical_turn", |b| {
        b.iter(|| {
            is_echo(
                black_box("我觉得连接池这边可以先保持现状再观察"),
                black_box(&recent),
            )
        })
    });
    group.bench_function("normalize_200_chars", |b| {
        let text = "这是一个用来测量回声文本规一化开销的样例。".repeat(8);
        b.iter(|| normalize(black_box(&text)))
    });
    group.bench_function("gate_deadline_24k_bytes", |b| {
        b.iter(|| {
            let now = std::time::Instant::now();
            let deadline: Option<std::time::Instant> =
                Some(gate_drain_deadline_from_bytes(now, black_box(24_000)));
            gate_timers_expired(deadline, None, now)
        })
    });
    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .sample_size(30)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(1));
    targets = bench_pcm, bench_vad, bench_smart_turn, bench_segmenter, bench_echo_guard
);
criterion_main!(benches);
