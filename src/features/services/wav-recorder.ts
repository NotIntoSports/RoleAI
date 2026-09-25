// AudioWorklet 处理器源码：经 Blob URL 注入（构建期内嵌的自有代码，生产 CSP 需允许 script-src blob:）。
export const WORKLET_CODE = `class VoiceCaptureProcessor extends AudioWorkletProcessor {
  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (channel) this.port.postMessage(new Float32Array(channel));
    return true;
  }
}
registerProcessor("voice-capture", VoiceCaptureProcessor);`;

export const RECORD_MAX_MS = 30_000;
export const RECORD_MIN_MS = 3_000;
const OUTPUT_SAMPLE_RATE = 16_000;

export function downsampleTo16k(samples: Float32Array, inputSampleRate: number): Float32Array {
  if (inputSampleRate === OUTPUT_SAMPLE_RATE || samples.length === 0) return samples;
  const ratio = inputSampleRate / OUTPUT_SAMPLE_RATE;
  const outputLength = Math.max(1, Math.floor(samples.length / ratio));
  const output = new Float32Array(outputLength);
  for (let index = 0; index < outputLength; index++) {
    const start = Math.floor(index * ratio);
    const end = Math.min(samples.length, Math.floor((index + 1) * ratio) || start + 1);
    let sum = 0;
    for (let cursor = start; cursor < end; cursor++) sum += samples[cursor];
    output[index] = sum / Math.max(1, end - start);
  }
  return output;
}

export function encodeWav16k(samples: Float32Array, inputSampleRate: number): { bytes: Uint8Array; durationMs: number } {
  const resampled = downsampleTo16k(samples, inputSampleRate);
  const durationMs = Math.round((resampled.length / OUTPUT_SAMPLE_RATE) * 1000);
  const dataLength = resampled.length * 2;
  const bytes = new Uint8Array(44 + dataLength);
  const view = new DataView(bytes.buffer);
  const writeAscii = (offset: number, text: string) => {
    for (let index = 0; index < text.length; index++) view.setUint8(offset + index, text.charCodeAt(index));
  };
  writeAscii(0, "RIFF");
  view.setUint32(4, 36 + dataLength, true);
  writeAscii(8, "WAVE");
  writeAscii(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, OUTPUT_SAMPLE_RATE, true);
  view.setUint32(28, OUTPUT_SAMPLE_RATE * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  writeAscii(36, "data");
  view.setUint32(40, dataLength, true);
  let offset = 44;
  for (let index = 0; index < resampled.length; index++, offset += 2) {
    const clamped = Math.max(-1, Math.min(1, resampled[index]));
    view.setInt16(offset, clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff, true);
  }
  return { bytes, durationMs };
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 0x8000;
  for (let start = 0; start < bytes.length; start += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(start, start + chunkSize));
  }
  return btoa(binary);
}

export interface RecordingResult {
  base64: string;
  durationMs: number;
}

export class VoiceRecorder {
  private stream: MediaStream | null = null;
  private context: AudioContext | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private worklet: AudioWorkletNode | null = null;
  private chunks: Float32Array[] = [];
  private sampleCount = 0;

  get recording(): boolean {
    return this.context !== null;
  }

  get recordedMs(): number {
    if (!this.context) return 0;
    return Math.round((this.sampleCount / this.context.sampleRate) * 1000);
  }

  async start(): Promise<void> {
    if (this.context) throw new Error("recorder already started");
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
    });
    const context = new AudioContext();
    try {
      // WebView2 中 AudioContext 可能以 suspended 状态创建；不显式恢复会导致整个图
      // 不被拉动、一个采样都采不到（表现为停止后"没有录到声音"）。
      await context.resume();
      const blobUrl = URL.createObjectURL(new Blob([WORKLET_CODE], { type: "application/javascript" }));
      try {
        await context.audioWorklet.addModule(blobUrl);
      } finally {
        URL.revokeObjectURL(blobUrl);
      }
      if (context.state !== "running") {
        await context.resume();
      }
      if (context.state !== "running") {
        throw new Error(`audio context is ${context.state}`);
      }
      this.stream = stream;
      this.context = context;
      this.chunks = [];
      this.sampleCount = 0;
      const source = context.createMediaStreamSource(stream);
      const worklet = new AudioWorkletNode(context, "voice-capture");
      worklet.port.onmessage = (event) => {
        const chunk = event.data as Float32Array;
        this.chunks.push(chunk);
        this.sampleCount += chunk.length;
      };
      const mute = context.createGain();
      mute.gain.value = 0;
      source.connect(worklet);
      worklet.connect(mute);
      mute.connect(context.destination);
      this.source = source;
      this.worklet = worklet;
    } catch (error) {
      stream.getTracks().forEach((track) => track.stop());
      void context.close().catch(() => undefined);
      throw error;
    }
  }

  async stop(): Promise<RecordingResult | null> {
    if (!this.context) return null;
    const inputSampleRate = this.context.sampleRate;
    const flat = new Float32Array(this.sampleCount);
    let offset = 0;
    for (const chunk of this.chunks) {
      flat.set(chunk, offset);
      offset += chunk.length;
    }
    this.teardown();
    if (flat.length === 0) return null;
    const { bytes, durationMs } = encodeWav16k(flat, inputSampleRate);
    return { base64: bytesToBase64(bytes), durationMs };
  }

  cancel(): void {
    this.teardown();
  }

  private teardown(): void {
    this.worklet?.disconnect();
    this.source?.disconnect();
    this.worklet = null;
    this.source = null;
    this.stream?.getTracks().forEach((track) => track.stop());
    this.stream = null;
    void this.context?.close().catch(() => undefined);
    this.context = null;
    this.chunks = [];
    this.sampleCount = 0;
  }
}
