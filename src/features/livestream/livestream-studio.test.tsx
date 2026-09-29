import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import { LivestreamStudio } from "./livestream-studio";

vi.mock("../../api/commands", () => ({
  listMaterials: vi.fn(),
  generateLivestream: vi.fn(),
  createLivestreamDraft: vi.fn(),
  getLivestream: vi.fn(),
  controlLivestream: vi.fn(),
  insertLivestreamQuestion: vi.fn(),
  getObsRuntimeStatus: vi.fn(),
  startObsVirtualCamera: vi.fn(),
  stopObsVirtualCamera: vi.fn(),
  getObsPasswordStatus: vi.fn(),
  saveObsPassword: vi.fn(),
}));

const runtime = {
  script: {
    id: "script-1", title: "产品 A", loopEnabled: false, confirmed: false,
    currentIndex: null, state: "draft" as const,
    segments: [{ id: "seg-1", title: "开场", text: "欢迎了解产品 A", estimatedSeconds: 10, sources: ["product.pdf"], status: "draft" as const }],
  },
  stage: { productTitle: "产品 A", currentSubtitle: "", nextHint: "下一段：开场", state: "draft" as const, mediaPath: null, mediaKind: null, outputState: "idle" as const, outputErrorCode: null },
};

describe("LivestreamStudio", () => {
  beforeEach(() => {
    vi.mocked(commands.listMaterials).mockResolvedValue({ ok: true, data: [{ id: "mat-1", fileName: "product.pdf", contentSha256: "a".repeat(64), mediaType: "application/pdf", byteSize: 10, status: "text_ready", chunkCount: 1 }] });
    vi.mocked(commands.getLivestream).mockResolvedValue({ ok: false, error: { code: "LIVESTREAM_NOT_FOUND", message: "none", retryable: false, requestId: "r" } });
    vi.mocked(commands.generateLivestream).mockResolvedValue({ ok: true, data: runtime });
    vi.mocked(commands.controlLivestream).mockResolvedValue({ ok: true, data: { ...runtime, script: { ...runtime.script, confirmed: true, state: "ready" } } });
    vi.mocked(commands.insertLivestreamQuestion).mockResolvedValue({ ok: true, data: { ...runtime, script: { ...runtime.script, confirmed: true, state: "paused" }, stage: { ...runtime.stage, currentSubtitle: "适合会议讲解", state: "paused", outputState: "synthesizing" } } });
    vi.mocked(commands.getObsRuntimeStatus).mockResolvedValue({ ok: true, data: { connected: false, sceneReady: false, browserSourceReady: false, virtualCameraActive: false, errorCode: "OBS_CONNECT_FAILED" } });
    vi.mocked(commands.getObsPasswordStatus).mockResolvedValue({ ok: true, data: { reference: "obs/websocket-password", configured: false } });
    vi.mocked(commands.saveObsPassword).mockResolvedValue({ ok: true, data: { reference: "obs/websocket-password", configured: true } });
  });
  afterEach(() => { cleanup(); vi.clearAllMocks(); });

  it("generates from selected local material and requires confirmation before playback", async () => {
    render(<LivestreamStudio />);
    fireEvent.change(await screen.findByLabelText("产品标题"), { target: { value: "产品 A" } });
    fireEvent.click(await screen.findByRole("checkbox", { name: /product.pdf/ }));
    fireEvent.click(screen.getByRole("button", { name: "生成分段讲稿" }));
    await waitFor(() => expect(commands.generateLivestream).toHaveBeenCalledWith(expect.objectContaining({ title: "产品 A", materialIds: ["mat-1"] })));
    expect(await screen.findByDisplayValue("欢迎了解产品 A")).toBeTruthy();
    expect(screen.getByRole("button", { name: "开始播报" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "确认讲稿" }));
    await waitFor(() => expect(commands.controlLivestream).toHaveBeenCalledWith("confirm"));
    fireEvent.change(screen.getByLabelText("插入人工问题"), { target: { value: "适合什么场景？" } });
    fireEvent.click(screen.getByRole("button", { name: "回答并播报" }));
    await waitFor(() => expect(commands.insertLivestreamQuestion).toHaveBeenCalledWith("适合什么场景？"));
  });

  it("guides to the materials page when no ready materials exist", async () => {
    vi.mocked(commands.listMaterials).mockResolvedValue({ ok: true, data: [] });
    render(<LivestreamStudio />);
    expect(await screen.findByText("暂无已就绪资料，请先到“资料”页导入并建立索引。")).toBeTruthy();
    expect(screen.queryByRole("checkbox", { name: /product\.pdf/ })).toBeNull();
  });

  it("disables stopping the OBS virtual camera while it is not active", async () => {
    render(<LivestreamStudio />);
    expect(await screen.findByText("虚拟摄像头未启动")).toBeTruthy();
    expect(screen.getByRole("button", { name: "停止 OBS 输出" })).toBeDisabled();
  });

  it("disables actions while generation is in flight", async () => {
    let finishGenerate!: (result: { ok: true; data: typeof runtime }) => void;
    vi.mocked(commands.generateLivestream).mockImplementation(
      () => new Promise((resolve) => { finishGenerate = resolve; }),
    );
    render(<LivestreamStudio />);
    fireEvent.change(await screen.findByLabelText("产品标题"), { target: { value: "产品 A" } });
    fireEvent.click(await screen.findByRole("checkbox", { name: /product\.pdf/ }));
    const generateButton = screen.getByRole("button", { name: "生成分段讲稿" });
    expect(generateButton).toBeEnabled();
    fireEvent.click(generateButton);
    await waitFor(() => expect(generateButton).toBeDisabled());
    expect(screen.getByRole("button", { name: "停止 OBS 输出" })).toBeDisabled();
    finishGenerate({ ok: true, data: runtime });
    await waitFor(() => expect(generateButton).toBeEnabled());
  });

  it("shows the backend error code and message when generation fails", async () => {
    vi.mocked(commands.generateLivestream).mockResolvedValue({
      ok: false,
      error: { code: "LIVESTREAM_GENERATE_FAILED", message: "资料索引不可用", retryable: false, requestId: "r2" },
    });
    render(<LivestreamStudio />);
    fireEvent.change(await screen.findByLabelText("产品标题"), { target: { value: "产品 B" } });
    fireEvent.click(await screen.findByRole("checkbox", { name: /product\.pdf/ }));
    fireEvent.click(screen.getByRole("button", { name: "生成分段讲稿" }));
    expect(await screen.findByRole("status")).toHaveTextContent(
      "LIVESTREAM_GENERATE_FAILED：资料索引不可用",
    );
    expect(screen.queryByText("欢迎了解产品 A")).toBeNull();
  });
});
