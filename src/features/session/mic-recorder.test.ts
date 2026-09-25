import { describe, expect, it } from "vitest";

import { CHUNK_TARGET_MS, MicChunkBatcher, encodePcm16 } from "./mic-recorder";

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
