import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { joinDailyRoom } from "./daily-room";

const mocks = vi.hoisted(() => {
  const handlers = new Map<string, (event: unknown) => void>();
  const call = {
    on: vi.fn((event: string, handler: (event: unknown) => void) => {
      handlers.set(event, handler);
    }),
    join: vi.fn(async () => undefined),
    leave: vi.fn(async () => undefined),
    destroy: vi.fn(async () => undefined),
    setLocalAudio: vi.fn(() => undefined),
    localAudio: vi.fn(() => true),
  };
  const createCallObject = vi.fn(() => call);
  return { handlers, call, createCallObject };
});

vi.mock("@daily-co/daily-js", () => ({ default: { createCallObject: mocks.createCallObject } }));

// jsdom 没有 MediaStream/MediaStreamTrack：daily-room 只把它们包进 srcObject，用最小桩替代。
class FakeMediaStream {
  constructor(tracks: unknown[]) {
    this.getTracks = () => tracks;
  }
  getTracks: () => unknown[];
}

function mediaElement(tag: "video" | "audio"): HTMLMediaElement {
  const element = document.createElement(tag) as HTMLMediaElement;
  vi.spyOn(element, "play").mockResolvedValue(undefined);
  return element;
}

function remoteTrack(kind: "video" | "audio") {
  return { kind, enabled: true } as unknown as MediaStreamTrack;
}

function emit(event: string, payload: unknown) {
  const handler = mocks.handlers.get(event);
  if (!handler) throw new Error(`no handler registered for ${event}`);
  handler(payload);
}

describe("joinDailyRoom", () => {
  let video: HTMLVideoElement;
  let audio: HTMLAudioElement;

  beforeEach(() => {
    video = mediaElement("video") as HTMLVideoElement;
    audio = mediaElement("audio") as HTMLAudioElement;
    vi.stubGlobal("MediaStream", FakeMediaStream);
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
  });

  it("loads the call engine without eval, joins with mic publishing and cleans up on leave", async () => {
    const room = await joinDailyRoom("https://tavus.daily.co/c123", video, audio);
    expect(mocks.createCallObject).toHaveBeenCalledWith({ dailyConfig: { avoidEval: true } });
    expect(mocks.call.join).toHaveBeenCalledWith({
      url: "https://tavus.daily.co/c123",
      subscribeToTracksAutomatically: true,
      startVideoOff: true,
      startAudioOff: false,
    });
    expect(mocks.call.setLocalAudio).toHaveBeenCalledWith(true);
    await room.leave();
    expect(mocks.call.leave).toHaveBeenCalledTimes(1);
    expect(mocks.call.destroy).toHaveBeenCalledTimes(1);
  });

  it("binds remote camera video and audio tracks on fresh MediaStreams, ignoring local and screen tracks", () => {
    return joinDailyRoom("https://tavus.daily.co/c123", video, audio).then(() => {
      const remoteCamera = remoteTrack("video");
      emit("track-started", {
        participant: { local: false },
        track: remoteCamera,
        type: "video",
      });
      expect(video.srcObject).toBeInstanceOf(FakeMediaStream);
      expect((video.srcObject as unknown as FakeMediaStream).getTracks()).toEqual([remoteCamera]);

      const remoteVoice = remoteTrack("audio");
      emit("track-started", {
        participant: { local: false },
        track: remoteVoice,
        type: "audio",
      });
      expect((audio.srcObject as unknown as FakeMediaStream).getTracks()).toEqual([remoteVoice]);

      const screenShare = remoteTrack("video");
      emit("track-started", {
        participant: { local: false },
        track: screenShare,
        type: "screenVideo",
      });
      expect((video.srcObject as unknown as FakeMediaStream).getTracks()).toEqual([remoteCamera]);

      const localTrack = remoteTrack("video");
      emit("track-started", {
        participant: { local: true },
        track: localTrack,
        type: "video",
      });
      expect((video.srcObject as unknown as FakeMediaStream).getTracks()).toEqual([remoteCamera]);

      const replacement = remoteTrack("video");
      emit("track-started", {
        participant: { local: false },
        track: replacement,
        type: "video",
      });
      expect((video.srcObject as unknown as FakeMediaStream).getTracks()).toEqual([replacement]);
    });
  });

  it("clears the element when the track it is playing stops", async () => {
    await joinDailyRoom("https://tavus.daily.co/c123", video, audio);
    const stopped = remoteTrack("video");
    emit("track-started", { participant: { local: false }, track: stopped, type: "video" });
    emit("track-stopped", { participant: { local: false }, track: stopped, type: "video" });
    expect(video.srcObject).toBeNull();

    const other = remoteTrack("audio");
    emit("track-started", { participant: { local: false }, track: other, type: "audio" });
    emit("track-stopped", { participant: { local: false }, track: remoteTrack("audio"), type: "audio" });
    expect(audio.srcObject).toBeInstanceOf(FakeMediaStream);
  });

  it("forwards Tavus utterances and speaking states from app-messages", async () => {
    const onUtterance = vi.fn();
    const onSpeakingChange = vi.fn();
    await joinDailyRoom("https://tavus.daily.co/c123", video, audio, { onUtterance, onSpeakingChange });

    emit("app-message", {
      data: { event_type: "conversation.utterance", properties: { speech: "你好，请开始。", role: "replica" } },
      fromId: "tavus",
    });
    expect(onUtterance).toHaveBeenLastCalledWith("你好，请开始。", "replica");

    emit("app-message", {
      data: { event: { event_type: "conversation.utterance", data: { speech: "我的经历是……", role: "user" } } },
      fromId: "tavus",
    });
    expect(onUtterance).toHaveBeenLastCalledWith("我的经历是……", "user");

    emit("app-message", {
      data: { event_type: "conversation.replica.started_speaking" },
      fromId: "tavus",
    });
    expect(onSpeakingChange).toHaveBeenLastCalledWith("replica-speaking");
    emit("app-message", {
      data: { event_type: "conversation.replica.stopped_speaking" },
      fromId: "tavus",
    });
    expect(onSpeakingChange).toHaveBeenLastCalledWith("listening");
    emit("app-message", {
      data: { event_type: "conversation.user.started_speaking" },
      fromId: "tavus",
    });
    expect(onSpeakingChange).toHaveBeenLastCalledWith("user-speaking");
    emit("app-message", {
      data: { event_type: "conversation.user.stopped_speaking" },
      fromId: "tavus",
    });
    expect(onSpeakingChange).toHaveBeenLastCalledWith("listening");
    expect(onSpeakingChange).toHaveBeenCalledTimes(4);
  });

  it("releases the room and throws a Chinese reason when the microphone cannot be published", async () => {
    mocks.call.localAudio.mockReturnValueOnce(false);
    await expect(joinDailyRoom("https://tavus.daily.co/c123", video, audio)).rejects.toThrow(
      "无法打开麦克风，通话未接通。请检查系统麦克风权限后重试。",
    );
    expect(mocks.call.leave).toHaveBeenCalledTimes(1);
    expect(mocks.call.destroy).toHaveBeenCalledTimes(1);
  });

  it("releases the room when setLocalAudio throws", async () => {
    mocks.call.setLocalAudio.mockImplementationOnce(() => {
      throw new Error("getUserMedia failed");
    });
    await expect(joinDailyRoom("https://tavus.daily.co/c123", video, audio)).rejects.toThrow(
      "无法打开麦克风",
    );
    expect(mocks.call.leave).toHaveBeenCalledTimes(1);
    expect(mocks.call.destroy).toHaveBeenCalledTimes(1);
  });
});
