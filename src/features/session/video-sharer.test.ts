import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { VIDEO_FRAME_INTERVAL_MS, VideoSharer, extractJpegBase64 } from "./video-sharer";

function fakeTrack() {
  return { stop: vi.fn(), addEventListener: vi.fn(), kind: "video" };
}

function fakeMediaStream(track = fakeTrack()) {
  return { getVideoTracks: () => [track], getTracks: () => [track] } as unknown as MediaStream;
}

function fakeVideo(width: number, height: number) {
  return { videoWidth: width, videoHeight: height } as HTMLVideoElement;
}

function fakeCanvas(dataUrl = "data:image/jpeg;base64,QUJD") {
  const context = { drawImage: vi.fn() };
  const canvas = {
    width: 0,
    height: 0,
    getContext: vi.fn(() => context),
    toDataURL: vi.fn(() => dataUrl),
  };
  return { canvas: canvas as unknown as HTMLCanvasElement, context, toDataURL: canvas.toDataURL };
}

function installMediaDevices() {
  const getUserMedia = vi.fn(async () => fakeMediaStream());
  const getDisplayMedia = vi.fn(async () => fakeMediaStream());
  Object.defineProperty(navigator, "mediaDevices", {
    configurable: true,
    value: { getUserMedia, getDisplayMedia },
  });
  return { getUserMedia, getDisplayMedia };
}

describe("extractJpegBase64", () => {
  it("returns null when the video has no dimensions yet", () => {
    const { canvas } = fakeCanvas();
    expect(extractJpegBase64(fakeVideo(0, 0), canvas)).toBeNull();
  });

  it("scales 16:9 frames down to the 480P cap and keeps smaller frames untouched", () => {
    const large = fakeCanvas();
    const largeVideo = fakeVideo(1920, 1080);
    expect(extractJpegBase64(largeVideo, large.canvas)).toBe("QUJD");
    expect(large.canvas.width).toBe(854);
    expect(large.canvas.height).toBe(480);
    expect(large.context.drawImage).toHaveBeenCalledWith(largeVideo, 0, 0, 854, 480);

    const small = fakeCanvas();
    const smallVideo = fakeVideo(640, 480);
    extractJpegBase64(smallVideo, small.canvas);
    expect(small.canvas.width).toBe(640);
    expect(small.canvas.height).toBe(480);
    expect(small.context.drawImage).toHaveBeenCalledWith(smallVideo, 0, 0, 640, 480);
  });

  it("returns null when the data URL has no base64 payload", () => {
    const { canvas } = fakeCanvas("data:image/jpeg");
    expect(extractJpegBase64(fakeVideo(640, 480), canvas)).toBeNull();
  });
});

describe("VideoSharer", () => {
  let mediaDevices: ReturnType<typeof installMediaDevices>;

  beforeEach(() => {
    mediaDevices = installMediaDevices();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
      drawImage: vi.fn(),
    } as unknown as CanvasRenderingContext2D);
    vi.spyOn(HTMLCanvasElement.prototype, "toDataURL").mockReturnValue("data:image/jpeg;base64,QUJD");
    Object.defineProperty(HTMLVideoElement.prototype, "videoWidth", { configurable: true, get: () => 1280 });
    Object.defineProperty(HTMLVideoElement.prototype, "videoHeight", { configurable: true, get: () => 720 });
    vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
    Reflect.deleteProperty(navigator, "mediaDevices");
    Reflect.deleteProperty(HTMLVideoElement.prototype, "videoWidth");
    Reflect.deleteProperty(HTMLVideoElement.prototype, "videoHeight");
  });

  it("starts the camera with 720P constraints and exposes the stream", async () => {
    const stream = fakeMediaStream();
    mediaDevices.getUserMedia.mockResolvedValueOnce(stream);
    const sharer = new VideoSharer("camera", { onFrame: vi.fn(), onError: vi.fn(), onEnded: vi.fn() });
    await sharer.start();
    expect(mediaDevices.getUserMedia).toHaveBeenCalledWith({
      video: { width: { ideal: 1280 }, height: { ideal: 720 } },
      audio: false,
    });
    expect(sharer.stream).toBe(stream);
    sharer.stop();
  });

  it("starts screen sharing through getDisplayMedia", async () => {
    const sharer = new VideoSharer("screen", { onFrame: vi.fn(), onError: vi.fn(), onEnded: vi.fn() });
    await sharer.start();
    expect(mediaDevices.getDisplayMedia).toHaveBeenCalledWith({ video: true, audio: false });
    sharer.stop();
  });

  it("rejects a second start while already running", async () => {
    const sharer = new VideoSharer("camera", { onFrame: vi.fn(), onError: vi.fn(), onEnded: vi.fn() });
    await sharer.start();
    await expect(sharer.start()).rejects.toThrow("video sharer already started");
    sharer.stop();
  });

  it("pushes a JPEG frame about once per second and stops pushing after stop", async () => {
    vi.useFakeTimers();
    const onFrame = vi.fn();
    const sharer = new VideoSharer("camera", { onFrame, onError: vi.fn(), onEnded: vi.fn() });
    await sharer.start();
    await vi.advanceTimersByTimeAsync(VIDEO_FRAME_INTERVAL_MS);
    expect(onFrame).toHaveBeenCalledWith("QUJD");
    sharer.stop();
    onFrame.mockClear();
    await vi.advanceTimersByTimeAsync(VIDEO_FRAME_INTERVAL_MS * 3);
    expect(onFrame).not.toHaveBeenCalled();
  });

  it("stops tracks and is idempotent when called repeatedly", async () => {
    const track = fakeTrack();
    const stream = fakeMediaStream(track);
    mediaDevices.getUserMedia.mockResolvedValueOnce(stream);
    const sharer = new VideoSharer("camera", { onFrame: vi.fn(), onError: vi.fn(), onEnded: vi.fn() });
    await sharer.start();
    sharer.stop();
    expect(track.stop).toHaveBeenCalledTimes(1);
    expect(sharer.stream).toBeNull();
    expect(() => sharer.stop()).not.toThrow();
    expect(track.stop).toHaveBeenCalledTimes(1);
  });

  it("notifies onEnded from the track ended event while active, but not after stop", async () => {
    const track = fakeTrack();
    const stream = fakeMediaStream(track);
    mediaDevices.getUserMedia.mockResolvedValueOnce(stream);
    const onEnded = vi.fn();
    const sharer = new VideoSharer("camera", { onFrame: vi.fn(), onError: vi.fn(), onEnded });
    await sharer.start();
    const handler = track.addEventListener.mock.calls.find(([event]) => event === "ended")?.[1] as () => void;
    expect(handler).toBeTypeOf("function");
    handler();
    expect(onEnded).toHaveBeenCalledTimes(1);
    sharer.stop();
    handler();
    expect(onEnded).toHaveBeenCalledTimes(1);
  });
});
