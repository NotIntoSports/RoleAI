import { afterEach, describe, expect, it, vi } from "vitest";

import type { CommandResult, PublicConfig } from "../../generated/bindings";

import { handleDemoInvoke } from "./index";
import { resetDemoState } from "./state";

/** 领域处理器可能同步返回结果，await 统一；类型按调用点断言。 */
async function invoke<T>(cmd: string, payload?: Record<string, unknown>): Promise<CommandResult<T>> {
  return (await handleDemoInvoke(cmd, payload)) as CommandResult<T>;
}

function expectOk<T>(result: CommandResult<T>): T {
  if (!result.ok) throw new Error(`demo invoke failed: ${result.error.code} ${result.error.message}`);
  return result.data;
}

function expectErr(result: CommandResult<unknown>): { code: string; message: string } {
  if (result.ok) throw new Error("expected the demo invoke to fail");
  return result.error;
}

async function freshState() {
  vi.resetModules();
  const mod = await import("./state");
  return mod.getState();
}

describe("demo backend: config/roles/providers/voice", () => {
  afterEach(() => {
    window.localStorage.clear();
    resetDemoState();
  });

  it("seeds 4 preset-style roles and a ready active route, never leaking key material", async () => {
    const config = expectOk(await invoke<PublicConfig>("config_get_public"));
    expect(config.roleProfiles.map((role) => role.id)).toEqual([
      "preset-strict-interviewer",
      "preset-expression-coach",
      "preset-meeting",
      "preset-presenter",
    ]);
    expect(config.activeRoleProfileId).toBe("preset-strict-interviewer");
    const route = config.speech.voiceRoutes[0];
    expect(route.ready).toBe(true);
    expect(config.speech.activeVoiceRouteId).toBe(route.id);
    // 脱敏：任何配置视图都不允许出现 apiKey / 明文密钥。
    expect(JSON.stringify(config)).not.toMatch(/apiKey/);
    expect(config.models.providers[0].credential?.configured).toBe(true);
  });

  it("persists role edits across a simulated refresh (module reload reads localStorage)", async () => {
    const saved = expectOk(
      await invoke<{ configVersion: number; id: string }>("role_profile_save", {
        input: {
          id: "preset-strict-interviewer",
          name: "严苛面试官（改）",
          systemPrompt: "新的系统提示词。",
          openingMessage: "开场。",
          styleInstructions: "风格。",
        },
      }),
    );
    expect(saved.configVersion).toBe(2);

    // 模拟浏览器刷新：内存模块重置，状态从 localStorage 重新加载。
    const reloaded = await freshState();
    const edited = reloaded.roleProfiles.find((role) => role.id === "preset-strict-interviewer");
    expect(edited?.name).toBe("严苛面试官（改）");
    expect(edited?.configVersion).toBe(2);
  });

  it("activates a role and keeps the selection after reload", async () => {
    const activated = expectOk(
      await invoke<{ id: string; active: boolean }>("role_profile_activate", { roleId: "preset-meeting" }),
    );
    expect(activated.active).toBe(true);
    const reloaded = await freshState();
    expect(reloaded.activeRoleProfileId).toBe("preset-meeting");
  });

  it("tests provider connectivity with success and redacted credential", async () => {
    const test = expectOk(
      await invoke<{ reachable: boolean; modelCount: number }>("model_provider_test", {
        providerId: "provider-demo-cloud",
      }),
    );
    expect(test.reachable).toBe(true);
    expect(test.modelCount).toBeGreaterThan(0);
    const deps = expectOk(
      await invoke<Array<{ kind: string }>>("model_provider_dependencies", { providerId: "provider-demo-cloud" }),
    );
    expect(deps.map((d) => d.kind).sort()).toEqual(["embedding", "voiceRoute"]);
  });

  it("refuses to delete a provider still referenced by route/embedding", async () => {
    const error = expectErr(
      await invoke<unknown>("model_provider_delete", { providerId: "provider-demo-cloud" }),
    );
    expect(error.code).toBe("PROVIDER_IN_USE");
  });

  it("keeps bigint fields intact through localStorage persistence", async () => {
    const list = expectOk(
      await invoke<Array<{ byteSize: bigint; durationMs: bigint }>>("voice_reference_list"),
    );
    expect(list[0].byteSize).toBe(286722n);
    const reloaded = await freshState();
    expect(reloaded.voiceReferences[0].byteSize).toBe(286722n);
    expect(reloaded.voiceReferences[0].durationMs).toBe(9120n);
  });

  it("restore defaults reseeds the demo state", async () => {
    await invoke("role_profile_save", {
      input: { id: null, name: "临时角色", systemPrompt: "x", openingMessage: "", styleInstructions: "" },
    });
    expectOk(await invoke("config_restore_defaults"));
    const reloaded = await freshState();
    expect(reloaded.roleProfiles.some((role) => role.name === "临时角色")).toBe(false);
    expect(reloaded.roleProfiles).toHaveLength(4);
  });
});
