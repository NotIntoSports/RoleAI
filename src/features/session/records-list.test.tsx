import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { PublicConfig, SessionDetail, SessionSummary, SessionTurnView } from "../../generated/bindings";
import { RecordsList } from "./records-list";

vi.mock("../../api/commands", () => ({
  listSessions: vi.fn(),
  getSession: vi.fn(),
  exportSession: vi.fn(),
  exportSessionLatency: vi.fn(),
  deleteSession: vi.fn(),
  getConfigPublic: vi.fn(),
}));

function summary(overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    id: "sess-1",
    status: "completed",
    roleProfileId: "role-1",
    voiceRouteId: "route-1",
    transportMode: "direct",
    startedAt: "2026-09-05T10:00:00Z",
    finishedAt: "2026-09-05T10:05:00Z",
    updatedAt: "2026-09-05T10:05:00Z",
    ...overrides,
  };
}

function turn(overrides: Partial<SessionTurnView> = {}): SessionTurnView {
  return {
    id: "turn-1",
    turnIndex: 0,
    userText: "请介绍岗位",
    assistantText: "这是一个后端岗位",
    materialsUsed: false,
    citations: [{ materialId: "mat-1", chunkId: "chunk-1", snippet: "负责订单服务" }],
    createdAt: "2026-09-05T10:00:30Z",
    ...overrides,
  };
}

function detail(overrides: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: summary(),
    turns: [turn()],
    ...overrides,
  };
}

function emptyConfig(overrides: Partial<PublicConfig> = {}): PublicConfig {
  return {
    configVersion: 1,
    application: { locale: null },
    models: { providers: [], activeProviderId: null },
    speech: { voiceRoutes: [], activeVoiceRouteId: null },
    knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null },
    storage: { exportDirectory: null },
    roleProfiles: [{
      id: "role-1",
      name: "面试官",
      systemPrompt: "",
      openingMessage: "",
      styleInstructions: "",
      active: true,
      configVersion: 1,
    }],
    activeRoleProfileId: "role-1",
    diagnostics: { logRetentionDays: 14 },
    ...overrides,
  };
}

describe("RecordsList", () => {
  beforeEach(() => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [] });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: emptyConfig() });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("shows loading then empty state", async () => {
    render(<RecordsList />);
    expect(screen.getByRole("status").textContent).toContain("正在读取会话记录");
    expect(screen.queryByText("还没有记录。")).toBeNull();
    expect(await screen.findByText("还没有记录。")).toBeTruthy();
  });

  it("surfaces a load error", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue({
      ok: false,
      error: {
        code: "DATABASE_OPERATION_FAILED",
        message: "数据库不可用",
        requestId: "r1",
        field: "database",
        retryable: true,
      },
    });
    render(<RecordsList />);
    expect((await screen.findByRole("status")).textContent).toContain(
      "database：DATABASE_OPERATION_FAILED：数据库不可用",
    );
  });

  it("lists session id and status then opens turn detail", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary()] });
    vi.mocked(commands.getSession).mockResolvedValue({ ok: true, data: detail() });

    const { container } = render(<RecordsList />);
    expect(await screen.findByText("sess-1")).toBeTruthy();
    expect(container.textContent).toContain("已完成");
    expect(screen.getByText("角色 面试官")).toBeTruthy();
    expect(container.querySelector("time")?.getAttribute("datetime")).toBe("2026-09-05T10:00:00Z");
    fireEvent.click(screen.getByRole("button", { name: "查看" }));
    await waitFor(() => expect(commands.getSession).toHaveBeenCalledWith("sess-1"));
    expect(document.body.textContent).toContain("请介绍岗位");
    expect(document.body.textContent).toContain("这是一个后端岗位");
    expect(document.body.textContent).toContain("负责订单服务");
    expect(document.body.textContent).toContain("本轮未使用资料");
    expect(within(screen.getByRole("region", { name: "用户内容" })).getByText("请介绍岗位")).toBeTruthy();
    expect(within(screen.getByRole("region", { name: "AI 回复" })).getByText("这是一个后端岗位")).toBeTruthy();
    expect(within(screen.getByRole("region", { name: "资料引用" })).getByText("负责订单服务")).toBeTruthy();
    expect(container.innerHTML).not.toMatch(/password|apiKey|apiSecret|credential/i);
  });

  it("exports markdown json and text", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary()] });
    vi.mocked(commands.getSession).mockResolvedValue({ ok: true, data: detail() });
    vi.mocked(commands.exportSession).mockResolvedValue({
      ok: true,
      data: { path: "C:/data/exports/sess-1.md" },
    });

    render(<RecordsList />);
    fireEvent.click(await screen.findByRole("button", { name: "查看" }));
    await screen.findByText("请介绍岗位");
    fireEvent.click(screen.getByRole("button", { name: "导出 Markdown" }));
    await waitFor(() => expect(commands.exportSession).toHaveBeenCalledWith("sess-1", "markdown"));
    expect(screen.getByRole("status").textContent).toContain("C:/data/exports/sess-1.md");

    vi.mocked(commands.exportSession).mockResolvedValue({
      ok: true,
      data: { path: "C:/data/exports/sess-1.json" },
    });
    fireEvent.click(screen.getByRole("button", { name: "导出 JSON" }));
    await waitFor(() => expect(commands.exportSession).toHaveBeenCalledWith("sess-1", "json"));

    vi.mocked(commands.exportSession).mockResolvedValue({
      ok: true,
      data: { path: "C:/data/exports/sess-1.txt" },
    });
    fireEvent.click(screen.getByRole("button", { name: "导出文本" }));
    await waitFor(() => expect(commands.exportSession).toHaveBeenCalledWith("sess-1", "text"));
  });

  it("two-step deletes with cancel and never prompts", async () => {
    vi.mocked(commands.listSessions)
      .mockResolvedValueOnce({ ok: true, data: [summary()] })
      .mockResolvedValue({ ok: true, data: [] });
    vi.mocked(commands.deleteSession).mockResolvedValue({ ok: true, data: { ready: true } });
    const promptSpy = vi.spyOn(window, "prompt");
    const confirmSpy = vi.spyOn(window, "confirm");

    render(<RecordsList />);
    expect(await screen.findByText("sess-1")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "删除" }));
    expect(commands.deleteSession).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(commands.deleteSession).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "删除" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(commands.deleteSession).toHaveBeenCalledWith("sess-1"));
    expect(promptSpy).not.toHaveBeenCalled();
    expect(confirmSpy).not.toHaveBeenCalled();
    expect(await screen.findByText("还没有记录。")).toBeTruthy();
    promptSpy.mockRestore();
    confirmSpy.mockRestore();
  });

  it("keeps the list visible when deletion fails", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary()] });
    vi.mocked(commands.deleteSession).mockResolvedValue({
      ok: false,
      error: {
        code: "DATABASE_OPERATION_FAILED",
        message: "记录仍被引用，删除失败",
        requestId: "r3",
        retryable: true,
      },
    });

    render(<RecordsList />);
    expect(await screen.findByText("sess-1")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "删除" }));
    fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(commands.deleteSession).toHaveBeenCalledWith("sess-1"));
    expect((await screen.findByRole("status")).textContent).toContain(
      "DATABASE_OPERATION_FAILED：记录仍被引用，删除失败",
    );
    expect(screen.getByText("sess-1")).toBeTruthy();
    expect(screen.getByRole("button", { name: "查看" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "删除" })).toBeTruthy();
  });

  it("shows export field errors and IPC failures", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary()] });
    vi.mocked(commands.getSession).mockResolvedValue({ ok: true, data: detail() });
    vi.mocked(commands.exportSession).mockResolvedValue({
      ok: false,
      error: {
        code: "SESSION_EXPORT_FORMAT_INVALID",
        message: "格式无效",
        requestId: "r2",
        field: "format",
        retryable: false,
      },
    });

    render(<RecordsList />);
    fireEvent.click(await screen.findByRole("button", { name: "查看" }));
    await screen.findByText("请介绍岗位");
    fireEvent.click(screen.getByRole("button", { name: "导出 Markdown" }));
    expect((await screen.findByRole("status")).textContent).toContain(
      "format：SESSION_EXPORT_FORMAT_INVALID：格式无效",
    );

    vi.mocked(commands.exportSession).mockRejectedValue(new Error("ipc"));
    fireEvent.click(screen.getByRole("button", { name: "导出 JSON" }));
    expect((await screen.findByRole("status")).textContent).toContain("IPC_UNAVAILABLE：");
  });

  it("does not use fetch, /api/, or low-level Tauri imports", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    const { container } = render(<RecordsList />);
    await screen.findByText("还没有记录。");
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(container.innerHTML).not.toMatch(/\/api\//);
    expect(container.innerHTML).not.toContain("@tauri-apps/api");
    fetchSpy.mockRestore();
  });

  it("returns from an empty conversation to its existing list", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary()] });
    vi.mocked(commands.getSession).mockResolvedValue({ ok: true, data: detail({ turns: [] }) });
    render(<RecordsList />);
    fireEvent.click(await screen.findByRole("button", { name: "查看" }));
    expect(await screen.findByText("本次会话还没有对话内容。")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "返回列表" }));
    expect(screen.queryByRole("region", { name: "会话详情" })).toBeNull();
    expect(screen.getByRole("button", { name: "查看" })).toBeTruthy();
    expect(commands.listSessions).toHaveBeenCalledTimes(1);
  });

  it("uses the actual update time and neutral text for missing metadata", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary({ startedAt: null, roleProfileId: "", status: "interrupted" })] });
    const { container } = render(<RecordsList />);
    expect(await screen.findByText("已中断")).toBeTruthy();
    expect(screen.getByText("角色 未指定")).toBeTruthy();
    expect(container.querySelector("time")?.getAttribute("datetime")).toBe("2026-09-05T10:05:00Z");
  });

  it("shows 已删除角色 when the saved role no longer exists", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: emptyConfig({ roleProfiles: [], activeRoleProfileId: null }) });
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary()] });
    render(<RecordsList />);
    expect(await screen.findByText("角色 已删除角色")).toBeTruthy();
  });

  it("retains the list and re-enables its actions when opening a record fails", async () => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary()] });
    vi.mocked(commands.getSession).mockRejectedValue(new Error("ipc"));
    render(<RecordsList />);
    fireEvent.click(await screen.findByRole("button", { name: "查看" }));
    expect(await screen.findByText("IPC_UNAVAILABLE：本地操作失败")).toBeTruthy();
    expect((screen.getByRole("button", { name: "查看" }) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByRole("region", { name: "会话详情" })).toBeNull();
  });
});

describe("RecordsList 延迟导出", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.mocked(commands.listSessions).mockResolvedValue({ ok: true, data: [summary()] });
    vi.mocked(commands.getSession).mockResolvedValue({ ok: true, data: detail() });
  });

  it("导出延迟数据与时间线走 session_latency_export", async () => {
    vi.mocked(commands.exportSessionLatency).mockResolvedValue({
      ok: true,
      data: { path: "C:/data/exports/sess-1-latency.latency.csv" },
    });
    render(<RecordsList />);
    fireEvent.click(await screen.findByRole("button", { name: "查看" }));
    await screen.findByText("请介绍岗位");
    fireEvent.click(screen.getByRole("button", { name: "导出延迟数据 CSV" }));
    await waitFor(() =>
      expect(commands.exportSessionLatency).toHaveBeenCalledWith("sess-1", "csv"),
    );
    expect(screen.getByRole("status").textContent).toContain("sess-1-latency.latency.csv");

    vi.mocked(commands.exportSessionLatency).mockResolvedValue({
      ok: true,
      data: { path: "C:/data/exports/sess-1-latency.latency.trace.json" },
    });
    fireEvent.click(screen.getByRole("button", { name: "导出时间线 Chrome Trace JSON" }));
    await waitFor(() =>
      expect(commands.exportSessionLatency).toHaveBeenCalledWith("sess-1", "trace"),
    );
    expect(screen.getByRole("status").textContent).toContain("latency.trace.json");
  });

  it("导出失败显示错误码信息", async () => {
    vi.mocked(commands.exportSessionLatency).mockResolvedValue({
      ok: false,
      error: { code: "SESSION_EXPORT_WRITE_FAILED", message: "Session export failed", requestId: "r2", retryable: false },
    });
    render(<RecordsList />);
    fireEvent.click(await screen.findByRole("button", { name: "查看" }));
    await screen.findByText("请介绍岗位");
    fireEvent.click(screen.getByRole("button", { name: "导出延迟数据 CSV" }));
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("SESSION_EXPORT_WRITE_FAILED"),
    );
  });
});
