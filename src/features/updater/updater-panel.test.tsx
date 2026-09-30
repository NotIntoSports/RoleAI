import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { UpdaterPanel, type InstallableUpdate, type UpdateCheckResult } from "./updater-panel";

function fakeUpdate(version: string): InstallableUpdate {
  return {
    version,
    body: "修复与改进",
    downloadAndInstall: vi.fn().mockResolvedValue(undefined),
  };
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("updater panel", () => {
  it("reports unconfigured update source when the plugin check fails (no signing pubkey)", async () => {
    const checkForUpdate = vi.fn().mockRejectedValue(new Error("pubkey not configured"));
    render(<UpdaterPanel checkForUpdate={checkForUpdate} />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    const notice = await screen.findByRole("alert");
    expect(notice.textContent).toContain("更新源未配置或不可达");
    expect(notice.textContent).toContain("pubkey not configured");
  });

  it("offers install when a new version is available", async () => {
    const update = fakeUpdate("0.2.0");
    const checkForUpdate = vi.fn().mockResolvedValue({ status: "available", update } satisfies UpdateCheckResult);
    render(<UpdaterPanel checkForUpdate={checkForUpdate} />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    const status = await screen.findByRole("status");
    expect(status.textContent).toContain("发现新版本 0.2.0");
    fireEvent.click(screen.getByRole("button", { name: "下载并安装" }));
    expect(screen.getByRole("status").textContent).toContain("正在下载并安装更新");
    await act(async () => {
      await Promise.resolve();
    });
    expect(update.downloadAndInstall).toHaveBeenCalledTimes(1);
  });

  it("reports up-to-date when the server has no pending release", async () => {
    const checkForUpdate = vi.fn().mockResolvedValue({ status: "upToDate" } satisfies UpdateCheckResult);
    render(<UpdaterPanel checkForUpdate={checkForUpdate} />);
    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    const status = await screen.findByRole("status");
    expect(status.textContent).toContain("当前已是最新版本");
  });
});
