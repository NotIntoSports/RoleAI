// 麦克风信号探针：会话开始时逐候选设备开短暂探针流实测峰值 RMS。
// 绑定是否正确只有实测电平能证明——名称启发式（本模块前身的做法）在
// WebView2 枚举语义差异下会失手；探针即开即停，不进会话链路。
import { VIRTUAL_CAPTURE_PATTERN, type AudioInputInfo } from "./mic-device-selection";

export interface MicDeviceEvidence {
  deviceId: string;
  label: string;
  virtual: boolean;
  rmsPeak: number;
}

export interface MicProbeOptions {
  /** 每个候选的采样时长，默认 400ms。 */
  sampleMs?: number;
  /** 最多探测的候选数，默认 3（default 条目 + 两个物理设备通常足够）。 */
  maxCandidates?: number;
}

export type MicProbeFn = (deviceId: string) => Promise<number | null>;

// 信号判定阈值：rmsLevel 归一化后，真实麦克风安静房间的噪声底也远大于 0，
// 只有纯静音（静音键/未插好/驱动异常）才恒等于 0。
export const MIC_SIGNAL_EPSILON = 0.001;

export function isVirtualCaptureLabel(label: string): boolean {
  return VIRTUAL_CAPTURE_PATTERN.test(label);
}

/** 探测顺序：系统默认条目优先（命中即零改动），其次非虚拟物理设备，最后其余。 */
export function rankProbeCandidates(devices: AudioInputInfo[]): AudioInputInfo[] {
  const usable = devices.filter(
    (device) => device.deviceId && device.deviceId !== "communications",
  );
  const rank = (device: AudioInputInfo): number => {
    if (device.deviceId === "default") return 0;
    if (device.label && !isVirtualCaptureLabel(device.label)) return 1;
    return 2;
  };
  return [...usable].sort((a, b) => rank(a) - rank(b));
}

/**
 * 确定性快绑选择：第一个有信号的非虚拟设备；没有则退到第一个非虚拟设备
 * （仍绑定，静音事实交由 LLM 复核与看门狗呈现）；连非虚拟都没有则回退系统默认。
 */
export function chooseBoundDevice(evidence: MicDeviceEvidence[]): string | null {
  const physical = evidence.filter((device) => !device.virtual);
  const withSignal = physical.find((device) => device.rmsPeak > MIC_SIGNAL_EPSILON);
  return (withSignal ?? physical[0])?.deviceId ?? null;
}

/** 对单个设备开探针流并采样峰值 RMS；打开失败返回 null（设备不可用）。 */
export async function probeMicDevice(
  deviceId: string,
  baseAudio: MediaTrackConstraints,
  sampleMs: number,
): Promise<number | null> {
  let stream: MediaStream | null = null;
  let context: AudioContext | null = null;
  try {
    stream = await navigator.mediaDevices.getUserMedia({
      audio: { ...baseAudio, deviceId: { exact: deviceId } },
    });
    context = new AudioContext();
    await context.resume();
    const source = context.createMediaStreamSource(stream);
    const analyser = context.createAnalyser();
    analyser.fftSize = 2048;
    source.connect(analyser);
    const buffer = new Float32Array(analyser.fftSize);
    let peak = 0;
    const sample = () => {
      analyser.getFloatTimeDomainData(buffer);
      let sum = 0;
      for (let index = 0; index < buffer.length; index++) {
        sum += buffer[index] * buffer[index];
      }
      peak = Math.max(peak, Math.sqrt(sum / buffer.length));
    };
    const deadline = Date.now() + sampleMs;
    while (Date.now() < deadline) {
      sample();
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    return peak;
  } catch {
    return null;
  } finally {
    stream?.getTracks().forEach((track) => track.stop());
    if (context) void context.close().catch(() => undefined);
  }
}

/**
 * 采集证据表：按传入设备排序 → 逐候选探测。probe 可注入（测试用假探针）；
 * 单个候选探测失败不中断，整表失败由调用方降级。
 */
export async function collectMicEvidence(
  devices: AudioInputInfo[],
  probe: MicProbeFn,
  options: MicProbeOptions = {},
): Promise<MicDeviceEvidence[]> {
  const maxCandidates = options.maxCandidates ?? 3;
  const candidates = rankProbeCandidates(devices).slice(0, maxCandidates);
  const evidence: MicDeviceEvidence[] = [];
  for (const candidate of candidates) {
    const rmsPeak = await probe(candidate.deviceId);
    if (rmsPeak === null) continue;
    evidence.push({
      deviceId: candidate.deviceId,
      label: candidate.label,
      virtual: isVirtualCaptureLabel(candidate.label),
      rmsPeak,
    });
  }
  return evidence;
}

export function defaultMicProbe(
  baseAudio: MediaTrackConstraints,
  sampleMs: number,
): MicProbeFn {
  return (deviceId) => probeMicDevice(deviceId, baseAudio, sampleMs);
}
