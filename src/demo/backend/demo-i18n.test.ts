import { afterEach, describe, expect, it } from "vitest";

import { setLanguagePreference } from "../../i18n";
import { handleDemoInvoke } from "./index";
import { getState, resetDemoState } from "./state";

/** 类型收窄助手：demo 后端命令统一返回 CommandResult 形状。 */
async function invoke<T>(cmd: string, payload?: Record<string, unknown>): Promise<T> {
  const result = (await handleDemoInvoke(cmd, payload)) as { ok: boolean; data: T };
  if (!result.ok) throw new Error(`demo invoke failed: ${cmd}`);
  return result.data;
}

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

  it("generates English practice questions and error messages in English mode", async () => {
    setLanguagePreference("en");
    window.localStorage.clear();
    resetDemoState();
    const plan = await invoke<{
      position: string;
      questions: Array<{ prompt: string; focus: string }>;
    }>("practice_plan_generate", {
      input: { position: "Platform engineer", questionCount: 3, interviewerStyle: "Interviewer", difficulty: "Standard" },
    });
    expect(plan.position).toBe("Platform engineer");
    expect(plan.questions[0].focus).toBe("Project depth");
    expect(plan.questions[0].prompt).toContain("Platform engineer");

    const invalid = (await handleDemoInvoke("practice_plan_save", {
      input: { id: "", questions: [] },
    })) as { ok: boolean; error: { message: string } };
    expect(invalid.ok).toBe(false);
    expect(invalid.error.message).toBe("Invalid question plan parameters");
  });

  it("keeps Chinese seeds for the default (system) language", () => {
    window.localStorage.clear();
    resetDemoState();
    const state = getState();
    expect(state.roleProfiles.map((role) => role.name)).toContain("严苛面试官");
    expect(state.materials[0].fileName).toContain("云帆协同办公平台产品手册");
  });
});
