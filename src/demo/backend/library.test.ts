import { afterEach, describe, expect, it, vi } from "vitest";

import type { CommandResult, MaterialSearchHit, SessionDetail, SessionSummary } from "../../generated/bindings";

import { handleDemoInvoke } from "./index";
import { resetDemoState } from "./state";

async function invoke<T>(cmd: string, payload?: Record<string, unknown>): Promise<CommandResult<T>> {
  return (await handleDemoInvoke(cmd, payload)) as CommandResult<T>;
}

function expectOk<T>(result: CommandResult<T>): T {
  if (!result.ok) throw new Error(`demo invoke failed: ${result.error.code} ${result.error.message}`);
  return result.data;
}

describe("demo backend: materials library", () => {
  afterEach(() => {
    window.localStorage.clear();
    resetDemoState();
  });

  it("seeds 3 fictional materials", async () => {
    const materials = expectOk(await invoke<Array<{ id: string; status: string }>>("material_list"));
    expect(materials.map((m) => m.id)).toEqual(["mat-demo-handbook", "mat-demo-article", "mat-demo-jd"]);
    expect(materials.every((m) => m.status === "text_ready")).toBe(true);
  });

  it("ranks keyword search hits across materials with snippets", async () => {
    const hits = expectOk(
      await invoke<MaterialSearchHit[]>("material_search", { query: "缓存击穿" }),
    );
    expect(hits.length).toBeGreaterThan(0);
    expect(hits[0].materialId).toBe("mat-demo-article");
    expect(hits[0].snippet).toContain("缓存击穿");
    expect(hits[0].rank).toBeGreaterThan(0);
  });

  it("imports a path as a placeholder material and deletes it", async () => {
    const imported = expectOk(
      await invoke<{ id: string; fileName: string; status: string }>("material_import", {
        path: "D:\\资料\\我的简历.pdf",
      }),
    );
    expect(imported.fileName).toBe("我的简历.pdf");
    expect(imported.status).toBe("text_ready");
    const afterDelete = expectOk(await invoke<{ ready: boolean }>("material_delete", { id: imported.id }));
    expect(afterDelete.ready).toBe(true);
    const list = expectOk(await invoke<Array<{ id: string }>>("material_list"));
    expect(list.some((m) => m.id === imported.id)).toBe(false);
  });

  it("indexes all seeded chunks", async () => {
    const result = expectOk(await invoke<{ indexedChunks: number; status: string }>("material_index"));
    expect(result.status).toBe("ok");
    expect(result.indexedChunks).toBeGreaterThanOrEqual(66);
  });
});

describe("demo backend: session records", () => {
  afterEach(() => {
    window.localStorage.clear();
    resetDemoState();
  });

  it("seeds 4 professional records (2 interviews, 1 meeting, 1 livestream) with 8-9 turns", async () => {
    const sessions = expectOk(await invoke<SessionSummary[]>("session_list"));
    expect(sessions).toHaveLength(4);
    const roles = sessions.map((s) => s.roleProfileId).sort();
    expect(roles).toEqual([
      "preset-meeting",
      "preset-presenter",
      "preset-strict-interviewer",
      "preset-strict-interviewer",
    ]);
    for (const session of sessions) {
      const detail = expectOk(await invoke<SessionDetail>("session_get", { sessionId: session.id }));
      expect(detail.turns.length).toBeGreaterThanOrEqual(8);
      expect(detail.turns.length).toBeLessThanOrEqual(20);
      expect(detail.turns.every((turn) => turn.userText.length > 0 && turn.assistantText.length > 0)).toBe(true);
    }
  });

  it("exports a session as markdown content", async () => {
    const exported = expectOk(
      await invoke<{ path: string }>("session_export", {
        sessionId: "session-demo-interview-backend",
        format: "markdown",
      }),
    );
    expect(exported.path).toContain("session-demo-interview-backend");
    expect(exported.path).toContain(".md");
  });

  it("deletes a record", async () => {
    expectOk(await invoke("session_delete", { sessionId: "session-demo-livestream" }));
    const sessions = expectOk(await invoke<SessionSummary[]>("session_list"));
    expect(sessions).toHaveLength(3);
    const missing = await invoke<SessionDetail>("session_get", { sessionId: "session-demo-livestream" });
    expect(missing.ok).toBe(false);
  });
});
