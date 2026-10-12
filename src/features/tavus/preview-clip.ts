import type { TavusCaptionRole, TavusSpeakingStatus } from "./daily-room";
import clipUrl from "./digital-human-preview.mp4";
import { PREVIEW_BEATS, type PreviewBeatId, type PreviewCaptionKey } from "./preview-script";
import timing from "./preview-timing.json";

/** 开口聆听结束、会议助手开始说话的时间（秒）。 */
export const PREVIEW_READY_AT = timing.readyAt;
/** 参会者点名开始的时间（秒）。 */
export const PREVIEW_ASK_AT = timing.askAt;
/** 三句纪要开始的时间（秒）。 */
export const PREVIEW_SUMMARY_AT = timing.summaryAt;
/** 纪要说完、回到聆听的时间（秒）。 */
export const PREVIEW_LISTEN_AT = timing.listenAt;
/** 助手正在说话的一帧，截图停在这里。 */
export const PREVIEW_SPEAKING_FRAME_AT = timing.speakingFrameAt;
/** 整段循环时长（秒）。 */
export const PREVIEW_DURATION = timing.duration;

export const previewClipUrl = clipUrl;

export type { PreviewCaptionKey };

export type PreviewCue = {
  /** 开口和说完没有字幕。 */
  caption: PreviewCaptionKey | null;
  role: TavusCaptionRole;
  status: TavusSpeakingStatus;
};

const beatStart: Record<PreviewBeatId, number> = {
  ready: PREVIEW_READY_AT,
  ask: PREVIEW_ASK_AT,
  summary: PREVIEW_SUMMARY_AT,
};

export function previewCue(currentTime: number): PreviewCue {
  const listening: PreviewCue = { caption: null, role: "replica", status: "listening" };
  if (!Number.isFinite(currentTime) || currentTime < PREVIEW_READY_AT || currentTime >= PREVIEW_LISTEN_AT) {
    return listening;
  }
  const active = [...PREVIEW_BEATS].reverse().find((beat) => currentTime >= beatStart[beat.id]);
  if (!active) return listening;
  return {
    caption: active.captionKey,
    role: active.speaker,
    status: active.speaker === "user" ? "user-speaking" : "replica-speaking",
  };
}

/** 在用户点击的同一轮里开始播放，避免浏览器拦住有声自动播放。 */
export function attachPreviewClip(video: HTMLVideoElement): void {
  video.loop = true;
  video.muted = false;
  video.playsInline = true;
  const source = `${video.currentSrc} ${video.src}`;
  if (!source.includes("digital-human-preview")) {
    video.src = previewClipUrl;
  }
  try {
    const pending = video.play();
    void pending?.catch(() => undefined);
  } catch {
    // jsdom 没有媒体播放实现；浏览器里由点击手势触发。
  }
}

export function detachPreviewClip(video: HTMLVideoElement | null): void {
  if (!video) return;
  try {
    video.pause();
  } catch {
    // 忽略测试环境里未实现的媒体操作。
  }
  video.muted = true;
  video.loop = false;
  video.removeAttribute("src");
  try {
    video.load();
  } catch {
    // jsdom 可能拒绝空媒体重载。
  }
}
