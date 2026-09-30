//! 播报期间的打断监听：连续人声窗口达到阈值即触发，缓冲从开口起的原文。
use crate::audio::pcm::downsample_48k_to_16k;
use crate::audio::vad::VoiceActivityDetector;

const START_PROB: f32 = 0.4;
/// 滑动窗判定：最近 TRIGGER_WINDOWS 窗内 ≥TRIGGER_VOICED 个人声即触发。
/// 真机实测播报期 AEC 压近端，人声窗概率在 0.31~0.77 高频抖动、自然语音
/// 有词间空隙，「连续 N 窗」会被单个 <0.3 的窗清零（实测 run 最多 2~3），
/// 滑动计数容忍 1~2 窗空隙，触发延迟 ~200-260ms 量级不变。
const TRIGGER_WINDOWS: usize = 8;
const TRIGGER_VOICED: usize = 6;
const WINDOW_BYTES_48K: usize = 3072;

/// 能量辅助判定：播报期 WebView AEC 压低近端人声，Silero 概率在 0.31~0.77
/// 之间高频抖动（真机实测 60% 满量程的人声在播报期概率也只有 0.34~0.41），
/// 纯概率判定在双讲场景永远打不断。窗口概率 ≥ENERGY_ASSIST_PROB 且能量
/// 明显高于播报期回声底噪（≥3×）时也计为人声——回声窗的 RMS 贴着底噪走
/// （底噪就是从它追踪出来的），人声叠加时能量跳变 3~20×，两者分得开。
const ENERGY_ASSIST_PROB: f32 = 0.3;
const ENERGY_ASSIST_RATIO: f32 = 3.0;
/// 回声底噪建底期（~500ms）：只追踪不计数，避免播放起始的回声 onset 误判。
const FLOOR_PRIME_WINDOWS: usize = 16;
/// 底噪上浮速率：每窗向当前 RMS 逼近 2%（~1.6s 时间常数）。
const FLOOR_RISE_RATE: f32 = 0.02;

pub struct BargeInMonitor {
    detector: Box<dyn VoiceActivityDetector>,
    pending48: Vec<u8>,
    pending16: Vec<f32>,
    buffer: Vec<u8>,
    voiced_run: usize,
    /// 最近 TRIGGER_WINDOWS 窗的人声标记环形缓冲（滑动窗计数用）。
    voiced_ring: [bool; TRIGGER_WINDOWS],
    voiced_ring_len: usize,
    staged: Option<Vec<u8>>,
    /// 播报期回声底噪（窗口 RMS）：min 追踪 + 慢速上浮，只在本监听被喂
    /// 信号（即播报回声抑制窗内）时更新，两轮之间冻结。
    echo_floor: f32,
    floor_primed: usize,
}

// 检测器不可 Debug；CaptureState 派生 Debug 时只暴露可观测状态。
impl std::fmt::Debug for BargeInMonitor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BargeInMonitor")
            .field("buffered_windows", &(self.buffer.len() / WINDOW_BYTES_48K))
            .field("voiced_run", &self.voiced_run)
            .field("voiced_ring", &self.voiced_ring_len)
            .field("staged", &self.staged.is_some())
            .field("echo_floor", &self.echo_floor)
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
            voiced_ring: [false; TRIGGER_WINDOWS],
            voiced_ring_len: 0,
            staged: None,
            echo_floor: 0.0,
            floor_primed: 0,
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
            let voiced_prob = self.detector.process(&window);
            let window_rms = {
                let sum: f32 = window.iter().map(|s| s * s).sum();
                (sum / window.len() as f32).sqrt()
            };
            self.track_echo_floor(window_rms);
            let energy_assist =
                self.echo_floor > 0.0 && window_rms >= self.echo_floor * ENERGY_ASSIST_RATIO;
            let voiced =
                voiced_prob >= START_PROB || (energy_assist && voiced_prob >= ENERGY_ASSIST_PROB);
            self.buffer.extend_from_slice(&raw48);
            self.voiced_run = if voiced { self.voiced_run + 1 } else { 0 };
            let voiced_total = self.push_voiced(voiced);
            if voiced_prob >= 0.3 {
                // 只记接近阈值的窗口：打断不触发时看概率实际落在多少。
                tracing::debug!(
                    target: "audio_barge",
                    prob = format_args!("{:.2}", voiced_prob),
                    rms = format_args!("{:.4}", window_rms),
                    floor = format_args!("{:.4}", self.echo_floor),
                    voiced_total,
                    "Silero 窗口概率"
                );
            }
            if voiced_total >= TRIGGER_VOICED {
                self.staged = Some(std::mem::take(&mut self.buffer));
                self.voiced_run = 0;
                self.voiced_ring = [false; TRIGGER_WINDOWS];
                self.voiced_ring_len = 0;
                return;
            }
            while self.buffer.len() > 40 * WINDOW_BYTES_48K {
                // 缓冲上界 ~20s：只保留最近 40 窗，防止整段播报期间无限积累
                let drop = self.buffer.len() - 40 * WINDOW_BYTES_48K;
                self.buffer.drain(..drop);
            }
        }
    }

    /// 人声标记入环形缓冲，返回最近 TRIGGER_WINDOWS 窗内的人声窗数。
    fn push_voiced(&mut self, voiced: bool) -> usize {
        if self.voiced_ring_len < TRIGGER_WINDOWS {
            self.voiced_ring[self.voiced_ring_len] = voiced;
            self.voiced_ring_len += 1;
        } else {
            self.voiced_ring.copy_within(1.., 0);
            self.voiced_ring[TRIGGER_WINDOWS - 1] = voiced;
        }
        self.voiced_ring[..self.voiced_ring_len]
            .iter()
            .filter(|flag| **flag)
            .count()
    }

    /// 回声底噪追踪：建底期取 min（播放起始回声从静音爬升，先把底立住），
    /// 之后 min 追踪下探、慢速上浮——人声段的能量不会立刻抬高底噪。
    fn track_echo_floor(&mut self, window_rms: f32) {
        if self.floor_primed < FLOOR_PRIME_WINDOWS {
            self.floor_primed += 1;
            self.echo_floor = if self.echo_floor <= 0.0 {
                window_rms
            } else {
                self.echo_floor.min(window_rms)
            };
            return;
        }
        if window_rms < self.echo_floor || self.echo_floor <= 0.0 {
            self.echo_floor = window_rms;
        } else {
            self.echo_floor += (window_rms - self.echo_floor) * FLOOR_RISE_RATE;
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
        let mut probs = vec![0.9f32; 40]; // 滑动 8 窗内 ≥6 人声即触发
        probs.extend(vec![0.0; 40]);
        let mut m = monitor(&probs);
        m.ingest(&frames48(2000, 60)); // 60 帧 ≈ 37 窗语音 → 触发
        assert!(m.triggered());
        let utterance = m.take_staged().expect("staged");
        assert!(utterance.len() >= 6 * 3072);
        assert!(m.take_staged().is_none()); // 已取走
    }

    /// 自然语音的词间空隙（1~2 窗 <0.3）不清零滑动计数：8 窗内 6 个人声
    /// 即触发。连击式判定在这里会因单个空隙窗永远凑不齐。
    #[test]
    fn sliding_window_tolerates_short_speech_gaps() {
        let mut probs = vec![0.9f32; 3];
        probs.push(0.0);
        probs.extend(vec![0.9f32; 3]);
        probs.push(0.0);
        probs.extend(vec![0.9f32; 2]); // 第 9 窗时最近 8 窗 = 6 人声
        probs.extend(vec![0.0f32; 40]);
        let mut m = monitor(&probs);
        m.ingest(&frames48(2000, 60));
        assert!(
            m.triggered(),
            "6-of-8 sliding count must trigger despite gaps"
        );
    }

    /// 能量辅助判定（双讲核心用例）：播报期 AEC 压低近端人声，Silero 概率
    /// 抖动在 START_PROB 之下（0.30~0.39），纯概率永远凑不齐连续窗；人声
    /// 叠加让窗口 RMS 跳到回声底噪的 4× 以上，辅助条件接住 → 连续 6 窗触发。
    #[test]
    fn energy_assist_triggers_when_prob_flickers_below_start_prob() {
        // 概率恒 0.35：单看概率永不达标；能量判定交给窗口 RMS。
        let mut m = monitor(&[0.35f32; 120]);
        // 先喂 20 窗安静回声（建底期 16 窗 + 底噪稳定），再喂 6 窗响亮人声。
        for _ in 0..20 {
            m.ingest(&window48_bytes(300));
        }
        assert!(m.take_staged().is_none(), "quiet echo must not trigger");
        for _ in 0..6 {
            m.ingest(&window48_bytes(4000));
        }
        assert!(
            m.triggered(),
            "speech energy 4x above echo floor must trigger via assist"
        );
        assert!(m.take_staged().is_some());
    }

    /// 回声电平不误触发（防误打断回归）：概率 0.35 + 恒定回声能量（RMS 贴着
    /// 底噪走，底噪本身就是从它追踪的）——辅助条件不满足，永不触发。
    #[test]
    fn constant_echo_level_with_subthreshold_prob_never_triggers() {
        let mut m = monitor(&[0.35f32; 200]);
        for _ in 0..60 {
            m.ingest(&window48_bytes(3000)); // 恒定幅度：回声形态
        }
        assert!(m.take_staged().is_none());
    }

    /// 一个 16k 分析窗对应的 48k 原文（1536 样本 = 3072 字节），恒定幅值。
    fn window48_bytes(sample: i16) -> Vec<u8> {
        (0..1536).flat_map(|_| sample.to_le_bytes()).collect()
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

    /// 触发后未取走前停止收集；取走后从干净状态重新积累，
    /// 触发时刻滞留的旧 pending 不得混入下一轮话轮。
    #[test]
    fn staged_restart_collects_only_fresh_audio() {
        // 第一轮：响人声触发并取走。
        let mut probs = vec![0.9f32; 8];
        probs.extend(vec![0.0f32; 40]);
        let mut m = monitor(&probs);
        m.ingest(&frames48(2000, 20));
        assert!(m.triggered());
        let first = m.take_staged().expect("staged");
        assert_eq!(
            first.len() % WINDOW_BYTES_48K,
            0,
            "话轮必须是完整窗的整数倍"
        );

        // 第二轮：同一监听器改喂安静的 100 幅度音频。若触发时刻滞留在
        // pending 里的 2000 幅度旧字节泄漏进新一轮，最大样本幅度会是 2000。
        let mut probs = vec![0.9f32; 8];
        probs.extend(vec![0.0f32; 40]);
        let mut m = monitor(&probs);
        m.ingest(&frames48(100, 20));
        assert!(m.triggered(), "取走后必须能重新触发");
        let second = m.take_staged().expect("second staged");
        let max_abs = second
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert_eq!(
            max_abs, 100,
            "新一轮话轮必须只含取走后新收集的音频，不得混入旧 pending"
        );
    }

    /// 触发前的未取走状态：ingest 直接短路，缓冲冻结在触发时刻。
    #[test]
    fn staged_but_untaken_monitor_stops_buffering() {
        let mut probs = vec![0.9f32; 8];
        probs.extend(vec![0.0f32; 40]);
        let mut m = monitor(&probs);
        m.ingest(&frames48(2000, 20));
        assert!(m.triggered());
        let before = format!("{m:?}");
        // 触发未取走时继续喂音频：缓冲必须原封不动。
        m.ingest(&frames48(2000, 20));
        assert_eq!(
            format!("{m:?}"),
            before,
            "staged 未取走时 ingest 必须是空操作"
        );
    }

    /// `CaptureState` 派生 `Debug` 走这条手工实现：暴露可观测状态、不 panic。
    #[test]
    fn debug_impl_reports_observable_state() {
        let m = monitor(&[0.9f32; 4]);
        let text = format!("{m:?}");
        assert!(text.contains("BargeInMonitor"), "实际输出：{text}");
        assert!(text.contains("buffered_windows"));
        assert!(text.contains("echo_floor"));
    }

    /// 从 Debug 输出抽取 `key: value` 的浮点值（value 截到 ',' 或 '}'）。
    fn debug_field_f32(text: &str, key: &str) -> f32 {
        let needle = format!("{key}: ");
        let start = text.find(needle.as_str()).expect("字段存在") + needle.len();
        let rest = &text[start..];
        let end = rest.find([',', '}']).expect("字段结束");
        rest[..end].trim().parse().expect("浮点数值")
    }

    /// Debug 暴露的状态必须数值正确：人声连击数、缓冲窗数、回声底噪。
    /// 这是 voiced_run 维护（+→* 会把计数卡在 0）与 fmt 内算术变异的唯一观测面。
    #[test]
    fn debug_pins_voiced_run_buffer_and_floor_after_two_windows() {
        let mut m = monitor(&[0.9f32; 4]);
        m.ingest(&window48_bytes(2000));
        m.ingest(&window48_bytes(2000));
        let text = format!("{m:?}");
        assert!(text.contains("buffered_windows: 2"), "实际输出：{text}");
        assert!(text.contains("voiced_run: 2"), "实际输出：{text}");
        let floor = debug_field_f32(&text, "echo_floor");
        assert!(
            (floor - 2000f32 / 32768.0).abs() < 1e-6,
            "echo_floor 必须等于常数幅度窗口的 RMS（2000/32768），实际 {floor}"
        );
    }

    /// 回声底噪状态机三段式：建底期取 min（不吃大窗）、稳态只按 2% 慢上浮、
    /// 更安静窗立刻下探。底噪是能量辅助判定的基准，数值必须逐段钉住。
    #[test]
    fn echo_floor_primes_to_min_then_rises_slowly_and_drops_fast() {
        let mut m = monitor(&[0.1f32; 40]);
        // 建底期 16 窗幅度递增（100..1600）：底噪必须钉在首窗最小值 100/32768。
        // 递增序是刻意的：若"建底"退化成稳态规则，底噪会被逐窗上浮抬高。
        for k in 1..=16_i16 {
            m.ingest(&window48_bytes(100 * k));
        }
        let primed_floor = debug_field_f32(&format!("{m:?}"), "echo_floor");
        assert!(
            (primed_floor - 100f32 / 32768.0).abs() < 1e-9,
            "建底期底噪必须是首窗最小 RMS（100/32768），实际 {primed_floor}"
        );
        // 稳态大窗：底噪只上浮 2%，绝不能跳到窗口幅度。
        m.ingest(&window48_bytes(2000));
        let floor_before = 100f32 / 32768.0;
        let rms = 2000f32 / 32768.0;
        let expected = floor_before + (rms - floor_before) * 0.02;
        let risen = debug_field_f32(&format!("{m:?}"), "echo_floor");
        assert!(
            (risen - expected).abs() < 1e-6,
            "稳态底噪必须只上浮 2%：期望 {expected}，实际 {risen}"
        );
        // 更安静的一窗：底噪立刻下探到该窗 RMS（min 追踪，不等时间常数）。
        m.ingest(&window48_bytes(50));
        let dropped = debug_field_f32(&format!("{m:?}"), "echo_floor");
        assert!(
            (dropped - 50f32 / 32768.0).abs() < 1e-9,
            "更安静窗口必须立刻拉低底噪（min 追踪），实际 {dropped}"
        );
    }

    /// 缓冲上界（~20s）：久未触发的监听只保留最近 40 窗原文；
    /// 迟到触发的话轮必须恰好是 40 窗，而不是空缓冲或无界积累。
    #[test]
    fn late_trigger_stages_exactly_the_capped_40_window_history() {
        let mut probs = vec![0.1f32; 50]; // 50 窗安静：缓冲只保留最近 40 窗
        probs.extend(vec![0.9f32; 8]); // 滑动 8 窗内凑满 6 人声即触发
        let mut m = monitor(&probs);
        for _ in 0..50 {
            m.ingest(&window48_bytes(300));
        }
        for _ in 0..6 {
            m.ingest(&window48_bytes(2000));
        }
        assert!(m.triggered());
        let utterance = m.take_staged().expect("staged");
        // 触发快照发生在"扩窗之后、截断之前"：= 上限 40 窗 + 触发当窗 1 窗。
        assert_eq!(
            utterance.len(),
            41 * WINDOW_BYTES_48K,
            "迟到触发的话轮必须是封顶历史 + 触发当窗"
        );
    }

    /// 能量辅助判定的前提是"回声底噪已建底"：全程静音时底噪恒为 0，
    /// 概率落在 [ENERGY_ASSIST_PROB, START_PROB) 的窗口不得借道辅助判定触发。
    #[test]
    fn assisted_voicing_requires_a_built_echo_floor() {
        let mut m = monitor(&[0.35f32; 12]);
        for _ in 0..12 {
            m.ingest(&window48_bytes(0)); // 全静音：RMS=0，底噪始终为 0
        }
        assert!(!m.triggered(), "静音不得进入触发态");
        assert!(
            m.take_staged().is_none(),
            "静音 + 底噪未建底不得借道能量辅助触发"
        );
    }
}
