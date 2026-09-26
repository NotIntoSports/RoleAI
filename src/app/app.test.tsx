import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock the commands module before importing App
vi.mock("../api/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../api/commands")>()),
  getStartupState: vi.fn(),
  openAppDirectory: vi.fn(),
  restoreDefaultConfig: vi.fn(),
  restoreLastGoodConfig: vi.fn(),
}));

import { getStartupState } from "../api/commands";
import { App } from "./app";

const mockGetStartupState = getStartupState as ReturnType<typeof vi.fn>;

describe("App", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    window.location.hash = "";
  });
  afterEach(cleanup);

  it("renders Shell with workspace page when startup is ready", async () => {
    mockGetStartupState.mockResolvedValue({ ok: true, data: { kind: "ready" } });
    render(<App />);
    await waitFor(() => {
      expect(screen.getByRole("navigation", { name: "主导航" })).toBeTruthy();
    });
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("工作台");
  });

  it("renders Shell when startup is migrated", async () => {
    mockGetStartupState.mockResolvedValue({ ok: true, data: { kind: "migrated" } });
    render(<App />);
    await waitFor(() => {
      expect(screen.getByRole("navigation", { name: "主导航" })).toBeTruthy();
    });
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("工作台");
  });

  it("renders ConfigRepair when startup is recoverable", async () => {
    mockGetStartupState.mockResolvedValue({
      ok: true,
      data: { kind: "recoverable", error: { code: "CONFIG_RECOVERABLE", message: "配置可恢复", requestId: "local", retryable: true } },
    });
    render(<App />);
    await waitFor(() => {
      expect(screen.queryByRole("navigation", { name: "主导航" })).toBeNull();
    });
    // ConfigRepair should be visible (it has restore buttons)
    expect(screen.getByRole("button", { name: "恢复上次可用配置" })).toBeTruthy();
  });

  it("renders ConfigRepair when startup is invalid", async () => {
    mockGetStartupState.mockResolvedValue({
      ok: true,
      data: { kind: "invalid", error: { code: "CONFIG_INVALID", message: "配置无效", requestId: "local", retryable: false } },
    });
    render(<App />);
    await waitFor(() => {
      expect(screen.queryByRole("navigation", { name: "主导航" })).toBeNull();
    });
  });

  it("renders ConfigRepair when getStartupState rejects", async () => {
    mockGetStartupState.mockRejectedValue(new Error("network failure"));
    render(<App />);
    await waitFor(() => {
      expect(screen.queryByRole("navigation", { name: "主导航" })).toBeNull();
    });
  });

  it("does not render Tauri Foundation text when ready", async () => {
    mockGetStartupState.mockResolvedValue({ ok: true, data: { kind: "ready" } });
    render(<App />);
    await waitFor(() => {
      expect(screen.getByRole("navigation", { name: "主导航" })).toBeTruthy();
    });
    expect(screen.queryByText("Tauri Foundation")).toBeNull();
  });
});
