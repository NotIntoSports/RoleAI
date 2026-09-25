import { describe, expect, it } from "vitest";

import { RECORD_MAX_MS, downsampleTo16k, encodeWav16k } from "./wav-recorder";

function sineSamples(count: number, amplitude = 0.5): Float32Array {
  const samples = new Float32Array(count);
  for (let index = 0; index < count; index++) {
    samples[index] = amplitude * Math.sin((2 * Math.PI * 220 * index) / 48_000);
  }
  return samples;
}

describe("downsampleTo16k", () => {
  it("keeps 16 kHz input untouched", () => {
    const samples = new Float32Array([0.1, -0.2, 0.3]);
    expect(downsampleTo16k(samples, 16_000)).toBe(samples);
  });

  it("averages 48 kHz input into a third of the samples", () => {
    const samples = new Float32Array(48_000).fill(0.25);
    const output = downsampleTo16k(samples, 48_000);
    expect(output.length).toBe(16_000);
    expect(output[0]).toBeCloseTo(0.25, 5);
    expect(output[15_999]).toBeCloseTo(0.25, 5);
  });

  it("returns at least one sample for very short input", () => {
    const output = downsampleTo16k(new Float32Array([0.5]), 48_000);
    expect(output.length).toBe(1);
    expect(output[0]).toBeCloseTo(0.5, 5);
  });
});

describe("encodeWav16k", () => {
  it("writes a canonical 16 kHz mono 16-bit WAV header", () => {
    const samples = new Float32Array(16_000);
    const { bytes, durationMs } = encodeWav16k(samples, 16_000);
    const view = new DataView(bytes.buffer);
    expect(bytes.length).toBe(44 + 32_000);
    expect(durationMs).toBe(1000);
    expect(String.fromCharCode(...bytes.subarray(0, 4))).toBe("RIFF");
    expect(String.fromCharCode(...bytes.subarray(8, 12))).toBe("WAVE");
    expect(String.fromCharCode(...bytes.subarray(12, 16))).toBe("fmt ");
    expect(view.getUint16(20, true)).toBe(1);
    expect(view.getUint16(22, true)).toBe(1);
    expect(view.getUint32(24, true)).toBe(16_000);
    expect(view.getUint16(34, true)).toBe(16);
    expect(String.fromCharCode(...bytes.subarray(36, 40))).toBe("data");
    expect(view.getUint32(40, true)).toBe(32_000);
  });

  it("resamples 48 kHz input down and reports the resampled duration", () => {
    const samples = sineSamples(48_000);
    const { bytes, durationMs } = encodeWav16k(samples, 48_000);
    const view = new DataView(bytes.buffer);
    expect(durationMs).toBe(1000);
    expect(view.getUint32(40, true)).toBe(32_000);
    expect(view.getInt16(44, true)).not.toBe(0);
  });

  it("clamps out-of-range samples to int16 bounds", () => {
    const samples = new Float32Array([2, -2]);
    const { bytes } = encodeWav16k(samples, 16_000);
    const view = new DataView(bytes.buffer);
    expect(view.getInt16(44, true)).toBe(0x7fff);
    expect(view.getInt16(46, true)).toBe(-0x8000);
  });
});

describe("record limits", () => {
  it("caps recordings at 30 seconds", () => {
    expect(RECORD_MAX_MS).toBe(30_000);
  });
});
