import { describe, expect, it } from "vitest";

import {
  ACTIVE_PHASES,
  BAR_FACTORS,
  RATE_LIMIT_HINT,
  clockOf,
  errorText,
  formatDuration,
  humanizeRemoteError,
  roleScenario,
} from "./workspace-format";
import type { PublicConfig } from "../../generated/bindings";

function configWith(roles: Array<{ id: string; scenario?: PublicConfig["roleProfiles"][number]["scenario"] }>): PublicConfig {
  return {
    configVersion: 1,
    application: { locale: null },
    models: { providers: [], activeProviderId: null },
    speech: { activeVoiceRouteId: null, voiceRoutes: [] },
    knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null },
    storage: { exportDirectory: null },
    roleProfiles: roles.map((role) => ({
      id: role.id,
      name: role.id,
      systemPrompt: "",
      openingMessage: "",
      styleInstructions: "",
      active: true,
      configVersion: 1,
      scenario: role.scenario,
    })),
    activeRoleProfileId: roles[0]?.id ?? null,
    diagnostics: { logRetentionDays: 14 },
  };
}

describe("roleScenario", () => {
  it("returns the scenario declared on the matching role profile", () => {
    const config = configWith([{ id: "role-1", scenario: "interviewer" }, { id: "role-2" }]);
    expect(roleScenario(config, "role-1")).toBe("interviewer");
  });

  it("falls back to the preset mapping for preset role ids", () => {
    expect(roleScenario(configWith([]), "preset-meeting")).toBe("meetingAssistant");
    expect(roleScenario(configWith([]), "preset-presenter")).toBe("livestreamPresenter");
  });

  it("returns undefined for unknown roles and accepts a null config", () => {
    expect(roleScenario(configWith([{ id: "role-1" }]), "role-2")).toBeUndefined();
    expect(roleScenario(null, "preset-hr")).toBe("hr");
  });
});

describe("errorText", () => {
  it("maps known codes to actionable copy", () => {
    expect(errorText({ code: "SESSION_SIDECAR_MISSING", message: "raw" })).toBe(
      "缺少 AudioBridge 音频组件，请安装或修复音频组件后重试。",
    );
    expect(errorText({ code: "PLAYBACK_FAILED", message: "raw" })).toBe(
      "语音未播放成功，文字回答已保留。请检查所选音频设备。",
    );
  });

  it("falls back to code：message for unknown codes", () => {
    expect(errorText({ code: "SOMETHING_NEW", message: "原始信息" })).toBe("SOMETHING_NEW：原始信息");
  });

  it("prefixes the field when present", () => {
    expect(errorText({ code: "SOMETHING_NEW", message: "原始信息", field: "voiceRouteId" })).toBe(
      "voiceRouteId：SOMETHING_NEW：原始信息",
    );
  });
});

describe("humanizeRemoteError", () => {
  it("translates provider codes that prefix the message", () => {
    expect(humanizeRemoteError("downstream_reconnect_exceeded after 3 attempts")).toBe(
      "语音模型不可用：当前供应商账号可能未开通实时语音模型权限。请在服务页更换语音线路，或到供应商控制台开通后重试。",
    );
    expect(humanizeRemoteError("1113: no quota")).toBe(
      "供应商余额不足：账号没有可用的语音资源包，请充值或更换语音线路。",
    );
  });

  it("keeps unrelated messages unchanged", () => {
    expect(humanizeRemoteError("connection reset by peer")).toBe("connection reset by peer");
  });
});

describe("RATE_LIMIT_HINT", () => {
  it("is a non-empty actionable hint", () => {
    expect(RATE_LIMIT_HINT.length).toBeGreaterThan(0);
    expect(RATE_LIMIT_HINT).toContain("限流");
  });
});

describe("ACTIVE_PHASES", () => {
  it("treats listening and thinking as active", () => {
    expect(ACTIVE_PHASES.has("listening")).toBe(true);
    expect(ACTIVE_PHASES.has("thinking")).toBe(true);
  });

  it("does not treat idle or terminal phases as active", () => {
    expect(ACTIVE_PHASES.has("idle")).toBe(false);
    expect(ACTIVE_PHASES.has("completed")).toBe(false);
    expect(ACTIVE_PHASES.has("failed")).toBe(false);
  });
});

describe("BAR_FACTORS", () => {
  it("keeps four sensitivity factors with rising and falling shape", () => {
    expect([...BAR_FACTORS]).toEqual([0.45, 0.7, 0.6, 0.85]);
  });

  it("stays within the audible range", () => {
    for (const factor of BAR_FACTORS) {
      expect(factor).toBeGreaterThan(0);
      expect(factor).toBeLessThanOrEqual(1);
    }
  });
});

describe("formatDuration", () => {
  it("formats zero and sub-minute durations", () => {
    expect(formatDuration(0)).toBe("00:00");
    expect(formatDuration(9)).toBe("00:09");
  });

  it("formats minutes and wraps seconds below one hour", () => {
    expect(formatDuration(65)).toBe("01:05");
    expect(formatDuration(600)).toBe("10:00");
  });
});

describe("clockOf", () => {
  it("returns an empty string for missing or invalid timestamps", () => {
    expect(clockOf(undefined)).toBe("");
    expect(clockOf("not-a-date")).toBe("");
  });

  it("formats a valid timestamp as hour:minute", () => {
    expect(clockOf("2026-09-05T10:00:30Z")).toMatch(/^\d{1,2}:\d{2}$/);
  });
});
