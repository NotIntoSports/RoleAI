import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { PublicConfig, RoleProfileConfig } from "../../generated/bindings";
import { RoleEditor } from "./role-editor";

vi.mock("../../api/commands", () => ({
  getConfigPublic: vi.fn(),
  saveRoleProfile: vi.fn(),
  copyRoleProfile: vi.fn(),
  activateRoleProfile: vi.fn(),
  deleteRoleProfile: vi.fn(),
}));

const emptyConfig: PublicConfig = {
  configVersion: 1,
  application: { locale: null },
  models: { providers: [], activeProviderId: null },
  speech: { voiceRoutes: [], activeVoiceRouteId: null },
  knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null },
  storage: { exportDirectory: null },
  roleProfiles: [],
  activeRoleProfileId: null,
  diagnostics: { logRetentionDays: 14 },
};

function profile(overrides: Partial<RoleProfileConfig> = {}): RoleProfileConfig {
  return {
    id: "interviewer",
    name: "Interviewer",
    systemPrompt: "Ask one question",
    openingMessage: "Hello",
    styleInstructions: "Concise",
    active: false,
    configVersion: 1,
    ...overrides,
  };
}

describe("RoleEditor", () => {
  beforeEach(() => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: emptyConfig });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("shows loading then empty state", async () => {
    render(<RoleEditor />);
    expect(screen.getByRole("status").textContent).toContain("正在读取本地配置");
    expect(await screen.findByText("还没有角色。")).toBeTruthy();
  });

  it("surfaces a load error", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: false,
      error: { code: "CONFIG_INVALID", message: "配置无效", requestId: "r1", field: "roleProfiles", retryable: false },
    });
    render(<RoleEditor />);
    expect((await screen.findByRole("status")).textContent).toContain("roleProfiles：CONFIG_INVALID：配置无效");
  });

  it("creates, edits, copies, activates and two-step deletes a role", async () => {
    const saved = profile();
    vi.mocked(commands.getConfigPublic)
      .mockResolvedValueOnce({ ok: true, data: emptyConfig })
      .mockResolvedValue({ ok: true, data: { ...emptyConfig, roleProfiles: [saved], activeRoleProfileId: null } });
    vi.mocked(commands.saveRoleProfile).mockResolvedValue({ ok: true, data: saved });
    vi.mocked(commands.copyRoleProfile).mockResolvedValue({ ok: true, data: profile({ id: "copy", name: "Interviewer" }) });
    vi.mocked(commands.activateRoleProfile).mockResolvedValue({ ok: true, data: profile({ active: true }) });
    vi.mocked(commands.deleteRoleProfile).mockResolvedValue({ ok: true, data: { ready: true } });

    render(<RoleEditor />);
    await screen.findByText("还没有角色。");
    fireEvent.change(screen.getByLabelText("显示名称"), { target: { value: "Interviewer" } });
    fireEvent.change(screen.getByLabelText("系统提示"), { target: { value: "Ask one question" } });
    fireEvent.change(screen.getByLabelText("开场白"), { target: { value: "Hello" } });
    fireEvent.change(screen.getByLabelText("风格说明"), { target: { value: "Concise" } });
    fireEvent.click(screen.getByRole("button", { name: "保存角色" }));
    await waitFor(() => expect(commands.saveRoleProfile).toHaveBeenCalledWith({
      id: null,
      name: "Interviewer",
      systemPrompt: "Ask one question",
      openingMessage: "Hello",
      styleInstructions: "Concise",
    }));
    expect(await screen.findByText("Interviewer")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "编辑 Interviewer" }));
    expect((screen.getByLabelText("系统提示") as HTMLTextAreaElement).value).toBe("Ask one question");
    fireEvent.click(screen.getByRole("button", { name: "复制" }));
    await waitFor(() => expect(commands.copyRoleProfile).toHaveBeenCalledWith({ sourceId: "interviewer", id: null }));
    fireEvent.click(screen.getByRole("button", { name: "设为默认" }));
    await waitFor(() => expect(commands.activateRoleProfile).toHaveBeenCalledWith("interviewer"));
    fireEvent.click(screen.getByRole("button", { name: "删除" }));
    expect(commands.deleteRoleProfile).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
    await waitFor(() => expect(commands.deleteRoleProfile).toHaveBeenCalledWith("interviewer"));
  });

  it("shows the active badge and field errors after reload", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, roleProfiles: [profile({ active: true })], activeRoleProfileId: "interviewer" },
    });
    vi.mocked(commands.saveRoleProfile).mockResolvedValue({
      ok: false,
      error: { code: "ROLE_PROFILE_FIELDS_INVALID", message: "字段无效", requestId: "r2", field: "name", retryable: false },
    });
    render(<RoleEditor />);
    expect(await screen.findByText("当前启用")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("显示名称"), { target: { value: "X" } });
    fireEvent.click(screen.getByRole("button", { name: "保存角色" }));
    expect((await screen.findByRole("status")).textContent).toContain("name：ROLE_PROFILE_FIELDS_INVALID");
    expect(screen.queryByLabelText("角色 ID")).toBeNull();
    expect(screen.getByLabelText("系统提示").getAttribute("maxLength")).toBe("32768");
    expect(screen.getByLabelText("开场白").getAttribute("maxLength")).toBe("4096");
    expect(screen.getByLabelText("风格说明").getAttribute("maxLength")).toBe("8192");
  });

  it("does not use fetch, /api/, login, or low-level Tauri imports", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    const { container } = render(<RoleEditor />);
    await screen.findByText("还没有角色。");
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(container.innerHTML).not.toMatch(/\/api\//);
    expect(container.innerHTML).not.toMatch(/login|logout|management/i);
    expect(container.innerHTML).not.toContain("@tauri-apps/api");
    fetchSpy.mockRestore();
  });
});
