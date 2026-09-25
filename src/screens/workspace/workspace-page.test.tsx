import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import { WorkspacePage } from "./workspace-page";

vi.mock("../../api/commands", () => ({
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

  it("renders the conversation without a manual text composer", () => {
    render(<WorkspacePage />);
    expect(screen.getByRole("region", { name: "当前轮对话" })).toBeTruthy();
    expect(screen.queryByRole("textbox", { name: "语句输入" })).toBeNull();
    expect(screen.queryByRole("button", { name: "发送" })).toBeNull();
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
