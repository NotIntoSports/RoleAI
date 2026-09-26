//! Silero VAD 包装器：把 16kHz/512 采样窗口映射为语音概率。
use silero_vad_rust::silero_vad::model::OnnxModel;

pub const VAD_WINDOW_SAMPLES: usize = 512; // 32 ms @16 kHz

/// Silero 推理采样率；`forward_chunk` 只接受 8k/16k，采集管线统一降采样到 16k。
const VAD_SAMPLE_RATE: u32 = 16_000;

pub trait VoiceActivityDetector: Send {
    /// 返回窗口内含人声的概率 0.0..=1.0。
    fn process(&mut self, window: &[f32]) -> f32;
}

/// 持有 crate 内置的 Silero ONNX 模型（opset 16，CPU provider）。
pub struct SileroVad {
    model: OnnxModel,
}

impl SileroVad {
    pub fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let model = silero_vad_rust::load_silero_vad()?;
        Ok(Self { model })
    }
}

impl VoiceActivityDetector for SileroVad {
    fn process(&mut self, window: &[f32]) -> f32 {
        debug_assert_eq!(window.len(), VAD_WINDOW_SAMPLES);
        // 每窗重置循环状态：概率只取决于当前窗口，流式行为确定、可测。
        self.model.reset_states();
        match self.model.forward_chunk(window, VAD_SAMPLE_RATE) {
            Ok(probs) => probs[[0, 0]],
            // 推理失败按无语音处理，不让错误冒泡进实时采集路径。
            Err(error) => {
                tracing::debug!(?error, "silero vad forward_chunk failed");
                0.0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silero_loads_and_silence_scores_low() {
        let mut vad = SileroVad::new().expect("silero model must load");
        let window = vec![0f32; VAD_WINDOW_SAMPLES];
        let prob = vad.process(&window);
        assert!(prob < 0.2, "silence probability must be low, got {prob}");
    }
}
