import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import { WorkspacePage } from "./workspace-page";

vi.mock("../../api/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../api/commands")>()),
  startSession: vi.fn(),
  stopSession: vi.fn(),
  setSessionMode: vi.fn(),
  getRuntimeStatus: vi.fn(),
  getSession: vi.fn(),
  finalizeSessionUtterance: vi.fn(),
  sessionAgentCommand: vi.fn(),
}));

describe("WorkspacePage", () => {
  beforeEach(() => {
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: {
        phase: "idle",
        mode: "ai_active",
        seq: 0,
        unusedMaterials: false,
        lastErrorCode: null,
        revision: 0,
        realtimeStatus: "idle",
      },
    });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("renders the workspace heading", () => {
    render(<WorkspacePage />);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toContain("工作台");
  });

  it("renders the conversation with a manual text composer", () => {
    render(<WorkspacePage />);
    expect(screen.getByRole("region", { name: "会话对话" })).toBeTruthy();
    // 对齐官方体验：底部 Composer 输入卡提供文字轮次入口（会话未开始时禁用）。
    expect(screen.getByRole("textbox", { name: "输入内容" })).toBeTruthy();
    expect((screen.getByRole("button", { name: "发送" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("does not call fetch", () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    render(<WorkspacePage />);
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });

  it("does not contain /api/ paths", () => {
    const { container } = render(<WorkspacePage />);
    expect(container.innerHTML).not.toMatch(/\/api\//);
  });

  it("includes the workspace session and drops the leftover placeholder", async () => {
    render(<WorkspacePage />);
    expect(await screen.findByRole("heading", { name: "当前会话" })).toBeTruthy();
    expect(screen.queryByText(/尚未接入业务逻辑/)).toBeNull();
  });
});
