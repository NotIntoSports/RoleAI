import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "../../app/app";
import * as commands from "../../api/commands";

vi.mock("../../api/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../api/commands")>()),
  getStartupState: vi.fn(),
  restoreLastGoodConfig: vi.fn(),
  restoreDefaultConfig: vi.fn(),
}));

const getStartupState = vi.mocked(commands.getStartupState);

describe("configuration repair mode", () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(cleanup);

  it("renders the shell with the workspace page when startup is ready", async () => {
    getStartupState.mockResolvedValue({ ok: true, data: { kind: "ready" } });
    render(<App />);
    expect(await screen.findByRole("navigation", { name: "主导航" })).toBeTruthy();
    expect(await screen.findByRole("heading", { name: "工作台" })).toBeTruthy();
  });

  it("shows stable field errors and restores last-good configuration", async () => {
    getStartupState.mockResolvedValue({ ok: true, data: { kind: "recoverable", error: { code: "CONFIG_INVALID", message: "配置格式无效", requestId: "request", field: "models", retryable: false } } });
    vi.mocked(commands.restoreLastGoodConfig).mockResolvedValue({ ok: true, data: { kind: "ready" } });
    render(<App />);
    expect(await screen.findByText("CONFIG_INVALID")).toBeTruthy();
    expect(screen.getByText("models")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "恢复上次可用配置" }));
    await waitFor(() => expect(commands.restoreLastGoodConfig).toHaveBeenCalledOnce());
  });

  it("offers safe recovery actions for an invalid configuration", async () => {
    getStartupState.mockResolvedValue({ ok: true, data: { kind: "invalid", error: { code: "CONFIG_READ_FAILED", message: "无法读取配置文件", requestId: "request", retryable: false } } });
    render(<App />);
    await screen.findByText("CONFIG_READ_FAILED");
    expect(screen.getByRole("button", { name: "恢复默认配置" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "导入配置" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "打开配置文件" })).toBeTruthy();
  });
});
