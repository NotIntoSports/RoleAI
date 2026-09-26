//! Smart Turn 语义断句（回合完成度）分析器：Whisper 对数梅尔特征 + ONNX 推理。
//!
//! 模型：pipecat-ai/smart-turn-v3 的 `smart-turn-v3.2-cpu.onnx`（BSD-2-Clause，int8 量化，
//! 8,679,182 字节），随包放 `resources/models/`。
//! 推理管线对照 pipecat-ai/smart-turn `inference.py` 核对（Step 1 结论）：
//! 1. 窗长固定 8s@16kHz = 128_000 采样：超长截最后 8s、不足**左补零**（最近的语音贴着窗尾）
//!    （`audio_utils.truncate_audio_to_last_n_seconds`：`np.pad(audio, (padding, 0))`）。
//! 2. 波形零均值/单位方差归一化（`WhisperFeatureExtractor(do_normalize=True)`：
//!    `(x - mean) / sqrt(var + 1e-7)`，总体方差）。
//! 3. 对数梅尔谱（HF transformers `WhisperFeatureExtractor`，torch 路径）：hann(400, periodic)
//!    窗、hop=160、两侧反射补 200（`torch.stft(center=True)` 默认 reflect）、|STFT|² 取前 201
//!    频点、丢最后一帧得 800 帧、slaney/slaney 梅尔滤波器组（201→80，0–8kHz）、log10
//!    （下限 1e-10）、全域 max-8 钳制、`(x + 4) / 4`。
//! 4. ONNX 输入 `input_features` [1, 80, 800] f32（已从模型文件解析核对）；输出 `logits`
//!    [1, 1] —— 导出图内已过 sigmoid（train.py 前向返回 `torch.sigmoid(logits)`），读出即概率。
use std::path::Path;
use std::sync::Arc;

use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use rustfft::{num_complex::Complex, FftPlanner};

// —— Whisper 特征参数（transformers WhisperFeatureExtractor 默认值，8s 窗）——
const MEL_BINS: usize = 80;
const FREQUENCY_BINS: usize = 201; // n_fft / 2 + 1
const N_FFT: usize = 400;
const HOP_LENGTH: usize = 160;
const SAMPLE_RATE: usize = 16_000;
const CHUNK_SECONDS: usize = 8;
const FRAMES_PER_CHUNK: usize = CHUNK_SECONDS * SAMPLE_RATE / HOP_LENGTH; // 800
const LOG_MEL_FLOOR: f32 = 1e-10;

/// “回合已完成”概率阈值：官方示例 `probability > 0.5 → prediction = 1`。
pub const TURN_COMPLETE_PROB: f32 = 0.5;

/// 回合完成度检测器：输入一段 16kHz 单声道采样，返回“回合已完成”的概率 0.0..=1.0。
/// Task 2 的语义断句器消费此抽象。
pub trait TurnCompletenessDetector: Send {
    fn score(&mut self, samples_16k: &[f32]) -> f32;
}

pub struct SmartTurnAnalyzer {
    session: ort::session::Session,
    // 输入名/窗长/帧数按 Step 1 核对：input_features [batch, 80, 800]，窗 = 帧 × hop。
    input_name: String,
    window: usize,
    frames: usize,
    // 预计算的常量表与 FFT 计划。
    filters: Vec<f32>,   // [mel_bins × FREQUENCY_BINS] slaney/slaney 梅尔滤波器组
    window_fn: Vec<f32>, // hann periodic，N_FFT 点
    fft: Arc<dyn rustfft::Fft<f32>>,
    fft_scratch: Vec<Complex<f32>>,
    // 每次评分复用的缓冲区，避免实时路径反复分配。
    sample_buf: Vec<f32>,     // 左补零后的固定窗采样
    padded_buf: Vec<f32>,     // 两侧反射补边后 window + N_FFT
    frame_buf: Vec<Complex<f32>>,
    spectrum_buf: Vec<f32>,   // 单帧功率谱 FREQUENCY_BINS
    features: Vec<f32>,       // [mel_bins × frames] 对数梅尔特征
}

impl SmartTurnAnalyzer {
    pub fn new(model_path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let session = ort::session::Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level1)?
            .commit_from_file(model_path)?;
        // 帧数/梅尔数从模型输入形状读出（[batch, mel, frames]，batch 为动态 -1），
        // 读不出时按官方示例常量 80×800。
        let (mel_bins, frames) = match session.inputs.first().map(|input| &input.input_type) {
            Some(ort::value::ValueType::Tensor { shape, .. }) => {
                let m = shape.get(1).copied().unwrap_or(MEL_BINS as i64);
                let f = shape.get(2).copied().unwrap_or(FRAMES_PER_CHUNK as i64);
                (
                    if m > 0 { m as usize } else { MEL_BINS },
                    if f > 0 { f as usize } else { FRAMES_PER_CHUNK },
                )
            }
            _ => (MEL_BINS, FRAMES_PER_CHUNK),
        };
        let input_name = session
            .inputs
            .first()
            .map(|input| input.name.clone())
            .unwrap_or_else(|| "input_features".to_string());
        let window = frames * HOP_LENGTH;

        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N_FFT);
        let fft_scratch = vec![
            Complex::new(0f32, 0f32);
            fft.get_inplace_scratch_len()
        ];

        Ok(Self {
            session,
            input_name,
            window,
            frames,
            filters: mel_filterbank_slaney(mel_bins),
            window_fn: hann_window_periodic(N_FFT),
            fft,
            fft_scratch,
            sample_buf: vec![0.0; window],
            padded_buf: vec![0.0; window + N_FFT],
            frame_buf: vec![Complex::new(0.0, 0.0); N_FFT],
            spectrum_buf: vec![0.0; FREQUENCY_BINS],
            features: vec![0.0; mel_bins * frames],
        })
    }

    /// Step 1 核对 1：固定窗取尾，不足左补零（官方示例 pad at the beginning）。
    fn prepare_window(&mut self, samples_16k: &[f32]) {
        let window = self.window;
        let n = samples_16k.len().min(window);
        self.sample_buf.fill(0.0);
        self.sample_buf[window - n..]
            .copy_from_slice(&samples_16k[samples_16k.len() - n..]);
    }

    /// Step 1 核对 2：`(x - mean) / sqrt(var + 1e-7)`，总体方差。
    fn normalize(&mut self) {
        zero_mean_unit_variance(&mut self.sample_buf);
    }

    /// Step 1 核对 3：反射补边 + 加窗 STFT + slaney 梅尔 + log10 + max-8 + (x+4)/4，
    /// 结果写入 `self.features`（[mel_bins × frames]，梅尔维为主序）。
    fn compute_features(&mut self) {
        let (window, n_fft) = (self.window, N_FFT);
        // torch.stft(center=True)：两侧反射补 n_fft/2（reflect：补边不含边缘采样本身）。
        let half = n_fft / 2;
        for i in 0..half {
            self.padded_buf[i] = self.sample_buf[half - i];
        }
        self.padded_buf[half..half + window].copy_from_slice(&self.sample_buf);
        let right_at = half + window;
        for (j, slot) in self.padded_buf[right_at..].iter_mut().enumerate() {
            *slot = self.sample_buf[window - 2 - j];
        }

        let mel_bins = self.features.len() / self.frames;
        for t in 0..self.frames {
            // 最后一帧（t = frames）按官方实现丢弃，只算前 frames 帧。
            for (c, (&s, &w)) in self.frame_buf.iter_mut().zip(
                self.padded_buf[t * HOP_LENGTH..t * HOP_LENGTH + n_fft]
                    .iter()
                    .zip(self.window_fn.iter()),
            ) {
                *c = Complex::new(s * w, 0.0);
            }
            self.fft
                .process_with_scratch(&mut self.frame_buf, &mut self.fft_scratch);
            for (slot, c) in self.spectrum_buf.iter_mut().zip(self.frame_buf.iter()) {
                *slot = c.re * c.re + c.im * c.im;
            }
            for m in 0..mel_bins {
                let filter = &self.filters[m * FREQUENCY_BINS..(m + 1) * FREQUENCY_BINS];
                let sum: f32 = filter
                    .iter()
                    .zip(self.spectrum_buf.iter())
                    .map(|(f, p)| f * p)
                    .sum();
                self.features[m * self.frames + t] = sum;
            }
        }

        // log10（下限 1e-10）→ 全域 max-8 钳制 → (x+4)/4。
        let mut max_log = f32::NEG_INFINITY;
        for v in self.features.iter_mut() {
            *v = v.max(LOG_MEL_FLOOR).log10();
            max_log = max_log.max(*v);
        }
        for v in self.features.iter_mut() {
            *v = (v.max(max_log - 8.0) + 4.0) / 4.0;
        }
    }

    /// Step 1 核对 4：ONNX 推理，输出 `logits` 已过 sigmoid，读出即概率。
    fn run_inference(&mut self) -> Result<f32, Box<dyn std::error::Error + Send + Sync>> {
        let mel_bins = self.features.len() / self.frames;
        let tensor = Tensor::from_array((
            vec![1i64, mel_bins as i64, self.frames as i64],
            self.features.clone(),
        ))?;
        let outputs = self
            .session
            .run(ort::inputs! { self.input_name.as_str() => tensor })?;
        let (_, data) = outputs[0].try_extract_tensor::<f32>()?;
        let probability = data.first().copied().unwrap_or(0.0);
        // sigmoid 值域防御性钳制（量化模型反量化可能出界）。
        Ok(probability.clamp(0.0, 1.0))
    }
}

impl TurnCompletenessDetector for SmartTurnAnalyzer {
    fn score(&mut self, samples_16k: &[f32]) -> f32 {
        if samples_16k.is_empty() {
            return 0.0;
        }
        self.prepare_window(samples_16k);
        self.normalize();
        self.compute_features();
        // 推理失败按“未说完”处理（0.0），不让错误冒泡进实时采集路径。
        self.run_inference().unwrap_or_else(|error| {
            tracing::debug!(?error, "smart turn inference failed");
            0.0
        })
    }
}

/// transformers `WhisperFeatureExtractor.zero_mean_unit_var_norm`（有 attention_mask 分支，
/// 本项目窗内全为有效样本，等价对整窗归一化）：`(x - mean) / sqrt(var + 1e-7)`。
fn zero_mean_unit_variance(x: &mut [f32]) {
    let n = x.len() as f32;
    let mean = x.iter().sum::<f32>() / n;
    let var = x.iter().map(|v| (*v - mean) * (*v - mean)).sum::<f32>() / n;
    let inv = 1.0 / (var + 1e-7).sqrt();
    for v in x.iter_mut() {
        *v = (*v - mean) * inv;
    }
}

/// `torch.hann_window(n)`：periodic（非对称）窗 `w[i] = 0.5 - 0.5 * cos(2πi / n)`。
fn hann_window_periodic(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
        .collect()
}

/// slaney 刻度 Hz→mel（HF transformers `hertz_to_mel(.., mel_scale="slaney")`）：
/// 线性段 `3·hz/200`，≥1000Hz 为 `15 + ln(hz/1000)·27/ln(6.4)`。
fn hz_to_mel(hz: f64) -> f64 {
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = 15.0;
    // 27/ln(6.4)（测试断言与 27.0/6.4f64.ln() 一致）。
    const LOG_STEP: f64 = 14.545_078_505_785_56;
    if hz >= MIN_LOG_HZ {
        MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() * LOG_STEP
    } else {
        3.0 * hz / 200.0
    }
}

/// slaney 刻度 mel→Hz（HF transformers `mel_to_hertz(.., mel_scale="slaney")`）。
fn mel_to_hz(mel: f64) -> f64 {
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = 15.0;
    // ln(6.4)/27（测试断言与 6.4f64.ln()/27.0 一致）。
    const LOG_STEP: f64 = 0.068_751_777_420_949_12;
    if mel >= MIN_LOG_MEL {
        MIN_LOG_HZ * (LOG_STEP * (mel - MIN_LOG_MEL)).exp()
    } else {
        200.0 * mel / 3.0
    }
}

/// HF transformers `mel_filter_bank(201, 80, 0, 8000, 16000, norm="slaney", mel_scale="slaney")`：
/// slaney 频率刻度 + slaney 面积归一化，输出 [mel_bins × 201]。
fn mel_filterbank_slaney(mel_bins: usize) -> Vec<f32> {
    // 频点中心 0..8000 均分 201 点；梅尔三角中心（含两个虚拟边界）共 mel_bins + 2 个。
    let all_freqs: Vec<f64> = (0..FREQUENCY_BINS)
        .map(|i| i as f64 * (SAMPLE_RATE as f64 / 2.0) / (FREQUENCY_BINS - 1) as f64)
        .collect();
    let m_min = hz_to_mel(0.0);
    let m_max = hz_to_mel(SAMPLE_RATE as f64 / 2.0);
    let mel_pts: Vec<f64> = (0..mel_bins + 2)
        .map(|i| m_min + (m_max - m_min) * i as f64 / (mel_bins + 1) as f64)
        .collect();
    let freqs: Vec<f64> = mel_pts.iter().copied().map(mel_to_hz).collect();

    let mut filters = vec![0f32; mel_bins * FREQUENCY_BINS];
    for (m, filter) in filters.chunks_mut(FREQUENCY_BINS).enumerate() {
        let fdiff_low = freqs[m + 1] - freqs[m];
        let fdiff_high = freqs[m + 2] - freqs[m + 1];
        for (k, slot) in filter.iter_mut().enumerate() {
            let lower = (all_freqs[k] - freqs[m]) / fdiff_low;
            let upper = (freqs[m + 2] - all_freqs[k]) / fdiff_high;
            *slot = lower.min(upper).max(0.0) as f32;
        }
    }
    // slaney 面积归一化：enorm = 2 / (freqs[m+2] - freqs[m])。
    for (m, filter) in filters.chunks_mut(FREQUENCY_BINS).enumerate() {
        let enorm = 2.0 / (freqs[m + 2] - freqs[m]);
        for slot in filter.iter_mut() {
            *slot *= enorm as f32;
        }
    }
    filters
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_path() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/models/smart-turn-v3.2-cpu.onnx")
    }

    #[test]
    fn model_loads_from_resources() {
        let path = model_path();
        let mut a = SmartTurnAnalyzer::new(&path).expect("load");
        let score = a.score(&vec![0f32; 128_000]);
        assert!((0.0..=1.0).contains(&score));
    }

    #[test]
    fn silence_scores_in_sane_range() {
        // 核对结论：全零（数字静音）输入在本模型实测 ≈ 0.99——纯静音没有“话没说完”的韵律，
        // 模型判“回合已完成”。因此 Task 2 必须只在 VAD 判定语音结束后咨询 smart-turn，
        // 静音不得送入本分析器（否则会对静音开口）。这里仅断言 sane 值域。
        let mut a = SmartTurnAnalyzer::new(&model_path()).expect("load");
        let score = a.score(&vec![0f32; 128_000]);
        assert!((0.0..=1.0).contains(&score), "score out of range: {score}");
    }

    #[test]
    fn short_input_is_left_padded_like_window_input() {
        // 左补零语义：窗内不足 8s 时，结果必须与“零填充到整窗”的输入一致。
        let mut a = SmartTurnAnalyzer::new(&model_path()).expect("load");
        let samples: Vec<f32> = (0..32_000)
            .map(|i| (i as f32 * 0.01).sin() * 0.3)
            .collect();
        let short = a.score(&samples);
        let mut padded = vec![0f32; 128_000];
        padded[96_000..].copy_from_slice(&samples);
        let full = a.score(&padded);
        assert!(
            (short - full).abs() < 1e-6,
            "left-pad mismatch: short={short} full={full}"
        );
    }

    #[test]
    fn zero_mean_unit_variance_matches_reference_formula() {
        let mut x = vec![1.0, 2.0, 3.0, 4.0];
        zero_mean_unit_variance(&mut x);
        let mean: f32 = x.iter().sum::<f32>() / x.len() as f32;
        let var: f32 =
            x.iter().map(|v| (*v - mean) * (*v - mean)).sum::<f32>() / x.len() as f32;
        assert!(mean.abs() < 1e-6);
        assert!((var - 1.0).abs() < 1e-3, "var={var}");
    }

    /// 手动正例验证（默认 #[ignore]，不进 CI）：用真实/合成完整句与其中段截断对比。
    /// 运行：`SMART_TURN_SAMPLE_WAV=完整句.wav cargo test --lib audio::smart_turn -- --ignored --nocapture`
    /// 样本可用 Windows SAPI 生成（16kHz/16bit/mono WAV）：
    /// `Speak('今天天气怎么样。')` 输出即完整句；截断前 3/5 模拟"未说完"。
    #[test]
    #[ignore = "需要真实语音样本（SMART_TURN_SAMPLE_WAV）"]
    fn complete_sentence_scores_above_truncated() {
        let Ok(path) = std::env::var("SMART_TURN_SAMPLE_WAV") else {
            eprintln!("SMART_TURN_SAMPLE_WAV not set; skipped");
            return;
        };
        let samples = read_wav_mono_16bit(&path).expect("read wav");
        assert!(samples.len() >= SAMPLE_RATE, "sample must be at least 1s");
        let mut a = SmartTurnAnalyzer::new(&model_path()).expect("load");
        let full = a.score(&samples);
        let truncated = a.score(&samples[..samples.len() * 3 / 5]);
        eprintln!("full={full:.4} truncated(60%)={truncated:.4}");
        assert!(
            full > truncated,
            "complete sentence must score above mid-sentence truncation"
        );
    }

    /// 最小 16-bit PCM WAV 读取（仅用于上述手动验证样本，不进主流程）。
    fn read_wav_mono_16bit(path: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        let bytes = std::fs::read(path)?;
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let mut pos = 12;
        while pos + 8 <= bytes.len() {
            let tag = &bytes[pos..pos + 4];
            let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into()?) as usize;
            if tag == b"data" {
                let end = (pos + 8 + len).min(bytes.len());
                return Ok(bytes[pos + 8..end]
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                    .collect());
            }
            pos += 8 + len + (len & 1);
        }
        Err("no data chunk".into())
    }

    #[test]
    fn slaney_hz_mel_matches_reference() {
        // 步长常量须与 ln(6.4)/27 精确一致；1000Hz 恰为 15 mel，8000Hz ≈ 45.2456 mel。
        assert!((0.068_751_777_420_949_12 - 6.4f64.ln() / 27.0).abs() < 1e-15);
        assert!((14.545_078_505_785_56 - 27.0 / 6.4f64.ln()).abs() < 1e-12);
        assert!((hz_to_mel(1000.0) - 15.0).abs() < 1e-9);
        assert!((hz_to_mel(8000.0) - 45.245_6).abs() < 1e-3);
        assert!((mel_to_hz(hz_to_mel(440.0)) - 440.0).abs() < 1e-9);
        assert!((mel_to_hz(hz_to_mel(4000.0)) - 4000.0).abs() < 1e-6);
    }

    #[test]
    fn mel_filterbank_has_weight_at_voice_frequencies() {
        // 80 个滤波器全部有非零覆盖（三角带宽非零）。
        // 注：DC 与 8000Hz 频点本身的权重按三角定义为 0（三角在端点闭合），故检查各滤波器非零和。
        let filters = mel_filterbank_slaney(MEL_BINS);
        assert_eq!(filters.len(), MEL_BINS * FREQUENCY_BINS);
        for m in 0..MEL_BINS {
            let sum: f32 = filters[m * FREQUENCY_BINS..(m + 1) * FREQUENCY_BINS]
                .iter()
                .sum();
            assert!(sum > 0.0, "mel filter {m} must have nonzero coverage");
        }
    }
}
