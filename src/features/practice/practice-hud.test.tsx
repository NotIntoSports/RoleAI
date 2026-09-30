import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { AnswerMetrics, PracticeProgress, SessionTurnView } from "../../generated/bindings";
import { PracticeHud } from "./practice-hud";

vi.mock("../../api/commands", () => ({
  getPracticeSessionProgress: vi.fn(),
  getPracticeSessionTurnMetrics: vi.fn(),
  skipPracticeQuestion: vi.fn(),
  generatePracticeReport: vi.fn(),
}));

function progress(overrides: Partial<PracticeProgress> = {}): PracticeProgress {
  return {
    planId: "plan-1",
    questionIndex: 0,
    totalQuestions: 3,
    followupsUsed: 0,
    followupLimit: 2,
    finished: false,
    ...overrides,
  };
}

function answerMetrics(overrides: Partial<AnswerMetrics> = {}): AnswerMetrics {
  return {
    answerIndex: 0,
    durationSeconds: 60,
    chineseChars: 300,
    englishWords: 0,
    speechRate: { chinesePerMinute: 240, englishPerMinute: 0 },
    fillers: [{ word: "嗯", count: 1 }],
    structureSignals: [],
    starCoverage: [],
    ...overrides,
  };
}

function turn(overrides: Partial<SessionTurnView> = {}): SessionTurnView {
  return {
    id: "turn-1",
    turnIndex: 0,
    userText: "回答",
    assistantText: "追问",
    materialsUsed: false,
    citations: [],
    createdAt: "2026-10-01T00:00:00Z",
    ...overrides,
  };
}

function okProgress() {
  return { ok: true as const, data: progress() };
}

describe("PracticeHud", () => {
  beforeEach(() => {
    vi.mocked(commands.getPracticeSessionProgress).mockResolvedValue(okProgress());
    vi.mocked(commands.getPracticeSessionTurnMetrics).mockResolvedValue({ ok: true, data: null });
    vi.mocked(commands.skipPracticeQuestion).mockResolvedValue({ ok: true, data: progress() });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("renders nothing before a session exists", () => {
    const { container } = render(<PracticeHud sessionId={null} active={false} turns={[]} />);
    expect(container).toBeEmptyDOMElement();
    expect(commands.getPracticeSessionProgress).not.toHaveBeenCalled();
  });

  it("hides itself for non-practice sessions (progress probe fails)", async () => {
    vi.mocked(commands.getPracticeSessionProgress).mockResolvedValue({
      ok: false,
      error: { code: "PRACTICE_SESSION_STATE_INVALID", message: "该会话不是模拟面试训练", requestId: "r", retryable: false },
    });
    const { container } = render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    await waitFor(() => expect(commands.getPracticeSessionProgress).toHaveBeenCalledTimes(1));
    expect(container).toBeEmptyDOMElement();
    expect(screen.queryByRole("region", { name: "模拟面试训练进度" })).toBeNull();
  });

  it("survives a probe rejection (IPC unavailable) and stays hidden", async () => {
    vi.mocked(commands.getPracticeSessionProgress).mockRejectedValue(new Error("ipc"));
    const { container } = render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    await waitFor(() => expect(commands.getPracticeSessionProgress).toHaveBeenCalledTimes(1));
    expect(container).toBeEmptyDOMElement();
  });

  it("shows question position and hides skip controls when finished", async () => {
    vi.mocked(commands.getPracticeSessionProgress).mockResolvedValue({
      ok: true,
      data: progress({ questionIndex: 2, totalQuestions: 3, finished: true }),
    });
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    const hud = await screen.findByRole("region", { name: "模拟面试训练进度" });
    expect(hud.textContent).toContain("全部 3 题已完成");
    expect(screen.queryByRole("button", { name: "跳过本题" })).toBeNull();
    expect(screen.queryByRole("timer")).toBeNull();
  });

  it("generates the training report from the finished-state entry", async () => {
    vi.mocked(commands.getPracticeSessionProgress).mockResolvedValue({
      ok: true,
      data: progress({ questionIndex: 1, totalQuestions: 2, finished: true }),
    });
    vi.mocked(commands.generatePracticeReport).mockResolvedValue({
      ok: true,
      data: {
        sessionId: "sess-1", planId: "plan-1", position: "后端", interviewerStyle: "面试官",
        llmAvailable: true, totalScore: 4, dimensions: { contentDepth: 4, structureClarity: 4, fluency: 4, jobFit: 4 },
        perQuestion: [], topSuggestions: [],
        objective: { answers: [], totalDurationSeconds: null, averageAnswerSeconds: null, longPauses: null, topFillers: [] },
        createdAt: "2026-10-02T08:00:00Z",
      },
    });
    const onNotify = vi.fn();
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} onNotify={onNotify} />);
    fireEvent.click(await screen.findByRole("button", { name: "生成训练报告" }));
    await waitFor(() => expect(commands.generatePracticeReport).toHaveBeenCalledWith("sess-1"));
    await waitFor(() => expect(onNotify).toHaveBeenCalledWith(expect.stringContaining("训练报告已生成")));
  });

  it("surfaces report generation failure through onNotify", async () => {
    vi.mocked(commands.getPracticeSessionProgress).mockResolvedValue({
      ok: true,
      data: progress({ questionIndex: 1, totalQuestions: 2, finished: true }),
    });
    vi.mocked(commands.generatePracticeReport).mockResolvedValue({
      ok: false,
      error: { code: "PRACTICE_SESSION_STATE_INVALID", message: "状态无效", requestId: "r", retryable: false },
    });
    const onNotify = vi.fn();
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} onNotify={onNotify} />);
    fireEvent.click(await screen.findByRole("button", { name: "生成训练报告" }));
    await waitFor(() => expect(onNotify).toHaveBeenCalledWith("报告生成失败：PRACTICE_SESSION_STATE_INVALID"));
  });

  it("shows per-question position, question timer, and skip button", async () => {
    // shouldAdvanceTime 让 waitFor/findBy 的内部定时器照常推进，同时保留手动拨表。
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      vi.mocked(commands.getPracticeSessionTurnMetrics).mockResolvedValue({ ok: true, data: null });
      render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
      const hud = await screen.findByRole("region", { name: "模拟面试训练进度" });
      expect(hud.textContent).toContain("第 1 / 3 题");
      expect(screen.getByRole("timer").textContent).toContain("本题 00:00");
      await act(async () => {
        vi.advanceTimersByTime(65000);
      });
      expect(screen.getByRole("timer").textContent).toContain("本题 01:05");
      expect(screen.getByRole("button", { name: "跳过本题" })).not.toBeDisabled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("pauses the question timer when the session is not active", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      render(<PracticeHud sessionId="sess-1" active={false} turns={[turn()]} />);
      await screen.findByRole("region", { name: "模拟面试训练进度" });
      expect(screen.getByRole("timer").textContent).toContain("本题 00:00");
      await act(async () => {
        vi.advanceTimersByTime(30000);
      });
      expect(screen.getByRole("timer").textContent).toContain("本题 00:00");
    } finally {
      vi.useRealTimers();
    }
  });

  it("warns about fast chinese speech from the latest answer metrics", async () => {
    vi.mocked(commands.getPracticeSessionTurnMetrics).mockResolvedValue({
      ok: true,
      data: answerMetrics({ speechRate: { chinesePerMinute: 340, englishPerMinute: 0 } }),
    });
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    const hud = await screen.findByRole("region", { name: "模拟面试训练进度" });
    await waitFor(() => expect(hud.textContent).toContain("语速过快"));
    expect(hud.textContent).toContain("340 字/分钟");
  });

  it("warns about fast english speech", async () => {
    vi.mocked(commands.getPracticeSessionTurnMetrics).mockResolvedValue({
      ok: true,
      data: answerMetrics({ speechRate: { chinesePerMinute: 0, englishPerMinute: 200 } }),
    });
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    const hud = await screen.findByRole("region", { name: "模拟面试训练进度" });
    await waitFor(() => expect(hud.textContent).toContain("200 词/分钟"));
  });

  it("does not warn when speech rate is within the normal band", async () => {
    vi.mocked(commands.getPracticeSessionTurnMetrics).mockResolvedValue({
      ok: true,
      data: answerMetrics({ speechRate: { chinesePerMinute: 240, englishPerMinute: 0 } }),
    });
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    const hud = await screen.findByRole("region", { name: "模拟面试训练进度" });
    await waitFor(() => expect(commands.getPracticeSessionTurnMetrics).toHaveBeenCalledTimes(1));
    expect(hud.textContent).not.toContain("语速过快");
    expect(hud.textContent).not.toContain("口头禅较多");
  });

  it("warns when fillers add up across the latest answer", async () => {
    vi.mocked(commands.getPracticeSessionTurnMetrics).mockResolvedValue({
      ok: true,
      data: answerMetrics({
        fillers: [{ word: "嗯", count: 3 }, { word: "那个", count: 2 }],
      }),
    });
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    const hud = await screen.findByRole("region", { name: "模拟面试训练进度" });
    await waitFor(() => expect(hud.textContent).toContain("口头禅较多"));
    expect(hud.textContent).toContain("共 5 次");
  });

  it("skips the current question and adopts the returned progress", async () => {
    vi.mocked(commands.skipPracticeQuestion).mockResolvedValue({
      ok: true,
      data: progress({ questionIndex: 1, followupsUsed: 0 }),
    });
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    await screen.findByRole("region", { name: "模拟面试训练进度" });
    fireEvent.click(screen.getByRole("button", { name: "跳过本题" }));
    await waitFor(() => expect(commands.skipPracticeQuestion).toHaveBeenCalledWith("sess-1"));
    await waitFor(() => expect(screen.getByRole("region", { name: "模拟面试训练进度" }).textContent).toContain("第 2 / 3 题"));
  });

  it("surfaces a skip failure through onNotify", async () => {
    vi.mocked(commands.skipPracticeQuestion).mockResolvedValue({
      ok: false,
      error: { code: "PRACTICE_SESSION_STATE_INVALID", message: "状态无效", requestId: "r", retryable: false },
    });
    const onNotify = vi.fn();
    render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} onNotify={onNotify} />);
    await screen.findByRole("region", { name: "模拟面试训练进度" });
    fireEvent.click(screen.getByRole("button", { name: "跳过本题" }));
    await waitFor(() => expect(onNotify).toHaveBeenCalledWith("跳过失败：PRACTICE_SESSION_STATE_INVALID"));
  });

  it("refetches progress and metrics when a new turn lands", async () => {
    const { rerender } = render(<PracticeHud sessionId="sess-1" active={true} turns={[turn()]} />);
    await waitFor(() => expect(commands.getPracticeSessionProgress).toHaveBeenCalledTimes(1));
    expect(commands.getPracticeSessionTurnMetrics).toHaveBeenCalledTimes(1);
    rerender(<PracticeHud sessionId="sess-1" active={true} turns={[turn({ id: "turn-2" })]} />);
    await waitFor(() => expect(commands.getPracticeSessionProgress).toHaveBeenCalledTimes(2));
    expect(commands.getPracticeSessionTurnMetrics).toHaveBeenCalledTimes(2);
  });
});
