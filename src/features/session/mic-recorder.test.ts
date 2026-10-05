import { afterEach, describe, expect, it, vi } from "vitest";

import { CHUNK_TARGET_MS, MicChunkBatcher, MicStreamer, encodePcm16 } from "./mic-recorder";

// 探针会 new AudioContext() 采样：给一个带 Analyser 的假上下文，
// getFloatTimeDomainData 填 0.1 → 探针峰值 RMS ≈ 0.1（视为有信号）。
function stubProbeAudioContext() {
  vi.stubGlobal("AudioContext", class {
    state = "running";
    destination = {};
    resume = vi.fn(async () => {});
    close = vi.fn(async () => {});
    createMediaStreamSource = () => ({ connect: vi.fn(), disconnect: vi.fn() });
    createGain = () => ({ connect: vi.fn(), disconnect: vi.fn(), gain: { value: 1 } });
    createAnalyser = () => ({
      fftSize: 0,
      getFloatTimeDomainData: (buffer: Float32Array) => buffer.fill(0.1),
    });
  });
}

function stubWorkletGlobals() {
  vi.stubGlobal("URL", { createObjectURL: () => "blob:worklet", revokeObjectURL: () => {} });
  vi.stubGlobal("Blob", class {});
  vi.stubGlobal("AudioWorkletNode", class {
    port: { onmessage: ((event: MessageEvent) => void) | null } = { onmessage: null };
    connect() {}
    disconnect() {}
  });
}

function workletContextStub() {
  return {
    state: "running",
    sampleRate: 48_000,
    destination: {},
    resume: vi.fn(async () => {}),
    close: vi.fn(async () => {}),
    audioWorklet: { addModule: vi.fn(async () => {}) },
    createMediaStreamSource: vi.fn(() => ({ connect: vi.fn(), disconnect: vi.fn() })),
    createGain: vi.fn(() => ({ connect: vi.fn(), disconnect: vi.fn(), gain: { value: 1 } })),
  } as unknown as AudioContext;
}

interface FakeTrack {
  stop: ReturnType<typeof vi.fn>;
  getSettings: () => { deviceId: string };
}

function makeStream(track: FakeTrack) {
  return { getTracks: () => [track], getAudioTracks: () => [track] };
}

describe("MicStreamer with a shared AudioContext", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("starts on the shared context and leaves it open on stop", async () => {
    const track: FakeTrack = { stop: vi.fn(), getSettings: () => ({ deviceId: "d" }) };
    vi.stubGlobal("navigator", {
      mediaDevices: { getUserMedia: vi.fn(async () => makeStream(track)) },
    });
    stubProbeAudioContext();
    stubWorkletGlobals();
    const context = workletContextStub();

    const streamer = new MicStreamer({ onChunk: () => {}, onError: () => {} }, context, 20);
    const evidence = await streamer.start();
    expect(evidence).toEqual([]);
    streamer.stop();

    expect(track.stop).toHaveBeenCalled();
    expect(context.close).not.toHaveBeenCalled();
  });
});

describe("MicStreamer signal-evidence binding", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("probes candidates and binds the first physical device with signal via exact id", async () => {
    const cableTrack: FakeTrack = { stop: vi.fn(), getSettings: () => ({ deviceId: "guid-cable" }) };
    const micTrack: FakeTrack = { stop: vi.fn(), getSettings: () => ({ deviceId: "guid-mic" }) };
    const getUserMedia = vi.fn(async (constraints: MediaStreamConstraints) => {
      const audio = constraints.audio as MediaTrackConstraints;
      const deviceId = (audio.deviceId as { exact?: string } | undefined)?.exact ?? "";
      if (deviceId === "guid-cable") {
        // 虚拟端点探测：给非零信号也不应被选为绑定目标。
        return makeStream(cableTrack);
      }
      return makeStream(micTrack);
    });
    vi.stubGlobal("navigator", {
      mediaDevices: {
        getUserMedia,
        enumerateDevices: vi.fn(async () => [
          { kind: "audioinput", deviceId: "default", label: "默认 - CABLE Output (VB-Audio Virtual Cable)" },
          { kind: "audioinput", deviceId: "guid-cable", label: "CABLE Output (VB-Audio Virtual Cable)" },
          { kind: "audioinput", deviceId: "guid-mic", label: "Microphone Array (Intel Smart Sound)" },
        ]),
      },
    });
    stubProbeAudioContext();
    stubWorkletGlobals();
    const context = workletContextStub();

    const streamer = new MicStreamer({ onChunk: () => {}, onError: () => {} }, context, 20);
    const evidence = await streamer.start();

    expect(evidence.map((entry) => entry.deviceId)).toEqual(["default", "guid-mic", "guid-cable"]);
    const bindCall = getUserMedia.mock.calls.at(-1)![0] as MediaStreamConstraints;
    expect((bindCall.audio as MediaTrackConstraints).deviceId).toEqual({ exact: "guid-mic" });
    streamer.stop();
    expect(micTrack.stop).toHaveBeenCalled();
  });

  it("binds without a constraint when enumeration yields no candidates", async () => {
    const track: FakeTrack = { stop: vi.fn(), getSettings: () => ({ deviceId: "" }) };
    const getUserMedia = vi.fn(async (_constraints: MediaStreamConstraints) => makeStream(track));
    vi.stubGlobal("navigator", { mediaDevices: { getUserMedia } });
    stubProbeAudioContext();
    stubWorkletGlobals();
    const context = workletContextStub();

    const streamer = new MicStreamer({ onChunk: () => {}, onError: () => {} }, context, 20);
    const evidence = await streamer.start();

    expect(evidence).toEqual([]);
    expect(getUserMedia).toHaveBeenCalledTimes(1);
    const audio = getUserMedia.mock.calls[0]![0].audio as MediaTrackConstraints;
    expect(audio.deviceId).toBeUndefined();
    streamer.stop();
  });

  it("switchDevice rebinds to the requested device and stops the previous stream", async () => {
    const firstTrack: FakeTrack = { stop: vi.fn(), getSettings: () => ({ deviceId: "guid-mic" }) };
    const secondTrack: FakeTrack = { stop: vi.fn(), getSettings: () => ({ deviceId: "guid-headset" }) };
    const getUserMedia = vi.fn(async (constraints: MediaStreamConstraints) => {
      const audio = constraints.audio as MediaTrackConstraints;
      const deviceId = (audio.deviceId as { exact?: string } | undefined)?.exact ?? "";
      return makeStream(deviceId === "guid-headset" ? secondTrack : firstTrack);
    });
    vi.stubGlobal("navigator", {
      mediaDevices: {
        getUserMedia,
        enumerateDevices: vi.fn(async () => [
          { kind: "audioinput", deviceId: "guid-mic", label: "Microphone Array" },
        ]),
      },
    });
    stubProbeAudioContext();
    stubWorkletGlobals();
    const context = workletContextStub();

    const streamer = new MicStreamer({ onChunk: () => {}, onError: () => {} }, context, 20);
    await streamer.start();
    await streamer.switchDevice("guid-headset");

    const switchCall = getUserMedia.mock.calls.at(-1)![0] as MediaStreamConstraints;
    expect((switchCall.audio as MediaTrackConstraints).deviceId).toEqual({ exact: "guid-headset" });
    expect(firstTrack.stop).toHaveBeenCalled();
    streamer.stop();
    expect(secondTrack.stop).toHaveBeenCalled();
  });
});

describe("encodePcm16", () => {
  it("converts float samples to little-endian int16 and clamps overflow", () => {
    const bytes = encodePcm16(new Float32Array([0.5, -0.5, 2, -2]));
    const view = new DataView(bytes.buffer);
    expect(bytes.length).toBe(8);
    expect(view.getInt16(0, true)).toBe(Math.trunc(0.5 * 0x7fff));
    expect(view.getInt16(2, true)).toBe(-Math.trunc(0.5 * 0x8000));
    expect(view.getInt16(4, true)).toBe(0x7fff);
    expect(view.getInt16(6, true)).toBe(-0x8000);
  });
});

describe("MicChunkBatcher", () => {
  it("buffers small worklet frames until the chunk target is reached", () => {
    const batcher = new MicChunkBatcher(48_000);
    const frame = new Float32Array(128).fill(0.1);
    for (let index = 0; index < 37; index++) {
      expect(batcher.push(frame)).toBeNull();
    }
    const chunk = batcher.push(frame);
    expect(chunk).not.toBeNull();
    expect(chunk!.length).toBe(38 * 128);
  });

  it("keeps pushing after an emit and reports the 100ms target", () => {
    expect(CHUNK_TARGET_MS).toBe(100);
    const batcher = new MicChunkBatcher(48_000);
    const first = batcher.push(new Float32Array(4_800));
    expect(first!.length).toBe(4_800);
    expect(batcher.push(new Float32Array(128))).toBeNull();
    const second = batcher.push(new Float32Array(4_800));
    expect(second!.length).toBe(4_928);
  });
});
