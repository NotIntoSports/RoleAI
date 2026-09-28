import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { VoiceRouteConfig } from "../../generated/bindings";
import { ServicesPage } from "./services-page";

vi.mock("../../api/commands", () => ({
  getConfigPublic: vi.fn(),
  saveModelProvider: vi.fn(),
  testModelProvider: vi.fn(),
  discoverModelProvider: vi.fn(),
  activateModelProvider: vi.fn(),
  deleteModelProvider: vi.fn(),
  getModelProviderDependencies: vi.fn(),
  saveSpeechRoute: vi.fn(),
  testSpeechRoute: vi.fn(),
  activateSpeechRoute: vi.fn(),
  deleteSpeechRoute: vi.fn(),
  saveEmbeddingConfig: vi.fn(),
  testEmbeddingConfig: vi.fn(),
  activateEmbeddingConfig: vi.fn(),
  deleteEmbeddingConfig: vi.fn(),
  listVoiceReferences: vi.fn(),
  saveVoiceReference: vi.fn(),
  saveVoiceReferenceAudio: vi.fn(),
  updateVoiceReference: vi.fn(),
  cloneVoiceReference: vi.fn(),
  deleteVoiceReference: vi.fn(),
}));

vi.mock("../../features/services/wav-recorder", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../features/services/wav-recorder")>()),
  VoiceRecorder: class {
    start = vi.fn(async () => undefined);
    stop = vi.fn(async () => ({ base64: "UklGRg==", durationMs: 12000 }));
    cancel = vi.fn();
    get recordedMs() { return 0; }
    get recording() { return false; }
  },
}));

const emptyConfig = {
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

describe("ServicesPage", () => {
  beforeEach(() => {
    vi.mocked(commands.getModelProviderDependencies).mockResolvedValue({ ok: true, data: [] });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: emptyConfig });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("opens providers by default and exposes one category at a time", async () => {
    render(<ServicesPage />);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toContain("服务");
    expect(await screen.findByRole("heading", { name: "模型供应商" })).toBeTruthy();
    const navigation = within(screen.getByRole("navigation", { name: "服务分类" }));
    expect(navigation.getByRole("button", { name: "模型供应商" }).getAttribute("aria-current")).toBe("page");
    expect(screen.queryByRole("heading", { name: "语音线路" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "Embedding" })).toBeNull();
    expect(screen.queryByRole("heading", { name: "LiveKit" })).toBeNull();
    for (const name of ["语音线路", "音色克隆", "Embedding"]) {
      fireEvent.click(navigation.getByRole("button", { name }));
      expect(screen.getByRole("heading", { name })).toBeTruthy();
      expect(screen.queryByRole("heading", { name: "模型供应商" })).toBeNull();
    }
  });

  it("lists voice references and saves a picked audio file", async () => {
    const reference = {
      id: "ref-1", name: "我的音色", providerId: null, targetModel: null, mimeType: "audio/wav",
      byteSize: BigInt(32000), durationMs: BigInt(12000), transcript: "你好世界",
      remoteFileId: null, voiceId: null, cloneStatus: "pending", cloneError: null,
      createdAt: "2026-09-25T00:00:00Z", updatedAt: "2026-09-25T00:00:00Z",
    };
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [reference] });
    vi.mocked(commands.saveVoiceReferenceAudio).mockResolvedValue({ ok: true, data: { ...reference, id: "ref-2", name: "新音色" } });
    render(<ServicesPage />);
    const navigation = within(screen.getByRole("navigation", { name: "服务分类" }));
    fireEvent.click(navigation.getByRole("button", { name: "音色克隆" }));
    expect(await screen.findByRole("heading", { name: "音色克隆" })).toBeTruthy();
    expect(await screen.findByText("我的音色")).toBeTruthy();
    const wavBytes = new Uint8Array([82, 73, 70, 70, 0, 0, 0, 0, 87, 65, 86, 69]);
    const file = new File([wavBytes], "sample.wav", { type: "audio/wav" });
    const fileInput = document.querySelector('input[type="file"]') as HTMLInputElement;
    Object.defineProperty(fileInput, "files", { value: [file] });
    fireEvent.change(fileInput);
    expect(await screen.findByText(/已选择 sample\.wav/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "新音色" } });
    fireEvent.click(screen.getByRole("button", { name: "保存音色" }));
    expect(await screen.findByText("音色已保存，请点击「克隆」生成音色 ID")).toBeTruthy();
    const expectedBase64 = btoa(Array.from(wavBytes, (byte) => String.fromCharCode(byte)).join(""));
    expect(commands.saveVoiceReferenceAudio).toHaveBeenCalledWith(expect.objectContaining({ name: "新音色", audioBase64: expectedBase64 }));
  });

  it("refreshes providers when the voices panel becomes visible", async () => {
    const single = { ...emptyConfig, models: { providers: [{ id: "p1", name: "一号", baseUrl: "https://a.example.com/v1", credential: null }], activeProviderId: null } };
    const updated = { ...single, models: { providers: [...single.models.providers, { id: "p2", name: "阿里云", baseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1", credential: null }], activeProviderId: null } };
    vi.mocked(commands.getConfigPublic)
      .mockResolvedValueOnce({ ok: true, data: single })
      .mockResolvedValue({ ok: true, data: updated });
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [] });
    render(<ServicesPage />);
    await screen.findByRole("heading", { name: "模型供应商" });
    const callsAfterLoad = vi.mocked(commands.getConfigPublic).mock.calls.length;
    fireEvent.click(within(screen.getByRole("navigation", { name: "服务分类" })).getByRole("button", { name: "音色克隆" }));
    const voicesPanel = document.getElementById("services-panel-voices") as HTMLElement;
    await waitFor(() => expect(vi.mocked(commands.getConfigPublic).mock.calls.length).toBeGreaterThan(callsAfterLoad));
    const select = within(voicesPanel).getByLabelText("供应商") as HTMLSelectElement;
    expect(Array.from(select.options).some((option) => option.value === "p2")).toBe(true);
  });

  it("edits an existing reference and updates metadata without rerecording", async () => {
    const reference = {
      id: "ref-1", name: "我的音色", providerId: null, targetModel: null, mimeType: "audio/wav",
      byteSize: BigInt(32000), durationMs: BigInt(12000), transcript: "你好世界",
      remoteFileId: null, voiceId: null, cloneStatus: "pending", cloneError: null,
      createdAt: "2026-09-25T00:00:00Z", updatedAt: "2026-09-25T00:00:00Z",
    };
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [reference] });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, models: { providers: [{ id: "zcode", name: "zcode", baseUrl: "https://open.bigmodel.cn/api/paas/v4", credential: null }], activeProviderId: null } },
    });
    vi.mocked(commands.updateVoiceReference).mockResolvedValue({
      ok: true,
      data: { ...reference, name: "我的音色", providerId: "zcode" },
    });
    render(<ServicesPage />);
    const navigation = within(screen.getByRole("navigation", { name: "服务分类" }));
    fireEvent.click(navigation.getByRole("button", { name: "音色克隆" }));
    expect(await screen.findByText("我的音色")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "编辑 我的音色" }));
    expect(await screen.findByText(/正在编辑「我的音色」/)).toBeTruthy();
    const voicesPanel = document.getElementById("services-panel-voices") as HTMLElement;
    fireEvent.change(within(voicesPanel).getByLabelText("供应商"), { target: { value: "zcode" } });
    fireEvent.click(within(voicesPanel).getByRole("button", { name: "保存修改" }));
    expect(await screen.findByText("音色信息已更新，原音频与克隆状态保持不变")).toBeTruthy();
    expect(commands.updateVoiceReference).toHaveBeenCalledWith(expect.objectContaining({ id: "ref-1", providerId: "zcode" }));
    expect(commands.saveVoiceReferenceAudio).not.toHaveBeenCalled();
  });

  it("saves a picked audio file with the target model and echoes it when editing", async () => {
    const reference = {
      id: "ref-7", name: "omni 音色", providerId: "aliyun", targetModel: "qwen3.8-omni-flash-realtime", mimeType: "audio/wav",
      byteSize: BigInt(32000), durationMs: BigInt(12000), transcript: "",
      remoteFileId: null, voiceId: null, cloneStatus: "pending", cloneError: null,
      createdAt: "2026-09-27T00:00:00Z", updatedAt: "2026-09-27T00:00:00Z",
    };
    vi.mocked(commands.listVoiceReferences)
      .mockResolvedValueOnce({ ok: true, data: [] })
      .mockResolvedValue({ ok: true, data: [reference] });
    vi.mocked(commands.saveVoiceReferenceAudio).mockResolvedValue({ ok: true, data: reference });
    render(<ServicesPage />);
    const navigation = within(screen.getByRole("navigation", { name: "服务分类" }));
    fireEvent.click(navigation.getByRole("button", { name: "音色克隆" }));
    expect(await screen.findByRole("heading", { name: "音色克隆" })).toBeTruthy();
    const wavBytes = new Uint8Array([82, 73, 70, 70, 0, 0, 0, 0, 87, 65, 86, 69]);
    const file = new File([wavBytes], "sample.wav", { type: "audio/wav" });
    const fileInput = document.querySelector('input[type="file"]') as HTMLInputElement;
    Object.defineProperty(fileInput, "files", { value: [file] });
    fireEvent.change(fileInput);
    expect(await screen.findByText(/已选择 sample\.wav/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "omni 音色" } });
    fireEvent.change(screen.getByLabelText(/目标模型/), { target: { value: "qwen3.8-omni-flash-realtime" } });
    fireEvent.click(screen.getByRole("button", { name: "保存音色" }));
    expect(await screen.findByText("音色已保存，请点击「克隆」生成音色 ID")).toBeTruthy();
    expect(commands.saveVoiceReferenceAudio).toHaveBeenCalledWith(expect.objectContaining({
      name: "omni 音色",
      targetModel: "qwen3.8-omni-flash-realtime",
    }));
    // 保存后列表回显，再点编辑：目标模型输入框应带出已存值。
    fireEvent.click(screen.getByRole("button", { name: "编辑 omni 音色" }));
    const voicesPanel = document.getElementById("services-panel-voices") as HTMLElement;
    const targetInput = within(voicesPanel).getByLabelText(/目标模型/) as HTMLInputElement;
    expect(targetInput.value).toBe("qwen3.8-omni-flash-realtime");
  });

  it("records in-app audio and saves it as a voice reference", async () => {
    vi.mocked(commands.listVoiceReferences).mockResolvedValue({ ok: true, data: [] });
    vi.mocked(commands.saveVoiceReferenceAudio).mockResolvedValue({
      ok: true,
      data: {
        id: "ref-9", name: "录音音色", providerId: null, targetModel: null, mimeType: "audio/wav",
        byteSize: BigInt(192044), durationMs: BigInt(12000), transcript: "",
        remoteFileId: null, voiceId: null, cloneStatus: "pending", cloneError: null,
        createdAt: "2026-09-25T00:00:00Z", updatedAt: "2026-09-25T00:00:00Z",
      },
    });
    render(<ServicesPage />);
    const navigation = within(screen.getByRole("navigation", { name: "服务分类" }));
    fireEvent.click(navigation.getByRole("button", { name: "音色克隆" }));
    fireEvent.click(await screen.findByRole("button", { name: "开始录音" }));
    fireEvent.click(await screen.findByRole("button", { name: /停止录音/ }));
    expect(await screen.findByText(/已录制 12 秒/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "录音音色" } });
    fireEvent.click(screen.getByRole("button", { name: "保存音色" }));
    expect(await screen.findByText("音色已保存，请点击「克隆」生成音色 ID")).toBeTruthy();
    expect(commands.saveVoiceReferenceAudio).toHaveBeenCalledWith(expect.objectContaining({ name: "录音音色", audioBase64: "UklGRg==" }));
  });

  it("preserves provider, route and embedding drafts across category switches", async () => {
    render(<ServicesPage />);
    await screen.findByText("还没有供应商。");
    const providerPanel = within(screen.getByRole("region", { name: "模型供应商" }));
    fireEvent.change(providerPanel.getByLabelText("显示名称"), { target: { value: "草稿供应商" } });
    fireEvent.click(screen.getByRole("button", { name: "语音线路" }));
    fireEvent.change(screen.getByLabelText("线路名称"), { target: { value: "草稿线路" } });
    expect(screen.queryByRole("textbox", { name: "显示名称" })).toBeNull();
    expect(document.getElementById("services-panel-providers")?.hidden).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "Embedding" }));
    fireEvent.change(screen.getByLabelText("模型"), { target: { value: "embedding-draft" } });
    fireEvent.click(screen.getByRole("button", { name: "模型供应商" }));
    expect((providerPanel.getByLabelText("显示名称") as HTMLInputElement).value).toBe("草稿供应商");
    fireEvent.click(screen.getByRole("button", { name: "语音线路" }));
    expect((screen.getByLabelText("线路名称") as HTMLInputElement).value).toBe("草稿线路");
    fireEvent.click(screen.getByRole("button", { name: "Embedding" }));
    expect((screen.getByLabelText("模型") as HTMLInputElement).value).toBe("embedding-draft");
  });

  it("submits a provider key and clears the password field", async () => {
    vi.mocked(commands.saveModelProvider).mockResolvedValue({
      ok: true,
      data: { id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: { reference: "providers/openai/api-key", configured: true } },
    });
    render(<ServicesPage />);
    await screen.findByRole("heading", { name: "模型供应商" });
    fireEvent.change(screen.getByLabelText("显示名称"), { target: { value: "OpenAI" } });
    fireEvent.change(screen.getAllByLabelText("接入地址")[0], { target: { value: "https://example.test/v1" } });
    const key = screen.getAllByLabelText(/API Key/)[0] as HTMLInputElement;
    fireEvent.change(key, { target: { value: "secret-marker" } });
    fireEvent.click(screen.getByRole("button", { name: "保存供应商" }));
    await waitFor(() => expect(commands.saveModelProvider).toHaveBeenCalledWith({
      id: null, name: "OpenAI", baseUrl: "https://example.test/v1", apiKey: "secret-marker",
    }));
    expect(key.value).toBe("");
    expect(document.body.textContent).not.toContain("secret-marker");
  });

  it("does not call fetch or contain old API paths", async () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    const { container } = render(<ServicesPage />);
    await screen.findByRole("heading", { name: "模型供应商" });
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(container.innerHTML).not.toMatch(/\/api\//);
    fetchSpy.mockRestore();
  });

  it("loads an existing provider into the editor without exposing its key", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: {
        ...emptyConfig,
        models: { providers: [{ id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: { reference: "providers/openai/api-key", configured: true } }], activeProviderId: null },
      },
    });
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑 OpenAI" }));
    expect((screen.getByLabelText("显示名称") as HTMLInputElement).value).toBe("OpenAI");
    expect((screen.getAllByLabelText(/API Key/)[0] as HTMLInputElement).value).toBe("");
  });

  it("shows provider test progress then a success count", async () => {
    let finishTest: (result: Awaited<ReturnType<typeof commands.testModelProvider>>) => void = () => undefined;
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: {
        ...emptyConfig,
        models: { providers: [{ id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: { reference: "providers/openai/api-key", configured: true } }], activeProviderId: null },
      },
    });
    vi.mocked(commands.testModelProvider).mockImplementation(
      () => new Promise((resolve) => {
        finishTest = resolve;
      }),
    );
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "测试 OpenAI" }));
    expect(await screen.findAllByText("正在测试连接…")).not.toHaveLength(0);
    finishTest({ ok: true, data: { providerId: "openai", reachable: true, modelCount: 3, webStatus: "disabled", webSourceCount: 0 } });
    expect(await screen.findAllByText("连接测试通过，发现 3 个模型；未启用联网")).not.toHaveLength(0);
  });
  it("shows provider references instead of deleting and navigates to the route", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: { ...emptyConfig, models: { providers: [{ id: "p1", name: "千问", baseUrl: "https://example.test", credential: null }], activeProviderId: null } } });
    vi.mocked(commands.getModelProviderDependencies).mockResolvedValue({ ok: true, data: [{ kind: "voiceRoute", id: "r1", name: "面试线路" }] });
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "删除" }));
    expect(await screen.findByText(/面试线路/)).toBeTruthy();
    expect(commands.deleteModelProvider).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /处理.*面试线路/ }));
    expect(screen.getByRole("heading", { name: "语音线路" })).toBeTruthy();
  });

  it("opens the referenced embedding editor instead of only switching category", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: {
        ...emptyConfig,
        models: { providers: [{ id: "p1", name: "千问", baseUrl: "https://example.test", credential: null }], activeProviderId: null },
        knowledge: {
          embeddingConfigs: [{
            id: "emb-9", providerId: "p1", baseUrl: null, credential: null, modelId: "text-embedding-v3",
            dimensions: 1024, distance: "cosine", normalized: true, active: false, ready: false, status: null, configVersion: 1,
          }],
          activeEmbeddingConfigId: null,
        },
      },
    });
    vi.mocked(commands.getModelProviderDependencies).mockResolvedValue({
      ok: true,
      data: [{ kind: "embedding", id: "emb-9", name: "text-embedding-v3" }],
    });
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "删除" }));
    fireEvent.click(await screen.findByRole("button", { name: /处理.*text-embedding-v3/ }));
    expect(screen.getByRole("heading", { name: "Embedding" })).toBeTruthy();
    expect(await screen.findByRole("heading", { name: "编辑配置" })).toBeTruthy();
    expect((screen.getByLabelText("模型") as HTMLInputElement).value).toBe("text-embedding-v3");
  });

  it("requires confirmation before deleting an unreferenced provider", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: { ...emptyConfig, models: { providers: [{ id: "p1", name: "测试供应商", baseUrl: "https://example.test", credential: null }], activeProviderId: null } } });
    vi.mocked(commands.deleteModelProvider).mockResolvedValue({ ok: true, data: { ready: true } });
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "删除" }));
    const confirm = await screen.findByRole("button", { name: "确认删除供应商" });
    expect(commands.deleteModelProvider).not.toHaveBeenCalled();
    fireEvent.click(confirm);
    await waitFor(() => expect(commands.deleteModelProvider).toHaveBeenCalledWith("p1"));
  });

  it("keeps a retry action when only credential cleanup fails", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: { ...emptyConfig, models: { providers: [{ id: "p1", name: "测试供应商", baseUrl: "https://example.test", credential: null }], activeProviderId: null } } });
    vi.mocked(commands.deleteModelProvider).mockResolvedValueOnce({ ok: false, error: { code: "SECRET_CLEANUP_FAILED", message: "failed", retryable: false, requestId: "test" } }).mockResolvedValueOnce({ ok: true, data: { ready: true } });
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "删除" }));
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: emptyConfig });
    fireEvent.click(await screen.findByRole("button", { name: "确认删除供应商" }));
    const retry = await screen.findByRole("button", { name: "重试清理密钥" });
    expect(await screen.findByText("还没有供应商。")).toBeTruthy();
    fireEvent.click(retry);
    await screen.findByText("供应商及密钥已删除");
    expect(commands.deleteModelProvider).toHaveBeenCalledTimes(2);
  });

  it("does not hide a refresh failure behind a successful deletion message", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: { ...emptyConfig, models: { providers: [{ id: "p1", name: "测试供应商", baseUrl: "https://example.test", credential: null }], activeProviderId: null } } });
    vi.mocked(commands.deleteModelProvider).mockResolvedValue({ ok: true, data: { ready: true } });
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "删除" }));
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: false, error: { code: "CONFIG_READ_FAILED", message: "read failed", retryable: false, requestId: "test" } });
    fireEvent.click(await screen.findByRole("button", { name: "确认删除供应商" }));
    await screen.findByText(/操作已完成，但无法重新读取配置/);
    expect(screen.queryByText("供应商及密钥已删除")).toBeNull();
  });

  it("reports verified provider web-search capability and sources", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: {
        ...emptyConfig,
        models: { providers: [{ id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: { reference: "providers/openai/api-key", configured: true }, webCapability: "openai_responses_web_search" }], activeProviderId: null },
      },
    });
    vi.mocked(commands.testModelProvider).mockResolvedValue({
      ok: true,
      data: { providerId: "openai", reachable: true, modelCount: 1, webStatus: "available", webSourceCount: 2 },
    });
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "测试 OpenAI" }));
    expect(await screen.findAllByText("连接测试通过，发现 1 个模型；联网可用，返回 2 个来源")).not.toHaveLength(0);
  });

  it("shows a provider test failure on the card and status line", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: {
        ...emptyConfig,
        models: { providers: [{ id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: { reference: "providers/openai/api-key", configured: true } }], activeProviderId: null },
      },
    });
    vi.mocked(commands.testModelProvider).mockResolvedValue({
      ok: false,
      error: { code: "PROVIDER_TIMEOUT", message: "Provider operation failed", requestId: "test", retryable: true },
    });
    render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "测试 OpenAI" }));
    expect(await screen.findAllByText("连接超时，请检查接入地址或网络")).not.toHaveLength(0);
  });

  it("offers discovered models to voice route fields", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, models: { providers: [{ id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: null }], activeProviderId: null } },
    });
    vi.mocked(commands.discoverModelProvider).mockResolvedValue({
      ok: true,
      data: { providerId: "openai", models: [{ id: "model-a" }] },
    });
    const { container } = render(<ServicesPage />);
    fireEvent.click(await screen.findByRole("button", { name: "发现模型" }));
    fireEvent.click(screen.getByRole("button", { name: "语音线路" }));
    fireEvent.change(screen.getByLabelText("ASR 供应商"), { target: { value: "openai" } });
    await waitFor(() => expect(container.querySelector('#models-asr option[value="model-a"]')).not.toBeNull());
    expect((screen.getByLabelText("ASR 模型") as HTMLInputElement).getAttribute("list")).toBe("models-asr");
  });

  it("discovers models automatically when a route provider is selected", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, models: { providers: [{ id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: null }], activeProviderId: null } },
    });
    vi.mocked(commands.discoverModelProvider).mockResolvedValue({
      ok: true,
      data: { providerId: "openai", models: [{ id: "model-a" }, { id: "model-b" }] },
    });
    const { container } = render(<ServicesPage />);
    // 无需先到供应商页点"发现模型"：在线路表单选中供应商即自动发现。
    fireEvent.click(await screen.findByRole("button", { name: "语音线路" }));
    fireEvent.change(screen.getByLabelText("ASR 供应商"), { target: { value: "openai" } });
    await waitFor(() => expect(commands.discoverModelProvider).toHaveBeenCalledWith("openai"));
    await waitFor(() => expect(container.querySelector('#models-asr option[value="model-a"]')).not.toBeNull());
    fireEvent.change(screen.getByLabelText("LLM 供应商"), { target: { value: "openai" } });
    await waitFor(() => expect(container.querySelector('#models-llm option[value="model-b"]')).not.toBeNull());
  });

  it("saves an end-to-end route with only realtime fields and requires a successful test before activation", async () => {
    const saved: VoiceRouteConfig = {
      id: "realtime", name: "实时线路", mode: "e2e",
      asrProviderId: null, asrModelId: null, llmProviderId: null, llmModelId: null,
      ttsProviderId: null, ttsModelId: null, voiceId: "alloy",
      e2eProviderId: "openai", e2eModelId: "realtime-model",
      active: false, ready: false, status: "not_tested", configVersion: 1,
    };
    const config = {
      ...emptyConfig,
      models: { providers: [{ id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: null }], activeProviderId: null },
    };
    let ready = false;
    let hasRoute = false;
    vi.mocked(commands.getConfigPublic).mockImplementation(async () => ({
      ok: true,
      data: { ...config, speech: { voiceRoutes: hasRoute ? [{ ...saved, ready }] : [], activeVoiceRouteId: null } },
    }));
    vi.mocked(commands.saveSpeechRoute).mockImplementation(async () => {
      hasRoute = true;
      return { ok: true, data: saved };
    });
    vi.mocked(commands.testSpeechRoute).mockImplementation(async () => {
      ready = true;
      return { ok: true, data: { routeId: "realtime", ready: true, checkedProviderIds: ["openai"] } };
    });
    vi.mocked(commands.activateSpeechRoute).mockResolvedValue({ ok: true, data: { ...saved, ready: true, active: true } });
    render(<ServicesPage />);
    await screen.findByRole("button", { name: "编辑 OpenAI" });
    fireEvent.click(screen.getByRole("button", { name: "语音线路" }));
    fireEvent.change(screen.getByLabelText("线路名称"), { target: { value: "实时线路" } });
    fireEvent.change(screen.getByLabelText("ASR 模型"), { target: { value: "discarded-asr" } });
    fireEvent.change(screen.getByLabelText("模式"), { target: { value: "e2e" } });
    fireEvent.change(screen.getByLabelText("Realtime 供应商"), { target: { value: "openai" } });
    fireEvent.change(screen.getByLabelText("Realtime 模型"), { target: { value: "realtime-model" } });
    fireEvent.change(screen.getByLabelText("音色 ID（可选）"), { target: { value: "alloy" } });
    fireEvent.click(screen.getByRole("button", { name: "保存语音线路" }));
    await waitFor(() => expect(commands.saveSpeechRoute).toHaveBeenCalledWith({
      id: null, name: "实时线路", mode: "e2e",
      asrProviderId: null, asrModelId: null, llmProviderId: null, llmModelId: null,
      ttsProviderId: null, ttsModelId: null, voiceId: "alloy",
      e2eProviderId: "openai", e2eModelId: "realtime-model",
    }));
    expect((screen.getByRole("button", { name: "启用" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "测试" }));
    await waitFor(() => expect((screen.getByRole("button", { name: "启用" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "启用" }));
    await waitFor(() => expect(commands.activateSpeechRoute).toHaveBeenCalledWith("realtime"));
  });
});
