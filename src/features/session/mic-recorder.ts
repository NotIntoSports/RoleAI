// 本机麦克风对练：把 WebView 麦克风采集复用为会话的流式输入源。
// 采样率原样上报，由后端 session_push_mic_pcm 统一重采样到 48kHz。
import { WORKLET_CODE, bytesToBase64 } from "../services/wav-recorder";

export const CHUNK_TARGET_MS = 100;

export function encodePcm16(samples: Float32Array): Uint8Array {
  const bytes = new Uint8Array(samples.length * 2);
  const view = new DataView(bytes.buffer);
  for (let index = 0; index < samples.length; index++) {
    const clamped = Math.max(-1, Math.min(1, samples[index]));
    view.setInt16(index * 2, clamped < 0 ? clamped * 0x8000 : clamped * 0x7fff, true);
  }
  return bytes;
}

// AudioWorklet 每 128 样本回调一次，攒够约 CHUNK_TARGET_MS 再走一次 IPC。
export class MicChunkBatcher {
  private buffer: Float32Array[] = [];
  private buffered = 0;

  constructor(
    readonly sampleRate: number,
    private chunkSamples = Math.max(1, Math.round((sampleRate / 1000) * CHUNK_TARGET_MS)),
  ) {}

  push(frame: Float32Array): Float32Array | null {
    this.buffer.push(frame);
    this.buffered += frame.length;
    if (this.buffered < this.chunkSamples) return null;
    const merged = new Float32Array(this.buffered);
    let offset = 0;
    for (const part of this.buffer) {
      merged.set(part, offset);
      offset += part.length;
    }
    this.buffer = [];
    this.buffered = 0;
    return merged;
  }
}

export interface MicStreamController {
  start(): Promise<void>;
  stop(): void;
}

export interface MicStreamCallbacks {
  onChunk: (pcmBase64: string, sampleRate: number) => void;
  onError: (message: string) => void;
}

export class MicStreamer implements MicStreamController {
  private stream: MediaStream | null = null;
  private context: AudioContext | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private worklet: AudioWorkletNode | null = null;
  private batcher: MicChunkBatcher | null = null;

  constructor(private callbacks: MicStreamCallbacks) {}

  async start(): Promise<void> {
    if (this.context) throw new Error("mic streamer already started");
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true },
    });
    const context = new AudioContext();
    try {
      // WebView2 中 AudioContext 可能以 suspended 状态创建；不显式恢复会导致
      // 整个图不被拉动、一个采样都采不到（与音色克隆录音相同的问题）。
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
      this.batcher = new MicChunkBatcher(context.sampleRate);
      const source = context.createMediaStreamSource(stream);
      const worklet = new AudioWorkletNode(context, "voice-capture");
      worklet.port.onmessage = (event) => {
        const chunk = this.batcher?.push(event.data as Float32Array);
        if (chunk) this.callbacks.onChunk(bytesToBase64(encodePcm16(chunk)), context.sampleRate);
      };
      const mute = context.createGain();
      mute.gain.value = 0;
      source.connect(worklet);
      worklet.connect(mute);
      mute.connect(context.destination);
      this.source = source;
      this.worklet = worklet;
    } catch (error) {
      this.teardown();
      throw error;
    }
  }

  stop(): void {
    this.teardown();
  }

  private teardown(): void {
    this.worklet?.disconnect();
    this.source?.disconnect();
    this.worklet = null;
    this.source = null;
    this.batcher = null;
    this.stream?.getTracks().forEach((track) => track.stop());
    this.stream = null;
    void this.context?.close().catch(() => undefined);
    this.context = null;
  }
}
