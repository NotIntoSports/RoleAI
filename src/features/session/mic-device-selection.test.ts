import { afterEach, describe, expect, it, vi } from "vitest";

import { enumerateAudioInputs, VIRTUAL_CAPTURE_PATTERN } from "./mic-device-selection";

describe("VIRTUAL_CAPTURE_PATTERN", () => {
  it("matches the virtual cable capture endpoint in either language", () => {
    expect(VIRTUAL_CAPTURE_PATTERN.test("CABLE Output (VB-Audio Virtual Cable)")).toBe(true);
    expect(VIRTUAL_CAPTURE_PATTERN.test("默认 - CABLE Output (VB-Audio Virtual Cable)")).toBe(true);
    expect(VIRTUAL_CAPTURE_PATTERN.test("Microphone Array (Intel Smart Sound)")).toBe(false);
    expect(VIRTUAL_CAPTURE_PATTERN.test("")).toBe(false);
  });
});

describe("enumerateAudioInputs", () => {
  const stubNavigator = (mediaDevices: unknown) => {
    vi.stubGlobal("navigator", { mediaDevices });
  };

  afterEach(() => vi.unstubAllGlobals());

  it("maps audioinput entries and ignores other kinds", async () => {
    stubNavigator({
      enumerateDevices: vi.fn(async () => [
        { kind: "audioinput", deviceId: "a", label: "Mic" },
        { kind: "audiooutput", deviceId: "b", label: "Speaker" },
        { kind: "videoinput", deviceId: "c", label: "Camera" },
      ]),
    });
    expect(await enumerateAudioInputs()).toEqual([{ deviceId: "a", label: "Mic" }]);
  });

  it("returns an empty list when mediaDevices is unavailable or enumeration fails", async () => {
    stubNavigator(undefined);
    expect(await enumerateAudioInputs()).toEqual([]);
    stubNavigator({ enumerateDevices: vi.fn(async () => { throw new Error("denied"); }) });
    expect(await enumerateAudioInputs()).toEqual([]);
  });
});
