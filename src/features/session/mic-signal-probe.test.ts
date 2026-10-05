import { describe, expect, it, vi } from "vitest";

import type { AudioInputInfo } from "./mic-device-selection";
import {
  MIC_SIGNAL_EPSILON,
  chooseBoundDevice,
  collectMicEvidence,
  isVirtualCaptureLabel,
  rankProbeCandidates,
  type MicDeviceEvidence,
} from "./mic-signal-probe";

const REAL_MIC: AudioInputInfo = { deviceId: "guid-mic", label: "Microphone Array (Intel Smart Sound)" };
const REAL_MIC_DEFAULT: AudioInputInfo = { deviceId: "default", label: "默认 - Microphone Array (Intel Smart Sound)" };
const CABLE: AudioInputInfo = { deviceId: "guid-cable", label: "CABLE Output (VB-Audio Virtual Cable)" };
const CABLE_DEFAULT: AudioInputInfo = { deviceId: "default", label: "默认 - CABLE Output (VB-Audio Virtual Cable)" };

describe("isVirtualCaptureLabel", () => {
  it("marks the virtual cable endpoints", () => {
    expect(isVirtualCaptureLabel(CABLE.label)).toBe(true);
    expect(isVirtualCaptureLabel(REAL_MIC.label)).toBe(false);
  });
});

describe("rankProbeCandidates", () => {
  it("probes the default entry first, then physical devices, then virtual ones", () => {
    const ranked = rankProbeCandidates([CABLE, REAL_MIC, CABLE_DEFAULT, { deviceId: "communications", label: "通信默认 - CABLE Output" }]);
    expect(ranked.map((device) => device.deviceId)).toEqual(["default", "guid-mic", "guid-cable"]);
  });

  it("keeps going with whatever exists when labels are hidden", () => {
    const devices: AudioInputInfo[] = [
      { deviceId: "default", label: "" },
      { deviceId: "guid-mic", label: "" },
    ];
    expect(rankProbeCandidates(devices).map((device) => device.deviceId)).toEqual(["default", "guid-mic"]);
  });
});

describe("chooseBoundDevice", () => {
  const evidence = (overrides: Partial<MicDeviceEvidence> & { deviceId: string }): MicDeviceEvidence => ({
    label: "",
    virtual: false,
    rmsPeak: MIC_SIGNAL_EPSILON * 10,
    ...overrides,
  });

  it("prefers the first physical device with signal", () => {
    expect(chooseBoundDevice([
      evidence({ deviceId: "a", rmsPeak: 0 }),
      evidence({ deviceId: "b", rmsPeak: 0.02 }),
      evidence({ deviceId: "c", rmsPeak: 0.05 }),
    ])).toBe("b");
  });

  it("falls back to the first silent physical device so the watchdog can report", () => {
    expect(chooseBoundDevice([
      evidence({ deviceId: "a", rmsPeak: 0 }),
      evidence({ deviceId: "b", rmsPeak: 0 }),
    ])).toBe("a");
  });

  it("never binds a virtual endpoint while a physical one exists", () => {
    expect(chooseBoundDevice([
      evidence({ deviceId: "cable", virtual: true, rmsPeak: 0.5 }),
      evidence({ deviceId: "mic", rmsPeak: 0 }),
    ])).toBe("mic");
  });

  it("returns null when only virtual devices exist", () => {
    expect(chooseBoundDevice([evidence({ deviceId: "cable", virtual: true, rmsPeak: 0 })])).toBeNull();
  });

  it("returns null for an empty evidence list", () => {
    expect(chooseBoundDevice([])).toBeNull();
  });
});

describe("collectMicEvidence", () => {
  it("probes ranked candidates, drops failed probes, and caps at maxCandidates", async () => {
    const fake = vi.fn(async (deviceId: string) => (deviceId === "broken" ? null : 0.03));
    const devices = [
      CABLE_DEFAULT,
      { deviceId: "broken", label: "Broken Mic" },
      REAL_MIC,
      { deviceId: "extra", label: "Extra Mic" },
    ];
    const probed: string[] = [];
    const evidence = await collectMicEvidence(
      devices,
      async (deviceId) => {
        probed.push(deviceId);
        return fake(deviceId);
      },
      { maxCandidates: 3 },
    );
    expect(probed).toEqual(["default", "broken", "guid-mic"]);
    expect(evidence).toEqual([
      { deviceId: "default", label: CABLE_DEFAULT.label, virtual: true, rmsPeak: 0.03 },
      { deviceId: "guid-mic", label: REAL_MIC.label, virtual: false, rmsPeak: 0.03 },
    ]);
  });
});
