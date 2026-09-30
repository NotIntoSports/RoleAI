import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PracticeReportSummary } from "../../generated/bindings";
import { PracticeGrowthChart, PracticeHistorySection } from "./practice-history";

function summary(overrides: Partial<PracticeReportSummary> = {}): PracticeReportSummary {
  return {
    sessionId: "sess-r1",
    planId: "plan-1",
    position: "后端开发工程师",
    interviewerStyle: "严苛面试官",
    totalScore: 3.5,
    dimensions: { contentDepth: 3, structureClarity: 3, fluency: 3, jobFit: 3 },
    durationSeconds: 720,
    createdAt: "2026-10-01T08:00:00Z",
    ...overrides,
  };
}

const REPORTS = [
  summary(),
  summary({
    sessionId: "sess-r2",
    position: "前端开发工程师",
    interviewerStyle: "HR",
    totalScore: 4,
    durationSeconds: null,
    createdAt: "2026-10-08T08:00:00Z",
    dimensions: { contentDepth: 4, structureClarity: 4.5, fluency: 3.5, jobFit: 4 },
  }),
];

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("PracticeHistorySection", () => {
  it("lists reports with position, score, duration and date; missing duration degrades", () => {
    const onSelect = vi.fn();
    render(<PracticeHistorySection reports={REPORTS} selectedSessionId="sess-r1" onSelect={onSelect} />);
    const items = screen.getAllByRole("button");
    expect(items).toHaveLength(2);
    expect(items[0].textContent).toContain("后端开发工程师");
    expect(items[0].textContent).toContain("3.5 / 5");
    expect(items[0].textContent).toContain("12:00");
    expect(items[0].getAttribute("data-selected")).toBe("true");
    expect(items[1].textContent).toContain("前端开发工程师");
    expect(items[1].textContent).toContain("时长不可用");
    expect(items[1].getAttribute("aria-pressed")).toBe("false");
  });

  it("selects a report from the list", () => {
    const onSelect = vi.fn();
    render(<PracticeHistorySection reports={REPORTS} selectedSessionId="sess-r1" onSelect={onSelect} />);
    fireEvent.click(screen.getAllByRole("button")[1]);
    expect(onSelect).toHaveBeenCalledWith("sess-r2");
  });

  it("draws one growth polyline per dimension over ascending time with a legend", () => {
    render(<PracticeHistorySection reports={[...REPORTS].reverse()} selectedSessionId={null} onSelect={vi.fn()} />);
    const chart = screen.getByRole("img", { name: /各维度成长曲线/ });
    // 时间升序：sess-r1（10-01）在左，sess-r2（10-08）在右。
    expect(chart.getAttribute("aria-label")).toContain("内容深度（3→4）");
    expect(chart.getAttribute("aria-label")).toContain("结构清晰（3→4.5）");
    expect(chart.querySelectorAll("polyline")).toHaveLength(4);
    expect(screen.getByText("内容深度")).toBeTruthy();
    expect(screen.getByText("岗位匹配")).toBeTruthy();
    expect(chart.textContent).toContain("2026-10-01");
    expect(chart.textContent).toContain("2026-10-08");
  });

  it("renders data points without polylines for a single report", () => {
    render(<PracticeHistorySection reports={[summary()]} selectedSessionId={null} onSelect={vi.fn()} />);
    const chart = screen.getByRole("img", { name: /各维度成长曲线/ });
    expect(chart.querySelectorAll("polyline")).toHaveLength(0);
    expect(chart.querySelectorAll("circle")).toHaveLength(4);
  });
});

describe("PracticeGrowthChart", () => {
  it("clamps out-of-range dimension scores", () => {
    const { container } = render(
      <PracticeGrowthChart
        reports={[
          summary({ dimensions: { contentDepth: 9, structureClarity: -1, fluency: 5, jobFit: 0 } }),
          summary({ sessionId: "sess-r2", createdAt: "2026-10-02T08:00:00Z" }),
        ]}
      />,
    );
    const chart = container.querySelector("svg") as SVGSVGElement;
    expect(chart.getAttribute("aria-label")).toContain("内容深度（5→3）");
    expect(chart.getAttribute("aria-label")).toContain("结构清晰（0→3）");
  });
});
