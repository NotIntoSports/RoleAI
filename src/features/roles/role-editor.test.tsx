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
  listTavusFaces: vi.fn(),
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
    vi.mocked(commands.listTavusFaces).mockResolvedValue({ ok: true, data: [] });
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

  it("says a saved built-in role became a personal copy and the current role", async () => {
    const preset = profile({ id: "preset-interviewer", name: "面试官" });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, roleProfiles: [preset], activeRoleProfileId: "preset-interviewer" },
    });
    vi.mocked(commands.saveRoleProfile).mockResolvedValue({
      ok: true,
      data: profile({ id: "copy-1", name: "面试官", active: true, systemPrompt: "改过的提示" }),
    });
    render(<RoleEditor />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 面试官" }));
    fireEvent.change(screen.getByLabelText("系统提示"), { target: { value: "改过的提示" } });
    fireEvent.click(screen.getByRole("button", { name: "保存角色" }));
    await waitFor(() => expect(commands.saveRoleProfile).toHaveBeenCalledWith(expect.objectContaining({ id: "preset-interviewer" })));
    const status = await screen.findByRole("status");
    expect(status.textContent).toContain("已另存为个人副本，并设为当前角色");
    expect(status.getAttribute("data-tone")).toBe("success");
    expect(status.parentElement).toBe(screen.getByRole("button", { name: "保存角色" }).parentElement);
  });

  it("says an edited built-in role was written onto the current personal role", async () => {
    const preset = profile({ id: "preset-interviewer", name: "面试官" });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, roleProfiles: [preset], activeRoleProfileId: "preset-meeting" },
    });
    vi.mocked(commands.saveRoleProfile).mockResolvedValue({
      ok: true,
      data: profile({ id: "copy-1", name: "面试官", active: true, configVersion: 2, tavusFaceId: "r9d30b0e55ac" }),
    });
    render(<RoleEditor />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 面试官" }));
    fireEvent.click(screen.getByRole("button", { name: "保存角色" }));
    const status = await screen.findByRole("status");
    expect(status.textContent).toContain("已保存，并设为当前角色");
    expect(status.getAttribute("data-tone")).toBe("success");
  });

  it("marks a role card that has a video face", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, roleProfiles: [profile({ tavusFaceId: "r9d30b0e55ac" })] },
    });
    render(<RoleEditor />);
    expect(await screen.findByText("有视频形象")).toBeTruthy();
  });

  it("explains a missing Tavus key and offers a jump to the Tavus panel", async () => {
    vi.mocked(commands.listTavusFaces).mockResolvedValue({
      ok: false,
      error: { code: "TAVUS_KEY_MISSING", message: "请先在设置里保存 Tavus 密钥", requestId: "r3", retryable: false },
    });
    const onOpenTavus = vi.fn();
    render(<RoleEditor onOpenTavus={onOpenTavus} />);
    expect(await screen.findByText("还没有保存 Tavus 密钥，暂时没有形象可选。")).toBeTruthy();
    expect(screen.queryByText("选一张库存形象后，工作台可以和这个角色进行中文视频通话。")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "去「视频通话（Tavus）」保存密钥" }));
    expect(onOpenTavus).toHaveBeenCalledTimes(1);
  });

  it("shows other Tavus errors next to the same jump button", async () => {
    vi.mocked(commands.listTavusFaces).mockResolvedValue({
      ok: false,
      error: { code: "TAVUS_UNAUTHORIZED", message: "Tavus 密钥被拒绝", requestId: "r4", retryable: false },
    });
    render(<RoleEditor onOpenTavus={vi.fn()} />);
    expect(await screen.findByText("TAVUS_UNAUTHORIZED：Tavus 密钥被拒绝")).toBeTruthy();
    expect(screen.getByRole("button", { name: "去「视频通话（Tavus）」保存密钥" })).toBeTruthy();
  });

  it("loads faces only while visible and refreshes them each time it is shown", async () => {
    vi.mocked(commands.listTavusFaces)
      .mockResolvedValueOnce({
        ok: false,
        error: { code: "TAVUS_KEY_MISSING", message: "请先在设置里保存 Tavus 密钥", requestId: "r5", retryable: false },
      })
      .mockResolvedValue({ ok: true, data: [{ id: "r9d30b0e55ac", name: "Luna" }] });
    const { rerender } = render(<RoleEditor visible={false} />);
    await screen.findByText("还没有角色。");
    expect(commands.listTavusFaces).not.toHaveBeenCalled();

    rerender(<RoleEditor visible />);
    expect(await screen.findByText("还没有保存 Tavus 密钥，暂时没有形象可选。")).toBeTruthy();

    rerender(<RoleEditor visible={false} />);
    rerender(<RoleEditor visible />);
    expect(await screen.findByRole("option", { name: "Luna" })).toBeTruthy();
    expect(screen.queryByText("还没有保存 Tavus 密钥，暂时没有形象可选。")).toBeNull();
    expect(screen.getByText("选一张库存形象后，工作台可以和这个角色进行中文视频通话。")).toBeTruthy();
    expect(commands.listTavusFaces).toHaveBeenCalledTimes(2);
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
