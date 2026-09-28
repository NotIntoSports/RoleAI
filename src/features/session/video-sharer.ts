// 摄像头/桌面共享 → 抽帧 JPEG 推给实时会话（Qwen-Omni 系 input_image_buffer）。
// 官方约束：仅 JPEG、约 1 帧/秒、建议 480P（≤1080P）、单帧 base64 ≤ 256KB。
export const VIDEO_FRAME_INTERVAL_MS = 1000;

// 480P 上限：等比缩到最大 854 宽（16:9 即 480 高），兼顾清晰度与体积约束。
const MAX_FRAME_WIDTH = 854;
const JPEG_QUALITY = 0.72;

export type VideoShareKind = "camera" | "screen";

export interface VideoSharerController {
  start(): Promise<void>;
  stop(): void;
  /** 采集流就绪后由调用方挂到预览 <video>（PiP）上。 */
  readonly stream: MediaStream | null;
}

export interface VideoSharerCallbacks {
  onFrame: (jpegBase64: string) => void;
  onError: (message: string) => void;
  /** 用户在系统 UI 点"停止共享"或摄像头被拔出。 */
  onEnded: () => void;
}

// 纯函数：video → 画布抽帧 → JPEG base64（去 data: 前缀）。导出供单测。
export function extractJpegBase64(
  video: HTMLVideoElement,
  canvas: HTMLCanvasElement,
): string | null {
  const width = video.videoWidth;
  const height = video.videoHeight;
  if (!width || !height) return null;
  const scale = Math.min(1, MAX_FRAME_WIDTH / width);
  canvas.width = Math.round(width * scale);
  canvas.height = Math.round(height * scale);
  const context = canvas.getContext("2d");
  if (!context) return null;
  context.drawImage(video, 0, 0, canvas.width, canvas.height);
  const dataUrl = canvas.toDataURL("image/jpeg", JPEG_QUALITY);
  const comma = dataUrl.indexOf(",");
  return comma >= 0 ? dataUrl.slice(comma + 1) : null;
}

export class VideoSharer implements VideoSharerController {
  private media: MediaStream | null = null;
  private video: HTMLVideoElement | null = null;
  private canvas: HTMLCanvasElement | null = null;
  private timer = 0;

  constructor(
    readonly kind: VideoShareKind,
    private callbacks: VideoSharerCallbacks,
  ) {}

  get stream(): MediaStream | null {
    return this.media;
  }

  async start(): Promise<void> {
    if (this.media) throw new Error("video sharer already started");
    const media = this.kind === "camera"
      ? await navigator.mediaDevices.getUserMedia({
        video: { width: { ideal: 1280 }, height: { ideal: 720 } },
        audio: false,
      })
      : await navigator.mediaDevices.getDisplayMedia({ video: true, audio: false });
    this.media = media;
    // 桌面共享由系统 UI 停止（停止共享按钮/关窗）时通知调用方复位状态。
    for (const track of media.getVideoTracks()) {
      track.addEventListener("ended", () => {
        if (this.media === media) this.callbacks.onEnded();
      });
    }
    const video = document.createElement("video");
    video.srcObject = media;
    video.muted = true;
    await video.play();
    this.video = video;
    this.canvas = document.createElement("canvas");
    this.timer = window.setInterval(() => {
      if (!this.video || !this.canvas) return;
      const frame = extractJpegBase64(this.video, this.canvas);
      if (frame) this.callbacks.onFrame(frame);
    }, VIDEO_FRAME_INTERVAL_MS);
  }

  stop(): void {
    if (this.timer) window.clearInterval(this.timer);
    this.timer = 0;
    this.media?.getTracks().forEach((track) => track.stop());
    this.media = null;
    if (this.video) {
      this.video.srcObject = null;
      this.video = null;
    }
    this.canvas = null;
  }
}
