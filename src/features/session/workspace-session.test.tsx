import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type {
  RuntimeStatus,
  SessionDetail,
  SessionReplyEvent,
  SessionSummary,
  SessionTranscriptEvent,
  SessionTurnView,
  PublicConfig,
} from "../../generated/bindings";
import { WorkspaceSession } from "./workspace-session";

vi.mock("../../api/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../api/commands")>()),
  getConfigPublic: vi.fn(),
  listMeetingProcesses: vi.fn(),
  getVirtualAudioStatus: vi.fn(),
  installVirtualAudio: vi.fn(),
  listAudioOutputs: vi.fn(),
  startSession: vi.fn(),
  stopSession: vi.fn(),
  setSessionMode: vi.fn(),
  getRuntimeStatus: vi.fn(),
  getSession: vi.fn(),
  finalizeSessionUtterance: vi.fn(),
  triggerMeetingAssistant: vi.fn(),
  sessionAgentCommand: vi.fn(),
  isSessionAudioReady: vi.fn(),
  pushMicPcm: vi.fn(),
}));


function summary(overrides: Partial<SessionSummary> = {}): SessionSummary {
  return {
    id: "sess-1",
    status: "listening",
    roleProfileId: "role-1",
    voiceRouteId: "route-1",
    transportMode: "direct",
    startedAt: "2026-09-05T10:00:00Z",
    finishedAt: null,
    updatedAt: "2026-09-05T10:00:00Z",
    ...overrides,
  };
}

function status(overrides: Partial<RuntimeStatus> = {}): RuntimeStatus {
  return {
    phase: "idle",
    mode: "ai_active",
    seq: 1,
    unusedMaterials: false,
    lastErrorCode: null,
    revision: 0,
    ...overrides,
  };
}

function commandResult(
  overrides: Partial<{
    commandId: string;
    action: string;
    ok: boolean;
    result: Record<string, unknown>;
    error: string;
  }> = {},
) {
  return {
    commandId: "cmd-1",
    action: "say",
    ok: true,
    result: {},
    error: "",
    ...overrides,
  };
}

function turn(overrides: Partial<SessionTurnView> = {}): SessionTurnView {
  return {
    id: "turn-1",
    turnIndex: 0,
    userText: "请介绍岗位",
    assistantText: "这是一个后端岗位",
    materialsUsed: true,
    citations: [],
    ...overrides,
  };
}

function detail(overrides: Partial<SessionDetail> = {}): SessionDetail {
  return {
    session: summary(),
    turns: [turn()],
    ...overrides,
  };
}

describe("WorkspaceSession", () => {
  function configuredSession(): PublicConfig {
    return {
      configVersion: 1, application: { locale: null }, models: { providers: [], activeProviderId: null },
      speech: { activeVoiceRouteId: "route-1", voiceRoutes: [{ id: "route-1", name: "测试线路", mode: "cascaded", asrProviderId: null, asrModelId: null, llmProviderId: null, llmModelId: null, ttsProviderId: null, ttsModelId: null, voiceId: null, e2eProviderId: null, e2eModelId: null, active: true, ready: true, status: null, configVersion: 1 }] },
      knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null }, storage: { exportDirectory: null },
      roleProfiles: [{ id: "role-1", name: "会议助手", systemPrompt: "listen", openingMessage: "", styleInstructions: "", active: true, configVersion: 1 }],
      activeRoleProfileId: "role-1", diagnostics: { logRetentionDays: 14 },
    };
  }
  beforeEach(() => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: false, error: { code: "TEST_CONFIG_UNAVAILABLE", message: "unavailable", retryable: false, requestId: "test" } });
    vi.mocked(commands.getVirtualAudioStatus).mockResolvedValue({ ok: true, data: { state: "ready", installed: true, rebootRequired: false, detail: "ready", renderEndpointId: "cable-input", captureEndpointId: "cable-output" } });
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({ ok: true, data: status() });
    vi.mocked(commands.startSession).mockResolvedValue({
      ok: true,
      data: { kind: "started", session: summary() },
    });
    vi.mocked(commands.stopSession).mockResolvedValue({
      ok: true,
      data: summary({ status: "completed", finishedAt: "2026-09-05T10:05:00Z" }),
    });
    vi.mocked(commands.setSessionMode).mockResolvedValue({
      ok: true,
      data: status({ mode: "operator_speaking", phase: "listening", seq: 2 }),
    });
    vi.mocked(commands.getSession).mockResolvedValue({
      ok: true,
      data: { session: summary(), turns: [] },
    });
    vi.mocked(commands.finalizeSessionUtterance).mockResolvedValue({
      ok: true,
      data: turn(),
    });
    vi.mocked(commands.triggerMeetingAssistant).mockResolvedValue({ ok: true, data: turn() });
    vi.mocked(commands.sessionAgentCommand).mockResolvedValue({
      ok: true,
      data: commandResult(),
    });
    vi.mocked(commands.isSessionAudioReady).mockResolvedValue({ ok: true, data: { ready: false } });
    vi.mocked(commands.pushMicPcm).mockResolvedValue({ ok: true, data: { accepted: true } });
    vi.mocked(commands.listAudioOutputs).mockResolvedValue({
      ok: true,
      data: [{ id: "spk-1", name: "扬声器" }],
    });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("shows loading then idle start controls", async () => {
    render(<WorkspaceSession />);
    expect(screen.getByRole("status").textContent).toContain("正在读取会话状态");
    const start = await screen.findByRole("button", { name: "开始会话" });
    expect(start).toBeTruthy();
    expect((start as HTMLButtonElement).disabled).toBe(false);
    expect(document.body.textContent).toContain("点击「开始会话」");
    expect((screen.getByRole("button", { name: "停止" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole("heading", { name: "当前会话" })).toBeTruthy();
    expect(document.body.textContent).toContain("未开始");
  });

  it("requires an explicit choice with multiple meetings and sends the chosen PID", async () => {
    const config: PublicConfig = {
      configVersion: 1, application: { locale: null }, models: { providers: [], activeProviderId: null },
      speech: { activeVoiceRouteId: "route-1", voiceRoutes: [{ id: "route-1", name: "测试线路", mode: "cascaded", asrProviderId: null, asrModelId: null, llmProviderId: null, llmModelId: null, ttsProviderId: null, ttsModelId: null, voiceId: null, e2eProviderId: null, e2eModelId: null, active: true, ready: true, status: null, configVersion: 1 }] },
      knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null }, storage: { exportDirectory: null },
      roleProfiles: [{ id: "role-1", name: "会议助手", systemPrompt: "listen", openingMessage: "", styleInstructions: "", active: true, configVersion: 1 }],
      activeRoleProfileId: "role-1", diagnostics: { logRetentionDays: 14 },
    };
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: config });
    vi.mocked(commands.listMeetingProcesses).mockResolvedValue({ ok: true, data: [
      { pid: 101, name: "zoom.exe", title: "Meeting A" }, { pid: 202, name: "wemeetapp.exe", title: "Meeting B" },
    ] });
    render(<WorkspaceSession />);
    await screen.findByLabelText("输入来源");
    if (screen.getByRole("button", { name: "会话配置" }).getAttribute("aria-expanded") === "false") fireEvent.click(screen.getByRole("button", { name: "会话配置" }));
    fireEvent.change(screen.getByLabelText("输入来源"), { target: { value: "meeting" } });
    await screen.findByRole("option", { name: /Meeting B/ });
    expect(screen.getByLabelText("会议进程")).toHaveValue("");
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await screen.findByText("请刷新并选择会议进程。");
    expect(commands.startSession).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("会议进程"), { target: { value: "202" } });
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(commands.startSession).toHaveBeenCalledWith(expect.objectContaining({ meetingPid: 202 })));
  });

  it("answers once when the Rust global-hotkey event reaches an active meeting assistant", async () => {
    const config: PublicConfig = {
      configVersion: 1, application: { locale: null }, models: { providers: [], activeProviderId: null },
      speech: { activeVoiceRouteId: "route-1", voiceRoutes: [{ id: "route-1", name: "测试线路", mode: "cascaded", asrProviderId: null, asrModelId: null, llmProviderId: null, llmModelId: null, ttsProviderId: null, ttsModelId: null, voiceId: null, e2eProviderId: null, e2eModelId: null, active: true, ready: true, status: null, configVersion: 1 }] },
      knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null }, storage: { exportDirectory: null },
      roleProfiles: [{ id: "role-1", name: "会议助手", systemPrompt: "listen", openingMessage: "", styleInstructions: "", scenario: "meetingAssistant", active: true, configVersion: 1 }],
      activeRoleProfileId: "role-1", diagnostics: { logRetentionDays: 14 },
    };
    let hotkey: (() => void) | undefined;
    const listen = vi.fn(async (event: string, handler: (payload: never) => void) => {
      if (event === "session.assistant_hotkey.v1") hotkey = handler as () => void;
      return () => {};
    });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: config });
    vi.mocked(commands.listMeetingProcesses).mockResolvedValue({ ok: true, data: [{ pid: 101, name: "zoom.exe", title: "Meeting" }] });
    render(<WorkspaceSession listen={listen} />);
    await screen.findByLabelText("输入来源");
    if (screen.getByRole("button", { name: "会话配置" }).getAttribute("aria-expanded") === "false") fireEvent.click(screen.getByRole("button", { name: "会话配置" }));
    fireEvent.change(screen.getByLabelText("输入来源"), { target: { value: "meeting" } });
    await screen.findByRole("option", { name: /Meeting/ });
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(hotkey).toBeTypeOf("function"));
    act(() => hotkey?.());
    await waitFor(() => expect(commands.triggerMeetingAssistant).toHaveBeenCalledTimes(1));
  });

  it("starts a session and shows the listening phase", async () => {
    vi.mocked(commands.getRuntimeStatus)
      .mockResolvedValueOnce({ ok: true, data: status() })
      .mockResolvedValue({ ok: true, data: status({ phase: "listening", seq: 2 }) });

    render(<WorkspaceSession />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(document.body.textContent).toContain("聆听中"));
    expect(commands.startSession).toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "开始会话" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "停止" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("shows the barge-in switch checked by default and passes allowBargeIn true", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: configuredSession() });
    render(<WorkspaceSession />);
    const barge = (await screen.findByLabelText("允许语音打断（说话即可停止 AI 播报）")) as HTMLInputElement;
    expect(barge.checked).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await waitFor(() =>
      expect(commands.startSession).toHaveBeenCalledWith(expect.objectContaining({ allowBargeIn: true })),
    );
  });

  it("unchecking the barge-in switch sends allowBargeIn false", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: configuredSession() });
    render(<WorkspaceSession />);
    const barge = (await screen.findByLabelText("允许语音打断（说话即可停止 AI 播报）")) as HTMLInputElement;
    fireEvent.click(barge);
    expect(barge.checked).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await waitFor(() =>
      expect(commands.startSession).toHaveBeenCalledWith(expect.objectContaining({ allowBargeIn: false })),
    );
  });

  it("keeps session tools collapsed and preserves input when reopened", async () => {
    render(<WorkspaceSession />);
    await screen.findByText("未开始");
    const toggle = screen.getByText("会话工具", { selector: "summary" });
    const tools = toggle.closest("details") as HTMLDetailsElement;
    expect(tools.open).toBe(false);
    expect(screen.getByRole("button", { name: "朗读" })).not.toBeVisible();
    fireEvent.click(toggle);
    expect(tools.open).toBe(true);
    fireEvent.change(screen.getByLabelText("朗读文本"), { target: { value: "保留朗读草稿" } });
    fireEvent.change(screen.getByLabelText("纠正内容"), { target: { value: "保留纠正草稿" } });
    fireEvent.click(toggle);
    expect(screen.getByRole("button", { name: "纠正" })).not.toBeVisible();
    fireEvent.click(toggle);
    expect((screen.getByLabelText("朗读文本") as HTMLInputElement).value).toBe("保留朗读草稿");
    expect((screen.getByLabelText("纠正内容") as HTMLInputElement).value).toBe("保留纠正草稿");
    expect(commands.sessionAgentCommand).not.toHaveBeenCalled();
  });

  it("exposes a named conversation region for the full session dialogue", async () => {
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "listening", seq: 2 }),
    });
    render(<WorkspaceSession />);
    await screen.findByText("聆听中");
    const conversation = screen.getByRole("region", { name: "会话对话" });
    expect(conversation.tabIndex).toBe(0);
  });

  it("surfaces blocked preflight issues and start field errors", async () => {
    vi.mocked(commands.startSession).mockResolvedValueOnce({
      ok: true,
      data: {
        kind: "blocked",
        issues: [
          { code: "SESSION_ROUTE_REQUIRED", area: "speech", action: "open_services" },
          { code: "SESSION_ROLE_REQUIRED", area: "role", action: "open_services" },
        ],
      },
    });
    render(<WorkspaceSession />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    expect((await screen.findByRole("status")).textContent).toContain(
      "请先配置并启用语音线路。",
    );
    expect(screen.getByRole("status").textContent).toContain("请选择一个会话角色。");
    expect(screen.getByRole("link", { name: "选择或创建角色" })).toHaveAttribute("href", "/settings?category=roles");
    expect(screen.getByRole("link", { name: "配置语音线路" })).toHaveAttribute("href", "/services?category=routes");

    vi.mocked(commands.startSession).mockResolvedValueOnce({
      ok: false,
      error: {
        code: "SESSION_ALREADY_ACTIVE",
        message: "会话已在进行",
        requestId: "r1",
        field: "session",
        retryable: false,
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    expect((await screen.findByRole("status")).textContent).toContain(
      "session：SESSION_ALREADY_ACTIVE：会话已在进行",
    );
  });

  it("stops the session and returns to a completed phase", async () => {
    vi.mocked(commands.getRuntimeStatus)
      .mockResolvedValueOnce({ ok: true, data: status({ phase: "listening", seq: 2 }) })
      .mockResolvedValue({ ok: true, data: status({ phase: "completed", seq: 3 }) });

    render(<WorkspaceSession />);
    await screen.findByText("聆听中");
    fireEvent.click(screen.getByRole("button", { name: "停止" }));
    await waitFor(() => expect(commands.stopSession).toHaveBeenCalled());
    expect(document.body.textContent).toContain("已结束");
  });

  it("takeover switches 接管 / 恢复 AI / 静音 without prompting", async () => {
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "listening", seq: 2 }),
    });
    const promptSpy = vi.spyOn(window, "prompt");
    const confirmSpy = vi.spyOn(window, "confirm");

    render(<WorkspaceSession />);
    await screen.findByText("聆听中");
    fireEvent.click(screen.getByRole("button", { name: "接管" }));
    await waitFor(() => expect(commands.setSessionMode).toHaveBeenCalledWith("operator_speaking"));
    fireEvent.click(screen.getByRole("button", { name: "恢复 AI" }));
    await waitFor(() => expect(commands.setSessionMode).toHaveBeenCalledWith("ai_active"));
    fireEvent.click(screen.getByRole("button", { name: "静音" }));
    await waitFor(() => expect(commands.setSessionMode).toHaveBeenCalledWith("muted"));
    expect(promptSpy).not.toHaveBeenCalled();
    expect(confirmSpy).not.toHaveBeenCalled();
    promptSpy.mockRestore();
    confirmSpy.mockRestore();
  });

  it("default finalize hook calls session_finalize_utterance then renders reply", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: micConfig });
    vi.mocked(commands.isSessionAudioReady).mockResolvedValue({ ok: true, data: { ready: true } });
    vi.mocked(commands.finalizeSessionUtterance).mockImplementation(async () => {
      vi.mocked(commands.getSession).mockResolvedValue({ ok: true, data: detail() });
      vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
        ok: true,
        data: status({ phase: "listening", unusedMaterials: false, seq: 4 }),
      });
      return { ok: true, data: turn() };
    });

    render(<WorkspaceSession />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(commands.finalizeSessionUtterance).toHaveBeenCalledWith(""));
    expect(await screen.findByText("请介绍岗位")).toBeTruthy();
    expect(document.body.textContent).toContain("这是一个后端岗位");
    expect(document.body.textContent).not.toContain("本轮未使用资料");
  });

  it("keeps 停止 and 接管 enabled while auto-finalize is in flight", async () => {
    let release!: () => void;
    const blocked = new Promise<void>((resolve) => {
      release = resolve;
    });
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: micConfig });
    vi.mocked(commands.isSessionAudioReady).mockResolvedValue({ ok: true, data: { ready: true } });
    const finalize = vi.fn().mockImplementation(async () => {
      await blocked;
      return;
    });
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "listening", seq: 2 }),
    });

    render(<WorkspaceSession finalizeUtterance={finalize} />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(finalize).toHaveBeenCalledWith(""));
    expect((screen.getByRole("button", { name: "停止" }) as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByRole("button", { name: "接管" }) as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByRole("button", { name: "开始会话" }) as HTMLButtonElement).disabled).toBe(true);
    release();
  });

  it("keeps role and controls visible while configured options collapse without losing values", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: configuredSession() });
    render(<WorkspaceSession />);
    await screen.findByRole("combobox", { name: "角色" });
    const toggle = screen.getByRole("button", { name: "会话配置" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByLabelText("输入来源")).not.toBeVisible();
    fireEvent.click(toggle);
    expect(screen.getByLabelText("输入来源")).toBeVisible();
    fireEvent.click(toggle);
    fireEvent.click(toggle);
    expect(screen.getByRole("button", { name: "开始会话" })).toBeVisible();
  });

  it("opens missing configuration and reopens after blocked preflight", async () => {
    const config = configuredSession();
    config.speech.activeVoiceRouteId = null;
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: config });
    render(<WorkspaceSession />);
    await screen.findByRole("combobox", { name: "角色" });
    const toggle = screen.getByRole("button", { name: "会话配置" });
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(toggle);
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(toggle).toHaveAttribute("aria-expanded", "true"));
    expect(commands.startSession).not.toHaveBeenCalled();
  });

  it("reopens configuration on start failure and preserves active configuration locks", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: configuredSession() });
    vi.mocked(commands.startSession).mockResolvedValueOnce({ ok: false, error: { code: "REALTIME_UNAUTHORIZED", message: "unauthorized", retryable: false, requestId: "test" } });
    render(<WorkspaceSession />);
    await screen.findByRole("combobox", { name: "角色" });
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await screen.findByText(/实时语音鉴权失败/);
    expect(screen.getByRole("button", { name: "会话配置" })).toHaveAttribute("aria-expanded", "true");
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({ ok: true, data: status({ phase: "listening", seq: 2 }) });
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "停止" })).toBeEnabled());
    expect(screen.getByLabelText("角色")).toBeDisabled();
    expect(screen.getByLabelText("输入来源")).toBeDisabled();
  });

  it("shows 本轮未使用资料 after finalize when materials_used is false", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: micConfig });
    vi.mocked(commands.isSessionAudioReady).mockResolvedValue({ ok: true, data: { ready: true } });
    vi.mocked(commands.finalizeSessionUtterance).mockImplementation(async () => {
      vi.mocked(commands.getSession).mockResolvedValue({
        ok: true,
        data: detail({ turns: [turn({ materialsUsed: false })] }),
      });
      vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
        ok: true,
        data: status({ phase: "listening", unusedMaterials: true, seq: 5 }),
      });
      return { ok: true, data: turn({ materialsUsed: false }) };
    });

    render(<WorkspaceSession />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(commands.finalizeSessionUtterance).toHaveBeenCalledWith(""));
    expect(await screen.findByText("本轮未使用资料")).toBeTruthy();
  });

  it("clears previous turn text when a new session starts", async () => {
    vi.mocked(commands.getSession).mockResolvedValue({ ok: true, data: detail() });
    vi.mocked(commands.getRuntimeStatus)
      .mockResolvedValueOnce({ ok: true, data: status() })
      .mockResolvedValue({ ok: true, data: status({ phase: "listening", seq: 2 }) });

    render(<WorkspaceSession />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(document.body.textContent).toContain("这是一个后端岗位"));

    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "completed", seq: 5 }),
    });
    fireEvent.click(screen.getByRole("button", { name: "停止" }));
    await waitFor(() => expect(document.body.textContent).toContain("已结束"));

    vi.mocked(commands.getSession).mockResolvedValue({
      ok: true,
      data: { session: summary({ id: "sess-2" }), turns: [] },
    });
    vi.mocked(commands.startSession).mockResolvedValue({
      ok: true,
      data: { kind: "started", session: summary({ id: "sess-2" }) },
    });
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
    await waitFor(() => {
      expect(commands.startSession).toHaveBeenCalledTimes(2);
      expect(document.body.textContent).not.toContain("转写");
      expect(document.body.textContent).not.toContain("这是一个后端岗位");
    });
  });

  it("keeps previous rounds visible and shows each turn exactly once", async () => {
    const listeners: {
      transcript?: (payload: SessionTranscriptEvent) => void;
      reply?: (payload: SessionReplyEvent) => void;
    } = {};
    const listen = vi.fn(async (event: string, handler: (payload: never) => void) => {
      if (event === "session.transcript.v1") {
        listeners.transcript = handler as (payload: SessionTranscriptEvent) => void;
      }
      if (event === "session.reply.v1") listeners.reply = handler as (payload: SessionReplyEvent) => void;
      return () => {};
    });
    let storedTurns = [
      turn({ id: "turn-1", turnIndex: 0, userText: "现在什么时间？", assistantText: "现在是下午三点。", materialsUsed: false }),
      turn({ id: "turn-2", turnIndex: 1, userText: "我第一句话问你的是什么？", assistantText: "你问我第一句话是什么。", materialsUsed: false }),
    ];
    vi.mocked(commands.getSession).mockImplementation(async () => ({
      ok: true,
      data: detail({ turns: storedTurns }),
    }));

    render(<WorkspaceSession listen={listen} />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    await waitFor(() => expect(screen.getByText("现在什么时间？")).toBeTruthy());
    expect(screen.getByText("我第一句话问你的是什么？")).toBeTruthy();

    storedTurns = [
      ...storedTurns,
      turn({ id: "turn-3", turnIndex: 2, userText: "第三轮问题", assistantText: "第三轮回答", materialsUsed: false }),
    ];
    await act(async () => {
      listeners.transcript?.({ seq: 4, text: "第三轮问题" });
      listeners.reply?.({ seq: 5, text: "第三轮回答" });
    });
    await waitFor(() => expect(screen.getByText("第三轮回答")).toBeTruthy());
    await waitFor(() => expect(commands.getSession).toHaveBeenCalledTimes(3));
    expect(screen.getAllByText("第三轮回答")).toHaveLength(1);
    expect(screen.getByText("现在什么时间？")).toBeTruthy();
    expect(screen.getByText("我第一句话问你的是什么？")).toBeTruthy();
  });

  it("applies live events and drops stale seq per topic", async () => {
    const listeners: {
      status?: (payload: RuntimeStatus) => void;
      transcript?: (payload: SessionTranscriptEvent) => void;
      reply?: (payload: SessionReplyEvent) => void;
    } = {};
    const listen = vi.fn(async (event: string, handler: (payload: never) => void) => {
      if (event === "runtime.status.v1") listeners.status = handler as (payload: RuntimeStatus) => void;
      if (event === "session.transcript.v1") {
        listeners.transcript = handler as (payload: SessionTranscriptEvent) => void;
      }
      if (event === "session.reply.v1") listeners.reply = handler as (payload: SessionReplyEvent) => void;
      return () => {};
    });

    render(<WorkspaceSession listen={listen} />);
    await waitFor(() => expect(listen).toHaveBeenCalledWith("session.reply.v1", expect.any(Function)));

    act(() => {
      listeners.status?.({
        phase: "thinking",
        mode: "ai_active",
        seq: 3,
        unusedMaterials: false,
        lastErrorCode: null,
        revision: 0,
      });
    });
    expect(document.body.textContent).toContain("思考中");
    act(() => {
      listeners.status?.({
        phase: "idle",
        mode: "ai_active",
        seq: 2,
        unusedMaterials: false,
        lastErrorCode: null,
        revision: 0,
      });
    });
    expect(document.body.textContent).toContain("思考中");
    expect(document.body.textContent).not.toMatch(/未开始/);

    act(() => {
      listeners.transcript?.({ seq: 4, text: "新转写" });
    });
    expect(document.body.textContent).toContain("新转写");
    act(() => {
      listeners.transcript?.({ seq: 1, text: "旧转写" });
    });
    expect(document.body.textContent).toContain("新转写");
    expect(document.body.textContent).not.toContain("旧转写");

    act(() => {
      listeners.reply?.({ seq: 5, text: "新回复" });
    });
    expect(document.body.textContent).toContain("新回复");
    act(() => {
      listeners.reply?.({ seq: 2, text: "旧回复" });
    });
    expect(document.body.textContent).toContain("新回复");
    expect(document.body.textContent).not.toContain("旧回复");
  });

  it("shows IPC failures and never uses fetch or meeting-bridge copy", async () => {
    vi.mocked(commands.startSession).mockRejectedValue(new Error("ipc"));
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    const { container } = render(<WorkspaceSession />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    expect((await screen.findByRole("status")).textContent).toContain("IPC_UNAVAILABLE：");
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(container.innerHTML).not.toMatch(/\/api\//);
    expect(container.innerHTML).not.toContain("@tauri-apps/api");
    expect(container.textContent).not.toContain("会议桥接");
    fetchSpy.mockRestore();
  });

  it("keeps command buttons disabled while the session is inactive", async () => {
    render(<WorkspaceSession />);
    fireEvent.click(screen.getByText("会话工具", { selector: "summary" }));
    expect((await screen.findByRole("button", { name: "朗读" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect((screen.getByRole("button", { name: "重试" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "纠正" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "报告" }) as HTMLButtonElement).disabled).toBe(true);
    expect(commands.sessionAgentCommand).not.toHaveBeenCalled();
  });

  it("sends say with the dedicated input and current expectedRevision", async () => {
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "listening", seq: 2, revision: 3 }),
    });
    vi.spyOn(globalThis.crypto, "randomUUID").mockReturnValue("00000000-0000-0000-0000-000000000001");

    render(<WorkspaceSession />);
    fireEvent.click(screen.getByText("会话工具", { selector: "summary" }));
    await screen.findByText("聆听中");
    fireEvent.change(screen.getByLabelText("朗读文本"), { target: { value: "请开始自我介绍" } });
    fireEvent.click(screen.getByRole("button", { name: "朗读" }));
    await waitFor(() =>
      expect(commands.sessionAgentCommand).toHaveBeenCalledWith({
        id: "00000000-0000-0000-0000-000000000001",
        action: "say",
        text: "请开始自我介绍",
        answer: null,
        mode: null,
        expectedRevision: 3,
      }),
    );
    vi.mocked(globalThis.crypto.randomUUID).mockRestore();
  });

  it("hold-to-talk enters operator mode only while the control is held", async () => {
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "listening", seq: 2 }),
    });
    render(<WorkspaceSession />);
    await screen.findByText("聆听中");
    const hold = screen.getByRole("button", { name: "按住人工发言" });
    fireEvent.pointerDown(hold);
    await waitFor(() => expect(commands.setSessionMode).toHaveBeenCalledWith("operator_speaking"));
    fireEvent.pointerUp(hold);
    await waitFor(() => expect(commands.setSessionMode).toHaveBeenCalledWith("ai_active"));
  });

  it("lets a candidate edit and explicitly confirm only the current suggestion", async () => {
    vi.mocked(commands.getRuntimeStatus)
      .mockResolvedValueOnce({ ok: true, data: status() })
      .mockResolvedValue({ ok: true, data: status({ phase: "listening", seq: 2, revision: 7 }) });
    vi.mocked(commands.getSession).mockResolvedValue({
      ok: true,
      data: detail({
        turns: [turn({
          assistantText: "建议原稿",
          userConfirmed: false,
          playbackStatus: "pending_confirmation",
        })],
      }),
    });
    vi.spyOn(globalThis.crypto, "randomUUID").mockReturnValue("00000000-0000-0000-0000-000000000009");

    render(<WorkspaceSession />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    const answer = await screen.findByLabelText("确认播报内容");
    expect(answer).toHaveValue("建议原稿");
    fireEvent.change(answer, { target: { value: "编辑后的回答" } });
    fireEvent.click(screen.getByRole("button", { name: "确认并播报" }));

    await waitFor(() => expect(commands.sessionAgentCommand).toHaveBeenCalledWith({
      id: "00000000-0000-0000-0000-000000000009",
      action: "confirm_candidate",
      text: "编辑后的回答",
      answer: null,
      mode: null,
      expectedRevision: 7,
    }));
    vi.mocked(globalThis.crypto.randomUUID).mockRestore();
  });

  it("sends retry, correct, and report with the expected fields", async () => {
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "listening", seq: 2, revision: 4 }),
    });
    vi.mocked(commands.sessionAgentCommand)
      .mockResolvedValueOnce({ ok: true, data: commandResult({ action: "retry" }) })
      .mockResolvedValueOnce({ ok: true, data: commandResult({ action: "correct" }) })
      .mockResolvedValueOnce({
        ok: true,
        data: commandResult({
          action: "report",
          result: {
            summary: "本轮表现稳定",
            report: { summary: "本轮表现稳定", strengths: ["表达清晰"] },
          },
        }),
      });
    const uuid = vi.spyOn(globalThis.crypto, "randomUUID");
    uuid
      .mockReturnValueOnce("00000000-0000-0000-0000-000000000001")
      .mockReturnValueOnce("00000000-0000-0000-0000-000000000002")
      .mockReturnValueOnce("00000000-0000-0000-0000-000000000003");

    render(<WorkspaceSession />);
    fireEvent.click(screen.getByText("会话工具", { selector: "summary" }));
    await screen.findByText("聆听中");
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() =>
      expect(commands.sessionAgentCommand).toHaveBeenCalledWith({
        id: "00000000-0000-0000-0000-000000000001",
        action: "retry",
        text: null,
        answer: null,
        mode: null,
        expectedRevision: 4,
      }),
    );

    fireEvent.change(screen.getByLabelText("纠正内容"), { target: { value: "改成这句" } });
    fireEvent.click(screen.getByRole("button", { name: "纠正" }));
    await waitFor(() =>
      expect(commands.sessionAgentCommand).toHaveBeenCalledWith({
        id: "00000000-0000-0000-0000-000000000002",
        action: "correct",
        text: null,
        answer: "改成这句",
        mode: null,
        expectedRevision: 4,
      }),
    );

    fireEvent.click(screen.getByRole("button", { name: "报告" }));
    await waitFor(() =>
      expect(commands.sessionAgentCommand).toHaveBeenCalledWith({
        id: "00000000-0000-0000-0000-000000000003",
        action: "report",
        text: null,
        answer: null,
        mode: null,
        expectedRevision: 4,
      }),
    );
    expect(screen.getByText(/本轮表现稳定/).textContent).toContain("本轮表现稳定");
    expect(document.body.textContent).toContain("表达清晰");
    uuid.mockRestore();
  });

  it("shows SESSION_CHANGED when the command result is not ok", async () => {
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "listening", seq: 2, revision: 1 }),
    });
    vi.mocked(commands.sessionAgentCommand).mockResolvedValue({
      ok: true,
      data: commandResult({ action: "retry", ok: false, error: "SESSION_CHANGED" }),
    });

    render(<WorkspaceSession />);
    fireEvent.click(screen.getByText("会话工具", { selector: "summary" }));
    await screen.findByText("聆听中");
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    expect((await screen.findByRole("status")).textContent).toContain("SESSION_CHANGED");
    expect(screen.getByRole("status").textContent).not.toContain("pcm");
  });

  it("keeps 停止 and 接管 enabled while a command is in flight", async () => {
    let release!: () => void;
    const blocked = new Promise<void>((resolve) => {
      release = resolve;
    });
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({
      ok: true,
      data: status({ phase: "listening", seq: 2, revision: 2 }),
    });
    vi.mocked(commands.sessionAgentCommand).mockImplementation(async () => {
      await blocked;
      return { ok: true, data: commandResult({ action: "retry" }) };
    });

    render(<WorkspaceSession />);
    fireEvent.click(screen.getByText("会话工具", { selector: "summary" }));
    await screen.findByText("聆听中");
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => {
      expect((screen.getByRole("button", { name: "朗读" }) as HTMLButtonElement).disabled).toBe(true);
      expect((screen.getByRole("button", { name: "重试" }) as HTMLButtonElement).disabled).toBe(true);
    });
    expect((screen.getByRole("button", { name: "停止" }) as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByRole("button", { name: "接管" }) as HTMLButtonElement).disabled).toBe(false);
    release();
    await waitFor(() => expect(commands.sessionAgentCommand).toHaveBeenCalled());
  });

  const meetingConfig: PublicConfig = {
    configVersion: 1, application: { locale: null }, models: { providers: [], activeProviderId: null },
    speech: { activeVoiceRouteId: "route-1", voiceRoutes: [{ id: "route-1", name: "测试线路", mode: "cascaded", asrProviderId: null, asrModelId: null, llmProviderId: null, llmModelId: null, ttsProviderId: null, ttsModelId: null, voiceId: null, e2eProviderId: null, e2eModelId: null, active: true, ready: true, status: null, configVersion: 1 }] },
    knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null }, storage: { exportDirectory: null },
    roleProfiles: [{ id: "role-1", name: "会议助手", systemPrompt: "listen", openingMessage: "", styleInstructions: "", scenario: "meetingAssistant", active: true, configVersion: 1 }],
    activeRoleProfileId: "role-1", diagnostics: { logRetentionDays: 14 },
  };

  async function openMeetingSource() {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: meetingConfig });
    vi.mocked(commands.listMeetingProcesses).mockResolvedValue({ ok: true, data: [{ pid: 101, name: "zoom.exe", title: "Meeting" }] });
    render(<WorkspaceSession />);
    await screen.findByLabelText("输入来源");
    if (screen.getByRole("button", { name: "会话配置" }).getAttribute("aria-expanded") === "false") fireEvent.click(screen.getByRole("button", { name: "会话配置" }));
    fireEvent.change(screen.getByLabelText("输入来源"), { target: { value: "meeting" } });
    await screen.findByRole("button", { name: "重新检测虚拟声卡" });
  }

  it("offers install only when the virtual audio device is missing", async () => {
    vi.mocked(commands.getVirtualAudioStatus).mockResolvedValue({
      ok: true,
      data: { state: "missing", installed: false, rebootRequired: false, detail: "未检测到", renderEndpointId: null, captureEndpointId: null },
    });
    await openMeetingSource();
    expect(screen.getByText(/检测到缺少虚拟声卡，是否安装/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "是，自动安装" })).toBeTruthy();
  });

  it("does not offer install for disabled, incomplete or driver-present devices", async () => {
    for (const state of ["disabled", "incomplete", "driver_present"] as const) {
      cleanup();
      vi.mocked(commands.getVirtualAudioStatus).mockResolvedValue({
        ok: true,
        data: { state, installed: false, rebootRequired: false, detail: `${state} 详情`, renderEndpointId: null, captureEndpointId: null },
      });
      await openMeetingSource();
      expect(screen.queryByText(/检测到缺少虚拟声卡，是否安装/)).toBeNull();
      expect(screen.queryByRole("button", { name: "是，自动安装" })).toBeNull();
      expect(screen.getByText(`${state} 详情`)).toBeTruthy();
    }
  });

  it("asks the user to reboot when the driver requires it and does not auto-restart", async () => {
    vi.mocked(commands.getVirtualAudioStatus).mockResolvedValue({
      ok: true,
      data: { state: "reboot_required", installed: false, rebootRequired: true, detail: "需要重启", renderEndpointId: null, captureEndpointId: null },
    });
    await openMeetingSource();
    expect(screen.getByText(/需要重启 Windows 后继续/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "是，自动安装" })).toBeNull();
    expect(screen.getByText(/软件不会自动重启电脑/)).toBeTruthy();
  });

  it("blocks repeat install after timeout until the user rechecks", async () => {
    vi.mocked(commands.getVirtualAudioStatus).mockResolvedValue({
      ok: true,
      data: { state: "missing", installed: false, rebootRequired: false, detail: "未检测到", renderEndpointId: null, captureEndpointId: null },
    });
    vi.mocked(commands.installVirtualAudio).mockResolvedValue({
      ok: false,
      error: { code: "PREREQUISITE_TIMEOUT", message: "安装等待超时，提权任务可能仍在运行。请先检查状态，不要重复安装。", retryable: false, requestId: "t1" },
    });
    await openMeetingSource();
    fireEvent.click(screen.getByRole("button", { name: "是，自动安装" }));
    expect(await screen.findByText(/请先重新检测/)).toBeTruthy();
    expect((screen.getByRole("button", { name: "是，自动安装" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "是，自动安装" }));
    expect(commands.installVirtualAudio).toHaveBeenCalledTimes(1);
    vi.mocked(commands.getVirtualAudioStatus).mockResolvedValue({
      ok: true,
      data: { state: "missing", installed: false, rebootRequired: false, detail: "仍缺失", renderEndpointId: null, captureEndpointId: null },
    });
    fireEvent.click(screen.getByRole("button", { name: "重新检测虚拟声卡" }));
    await waitFor(() => expect((screen.getByRole("button", { name: "是，自动安装" }) as HTMLButtonElement).disabled).toBe(false));
  });

  it("shows UAC cancellation and busy states without offering a missing-device install", async () => {
    vi.mocked(commands.getVirtualAudioStatus).mockResolvedValue({
      ok: true,
      data: { state: "missing", installed: false, rebootRequired: false, detail: "未检测到", renderEndpointId: null, captureEndpointId: null },
    });
    vi.mocked(commands.installVirtualAudio).mockResolvedValue({
      ok: false,
      error: { code: "PREREQUISITE_UAC_CANCELLED", message: "已取消管理员授权，尚未完成安装。可以重新点击安装。", retryable: true, requestId: "uac" },
    });
    await openMeetingSource();
    fireEvent.click(screen.getByRole("button", { name: "是，自动安装" }));
    expect(await screen.findByText(/已取消管理员授权/)).toBeTruthy();
    cleanup();
    vi.mocked(commands.getVirtualAudioStatus).mockResolvedValue({
      ok: true,
      data: { state: "installing", installed: false, rebootRequired: false, detail: "提权安装任务仍在运行", renderEndpointId: null, captureEndpointId: null },
    });
    await openMeetingSource();
    expect(screen.getByText(/提权安装任务仍在运行/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "是，自动安装" })).toBeNull();
  });

  it("unsubscribes the preparation listener on unmount", async () => {
    const unlisten = vi.fn();
    const listen = vi.fn(async () => unlisten);
    const { unmount } = render(<WorkspaceSession listen={listen} />);
    await waitFor(() => expect(listen).toHaveBeenCalled());
    unmount();
    expect(unlisten).toHaveBeenCalled();
  });

  it("hides candidate confirmation after takeover even if turn metadata is still pending", async () => {
    vi.mocked(commands.getRuntimeStatus)
      .mockResolvedValueOnce({ ok: true, data: status() })
      .mockResolvedValueOnce({ ok: true, data: status({ phase: "listening", seq: 2, revision: 7 }) })
      .mockResolvedValue({ ok: true, data: status({ phase: "listening", mode: "operator_speaking", seq: 3, revision: 7 }) });
    vi.mocked(commands.getSession).mockResolvedValue({
      ok: true,
      data: detail({
        turns: [turn({
          assistantText: "建议原稿",
          userConfirmed: false,
          playbackStatus: "pending_confirmation",
        })],
      }),
    });
    vi.mocked(commands.setSessionMode).mockResolvedValue({
      ok: true,
      data: status({ mode: "operator_speaking", phase: "listening", seq: 3 }),
    });
    render(<WorkspaceSession />);
    fireEvent.click(await screen.findByRole("button", { name: "开始会话" }));
    expect(await screen.findByLabelText("确认播报内容")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "接管" }));
    await waitFor(() => expect(screen.queryByLabelText("确认播报内容")).toBeNull());
  });

  const micConfig: PublicConfig = {
    configVersion: 1, application: { locale: null }, models: { providers: [], activeProviderId: null },
    speech: { activeVoiceRouteId: "route-1", voiceRoutes: [{ id: "route-1", name: "测试线路", mode: "cascaded", asrProviderId: null, asrModelId: null, llmProviderId: null, llmModelId: null, ttsProviderId: null, ttsModelId: null, voiceId: null, e2eProviderId: null, e2eModelId: null, active: true, ready: true, status: null, configVersion: 1 }] },
    knowledge: { embeddingConfigs: [], activeEmbeddingConfigId: null }, storage: { exportDirectory: null },
    roleProfiles: [{ id: "role-1", name: "面试官", systemPrompt: "listen", openingMessage: "", styleInstructions: "", scenario: "interviewer", active: true, configVersion: 1 }],
    activeRoleProfileId: "role-1", diagnostics: { logRetentionDays: 14 },
  };

  function micStreamerFactory() {
    const start = vi.fn(async () => {});
    const stop = vi.fn();
    let onChunk: ((pcm: string, sampleRate: number) => void) | undefined;
    const factory = vi.fn((callbacks: { onChunk: (pcm: string, sampleRate: number) => void }) => {
      onChunk = callbacks.onChunk;
      return { start, stop };
    });
    return { factory, start, stop, speak: (pcm: string, rate = 48_000) => onChunk?.(pcm, rate) };
  }

  async function startMicSession(factory: (callbacks: never) => unknown, runtime: Partial<RuntimeStatus> = {}) {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: micConfig });
    vi.mocked(commands.getRuntimeStatus).mockResolvedValue({ ok: true, data: status({ phase: "listening", seq: 2, ...runtime }) });
    render(<WorkspaceSession createMicStreamer={factory as never} />);
    await screen.findByLabelText("输入来源");
    if (screen.getByRole("button", { name: "会话配置" }).getAttribute("aria-expanded") === "false") fireEvent.click(screen.getByRole("button", { name: "会话配置" }));
    fireEvent.change(screen.getByLabelText("输入来源"), { target: { value: "mic" } });
    fireEvent.click(screen.getByRole("button", { name: "开始会话" }));
  }

  it("offers a local-microphone input source for personal rehearsal", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: micConfig });
    render(<WorkspaceSession />);
    await screen.findByLabelText("输入来源");
    if (screen.getByRole("button", { name: "会话配置" }).getAttribute("aria-expanded") === "false") fireEvent.click(screen.getByRole("button", { name: "会话配置" }));
    expect(screen.getByRole("option", { name: "本机麦克风" })).toBeTruthy();
  });

  it("streams microphone chunks into the session and auto-finalizes on pause", async () => {
    vi.mocked(commands.isSessionAudioReady).mockResolvedValue({ ok: true, data: { ready: true } });
    const streamer = micStreamerFactory();
    await startMicSession(streamer.factory as never);
    await waitFor(() => expect(streamer.start).toHaveBeenCalled());
    await act(async () => streamer.speak("QUJD"));
    await waitFor(() => expect(commands.pushMicPcm).toHaveBeenCalledWith("QUJD", 48_000));
    await waitFor(() => expect(commands.finalizeSessionUtterance).toHaveBeenCalledWith(""));
  });

  it("drops microphone chunks while the operator has taken over", async () => {
    const streamer = micStreamerFactory();
    await startMicSession(streamer.factory as never, { mode: "operator_speaking" });
    await waitFor(() => expect(streamer.start).toHaveBeenCalled());
    await screen.findByText("人工接管");
    await act(async () => streamer.speak("QUJD"));
    await act(async () => {});
    expect(commands.pushMicPcm).not.toHaveBeenCalled();
  });

  it("stops the microphone streamer when the session stops", async () => {
    const streamer = micStreamerFactory();
    await startMicSession(streamer.factory as never);
    await waitFor(() => expect(streamer.start).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "停止" }));
    await waitFor(() => expect(streamer.stop).toHaveBeenCalled());
  });

  it("loads audio output devices automatically for text and mic sessions", async () => {
    vi.mocked(commands.getConfigPublic).mockResolvedValue({ ok: true, data: micConfig });
    render(<WorkspaceSession />);
    await screen.findByLabelText("输入来源");
    await waitFor(() => expect(commands.listAudioOutputs).toHaveBeenCalled());
    if (screen.getByRole("button", { name: "会话配置" }).getAttribute("aria-expanded") === "false") fireEvent.click(screen.getByRole("button", { name: "会话配置" }));
    await screen.findByRole("option", { name: "扬声器" });
  });

  it("surfaces the underlying error when auto transcription fails", async () => {
    vi.mocked(commands.isSessionAudioReady).mockResolvedValue({ ok: true, data: { ready: true } });
    const streamer = micStreamerFactory();
    await startMicSession(streamer.factory as never);
    await waitFor(() => expect(streamer.start).toHaveBeenCalled());
    vi.mocked(commands.finalizeSessionUtterance).mockRejectedValue(
      new Error("REALTIME_REMOTE_ERROR：服务器拒绝了音频数据"),
    );
    await waitFor(() => expect(commands.finalizeSessionUtterance).toHaveBeenCalledWith(""));
    expect(await screen.findByText(/REALTIME_REMOTE_ERROR/)).toBeTruthy();
    expect(screen.queryByText("自动转写失败，已保留会话，可重试或人工接管。")).toBeNull();
  });

  it("explains a realtime permission failure instead of the raw provider code", async () => {
    vi.mocked(commands.isSessionAudioReady).mockResolvedValue({ ok: true, data: { ready: true } });
    const streamer = micStreamerFactory();
    await startMicSession(streamer.factory as never);
    await waitFor(() => expect(streamer.start).toHaveBeenCalled());
    vi.mocked(commands.finalizeSessionUtterance).mockRejectedValue(
      new Error("downstream_reconnect_exceeded：实时语音服务拒绝了本次请求，请检查模型配置或稍后重试。"),
    );
    await waitFor(() => expect(commands.finalizeSessionUtterance).toHaveBeenCalledWith(""));
    expect(await screen.findByText(/未开通.*语音模型权限/)).toBeTruthy();
    expect(screen.queryByText(/downstream_reconnect_exceeded/)).toBeNull();
  });

  it("pauses auto transcription while the operator has taken over", async () => {
    vi.mocked(commands.isSessionAudioReady).mockResolvedValue({ ok: true, data: { ready: true } });
    const streamer = micStreamerFactory();
    await startMicSession(streamer.factory as never, { mode: "operator_speaking" });
    await waitFor(() => expect(streamer.start).toHaveBeenCalled());
    await screen.findByText("人工接管");
    // 等过 250ms 轮询周期，确认接管期间不会自动提交转写。
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 400)); });
    expect(commands.finalizeSessionUtterance).not.toHaveBeenCalled();
  });
});
