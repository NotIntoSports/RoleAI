import { afterEach, describe, expect, it, vi } from "vitest";

import type { CommandResult, PublicConfig } from "../../generated/bindings";

import { handleDemoInvoke } from "./index";
import { getState, resetDemoState, updateState } from "./state";

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

  it("seeds 8 preset-style roles and a ready active route, never leaking key material", async () => {
    const config = expectOk(await invoke<PublicConfig>("config_get_public"));
    expect(config.roleProfiles.map((role) => role.id)).toEqual([
      "preset-meeting",
      "preset-strict-interviewer",
      "preset-expression-coach",
      "preset-presenter",
      "preset-debate-partner",
      "preset-english-partner",
      "preset-cs-drill",
      "preset-mock-candidate",
    ]);
    expect(config.activeRoleProfileId).toBe("preset-meeting");
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

  it("presents Tavus as not configured, like a desktop install without a saved key", async () => {
    const status = expectOk(await invoke<{ reference: string; configured: boolean }>("tavus_credential_status"));
    expect(status).toEqual({ reference: "tavus/api-key", configured: false });
    expect(expectErr(await invoke<unknown>("tavus_list_faces")).code).toBe("TAVUS_KEY_MISSING");
  });

  it("runs the video call as a local preview without asking for a Tavus key", async () => {
    const started = expectOk(
      await invoke<{ conversationId: string; conversationUrl: string }>("tavus_conversation_start", {
        roleProfileId: "preset-meeting",
      }),
    );
    expect(started).toEqual({ conversationId: "demo-preview", conversationUrl: "preview:roleai" });
    // 预览挂断：结束接口照常返回 ok（包括 demo-preview 这个预览 id）。
    const ended = expectOk(await invoke<{ ready: boolean }>("tavus_conversation_end", {
      conversationId: started.conversationId,
    }));
    expect(ended.ready).toBe(true);
  });

  it("rejects saving a personal role with a name another role already uses", async () => {
    const error = expectErr(
      await invoke("role_profile_save", {
        input: { id: null, name: "会议助手", systemPrompt: "x", openingMessage: "", styleInstructions: "" },
      }),
    );
    expect(error.code).toBe("ROLE_PROFILE_FIELDS_INVALID");
  });

  it("hides a preset template once a personal role carries the same name", async () => {
    updateState((s) => {
      s.roleProfiles = [
        ...s.roleProfiles,
        {
          id: "role-personal-meeting",
          name: "会议助手",
          systemPrompt: "个人提示",
          openingMessage: "",
          styleInstructions: "",
          scenario: undefined,
          active: false,
          configVersion: 1,
        },
      ];
    });
    const config = expectOk(await invoke<PublicConfig>("config_get_public"));
    expect(
      config.roleProfiles.filter((role) => role.name.trim() === "会议助手").map((role) => role.id),
    ).toEqual(["role-personal-meeting"]);
    // 模板仍留在状态里，只是不进公共配置视图。
    expect(getState().roleProfiles.some((role) => role.id === "preset-meeting")).toBe(true);
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
    expect(reloaded.roleProfiles).toHaveLength(8);
  });
});
