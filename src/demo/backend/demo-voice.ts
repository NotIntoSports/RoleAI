// 演示语音（默认开启）：播放预生成的真人音色 mp3——构建期由 scripts/generate-demo-voice.ts
// 用微软 Edge 神经音色生成并提交进仓库，运行时零网络请求。
// 关闭过一次会记住（存储 "0"）；缺资产、功能关闭或播放失败一律静默回退为纯文字，
// 绝不影响文字时间线。文件名约定与生成器保持一致（见 generate-demo-voice.ts）。
/// <reference types="vite/client" />
import { currentLanguage } from "../../i18n";

const VOICE_KEY = "roleai.demo.voice";

const ASSETS = import.meta.glob<string>("../assets/voice/*.mp3", {
  eager: true,
  query: "?url",
  import: "default",
}) as Record<string, string>;

export function isDemoVoiceEnabled(): boolean {
  try {
    return window.localStorage.getItem(VOICE_KEY) !== "0";
  } catch {
    return true;
  }
}

export function setDemoVoiceEnabled(enabled: boolean): void {
  try {
    // 默认即开启，无需落盘；只有显式关闭才记 "0"。
    if (enabled) window.localStorage.removeItem(VOICE_KEY);
    else window.localStorage.setItem(VOICE_KEY, "0");
  } catch {
    // 存储不可用时仅影响本次会话记忆。
  }
  if (!enabled) stopDemoVoice();
}

function languageTag(): "zh" | "en" {
  return currentLanguage() === "en" ? "en" : "zh";
}

function assetUrl(key: string): string | null {
  const url = ASSETS[`../assets/voice/${key}.mp3`];
  return typeof url === "string" ? url : null;
}

/** 脚本台词的音频键：<脚本id>.<语言>.t<轮次>.<user|ai>。 */
export function scriptLineKey(scriptId: string, turnIndex: number, side: "user" | "ai"): string {
  return `${scriptId}.${languageTag()}.t${turnIndex}.${side}`;
}

/** 用户插话兜底回答的音频键：tail.<语言>.<序号（1 起）>。 */
export function tailLineKey(index: number): string {
  return `tail.${languageTag()}.${index}`;
}

export interface DemoVoiceLine {
  /** 音频元数据就绪后的时长（毫秒）；3 秒内拿不到则 resolve null（调用方退回文字节奏）。 */
  ready: Promise<number | null>;
  /** 播放结束（自然播完/失败/被 stop）后 resolve。 */
  done: Promise<void>;
}

let current: { audio: HTMLAudioElement; finish: () => void } | null = null;

/** 播一行台词音频。功能关闭、缺资产或环境不支持时返回 null，调用方按纯文字处理。 */
export function startDemoLine(key: string): DemoVoiceLine | null {
  // 串行播放：新行开始前停掉上一行（用户插话/快速重试时避免叠音）。
  stopDemoVoice();
  if (!isDemoVoiceEnabled()) return null;
  const url = assetUrl(key);
  if (!url || typeof Audio !== "function") return null;

  const audio = new Audio(url);
  audio.preload = "auto";

  let settled = false;
  let resolveReady: (durationMs: number | null) => void = () => undefined;
  let resolveDone: () => void = () => undefined;
  const line: DemoVoiceLine = {
    ready: new Promise((resolve) => {
      resolveReady = resolve;
    }),
    done: new Promise((resolve) => {
      resolveDone = resolve;
    }),
  };

  const finish = () => {
    if (settled) return;
    settled = true;
    if (current && current.audio === audio) current = null;
    resolveDone();
  };
  audio.addEventListener("loadedmetadata", () => {
    resolveReady(Number.isFinite(audio.duration) && audio.duration > 0 ? audio.duration * 1000 : null);
  });
  audio.addEventListener("ended", finish);
  audio.addEventListener("error", () => {
    resolveReady(null);
    finish();
  });
  // 元数据迟迟不就绪（网络慢等）不让时间线卡死：退回文字节奏，音频继续播。
  window.setTimeout(() => resolveReady(null), 3000);

  current = { audio, finish };
  // 自动播放策略：演示里任何 play 前都发生过用户点击（开始会话），可以出声。
  audio.play().catch(() => {
    resolveReady(null);
    finish();
  });
  return line;
}

/** 停止当前播放（打断 / 结束会话 / 关闭开关时调用）。 */
export function stopDemoVoice(): void {
  const stopped = current;
  current = null;
  if (!stopped) return;
  try {
    stopped.audio.pause();
  } catch {
    // 忽略。
  }
  stopped.finish();
}
