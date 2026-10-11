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
  getDiagnosticsLatencySummary: vi.fn(),
  listTavusFaces: vi.fn(),
  getTavusCredentialStatus: vi.fn(),
  saveTavusCredential: vi.fn(),
  listVoiceReferences: vi.fn(),
}));

describe("SettingsPage", () => {
  beforeEach(() => {
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [] });
    vi.mocked(commands.getLegacyMigrationStatus).mockResolvedValue({
      ok: true,
      data: { applied: false, reenterSecrets: false, omitted: [] },
    });
    vi.mocked(commands.listTavusFaces).mockResolvedValue({ ok: true, data: [] });
    vi.mocked(commands.getTavusCredentialStatus).mockResolvedValue({
      ok: true,
      data: { reference: "tavus/api-key", configured: false },
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

  it("opens model providers by default and keeps inactive panels inaccessible", async () => {
    render(<SettingsPage />);
    const navigation = within(screen.getByRole("navigation", { name: "设置分类" }));
    expect(navigation.getByRole("button", { name: "模型供应商" }).getAttribute("aria-current")).toBe("page");
    expect(await screen.findByRole("heading", { name: "模型供应商" })).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "角色" })).toBeNull();
    expect(screen.queryByRole("textbox", { name: "角色 ID" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "导入旧会话" })).toBeNull();
    expect(document.getElementById("settings-panel-appearance")?.hidden).toBe(true);
    expect(document.getElementById("settings-panel-roles")?.hidden).toBe(true);
    expect(document.getElementById("settings-panel-migration")?.hidden).toBe(true);
  });

  it("lists the service categories first and splits the navigation into three groups", () => {
    render(<SettingsPage />);
    const buttons = within(screen.getByRole("navigation", { name: "设置分类" })).getAllByRole("button");
    expect(buttons.map((button) => button.textContent)).toEqual([
      "模型供应商", "语音线路", "音色克隆", "Embedding", "视频通话（Tavus）",
      "角色", "外观",
      "应用更新", "数据迁移", "诊断",
    ]);
    expect(buttons.filter((button) => button.hasAttribute("data-group-start")).map((button) => button.textContent)).toEqual(["角色", "应用更新"]);
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

  it("names the Tavus category and jumps there from the role editor hint", async () => {
    vi.mocked(commands.listTavusFaces).mockResolvedValue({
      ok: false,
      error: { code: "TAVUS_KEY_MISSING", message: "请先在设置里保存 Tavus 密钥", requestId: "r1", retryable: false },
    });
    render(<SettingsPage />);
    const navigation = within(screen.getByRole("navigation", { name: "设置分类" }));
    expect(navigation.getByRole("button", { name: "视频通话（Tavus）" })).toBeTruthy();
    expect(commands.listTavusFaces).not.toHaveBeenCalled();

    fireEvent.click(navigation.getByRole("button", { name: "角色" }));
    fireEvent.click(await screen.findByRole("button", { name: "去「视频通话（Tavus）」保存密钥" }));
    expect(document.getElementById("settings-panel-tavus")?.hidden).toBe(false);
    expect(document.getElementById("settings-panel-roles")?.hidden).toBe(true);
    expect(navigation.getByRole("button", { name: "视频通话（Tavus）" }).getAttribute("aria-current")).toBe("page");
  });

  it("loads the latency summary only after the diagnostics category is opened", async () => {
    vi.mocked(commands.getDiagnosticsLatencySummary).mockResolvedValue({
      ok: true,
      data: { sessionsScanned: 0, routes: [], recentTurns: [] },
    });
    render(<SettingsPage />);
    expect(commands.getDiagnosticsLatencySummary).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "诊断" }));
    expect(await screen.findByRole("heading", { name: "性能面板" })).toBeTruthy();
    expect(commands.getDiagnosticsLatencySummary).toHaveBeenCalledWith(20);
    expect(await screen.findByText("暂无线路延迟样本。")).toBeTruthy();
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

  it("opens model providers from the reenter banner without leaving the page", async () => {
    vi.mocked(commands.getLegacyMigrationStatus).mockResolvedValue({
      ok: true,
      data: { applied: true, reenterSecrets: true, omitted: [] },
    });
    render(<SettingsPage />);
    fireEvent.click(within(screen.getByRole("navigation", { name: "设置分类" })).getByRole("button", { name: "外观" }));
    fireEvent.click(await screen.findByRole("button", { name: "去「模型供应商」填写" }));
    expect(document.getElementById("settings-panel-providers")?.hidden).toBe(false);
    expect(document.getElementById("settings-panel-appearance")?.hidden).toBe(true);
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
