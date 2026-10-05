// 本机麦克风对练：把 WebView 麦克风采集复用为会话的流式输入源。
// 采样率原样上报，由后端 session_push_mic_pcm 统一重采样到 48kHz。
import { WORKLET_CODE, bytesToBase64 } from "../services/wav-recorder";
import { enumerateAudioInputs } from "./mic-device-selection";
import {
  chooseBoundDevice,
  collectMicEvidence,
  defaultMicProbe,
  type MicDeviceEvidence,
} from "./mic-signal-probe";

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
  /** 开流；实现可返回探针证据表供 LLM 复核，测试替身可只返回 void。 */
  start(): Promise<void | MicDeviceEvidence[]>;
  stop(): void;
  /** LLM 复核建议改绑时调用；可选，测试替身可不实现。 */
  switchDevice?(deviceId: string): Promise<void>;
}

export interface MicStreamCallbacks {
  onChunk: (pcmBase64: string, sampleRate: number) => void;
  onError: (message: string) => void;
  /** 每 ~100ms 上报一次 0..1 归一化 RMS 电平，驱动麦克风按钮音量条。 */
  onLevel?: (level: number) => void;
}

// 语音 RMS 通常落在 0.02–0.3：乘 4 放大并夹到 0..1，供音量条直接用作高度。
export function rmsLevel(samples: Float32Array): number {
  let sum = 0;
  for (let index = 0; index < samples.length; index++) {
    sum += samples[index] * samples[index];
  }
  return Math.min(1, Math.sqrt(sum / samples.length) * 4);
}

export const MIC_BASE_AUDIO: MediaTrackConstraints = {
  channelCount: 1,
  echoCancellation: true,
  noiseSuppression: true,
  autoGainControl: true,
};

export class MicStreamer implements MicStreamController {
  private stream: MediaStream | null = null;
  private context: AudioContext | null = null;
  private source: MediaStreamAudioSourceNode | null = null;
  private worklet: AudioWorkletNode | null = null;
  private batcher: MicChunkBatcher | null = null;

  constructor(
    private callbacks: MicStreamCallbacks,
    private readonly sharedContext?: AudioContext,
    private readonly probeSampleMs = 400,
  ) {}

  // 信号证据驱动的选型：先探针（逐候选试绑实测 RMS，即开即停），再绑
  // 第一个有信号的非虚拟设备（用 exact 约束）；全候选静音或无候选时回退
  // 系统默认。返回证据表供 LLM 复核；探针本身失败不阻断开流。
  async openWithEvidence(): Promise<MicDeviceEvidence[]> {
    const devices = await enumerateAudioInputs();
    const evidence = await collectMicEvidence(devices, defaultMicProbe(MIC_BASE_AUDIO, this.probeSampleMs));
    const chosen = chooseBoundDevice(evidence);
    const audio: MediaTrackConstraints = {
      ...MIC_BASE_AUDIO,
      ...(chosen ? { deviceId: { exact: chosen } } : {}),
    };
    const stream = await navigator.mediaDevices.getUserMedia({ audio });
    this.replaceStream(stream);
    return evidence;
  }

  /** LLM 复核建议改绑时调用：换流并重建采集源，采集图（worklet/静音汇）保持不变。 */
  async switchDevice(deviceId: string): Promise<void> {
    if (!this.context || !this.worklet) throw new Error("mic streamer not started");
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: { ...MIC_BASE_AUDIO, deviceId: { exact: deviceId } },
    });
    this.replaceStream(stream);
  }

  private replaceStream(stream: MediaStream): void {
    this.stream?.getTracks().forEach((track) => track.stop());
    this.stream = stream;
    if (this.context && this.worklet) {
      this.source?.disconnect();
      this.source = this.context.createMediaStreamSource(stream);
      this.source.connect(this.worklet);
    }
  }

  /** 开流并返回探针证据表（探针失败时为空数组，供调用方跳过 LLM 复核）。 */
  async start(): Promise<MicDeviceEvidence[]> {
    if (this.context) throw new Error("mic streamer already started");
    let evidence: MicDeviceEvidence[] = [];
    let stream: MediaStream;
    try {
      evidence = await this.openWithEvidence();
      const bound = this.stream;
      if (!bound) throw new Error("mic probe did not bind a stream");
      stream = bound;
    } catch {
      // 探针/证据选型失败（枚举不可用等）：回退系统默认开流，行为与未接入
      // 探针时一致，后续由静音看门狗兜底。
      evidence = [];
      stream = await navigator.mediaDevices.getUserMedia({ audio: { ...MIC_BASE_AUDIO } });
    }
    const context = this.sharedContext ?? new AudioContext();
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
        if (chunk) {
          this.callbacks.onLevel?.(rmsLevel(chunk));
          this.callbacks.onChunk(bytesToBase64(encodePcm16(chunk)), context.sampleRate);
        }
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
    return evidence;
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
    if (!this.sharedContext) {
      void this.context?.close().catch(() => undefined);
    }
    this.context = null;
  }
}
