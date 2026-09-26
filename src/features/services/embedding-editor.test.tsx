import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { EmbeddingConfig, PublicConfig } from "../../generated/bindings";
import { EmbeddingEditor } from "./embedding-editor";

vi.mock("../../api/commands", () => ({
  getConfigPublic: vi.fn(),
  saveEmbeddingConfig: vi.fn(),
  testEmbeddingConfig: vi.fn(),
  activateEmbeddingConfig: vi.fn(),
  deleteEmbeddingConfig: vi.fn(),
}));

const emptyConfig: PublicConfig = {
  configVersion: 1,
  application: { locale: null },
  models: {
    providers: [{ id: "openai", name: "OpenAI", baseUrl: "https://example.test/v1", credential: { reference: "providers/openai/api-key", configured: true } }],
    activeProviderId: "openai",
  },
  speech: { voiceRoutes: [], activeVoiceRouteId: null },
  knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null },
  storage: { exportDirectory: null },
  roleProfiles: [],
  activeRoleProfileId: null,
  diagnostics: { logRetentionDays: 14 },
};

function embedding(overrides: Partial<EmbeddingConfig> = {}): EmbeddingConfig {
  return {
    id: "primary",
    providerId: "openai",
    baseUrl: null,
    credential: null,
    modelId: "embed-3",
    dimensions: 8,
    distance: "cosine",
    normalized: true,
    active: false,
    ready: false,
    status: "not_tested",
    configVersion: 1,
    ...overrides,
  };
}

describe("EmbeddingEditor", () => {
  beforeEach(() => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: emptyConfig });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("saves provider model and dimensions then gates activation on test", async () => {
    const saved = embedding();
    const ready = embedding({ ready: true, status: "ready" });
    vi.mocked(commands.getConfigPublic)
      .mockResolvedValueOnce({ ok: true, data: emptyConfig })
      .mockResolvedValueOnce({ ok: true, data: { ...emptyConfig, knowledge: { embeddingConfigs: [saved], activeEmbeddingConfigId: null } } })
      .mockResolvedValue({ ok: true, data: { ...emptyConfig, knowledge: { embeddingConfigs: [ready], activeEmbeddingConfigId: null } } });
    vi.mocked(commands.saveEmbeddingConfig).mockResolvedValue({ ok: true, data: saved });
    vi.mocked(commands.testEmbeddingConfig).mockResolvedValue({ ok: true, data: { id: "primary", ready: true, dimensions: 8 } });
    vi.mocked(commands.activateEmbeddingConfig).mockResolvedValue({ ok: true, data: { ...ready, active: true } });

    render(<EmbeddingEditor />);
    expect(await screen.findByText(/可以选用已保存的供应商，或自行填写/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("供应商"), { target: { value: "openai" } });
    fireEvent.change(screen.getByLabelText("模型"), { target: { value: "embed-3" } });
    fireEvent.change(screen.getByLabelText("维度"), { target: { value: "8" } });
    expect(screen.getByLabelText("维度").getAttribute("min")).toBe("1");
    expect(screen.getByLabelText("维度").getAttribute("max")).toBe("65536");
    expect((screen.getByLabelText("距离") as HTMLInputElement).value).toBe("cosine");
    fireEvent.click(screen.getByRole("button", { name: "保存 Embedding" }));
    await waitFor(() =>
      expect(commands.saveEmbeddingConfig).toHaveBeenCalledWith({
        id: null,
        providerId: "openai",
        baseUrl: null,
        apiKey: null,
        modelId: "embed-3",
        dimensions: 8,
        normalized: true,
      }),
    );
    const enable = await screen.findByRole("button", { name: "启用" });
    expect((enable as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "测试" }));
    await waitFor(() => expect(commands.testEmbeddingConfig).toHaveBeenCalledWith("primary"));
    await waitFor(() => expect((screen.getByRole("button", { name: "启用" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole("button", { name: "启用" }));
    await waitFor(() => expect(commands.activateEmbeddingConfig).toHaveBeenCalledWith("primary"));
  });

  it("saves a custom URL when no provider is selected", async () => {
    vi.mocked(commands.saveEmbeddingConfig).mockResolvedValue({
      ok: true,
      data: embedding({ providerId: "", baseUrl: "http://127.0.0.1:8080/v1" }),
    });
    render(<EmbeddingEditor />);
    await screen.findByLabelText("模型");
    fireEvent.change(screen.getByLabelText("供应商"), { target: { value: "" } });
    fireEvent.change(screen.getByLabelText("接入地址"), { target: { value: "http://127.0.0.1:8080/v1" } });
    fireEvent.change(screen.getByLabelText("模型"), { target: { value: "BAAI/bge-m3" } });
    fireEvent.change(screen.getByLabelText("维度"), { target: { value: "1024" } });
    fireEvent.click(screen.getByRole("button", { name: "保存 Embedding" }));
    await waitFor(() =>
      expect(commands.saveEmbeddingConfig).toHaveBeenCalledWith({
        id: null,
        providerId: "",
        baseUrl: "http://127.0.0.1:8080/v1",
        apiKey: null,
        modelId: "BAAI/bge-m3",
        dimensions: 1024,
        normalized: true,
      }),
    );
  });

  it("clears a custom endpoint key after saving without exposing it", async () => {
    vi.mocked(commands.saveEmbeddingConfig).mockResolvedValue({
      ok: true,
      data: embedding({ providerId: "", baseUrl: "https://embed.test/v1" }),
    });
    render(<EmbeddingEditor />);
    await screen.findByText("还没有 Embedding 配置。");
    fireEvent.change(screen.getByLabelText("接入地址"), { target: { value: "https://embed.test/v1" } });
    fireEvent.change(screen.getByLabelText("模型"), { target: { value: "embed-model" } });
    const key = screen.getByLabelText(/API Key/) as HTMLInputElement;
    fireEvent.change(key, { target: { value: "embedding-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "保存 Embedding" }));
    await waitFor(() => expect(commands.saveEmbeddingConfig).toHaveBeenCalledWith(expect.objectContaining({
      providerId: "", baseUrl: "https://embed.test/v1", apiKey: "embedding-secret",
    })));
    await waitFor(() => expect(key.value).toBe(""));
    expect(document.body.textContent).not.toContain("embedding-secret");
  });

  it("disables after a failed retest and can delete", async () => {
    const failed = embedding({ ready: false, status: "test_failed", active: false });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, knowledge: { embeddingConfigs: [failed], activeEmbeddingConfigId: null } },
    });
    vi.mocked(commands.testEmbeddingConfig).mockResolvedValue({
      ok: false,
      error: { code: "EMBEDDING_UNAUTHORIZED", message: "未授权", requestId: "e1", retryable: false },
    });
    vi.mocked(commands.deleteEmbeddingConfig).mockResolvedValue({ ok: true, data: { ready: true } });
    render(<EmbeddingEditor />);
    expect((await screen.findByRole("button", { name: "启用" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "测试" }));
    expect((await screen.findByRole("status")).textContent).toContain("EMBEDDING_UNAUTHORIZED");
    fireEvent.click(screen.getByRole("button", { name: "删除" }));
    await waitFor(() => expect(commands.deleteEmbeddingConfig).toHaveBeenCalledWith("primary"));
  });

  it("loads the focused embedding into the editor", async () => {
    const target = embedding({ id: "embedding-2", modelId: "embed-focus", dimensions: 1024 });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, knowledge: { embeddingConfigs: [embedding(), target], activeEmbeddingConfigId: null } },
    });
    render(<EmbeddingEditor focusId="embedding-2" />);
    expect(await screen.findByRole("heading", { name: "编辑配置" })).toBeTruthy();
    expect((screen.getByLabelText("模型") as HTMLInputElement).value).toBe("embed-focus");
    expect((screen.getByLabelText("维度") as HTMLInputElement).value).toBe("1024");
  });

  it("clears a stale focused embedding that no longer exists", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({
      ok: true,
      data: { ...emptyConfig, knowledge: { embeddingConfigs: [embedding()], activeEmbeddingConfigId: null } },
    });
    render(<EmbeddingEditor focusId="missing-id" />);
    expect(await screen.findByText(/找不到 Embedding 配置（missing-id）/)).toBeTruthy();
    expect((screen.getByLabelText("模型") as HTMLInputElement).value).toBe("");
  });
});
