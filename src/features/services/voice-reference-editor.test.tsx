import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { PublicConfig, VoiceReferenceSummary } from "../../generated/bindings";
import { VoiceReferenceEditor } from "./voice-reference-editor";

vi.mock("../../api/commands", () => ({
  getConfigPublic: vi.fn(),
  listVoiceReferences: vi.fn(),
  saveVoiceReferenceAudio: vi.fn(),
  updateVoiceReference: vi.fn(),
  cloneVoiceReference: vi.fn(),
  deleteVoiceReference: vi.fn(),
}));

const config: PublicConfig = {
  configVersion: 1,
  application: { locale: null },
  models: {
    providers: [{ id: "aliyun", name: "阿里云", baseUrl: "https://example.test", credential: { reference: "providers/aliyun/api-key", configured: true } }],
    activeProviderId: "aliyun",
  },
  speech: { voiceRoutes: [], activeVoiceRouteId: null },
  knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null },
  storage: { exportDirectory: null },
  roleProfiles: [],
  activeRoleProfileId: null,
  diagnostics: { logRetentionDays: 14 },
};

function reference(overrides: Partial<VoiceReferenceSummary> = {}): VoiceReferenceSummary {
  return {
    id: "ref-1",
    name: "面试音色",
    providerId: "aliyun",
    targetModel: null,
    mimeType: "audio/wav",
    byteSize: BigInt(2048),
    durationMs: BigInt(8000),
    transcript: "",
    remoteFileId: null,
    voiceId: null,
    cloneStatus: "pending",
    cloneError: null,
    createdAt: "2026-09-29T10:00:00Z",
    updatedAt: "2026-09-29T10:00:00Z",
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

describe("VoiceReferenceEditor", () => {
  beforeEach(() => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: config });
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [] });
    vi.mocked(commands.updateVoiceReference).mockResolvedValue({
      ok: true,
      data: reference({ name: "新名字" }),
    });
    vi.mocked(commands.saveVoiceReferenceAudio).mockResolvedValue({
      ok: true,
      data: reference(),
    });
    vi.mocked(commands.cloneVoiceReference).mockResolvedValue({
      ok: true,
      data: { referenceId: "ref-1", voiceId: "voice-9", remoteFileId: "remote-1" },
    });
    vi.mocked(commands.deleteVoiceReference).mockResolvedValue({
      ok: true,
      data: undefined,
    });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("renders the panel and lists saved references with clone status", async () => {
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [reference()] });
    render(<VoiceReferenceEditor visible />);
    expect(screen.getByRole("status").textContent).toContain("正在读取音色");
    expect(await screen.findByText("面试音色")).toBeTruthy();
    expect(screen.getByText(/未克隆/)).toBeTruthy();
    expect(screen.getByText("添加音色")).toBeTruthy();
    expect(screen.getByRole("option", { name: "阿里云" })).toBeTruthy();
    expect(commands.getConfigPublic).toHaveBeenCalled();
  });

  it("shows the load error but still fills the provider dropdown from config", async () => {
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({
      ok: false,
      error: { code: "LIST_FAILED", message: "原始信息", retryable: false, requestId: "test" },
    });
    render(<VoiceReferenceEditor visible />);
    expect(await screen.findByRole("status").then((node) => node.textContent)).toContain("LIST_FAILED：原始信息");
    expect(screen.getByRole("option", { name: "阿里云" })).toBeTruthy();
  });

  it("shows the empty state when no references exist", async () => {
    render(<VoiceReferenceEditor visible />);
    expect(await screen.findByText("还没有音色。")).toBeTruthy();
  });

  it("blocks submit without audio and without an existing reference", async () => {
    render(<VoiceReferenceEditor visible />);
    await screen.findByText("还没有音色。");
    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "新音色" } });
    fireEvent.submit(screen.getByRole("button", { name: "保存音色" }).closest("form")!);
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("请先录制或选择参考音频。"));
    expect(commands.saveVoiceReferenceAudio).not.toHaveBeenCalled();
    expect(commands.updateVoiceReference).not.toHaveBeenCalled();
  });

  it("keeps controls disabled while saving an edit and shows the success message", async () => {
    const pending = deferred<{ ok: true; data: VoiceReferenceSummary }>();
    vi.mocked(commands.updateVoiceReference).mockReturnValue(pending.promise as never);
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [reference()] });
    render(<VoiceReferenceEditor visible />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 面试音色" }));
    expect(screen.getByText("编辑音色")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "  新名字  " } });
    fireEvent.click(screen.getByRole("button", { name: "保存修改" }));

    expect(await waitFor(() => expect(commands.updateVoiceReference).toHaveBeenCalled()));
    expect(screen.getByRole("button", { name: "保存修改" }).disabled).toBe(true);
    expect(screen.getByRole("button", { name: "开始录音" }).disabled).toBe(true);
    expect(screen.getByRole("button", { name: "选择音频文件…" }).disabled).toBe(true);

    pending.resolve({ ok: true, data: reference({ name: "新名字" }) });
    await waitFor(() => expect(screen.getByRole("button", { name: "保存修改" }).disabled).toBe(false));
    expect(screen.getByRole("status").textContent).toContain("音色信息已更新，原音频与克隆状态保持不变");
    expect(commands.updateVoiceReference).toHaveBeenCalledWith(
      expect.objectContaining({ id: "ref-1", name: "新名字", providerId: "aliyun" }),
    );
  });

  it("surfaces save errors in the shared code：message format", async () => {
    vi.mocked(commands.updateVoiceReference).mockResolvedValue({
      ok: false,
      error: { code: "UPDATE_FAILED", message: "原始信息", retryable: false, requestId: "test" },
    });
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [reference()] });
    render(<VoiceReferenceEditor visible />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 面试音色" }));
    fireEvent.click(screen.getByRole("button", { name: "保存修改" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("UPDATE_FAILED：原始信息"));
  });
});
