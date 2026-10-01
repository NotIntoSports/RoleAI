// demo-voice 单测：假 Audio 桩驱动播放/停止/时长语义；资产查找用真实生成的 mp3（按约定键）。
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setLanguagePreference } from "../../i18n";
import {
  isDemoVoiceEnabled,
  scriptLineKey,
  setDemoVoiceEnabled,
  startDemoLine,
  stopDemoVoice,
  tailLineKey,
} from "./demo-voice";

class FakeAudio {
  static instances: FakeAudio[] = [];
  /** 模拟“元数据迟迟不到”：置 false 时 play() 不触发 loadedmetadata。 */
  static emitMetadataOnPlay = true;

  preload = "";
  duration = 1.5;
  paused = true;
  playCalled = 0;
  pauseCalled = 0;
  private listeners = new Map<string, Array<() => void>>();

  constructor(public src: string) {
    FakeAudio.instances.push(this);
  }

  addEventListener(event: string, handler: () => void): void {
    const list = this.listeners.get(event) ?? [];
    list.push(handler);
    this.listeners.set(event, list);
  }

  async play(): Promise<void> {
    this.playCalled += 1;
    this.paused = false;
    if (FakeAudio.emitMetadataOnPlay) this.emit("loadedmetadata");
  }

  pause(): void {
    this.pauseCalled += 1;
    this.paused = true;
  }

  emit(event: string): void {
    for (const handler of [...(this.listeners.get(event) ?? [])]) handler();
  }
}

describe("demo-voice", () => {
  beforeEach(() => {
    window.localStorage.clear();
    FakeAudio.instances = [];
    vi.stubGlobal("Audio", FakeAudio as unknown as typeof Audio);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    setLanguagePreference("zh-CN");
  });

  it("is enabled by default without any stored preference", () => {
    expect(isDemoVoiceEnabled()).toBe(true);
  });

  it("remembers an explicit off as 0 and returns to default-on when re-enabled", () => {
    setDemoVoiceEnabled(false);
    expect(window.localStorage.getItem("roleai.demo.voice")).toBe("0");
    expect(isDemoVoiceEnabled()).toBe(false);
    setDemoVoiceEnabled(true);
    expect(window.localStorage.getItem("roleai.demo.voice")).toBeNull();
    expect(isDemoVoiceEnabled()).toBe(true);
  });

  it("returns null lines when disabled or when the asset key is unknown", () => {
    setDemoVoiceEnabled(false);
    expect(startDemoLine(scriptLineKey("interview-strict", 1, "user"))).toBeNull();
    setDemoVoiceEnabled(true);
    expect(startDemoLine("no-such-script.zh.t1.user")).toBeNull();
    expect(stopDemoVoice()).toBeUndefined();
  });

  it("builds language-scoped keys", () => {
    expect(scriptLineKey("interview-strict", 3, "ai")).toBe("interview-strict.zh.t3.ai");
    expect(tailLineKey(1)).toBe("tail.zh.1");
    setLanguagePreference("en");
    expect(scriptLineKey("interview-strict", 3, "ai")).toBe("interview-strict.en.t3.ai");
    expect(tailLineKey(2)).toBe("tail.en.2");
  });

  it("plays a known asset: ready resolves the duration, done resolves on ended", async () => {
    const line = startDemoLine(scriptLineKey("interview-strict", 1, "user"));
    expect(line).not.toBeNull();
    const audio = FakeAudio.instances.at(-1)!;
    expect(audio.src).toContain("interview-strict.zh.t1.user.mp3");
    expect(audio.playCalled).toBe(1);
    await expect(line!.ready).resolves.toBe(1500);
    audio.emit("ended");
    await expect(line!.done).resolves.toBeUndefined();
  });

  it("stopDemoVoice pauses the playing line and settles done", async () => {
    const line = startDemoLine(scriptLineKey("interview-strict", 1, "ai"));
    const audio = FakeAudio.instances.at(-1)!;
    stopDemoVoice();
    expect(audio.pauseCalled).toBe(1);
    await expect(line!.done).resolves.toBeUndefined();
    // 停止后再播新行是全新实例（串行播放，不叠音）。
    const next = startDemoLine(scriptLineKey("interview-strict", 2, "ai"));
    expect(FakeAudio.instances.at(-1)!.playCalled).toBe(1);
    expect(next).not.toBeNull();
  });

  it("falls back to a null duration when metadata never arrives, without blocking done", async () => {
    vi.useFakeTimers();
    FakeAudio.emitMetadataOnPlay = false;
    try {
      const line = startDemoLine(scriptLineKey("coach", 1, "user"));
      expect(line).not.toBeNull();
      await vi.advanceTimersByTimeAsync(3001);
      await expect(line!.ready).resolves.toBeNull();
      const audio = FakeAudio.instances.at(-1)!;
      audio.emit("ended");
      await expect(line!.done).resolves.toBeUndefined();
    } finally {
      FakeAudio.emitMetadataOnPlay = true;
      vi.useRealTimers();
    }
  });
});
