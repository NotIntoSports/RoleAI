import { afterEach, describe, expect, it } from "vitest";

import { setLanguagePreference } from "../../i18n";
import { getState, resetDemoState } from "./state";

describe("demo backend english datasets", () => {
  afterEach(() => {
    setLanguagePreference("system");
    window.localStorage.clear();
    resetDemoState();
  });

  it("seeds English roles, materials, sessions and resource names when the interface is English", () => {
    setLanguagePreference("en");
    window.localStorage.clear();
    resetDemoState();
    const state = getState();
    expect(state.roleProfiles.map((role) => role.name)).toContain("Strict interviewer");
    expect(state.roleProfiles).toHaveLength(4);
    expect(state.materials[0].fileName).toBe("Yunfan Collaboration Suite Product Handbook v2.3.pdf");
    expect(state.sessions).toHaveLength(4);
    expect(state.providers[0].name).toBe("Demo Cloud (fictional)");
    expect(state.voiceRoutes[0].status).toBe("Configured (demo)");
    expect(state.voiceReferences[0].name).toContain("Demo voice");
  });

  it("keeps Chinese seeds for the default (system) language", () => {
    window.localStorage.clear();
    resetDemoState();
    const state = getState();
    expect(state.roleProfiles.map((role) => role.name)).toContain("严苛面试官");
    expect(state.materials[0].fileName).toContain("云帆协同办公平台产品手册");
  });
});
