import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import { TavusCallProvider, useTavusCall, type TavusCallStartResult } from "./call-lock";
import { PREVIEW_CAPTION2_AT, PREVIEW_LISTEN_AT } from "./preview-clip";
import { TavusCall } from "./tavus-call";

const leave = vi.fn(async () => undefined);
const setLocalAudio = vi.fn();
const joinDailyRoom = vi.fn((..._args: unknown[]) => Promise.resolve({ leave, setLocalAudio }));

vi.mock("../../api/commands", () => ({
  getConfigPublic: vi.fn(),
  startTavusConversation: vi.fn(),
  endTavusConversation: vi.fn(),
  startSession: vi.fn(),
}));

vi.mock("./daily-room", () => ({
  joinDailyRoom: (...args: unknown[]) => joinDailyRoom(...(args as [])),
}));

function StartButton({ roleId, onResult }: { roleId: string; onResult?: (result: TavusCallStartResult) => void }) {
  const { startCall } = useTavusCall();
  return (
    <button type="button" onClick={() => void startCall(roleId).then((result) => onResult?.(result))}>
      发起通话
    </button>
  );
}

function MicToggle() {
  const { micEnabled, setMicEnabled } = useTavusCall();
  return (
    <button type="button" onClick={() => setMicEnabled(!micEnabled)}>
      切换麦克风
    </button>
  );
}

describe("TavusCall", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("joins a room without starting the voice session, then ends the Tavus conversation", async () => {
    vi.mocked(commands.startTavusConversation).mockResolvedValue({
      ok: true,
      data: { conversationId: "c123", conversationUrl: "https://tavus.daily.co/c123" },
    });
    vi.mocked(commands.endTavusConversation).mockResolvedValue({ ok: true, data: { ready: true } });

    render(
      <TavusCallProvider>
        <StartButton roleId="role-1" />
        <TavusCall />
      </TavusCallProvider>,
    );

    // 空闲时不渲染可见控件，媒体元素只为开始通话时接入做准备。
    expect(screen.queryByRole("button", { name: "挂断" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "发起通话" }));
    await waitFor(() => expect(commands.startTavusConversation).toHaveBeenCalledWith("role-1"));
    expect(commands.startSession).not.toHaveBeenCalled();
    expect(await screen.findByText("已接通。请用中文对话。")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "挂断" }));
    await waitFor(() => expect(commands.endTavusConversation).toHaveBeenCalledWith("c123"));
    expect(leave).toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: "挂断" })).toBeNull();
  });

  it("toggles the room microphone through the context switch", async () => {
    vi.mocked(commands.startTavusConversation).mockResolvedValue({
      ok: true,
      data: { conversationId: "c123", conversationUrl: "https://tavus.daily.co/c123" },
    });

    render(
      <TavusCallProvider>
        <StartButton roleId="role-1" />
        <MicToggle />
        <TavusCall />
      </TavusCallProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: "发起通话" }));
    expect(await screen.findByRole("button", { name: "挂断" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "切换麦克风" }));
    expect(setLocalAudio).toHaveBeenLastCalledWith(false);
    fireEvent.click(screen.getByRole("button", { name: "切换麦克风" }));
    expect(setLocalAudio).toHaveBeenLastCalledWith(true);
  });

  it("keeps the call unconnected when the microphone cannot be published", async () => {
    vi.mocked(commands.startTavusConversation).mockResolvedValue({
      ok: true,
      data: { conversationId: "c123", conversationUrl: "https://tavus.daily.co/c123" },
    });
    vi.mocked(commands.endTavusConversation).mockResolvedValue({ ok: true, data: { ready: true } });
    joinDailyRoom.mockRejectedValueOnce(
      new Error("无法打开麦克风，通话未接通。请检查系统麦克风权限后重试。"),
    );
    const onResult = vi.fn();

    render(
      <TavusCallProvider>
        <StartButton roleId="role-1" onResult={onResult} />
        <TavusCall />
      </TavusCallProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: "发起通话" }));
    await waitFor(() =>
      expect(onResult).toHaveBeenCalledWith({
        ok: false,
        message: "无法打开麦克风，通话未接通。请检查系统麦克风权限后重试。",
      }),
    );
    // 状态不显示「已接通」，挂断入口也不出现；已创建的 Tavus 会话被回滚结束。
    expect(screen.queryByText("已接通。请用中文对话。")).toBeNull();
    expect(screen.queryByRole("button", { name: "挂断" })).toBeNull();
    await waitFor(() => expect(commands.endTavusConversation).toHaveBeenCalledWith("c123"));
    expect(leave).not.toHaveBeenCalled();
  });

  it("runs a preview call without joining Daily and clears only local state on hangup", async () => {
    vi.mocked(commands.startTavusConversation).mockResolvedValue({
      ok: true,
      data: { conversationId: "demo-preview", conversationUrl: "preview:roleai" },
    });

    render(
      <TavusCallProvider>
        <StartButton roleId="role-1" />
        <TavusCall />
      </TavusCallProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: "发起通话" }));
    expect(await screen.findByText("你好，我在。你直接说就行，我看着你听。")).toBeTruthy();
    expect(screen.queryByText("需要的话，我可以接着问，也可以先把刚才的话总结给你。")).toBeNull();
    const video = screen.getByLabelText("数字人");
    expect(video.tagName).toBe("VIDEO");
    expect(video).toHaveProperty("loop", true);
    expect((video as HTMLVideoElement).src).toContain("digital-human-preview");
    expect(screen.queryByRole("img", { name: "数字人" })).toBeNull();
    expect(screen.getByText("对方说话中")).toBeTruthy();
    expect(joinDailyRoom).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "挂断" })).toBeTruthy();

    act(() => {
      (video as HTMLVideoElement).currentTime = PREVIEW_CAPTION2_AT + 0.05;
      video.dispatchEvent(new Event("timeupdate"));
    });
    expect(screen.getByText("需要的话，我可以接着问，也可以先把刚才的话总结给你。")).toBeTruthy();
    expect(screen.queryByText("你好，我在。你直接说就行，我看着你听。")).toBeNull();
    expect(screen.getByText("对方说话中")).toBeTruthy();

    act(() => {
      (video as HTMLVideoElement).currentTime = PREVIEW_LISTEN_AT + 0.05;
      video.dispatchEvent(new Event("timeupdate"));
    });
    expect(screen.getByText("聆听中")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "挂断" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "挂断" })).toBeNull());
    expect(video.getAttribute("src")).toBeNull();
    expect((video as HTMLVideoElement).loop).toBe(false);
    // 预览会话不打结束接口，也不进房间。
    expect(commands.endTavusConversation).not.toHaveBeenCalled();
    expect(leave).not.toHaveBeenCalled();
  });

  it("returns the failure to the caller when the conversation cannot start", async () => {
    vi.mocked(commands.startTavusConversation).mockResolvedValue({
      ok: false,
      error: { code: "TAVUS_KEY_MISSING", message: "请先在设置里保存 Tavus 密钥", retryable: false, requestId: "test" },
    });
    const onResult = vi.fn();

    render(
      <TavusCallProvider>
        <StartButton roleId="role-1" onResult={onResult} />
        <TavusCall />
      </TavusCallProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: "发起通话" }));
    await waitFor(() =>
      expect(onResult).toHaveBeenCalledWith({ ok: false, message: "TAVUS_KEY_MISSING：请先在设置里保存 Tavus 密钥" }),
    );
    expect(screen.queryByRole("button", { name: "挂断" })).toBeNull();
  });

  it("rejects a start while the voice session is active", async () => {
    const onResult = vi.fn();
    let releaseVoice: ((active: boolean) => void) | null = null;

    function VoiceGate() {
      const { setVoiceActive } = useTavusCall();
      releaseVoice = setVoiceActive;
      return null;
    }

    render(
      <TavusCallProvider>
        <VoiceGate />
        <StartButton roleId="role-1" onResult={onResult} />
        <TavusCall />
      </TavusCallProvider>,
    );

    act(() => releaseVoice?.(true));
    fireEvent.click(screen.getByRole("button", { name: "发起通话" }));
    await waitFor(() => expect(onResult).toHaveBeenCalledWith({ ok: false, message: "请先结束语音对练，再开始视频通话。" }));
    expect(commands.startTavusConversation).not.toHaveBeenCalled();
  });
});
