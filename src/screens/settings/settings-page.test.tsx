import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import { SettingsPage } from "./settings-page";

vi.mock("../../api/commands", () => ({
  getConfigPublic: vi.fn(),
  getLegacyMigrationStatus: vi.fn(),
  importLegacySource: vi.fn(),
  saveRoleProfile: vi.fn(),
  copyRoleProfile: vi.fn(),
  activateRoleProfile: vi.fn(),
  deleteRoleProfile: vi.fn(),
}));

describe("SettingsPage", () => {
  beforeEach(() => {
    vi.mocked(commands.getLegacyMigrationStatus).mockResolvedValue({
      ok: true,
      data: { applied: false, reenterSecrets: false, omitted: [] },
    });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: {
        configVersion: 1,
        application: { locale: null },
        models: { providers: [], activeProviderId: null },
        speech: { voiceRoutes: [], activeVoiceRouteId: null },
        knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null },
        storage: { exportDirectory: null },
        roleProfiles: [],
        activeRoleProfileId: null,
        diagnostics: { logRetentionDays: 14 },
      },
    });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("renders the settings heading", () => {
    render(<SettingsPage />);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("设置");
  });

  it("opens appearance by default and keeps inactive panels inaccessible", () => {
    render(<SettingsPage />);
    const navigation = within(screen.getByRole("navigation", { name: "设置分类" }));
    expect(navigation.getByRole("button", { name: "外观" }).getAttribute("aria-current")).toBe("page");
    expect(screen.queryByRole("heading", { name: "角色" })).toBeNull();
    expect(screen.queryByRole("textbox", { name: "角色 ID" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "导入旧会话" })).toBeNull();
    expect(document.getElementById("settings-panel-roles")?.hidden).toBe(true);
    expect(document.getElementById("settings-panel-migration")?.hidden).toBe(true);
  });

  it("preserves role and migration drafts while switching categories", async () => {
    render(<SettingsPage />);
    fireEvent.click(screen.getByRole("button", { name: "角色" }));
    await screen.findByText("还没有角色。");
    fireEvent.change(screen.getByLabelText("系统提示"), { target: { value: "一次只问一个问题" } });
    fireEvent.click(screen.getByRole("button", { name: "数据迁移" }));
    fireEvent.change(screen.getByLabelText("旧数据目录"), { target: { value: "C:\\old-data" } });
    fireEvent.click(screen.getByRole("button", { name: "外观" }));
    expect(screen.queryByRole("textbox", { name: "系统提示" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "角色" }));
    expect((screen.getByLabelText("系统提示") as HTMLTextAreaElement).value).toBe("一次只问一个问题");
    fireEvent.click(screen.getByRole("button", { name: "数据迁移" }));
    expect((screen.getByLabelText("旧数据目录") as HTMLInputElement).value).toBe("C:\\old-data");
  });

  it("does not call fetch", () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    render(<SettingsPage />);
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });

  it("does not contain /api/ paths", () => {
    const { container } = render(<SettingsPage />);
    expect(container.innerHTML).not.toMatch(/\/api\//);
  });

  it("includes the role editor", async () => {
    render(<SettingsPage />);
    fireEvent.click(screen.getByRole("button", { name: "角色" }));
    expect(await screen.findByRole("heading", { name: "角色" })).toBeTruthy();
  });

  it("includes the legacy session import panel", async () => {
    render(<SettingsPage />);
    fireEvent.click(screen.getByRole("button", { name: "数据迁移" }));
    expect(await screen.findByRole("heading", { name: "导入旧会话" })).toBeTruthy();
  });

  it("shows 需要重新填写密钥 when migration status asks to reenter secrets", async () => {
    vi.mocked(commands.getLegacyMigrationStatus).mockResolvedValue({
      ok: true,
      data: { applied: true, reenterSecrets: true, omitted: [] },
    });
    render(<SettingsPage />);
    const banner = await screen.findByText("需要重新填写密钥");
    expect(banner.textContent).toContain("需要重新填写密钥");
  });

  it("hides the reenter banner when secrets are already configured", async () => {
    vi.mocked(commands.getLegacyMigrationStatus).mockResolvedValue({
      ok: true,
      data: { applied: true, reenterSecrets: false, omitted: [] },
    });
    render(<SettingsPage />);
    await waitFor(() => expect(commands.getLegacyMigrationStatus).toHaveBeenCalled());
    expect(screen.queryByText("需要重新填写密钥")).toBeNull();
  });
});
