import { cleanup, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { PracticeReportSummary } from "../../generated/bindings";
import { PracticePage } from "./practice-page";

vi.mock("../../api/commands", () => ({
  listMaterials: vi.fn(),
  listPracticeReports: vi.fn(),
  getPracticeReport: vi.fn(),
}));

function summary(overrides: Partial<PracticeReportSummary> = {}): PracticeReportSummary {
  return {
    sessionId: "sess-r1",
    planId: "plan-1",
    position: "后端开发工程师",
    interviewerStyle: "严苛面试官",
    totalScore: 3.5,
    createdAt: "2026-10-02T08:00:00Z",
    ...overrides,
  };
}

const FULL_REPORT = {
  sessionId: "sess-r1",
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
      answer: "我负责支付网关。",
      score: 4,
      strengths: ["结构清晰"],
      issues: [],
      modelAnswer: "补充量化。",
    },
  ],
  topSuggestions: ["补充量化结果"],
  objective: {
    answers: [],
    totalDurationSeconds: null,
    averageAnswerSeconds: null,
    longPauses: null,
    topFillers: [],
  },
  createdAt: "2026-10-02T08:00:00Z",
};

describe("PracticePage 报告区", () => {
  beforeEach(() => {
    vi.mocked(commands.listMaterials).mockResolvedValue({ ok: true, data: [] });
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("shows the latest report when one exists", async () => {
    vi.mocked(commands.listPracticeReports).mockResolvedValue({ ok: true, data: [summary()] });
    vi.mocked(commands.getPracticeReport).mockResolvedValue({ ok: true, data: FULL_REPORT });
    render(<PracticePage />);
    expect(screen.getByText("正在读取训练报告…")).toBeTruthy();
    const detail = await screen.findByRole("article", { name: "训练报告详情" });
    expect(detail.textContent).toContain("后端开发工程师");
    await waitFor(() => expect(commands.getPracticeReport).toHaveBeenCalledWith("sess-r1"));
  });

  it("shows an empty state when no reports exist", async () => {
    vi.mocked(commands.listPracticeReports).mockResolvedValue({ ok: true, data: [] });
    render(<PracticePage />);
    expect(await screen.findByText("还没有训练报告。")).toBeTruthy();
    expect(commands.getPracticeReport).not.toHaveBeenCalled();
  });

  it("shows an empty state when the report list fails", async () => {
    vi.mocked(commands.listPracticeReports).mockResolvedValue({
      ok: false,
      error: { code: "DATABASE_OPERATION_FAILED", message: "数据库不可用", requestId: "r", retryable: true },
    });
    render(<PracticePage />);
    expect(await screen.findByText("还没有训练报告。")).toBeTruthy();
  });
});
