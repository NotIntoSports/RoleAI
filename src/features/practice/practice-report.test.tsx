import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { PracticeReport } from "../../generated/bindings";
import { PracticeRadarChart, PracticeReportView } from "./practice-report";

vi.mock("../../api/commands", () => ({
  getPracticeReport: vi.fn(),
  generatePracticeReport: vi.fn(),
  exportPracticeReport: vi.fn(),
}));

function report(overrides: Partial<PracticeReport> = {}): PracticeReport {
  return {
    sessionId: "sess-1",
    planId: "plan-1",
    position: "后端开发工程师",
    interviewerStyle: "严苛面试官",
    llmAvailable: true,
    totalScore: 3.5,
    dimensions: { contentDepth: 4, structureClarity: 3.5, fluency: 4, jobFit: 3 },
    perQuestion: [
      {
        index: 1,
        question: "介绍一个你负责的项目。",
        answer: "我负责支付网关，用令牌桶限流，p99 下降 30%。",
        score: 4,
        strengths: ["结构清晰"],
        issues: ["缺少量化"],
        modelAnswer: "补充 p99 数字与压测环境。",
      },
      {
        index: 2,
        question: "如何设计限流？",
        answer: "先限后熔。",
        score: 0,
        strengths: [],
        issues: [],
        modelAnswer: "",
      },
    ],
    topSuggestions: ["补充量化结果", "先讲结论", "控制语速"],
    objective: {
      answers: [
        {
          answerIndex: 0,
          durationSeconds: 120,
          chineseChars: 480,
          englishWords: 2,
          speechRate: { chinesePerMinute: 240, englishPerMinute: 1 },
          fillers: [{ word: "那个", count: 2 }],
          structureSignals: [],
          starCoverage: [],
        },
        {
          answerIndex: 1,
          durationSeconds: 60,
          chineseChars: 120,
          englishWords: 0,
          speechRate: { chinesePerMinute: 120, englishPerMinute: 0 },
          fillers: [{ word: "嗯", count: 1 }],
          structureSignals: [],
          starCoverage: [],
        },
      ],
      totalDurationSeconds: 180,
      averageAnswerSeconds: 90,
      longPauses: 1,
      topFillers: [{ word: "那个", count: 2 }, { word: "嗯", count: 1 }],
    },
    createdAt: "2026-10-02T08:00:00Z",
    ...overrides,
  };
}

describe("PracticeReportView", () => {
  beforeEach(() => {
    vi.mocked(commands.getPracticeReport).mockResolvedValue({ ok: true, data: report() });
    vi.mocked(commands.generatePracticeReport).mockResolvedValue({ ok: true, data: report() });
    vi.mocked(commands.exportPracticeReport).mockResolvedValue({
      ok: true,
      data: { path: "D:\\exports\\report.md" },
    });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("shows loading, then the full report with total score and disclaimer", async () => {
    render(<PracticeReportView sessionId="sess-1" />);
    expect(screen.getByRole("status").textContent).toContain("正在读取训练报告");
    const detail = await screen.findByRole("article", { name: "训练报告详情" });
    expect(detail.textContent).toContain("后端开发工程师 · 严苛面试官");
    expect(detail.textContent).toContain("总分 3.5 / 5");
    expect(detail.textContent).toContain("评分由 AI 生成，仅供练习参考");
    expect(detail.textContent).toContain("补充量化结果");
    expect(commands.getPracticeReport).toHaveBeenCalledWith("sess-1");
  });

  it("renders radar with four dimension labels and per-question cards", async () => {
    render(<PracticeReportView sessionId="sess-1" />);
    const radar = await screen.findByRole("img", { name: /维度雷达图/ });
    expect(radar.getAttribute("aria-label")).toContain("内容深度 4");
    expect(radar.getAttribute("aria-label")).toContain("岗位匹配 3");
    const questions = screen.getByRole("region", { name: "逐题点评" });
    expect(questions.textContent).toContain("介绍一个你负责的项目。");
    expect(questions.textContent).toContain("4 / 5");
    expect(questions.textContent).toContain("未评");
    expect(questions.textContent).toContain("优点：结构清晰");
    expect(questions.textContent).toContain("改进示范：补充 p99 数字与压测环境。");
  });

  it("expands the raw transcript of a question", async () => {
    render(<PracticeReportView sessionId="sess-1" />);
    const summaries = await screen.findAllByText("我的回答（原始转写）");
    const details = summaries[0].closest("details") as HTMLDetailsElement;
    expect(details.open).toBe(false);
    fireEvent.click(summaries[0]);
    expect(details.open).toBe(true);
  });

  it("shows objective metrics card with aggregated rate, fillers and durations", async () => {
    render(<PracticeReportView sessionId="sess-1" />);
    const card = await screen.findByRole("article", { name: "训练报告详情" });
    expect(card.textContent).toContain("约 180 字/分钟");
    expect(card.textContent).toContain("那个 ×2、嗯 ×1");
    expect(card.textContent).toContain("总 03:00 · 平均每次 01:30");
    expect(card.textContent).toContain("回答间长停顿：1 次");
  });

  it("offers generation when the report is missing", async () => {
    vi.mocked(commands.getPracticeReport).mockResolvedValue({
      ok: false,
      error: { code: "PRACTICE_REPORT_NOT_FOUND", message: "训练报告不存在", requestId: "r", retryable: false },
    });
    render(<PracticeReportView sessionId="sess-1" />);
    fireEvent.click(await screen.findByRole("button", { name: "生成训练报告" }));
    await waitFor(() => expect(commands.generatePracticeReport).toHaveBeenCalledWith("sess-1"));
    expect(await screen.findByRole("article", { name: "训练报告详情" })).toBeTruthy();
  });

  it("offers retry when the LLM review failed and marks the objective-only state", async () => {
    vi.mocked(commands.getPracticeReport).mockResolvedValue({ ok: true, data: report({ llmAvailable: false }) });
    render(<PracticeReportView sessionId="sess-1" />);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("定性点评生成失败，可重试");
    fireEvent.click(screen.getByRole("button", { name: /重试生成定性点评/ }));
    await waitFor(() => expect(commands.generatePracticeReport).toHaveBeenCalledWith("sess-1"));
  });

  it("exports the report as Markdown and surfaces the path", async () => {
    render(<PracticeReportView sessionId="sess-1" />);
    fireEvent.click(await screen.findByRole("button", { name: "导出 Markdown" }));
    await waitFor(() => expect(commands.exportPracticeReport).toHaveBeenCalledWith("sess-1", "markdown"));
    expect((await screen.findByRole("status")).textContent).toContain("D:\\exports\\report.md");
  });

  it("surfaces a load error other than missing report", async () => {
    vi.mocked(commands.getPracticeReport).mockResolvedValue({
      ok: false,
      error: { code: "DATABASE_OPERATION_FAILED", message: "数据库不可用", requestId: "r", retryable: true },
    });
    render(<PracticeReportView sessionId="sess-1" />);
    expect(await screen.findByText(/DATABASE_OPERATION_FAILED/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "生成训练报告" })).toBeNull();
  });
});

describe("PracticeRadarChart", () => {
  it("clamps out-of-range scores into 0..5", () => {
    render(
      <PracticeRadarChart dimensions={{ contentDepth: 9, structureClarity: -2, fluency: 3, jobFit: 5 }} />,
    );
    const radar = screen.getByRole("img", { name: /维度雷达图/ });
    expect(radar.getAttribute("aria-label")).toContain("内容深度 5");
    expect(radar.getAttribute("aria-label")).toContain("结构清晰 0");
    expect(radar.getAttribute("aria-label")).toContain("表达流畅 3");
    expect(radar.getAttribute("aria-label")).toContain("岗位匹配 5");
  });
});
