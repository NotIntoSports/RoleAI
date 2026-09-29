//! 断句与打断离线评测（lane-C C42）。
//!
//! 把 `scripts/eval-audio/build-fixtures.mjs` 生成的场景音频按 32ms 实时帧
//! 喂给真实的 Silero VAD + Smart Turn + 分段器 + 打断检测（离线、不连网），
//! 对照标注输出：断句延迟、过早截断率、漏判率、回声误触发率、打断检出率与检出延迟。
//!
//! 对比实验：
//! 1. 只用 VAD vs VAD + Smart Turn（两级断句）；
//! 2. 打断检测：生产 BargeInMonitor（含回声底噪门控）vs 评测器内的
//!    纯概率阈值对照（"闸门关"，非生产实现，仅作对照披露）。
//!
//! 运行（先生成场景）：`node scripts/eval-audio/build-fixtures.mjs`
//! 然后：`cargo run --manifest-path src-tauri/Cargo.toml --release --example eval_turns`
//! 结果写入 `target/eval-results.json`。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ai_virtual_assistant_desktop_lib::audio::barge_in::BargeInMonitor;
use ai_virtual_assistant_desktop_lib::audio::pcm::downsample_48k_to_16k;
use ai_virtual_assistant_desktop_lib::audio::segmenter::SpeechSegmenter;
use ai_virtual_assistant_desktop_lib::audio::smart_turn::SmartTurnAnalyzer;
use ai_virtual_assistant_desktop_lib::audio::vad::{
    SileroVad, VadSegmenter, VoiceActivityDetector,
};
use ai_virtual_assistant_desktop_lib::services::bench_support::normalize;

const CHUNK_BYTES: usize = 3072; // 32ms @48kHz 16-bit mono
const TARGET_RATE: u32 = 48_000;

// ------------------------------------------------------------ WAV 读取 ----

fn read_wav_48k(path: &Path) -> Result<Vec<u8>, String> {
    let buffer = fs::read(path).map_err(|e| e.to_string())?;
    if &buffer[0..4] != b"RIFF" || &buffer[8..12] != b"WAVE" {
        return Err("not RIFF/WAVE".into());
    }
    let mut offset = 12usize;
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    let mut data: Option<Vec<u8>> = None;
    while offset + 8 <= buffer.len() {
        let id = &buffer[offset..offset + 4];
        let size = u32::from_le_bytes(buffer[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let body = offset + 8;
        match id {
            b"fmt " => {
                channels = u16::from_le_bytes(buffer[body + 2..body + 4].try_into().unwrap());
                rate = u32::from_le_bytes(buffer[body + 4..body + 8].try_into().unwrap());
                bits = u16::from_le_bytes(buffer[body + 14..body + 16].try_into().unwrap());
            }
            b"data" => {
                let end = (body + size).min(buffer.len());
                data = Some(buffer[body..end].to_vec());
            }
            _ => {}
        }
        offset = body + size + (size % 2);
    }
    let data = data.ok_or("missing data chunk")?;
    if channels != 1 || bits != 16 {
        return Err(format!("unsupported layout: {channels}ch {bits}bit"));
    }
    if rate == TARGET_RATE {
        Ok(data)
    } else {
        // 评测固定件由 SAPI 生成（22050Hz），线性重采样到 48k（构建侧同口径）。
        Ok(resample_linear(&data, rate, TARGET_RATE))
    }
}

fn resample_linear(pcm: &[u8], from: u32, to: u32) -> Vec<u8> {
    let input: Vec<f32> = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect();
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len * 2);
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let left = src.floor() as usize;
        let right = (left + 1).min(input.len() - 1);
        let frac = (src - left as f64) as f32;
        let value = input[left] * (1.0 - frac) + input[right] * frac;
        out.extend_from_slice(&((value.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

// ------------------------------------------------------------ 标注模型 ----

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Annotation {
    #[allow(dead_code)] // 标注里的场景名与 manifest 冗余，保留以校验文件身份
    scene: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    speech_segments: Vec<Segment>,
    #[serde(default)]
    events: Vec<Event>,
    #[serde(default)]
    expect_barge_in: Option<bool>,
    #[serde(default)]
    echo_events: Vec<serde_json::Value>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Segment {
    start: usize,
    end: usize,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    barge_in_onset: usize,
}

// ------------------------------------------------------------ 分段器包装 ----

struct TakenUtterance {
    start_byte: usize,
    end_byte: usize,
}

/// 用分段器跑完整个场景，记录每次取出的语句位置。
fn run_segmenter(
    audio: &[u8],
    with_smart_turn: bool,
) -> Result<(Vec<TakenUtterance>, usize), Box<dyn std::error::Error + Send + Sync>> {
    let mut segmenter = VadSegmenter::new()?;
    if with_smart_turn {
        let model_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/models/smart-turn-v3.2-cpu.onnx");
        let analyzer = SmartTurnAnalyzer::new(&model_path)?;
        segmenter = segmenter.with_turn_detector(Box::new(analyzer));
    }
    let mut taken = Vec::new();
    let mut consumed = 0usize;
    for chunk in audio.chunks(CHUNK_BYTES) {
        segmenter.ingest(chunk, false);
        consumed += chunk.len();
        while let Some(utterance) = segmenter.take() {
            taken.push(TakenUtterance {
                start_byte: consumed.saturating_sub(utterance.len()),
                end_byte: consumed,
            });
        }
    }
    // 冲刷尾部
    for _ in 0..40 {
        segmenter.ingest(&[], false);
        consumed += 0;
        while let Some(utterance) = segmenter.take() {
            taken.push(TakenUtterance {
                start_byte: consumed.saturating_sub(utterance.len()),
                end_byte: consumed,
            });
        }
    }
    Ok((taken, consumed))
}

fn overlap(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> usize {
    // 两个区间的交集长度（字节）。
    let start = a_start.max(b_start);
    let end = a_end.min(b_end);
    end.saturating_sub(start)
}

/// 分段质量：把检测到的语句按最大重叠归属到金标段。
fn segmentation_metrics(gold: &[Segment], taken: &[TakenUtterance]) -> serde_json::Value {
    let mut latencies_ms: Vec<f64> = Vec::new();
    let mut premature = 0usize;
    let mut missed = 0usize;
    let mut matched = 0usize;
    const PREMATURE_SLACK_MS: f64 = 300.0;
    for segment in gold {
        let mut best: Option<(usize, &TakenUtterance)> = None;
        for utterance in taken {
            let overlap_bytes = overlap(
                segment.start,
                segment.end,
                utterance.start_byte,
                utterance.end_byte,
            );
            if best
                .map(|(best_overlap, _)| overlap_bytes > best_overlap)
                .unwrap_or(overlap_bytes > 0)
            {
                best = Some((overlap_bytes, utterance));
            }
        }
        match best {
            None => missed += 1,
            Some((_, utterance)) => {
                matched += 1;
                let latency_bytes = utterance.end_byte as i64 - segment.end as i64;
                let latency_ms = latency_bytes as f64 / (TARGET_RATE as f64 * 2.0) * 1000.0;
                latencies_ms.push(latency_ms);
                // 检出语句的终点比金标终点早超过 300ms → 过早截断。
                if latency_ms < -PREMATURE_SLACK_MS {
                    premature += 1;
                }
            }
        }
    }
    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let percentile = |p: f64| -> f64 {
        if latencies_ms.is_empty() {
            return f64::NAN;
        }
        let index = ((latencies_ms.len() as f64 - 1.0) * p).round() as usize;
        latencies_ms[index]
    };
    serde_json::json!({
        "goldSegments": gold.len(),
        "detectedUtterances": taken.len(),
        "matched": matched,
        "missed": missed,
        "prematureTruncations": premature,
        "prematureRate": if gold.is_empty() { serde_json::Value::Null } else { serde_json::json!(premature as f64 / gold.len() as f64) },
        "latencyMs": {
            "median": percentile(0.5),
            "p95": percentile(0.95),
        },
    })
}

/// 生产打断检测：返回 (触发位置列表)。
fn run_barge_in_monitor(audio: &[u8]) -> Vec<usize> {
    let mut monitor =
        BargeInMonitor::with_detector(Box::new(SileroVad::new().expect("silero model must load")));
    let mut triggered_at = Vec::new();
    let mut consumed = 0usize;
    // 1s 不应期：同一次打断只记一个触发事件（用户持续说话会反复触发）。
    let mut last_trigger: Option<usize> = None;
    for chunk in audio.chunks(CHUNK_BYTES) {
        monitor.ingest(chunk);
        consumed += chunk.len();
        if monitor.triggered()
            && last_trigger
                .map(|at| consumed.saturating_sub(at) > TARGET_RATE as usize)
                .unwrap_or(true)
        {
            triggered_at.push(consumed);
            last_trigger = Some(consumed);
            monitor.take_staged();
        }
    }
    triggered_at
}

/// 对照（"闸门关"）：评测器内的纯概率阈值判据——连续 2 窗 ≥0.5 触发，
/// 约 1s 静音后重新武装。非生产实现，仅用于量化生产 monitor 的回声底噪
/// 门控（energy-assist + 底噪追踪）带来的误触发抑制。
struct ThresholdControl {
    detector: SileroVad,
    pending: Vec<f32>,
    voiced_run: usize,
    silence_run: usize,
    armed: bool,
}

impl ThresholdControl {
    fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self {
            detector: SileroVad::new()?,
            pending: Vec::new(),
            voiced_run: 0,
            silence_run: 0,
            armed: true,
        })
    }

    fn ingest(&mut self, pcm: &[u8]) -> bool {
        let sixteen_k = downsample_48k_to_16k(pcm);
        self.pending.extend(
            sixteen_k
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0),
        );
        let mut fired_now = false;
        while self.pending.len() >= 512 {
            let window: Vec<f32> = self.pending.drain(..512).collect();
            let prob = self.detector.process(&window);
            if prob >= 0.5 {
                self.voiced_run += 1;
                self.silence_run = 0;
            } else {
                self.voiced_run = 0;
                self.silence_run += 1;
            }
            if self.silence_run >= 32 {
                self.armed = true; // ~1s 静音：允许计下一次打断
            }
            if self.armed && self.voiced_run >= 2 {
                self.armed = false;
                self.voiced_run = 0;
                fired_now = true;
            }
        }
        fired_now
    }
}

fn run_threshold_control(audio: &[u8]) -> Vec<usize> {
    let Ok(mut control) = ThresholdControl::new() else {
        return Vec::new();
    };
    let mut fired_at = Vec::new();
    let mut consumed = 0usize;
    for chunk in audio.chunks(CHUNK_BYTES) {
        consumed += chunk.len();
        if control.ingest(chunk) {
            fired_at.push(consumed);
        }
    }
    fired_at
}

fn triggered_in_window(positions: &[usize], from: usize, until: usize) -> Option<f64> {
    positions
        .iter()
        .copied()
        .find(|position| *position >= from && *position <= until)
        .map(|position| (position - from) as f64 / (TARGET_RATE as f64 * 2.0) * 1000.0)
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if values.is_empty() {
        f64::NAN
    } else {
        values[values.len() / 2]
    }
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/eval-fixtures");
    let manifest_path = fixtures.join("manifest.json");
    if !manifest_path.exists() {
        return Err(format!(
            "评测集不存在：{}。先运行 node scripts/eval-audio/build-fixtures.mjs",
            manifest_path.display()
        )
        .into());
    }
    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(&manifest_path)?)?;
    let mut report = Vec::new();

    for entry in manifest.as_array().expect("manifest array") {
        let scene = entry["scene"].as_str().expect("scene").to_owned();
        let wav = PathBuf::from(entry["audio"].as_str().expect("audio path"));
        let annotation: Annotation = serde_json::from_str(&fs::read_to_string(
            entry["annotation"].as_str().expect("annotation path"),
        )?)?;
        eprintln!("== {scene} ==");
        let audio = read_wav_48k(&wav)?;

        // 断句评测（两档）：只 VAD vs VAD + Smart Turn。
        let (vad_only_taken, _) = run_segmenter(&audio, false)?;
        let (vad_st_taken, _) = run_segmenter(&audio, true)?;
        let vad_only = segmentation_metrics(&annotation.speech_segments, &vad_only_taken);
        let vad_smart = segmentation_metrics(&annotation.speech_segments, &vad_st_taken);

        // 打断/回声场景：生产 monitor vs 阈值对照（"闸门关"）。
        // 语义约定：回声场景期望零触发，任何触发都是误触发；
        // 打断场景每个 onset 在其后 1.5s 内出现触发即视为检出。
        let mut barge_in = serde_json::Value::Null;
        if annotation.expect_barge_in.is_some() {
            let monitor_hits = run_barge_in_monitor(&audio);
            let control_hits = run_threshold_control(&audio);
            if scene == "barge_in" {
                let mut detection_latencies: Vec<f64> = Vec::new();
                let mut detected = 0usize;
                for event in &annotation.events {
                    let until = event.barge_in_onset + TARGET_RATE as usize * 3; // 1.5s 窗
                    if let Some(latency_ms) =
                        triggered_in_window(&monitor_hits, event.barge_in_onset, until)
                    {
                        detected += 1;
                        detection_latencies.push(latency_ms);
                    }
                }
                barge_in = serde_json::json!({
                    "expectedOnsets": annotation.events.len(),
                    "detectedOnsets": detected,
                    "firstTriggerLatencyMs": if detection_latencies.is_empty() { serde_json::Value::Null } else { serde_json::json!(median(&mut detection_latencies)) },
                    "monitorTriggerEvents": monitor_hits.len(),
                    "thresholdControlEvents": control_hits.len(),
                });
            } else {
                // 回声场景：闸门开（生产 monitor）与关（阈值对照）都应静默。
                barge_in = serde_json::json!({
                    "expectedOnsets": 0,
                    "monitorTriggerEvents": monitor_hits.len(),
                    "thresholdControlEvents": control_hits.len(),
                    "falseTriggered": !monitor_hits.is_empty(),
                    "echoEvents": annotation.echo_events.len(),
                });
            }
        }

        report.push(serde_json::json!({
            "scene": scene,
            "description": annotation.description,
            "segmentation": { "vadOnly": vad_only, "vadPlusSmartTurn": vad_smart },
            "bargeIn": barge_in,
        }));
    }

    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/eval-results.json");
    fs::write(&output, serde_json::to_string_pretty(&report)?)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    println!("written to {}", output.display());
    let _ = normalize; // bench_support 引入保留：与生产同源的文本规一化（供后续 ASR 文本侧评测）
    Ok(())
}

// serde derive 需要显式 allow 之外的最小依赖面：复用 crate 已有 serde/serde_json。
#[allow(dead_code)]
fn _assert_traits() {
    fn is_send<T: Send>() {}
    is_send::<SileroVad>();
    is_send::<VadSegmenter>();
    is_send::<Duration>();
}
