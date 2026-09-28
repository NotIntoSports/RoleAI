// WebAudio 全双工播放：与麦克风采集共用同一个 AudioContext，使 Chromium AEC
// 拿到同上下文的远端播放参考；500ms 初始 jitter buffer 吸收 delta 抖动。
export const WEB_AUDIO_JITTER_MS = 500;

/** 开发版经 window.__roleaiAudio 暴露，用于定位「收到音频却没出声」。 */
export interface AudioDiagnostics {
  eventsReceived: number;
  bytesReceived: number;
  chunksScheduled: number;
  clears: number;
  contextErrors: number;
  fallbackToDefault: boolean;
  contextState: string;
  sampleRate: number;
  sinkId: string;
  lastScheduledStartMs: number;
  lastCurrentTimeMs: number;
}

export const audioDiagnostics: AudioDiagnostics = {
  eventsReceived: 0,
  bytesReceived: 0,
  chunksScheduled: 0,
  clears: 0,
  contextErrors: 0,
  fallbackToDefault: false,
  contextState: "none",
  sampleRate: 0,
  sinkId: "",
  lastScheduledStartMs: 0,
  lastCurrentTimeMs: 0,
};

if ((import.meta as { env?: { DEV?: boolean } }).env?.DEV && typeof window !== "undefined") {
  (window as unknown as { __roleaiAudio: AudioDiagnostics }).__roleaiAudio = audioDiagnostics;
}

export function decodePcm16Base64(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export class PlaybackSchedule {
  private nextStart = 0;

  constructor(private readonly jitterMs = WEB_AUDIO_JITTER_MS) {}

  nextStartTime(currentTimeMs: number, durationMs: number): number {
    const earliest = currentTimeMs + this.jitterMs;
    if (this.nextStart < earliest) this.nextStart = earliest;
    const start = this.nextStart;
    this.nextStart = start + durationMs;
    return start;
  }

  clear(): void {
    this.nextStart = 0;
  }
}

interface AudioContextSinkOptions {
  sinkId?: string;
}

function recordContext(context: AudioContext, sinkId: string) {
  audioDiagnostics.contextState = context.state;
  audioDiagnostics.sampleRate = context.sampleRate;
  audioDiagnostics.sinkId = sinkId;
  context.addEventListener?.("statechange", () => {
    audioDiagnostics.contextState = context.state;
  });
}

/**
 * 按浏览器 deviceId 创建输出上下文。Chromium 128+ 遇到无效 sinkId 不抛异常，
 * 而是派发 error 事件并保持挂起；此时调用 onFallback 让调用方改用默认输出重建。
 */
export function createAudioContextForOutput(
  deviceId: string,
  onFallback?: () => void,
): {
  context: AudioContext;
  usedRequestedSink: boolean;
} {
  if (deviceId) {
    try {
      const context = new AudioContext({ sinkId: deviceId } as AudioContextOptions & AudioContextSinkOptions);
      recordContext(context, deviceId);
      context.addEventListener?.("error", () => {
        audioDiagnostics.contextErrors += 1;
        audioDiagnostics.fallbackToDefault = true;
        onFallback?.();
      }, { once: true });
      return { context, usedRequestedSink: true };
    } catch {
      // 旧版 WebView 同步抛出：直接回退默认输出。
      audioDiagnostics.fallbackToDefault = true;
    }
  }
  const context = new AudioContext();
  recordContext(context, "");
  return { context, usedRequestedSink: false };
}

/**
 * AudioBridge 列出的是 WASAPI 端点 ID，WebAudio 只认 enumerateDevices 的 deviceId。
 * 按设备名称对应；对应不上返回空串，表示使用系统默认输出。
 */
export async function resolveWebAudioSinkId(deviceName: string | undefined): Promise<string> {
  const name = deviceName?.trim();
  if (!name || typeof navigator === "undefined" || !navigator.mediaDevices?.enumerateDevices) return "";
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    const outputs = devices.filter((device) => device.kind === "audiooutput" && device.deviceId);
    const match = outputs.find((device) => device.label === name)
      ?? outputs.find((device) => device.label && (device.label.includes(name) || name.includes(device.label)));
    return match && match.deviceId !== "default" ? match.deviceId : "";
  } catch {
    return "";
  }
}

export class WebAudioPlayer {
  private readonly schedule = new PlaybackSchedule();
  private readonly sources = new Set<AudioBufferSourceNode>();

  constructor(
    private readonly context: AudioContext,
    private readonly sampleRate = 24_000,
  ) {}

  appendPcm16(pcm: Uint8Array): void {
    if (!this.context || this.context.state === "closed" || pcm.length < 2) return;
    const samples = Math.floor(pcm.length / 2);
    const buffer = this.context.createBuffer(1, samples, this.sampleRate);
    const channel = buffer.getChannelData(0);
    const view = new DataView(pcm.buffer, pcm.byteOffset, pcm.byteLength);
    for (let index = 0; index < samples; index++) {
      channel[index] = view.getInt16(index * 2, true) / 32_768;
    }
    const source = this.context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.context.destination);
    const durationMs = (samples * 1000) / this.sampleRate;
    const currentMs = this.context.currentTime * 1000;
    const startMs = this.schedule.nextStartTime(currentMs, durationMs);
    source.start(startMs / 1000);
    this.sources.add(source);
    source.onended = () => this.sources.delete(source);
    audioDiagnostics.chunksScheduled += 1;
    audioDiagnostics.lastScheduledStartMs = startMs;
    audioDiagnostics.lastCurrentTimeMs = currentMs;
    audioDiagnostics.contextState = this.context.state;
  }

  clear(): void {
    for (const source of this.sources) {
      try { source.stop(); } catch { /* already stopped */ }
      source.disconnect();
    }
    this.sources.clear();
    this.schedule.clear();
    audioDiagnostics.clears += 1;
  }

  async resume(): Promise<void> {
    if (this.context.state === "suspended") await this.context.resume();
    audioDiagnostics.contextState = this.context.state;
  }
}
