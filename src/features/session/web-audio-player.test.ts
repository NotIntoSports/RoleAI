import { afterEach, describe, expect, it, vi } from "vitest";

import {
  PlaybackSchedule,
  createAudioContextForOutput,
  decodePcm16Base64,
  resolveWebAudioSinkId,
} from "./web-audio-player";

describe("decodePcm16Base64", () => {
  it("decodes little-endian PCM", () => {
    // AQI= -> [1, 2]
    expect(Array.from(decodePcm16Base64("AQI="))).toEqual([1, 2]);
  });
});

describe("PlaybackSchedule", () => {
  it("delays the first chunk by the jitter target and keeps later chunks continuous", () => {
    const schedule = new PlaybackSchedule(500);
    const first = schedule.nextStartTime(10, 100);
    expect(first).toBe(510);
    expect(schedule.nextStartTime(10, 120)).toBe(610);
    expect(schedule.nextStartTime(10, 80)).toBe(730);
  });

  it("resets after an underrun instead of scheduling in the past", () => {
    const schedule = new PlaybackSchedule(500);
    expect(schedule.nextStartTime(10, 100)).toBe(510);
    expect(schedule.nextStartTime(700, 100)).toBe(1200);
    expect(schedule.nextStartTime(700, 100)).toBe(1300);
  });

  it("clear returns the next chunk to the full jitter target", () => {
    const schedule = new PlaybackSchedule(500);
    expect(schedule.nextStartTime(10, 100)).toBe(510);
    schedule.clear();
    expect(schedule.nextStartTime(20, 100)).toBe(520);
  });
});

describe("createAudioContextForOutput", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("uses the requested sink when AudioContext supports it", () => {
    const original = globalThis.AudioContext;
    class FakeAudioContext {
      constructor(public readonly options: { sinkId?: string }) {}
    }
    vi.stubGlobal("AudioContext", FakeAudioContext);
    const created = createAudioContextForOutput("speaker-1");
    expect(created.usedRequestedSink).toBe(true);
    expect((created.context as unknown as FakeAudioContext).options.sinkId).toBe("speaker-1");
    void original;
  });

  it("falls back to the default output when the requested sink fails", () => {
    class FailingSinkContext {
      constructor(options?: { sinkId?: string }) {
        if (options?.sinkId === "bad-sink") throw new Error("unsupported");
      }
    }
    vi.stubGlobal("AudioContext", FailingSinkContext);
    const created = createAudioContextForOutput("bad-sink");
    expect(created.usedRequestedSink).toBe(false);
    expect(created.context).toBeInstanceOf(FailingSinkContext);
  });

  it("reports an async error event for an invalid sink so callers can fall back", () => {
    class AsyncErrorContext extends EventTarget {
      state = "suspended";
      sampleRate = 48_000;
    }
    vi.stubGlobal("AudioContext", AsyncErrorContext);
    const onFallback = vi.fn();
    const created = createAudioContextForOutput("{0.0.0.00000000}.{wasapi}", onFallback);
    expect(created.usedRequestedSink).toBe(true);
    (created.context as unknown as EventTarget).dispatchEvent(new Event("error"));
    expect(onFallback).toHaveBeenCalledTimes(1);
  });
});

describe("resolveWebAudioSinkId", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("maps an AudioBridge device name to the browser deviceId", async () => {
    vi.stubGlobal("navigator", {
      mediaDevices: {
        enumerateDevices: async () => [
          { kind: "audiooutput", deviceId: "default", label: "默认 - 扬声器 (Realtek(R) Audio)" },
          { kind: "audiooutput", deviceId: "hash-speaker", label: "扬声器 (Realtek(R) Audio)" },
          { kind: "audioinput", deviceId: "hash-mic", label: "麦克风" },
        ],
      },
    });
    expect(await resolveWebAudioSinkId("扬声器 (Realtek(R) Audio)")).toBe("hash-speaker");
    expect(await resolveWebAudioSinkId("不存在的设备")).toBe("");
    expect(await resolveWebAudioSinkId(undefined)).toBe("");
  });
});
