import { cleanup, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { SessionTurnView } from "../../generated/bindings";
import { TranscriptPanel } from "./transcript-panel";

function turn(overrides: Partial<SessionTurnView> = {}): SessionTurnView {
  return {
    id: "turn-1",
    turnIndex: 0,
    userText: "请介绍岗位",
    assistantText: "这是一个后端岗位",
    materialsUsed: false,
    citations: [],
    createdAt: "2026-09-05T10:00:30Z",
    ...overrides,
  };
}

function panelProps(overrides: Partial<Parameters<typeof TranscriptPanel>[0]> = {}) {
  return {
    transcript: "",
    reply: "",
    turns: [turn()],
    historyTurns: [turn()],
    roleName: "会议助手",
    welcomeRoleName: "会议助手",
    active: true,
    pendingConfirmation: false,
    confirmationText: "",
    onConfirmationTextChange: () => {},
    onConfirmCandidate: () => {},
    busy: false,
    unusedMaterials: false,
    showTranscriptOnlyNotes: false,
    showLatency: false,
    onTriggerAssistant: () => {},
    onCopy: () => {},
    onAdjustConfiguration: () => {},
    ...overrides,
  };
}

function conversationRegion() {
  return screen.getByRole("region", { name: "会话对话" }) as HTMLElement;
}

function bottomSentinel(container: HTMLElement) {
  const sentinel = container.querySelector<HTMLElement>("div[aria-hidden=\"true\"]");
  if (!sentinel) throw new Error("missing conversation bottom sentinel");
  return sentinel;
}

function setScrollMetrics(
  container: HTMLElement,
  metrics: { scrollTop: number; scrollHeight: number; clientHeight: number },
) {
  Object.defineProperty(container, "scrollTop", { configurable: true, value: metrics.scrollTop });
  Object.defineProperty(container, "scrollHeight", { configurable: true, value: metrics.scrollHeight });
  Object.defineProperty(container, "clientHeight", { configurable: true, value: metrics.clientHeight });
}

describe("TranscriptPanel 自动滚动", () => {
  afterEach(cleanup);

  it("用户滚到上方时，新 partial 不触发 scrollIntoView", () => {
    const scrollIntoView = vi.fn();
    const view = render(<TranscriptPanel {...panelProps()} />);
    const container = conversationRegion();
    const bottom = bottomSentinel(container);
    Object.defineProperty(bottom, "scrollIntoView", { configurable: true, value: scrollIntoView });
    // 距底部 1200 - 0 - 500 = 700px，超过 400px 阈值。
    setScrollMetrics(container, { scrollTop: 0, scrollHeight: 1200, clientHeight: 500 });

    view.rerender(<TranscriptPanel {...panelProps({ transcript: "新的 partial 文本" })} />);

    expect(scrollIntoView).not.toHaveBeenCalled();
  });

  it("靠近底部时，新 partial 触发 scrollIntoView 到底", () => {
    const scrollIntoView = vi.fn();
    const view = render(<TranscriptPanel {...panelProps()} />);
    const container = conversationRegion();
    const bottom = bottomSentinel(container);
    Object.defineProperty(bottom, "scrollIntoView", { configurable: true, value: scrollIntoView });
    // 距底部 1200 - 750 - 500 = -50px，在 400px 阈值以内。
    setScrollMetrics(container, { scrollTop: 750, scrollHeight: 1200, clientHeight: 500 });

    view.rerender(<TranscriptPanel {...panelProps({ transcript: "新的 partial 文本" })} />);

    expect(scrollIntoView).toHaveBeenCalledWith({ block: "end" });
  });
});

describe("TranscriptPanel 延迟瀑布条", () => {
  const timedTurn = turn({
    latency: {
      routeId: "route-1",
      mode: "cascade",
      interrupted: false,
      timeline: {
        speechStartedMs: null,
        speechStoppedMs: null,
        transcriptDoneMs: null,
        responseCreatedMs: null,
        firstAudioMs: null,
        responseDoneMs: null,
        asrDoneMs: 100,
        retrievalDoneMs: 150,
        llmFirstTokenMs: 300,
        llmDoneMs: 500,
        ttsDoneMs: 700,
        playbackStartedMs: null,
        playbackDoneMs: null,
      },
    },
  });

  afterEach(() => cleanup());

  it("showLatency 且轮次有 latency 时，AI 回复下渲染瀑布条", () => {
    render(
      <TranscriptPanel
        {...panelProps({ historyTurns: [timedTurn], turns: [timedTurn], showLatency: true })}
      />,
    );
    expect(screen.getByRole("button", { name: /第 1 轮延迟 600 毫秒/ })).toBeTruthy();
  });

  it("showLatency=false 时不渲染瀑布条", () => {
    render(
      <TranscriptPanel
        {...panelProps({ historyTurns: [timedTurn], turns: [timedTurn], showLatency: false })}
      />,
    );
    expect(screen.queryByRole("button", { name: /延迟/ })).toBeNull();
  });
});
