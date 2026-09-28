import { afterEach, describe, expect, it, vi } from "vitest";

import { CHUNK_TARGET_MS, MicChunkBatcher, MicStreamer, encodePcm16 } from "./mic-recorder";

describe("MicStreamer with a shared AudioContext", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("starts on the shared context and leaves it open on stop", async () => {
    const track = { stop: vi.fn() };
    const stream = { getTracks: () => [track] };
    vi.stubGlobal("navigator", {
      mediaDevices: { getUserMedia: vi.fn(async () => stream) },
    });
    vi.stubGlobal("URL", { createObjectURL: () => "blob:worklet", revokeObjectURL: () => {} });
    vi.stubGlobal("Blob", class {});
    vi.stubGlobal("AudioWorkletNode", class {
      port: { onmessage: ((event: MessageEvent) => void) | null } = { onmessage: null };
      connect() {}
      disconnect() {}
    });
    const node = { connect: vi.fn(), disconnect: vi.fn(), gain: { value: 1 } };
    const context = {
      state: "running",
      sampleRate: 48_000,
      destination: {},
      resume: vi.fn(async () => {}),
      close: vi.fn(async () => {}),
      audioWorklet: { addModule: vi.fn(async () => {}) },
      createMediaStreamSource: vi.fn(() => node),
      createGain: vi.fn(() => node),
    } as unknown as AudioContext;

    const streamer = new MicStreamer({ onChunk: () => {}, onError: () => {} }, context);
    await expect(streamer.start()).resolves.toBeUndefined();
    streamer.stop();

    expect(track.stop).toHaveBeenCalled();
    expect(context.close).not.toHaveBeenCalled();
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
