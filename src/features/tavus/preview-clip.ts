import clipUrl from "./digital-human-preview.mp4";

/** 第二句字幕开始的时间（秒），对齐片段里第二句台词。 */
export const PREVIEW_CAPTION2_AT = 5.03;
/** 台词说完、状态回到聆听的时间（秒）。片段在这之后仍循环，画面不会停住。 */
export const PREVIEW_LISTEN_AT = 10.45;
/** 张嘴帧的时间（秒），截图停在这里。 */
export const PREVIEW_SPEAKING_FRAME_AT = 0.32;

export const previewClipUrl = clipUrl;

export type PreviewCue = {
  caption: "first" | "second";
  speaking: boolean;
};

export function previewCue(currentTime: number): PreviewCue {
  if (!Number.isFinite(currentTime) || currentTime < PREVIEW_CAPTION2_AT) {
    return { caption: "first", speaking: true };
  }
  if (currentTime < PREVIEW_LISTEN_AT) {
    return { caption: "second", speaking: true };
  }
  return { caption: "second", speaking: false };
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
