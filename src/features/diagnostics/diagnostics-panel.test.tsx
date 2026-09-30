import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it, vi } from "vitest";

import { LATENCY_WATERFALL_STORAGE_KEY } from "../session/latency-preference";
import type { DiagnosticsLatencySummary } from "./diagnostics-panel";
import { DiagnosticsPanel } from "./diagnostics-panel";

type LoadResult =
  | { ok: true; data: DiagnosticsLatencySummary }
  | { ok: false; error: { code: string; message: string } };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

const summary: DiagnosticsLatencySummary = {
  sessionsScanned: 12,
  recentTurns: [
    { routeId: "route-1", mode: "realtime", totalMs: 780, createdAt: "2026-09-30T09:59:00Z" },
    { routeId: "route-1", mode: "realtime", totalMs: 700, createdAt: "2026-09-30T09:58:00Z" },
    { routeId: "route-1", mode: "realtime", totalMs: 640, createdAt: "2026-09-30T09:57:00Z" },
    { routeId: "route-2", mode: "cascade", totalMs: 1_900, createdAt: "2026-09-30T09:56:00Z" },
    { routeId: "route-2", mode: "cascade", totalMs: null, createdAt: "2026-09-30T09:55:00Z" },
  ],
  routes: [
    {
      routeId: "route-1",
      routeLabel: "阿里云实时",
      mode: "realtime",
      samples: 8,
      p50Ms: 320,
      p95Ms: 780,
      stages: [
        { stage: "firstAudioMs", samples: 8, p50Ms: 320, p95Ms: 780 },
        { stage: "responseDoneMs", samples: 7, p50Ms: 2_100, p95Ms: 3_400 },
      ],
      ingressDroppedTotal: 3,
    },
    {
      routeId: "route-2",
      routeLabel: "级联线路",
      mode: "cascade",
      samples: 4,
      p50Ms: null,
      p95Ms: null,
      stages: [],
      ingressDroppedTotal: 0,
    },
  ],
};

describe("DiagnosticsPanel", () => {
  afterEach(cleanup);

  it("loads the latency summary on mount and renders the route table", async () => {
    const loadSummary = vi.fn(async (): Promise<LoadResult> => ({ ok: true, data: summary }));
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    expect(await screen.findByText("阿里云实时")).toBeTruthy();
    expect(loadSummary).toHaveBeenCalledWith(20);
    expect(screen.getByText("已扫描会话：12")).toBeTruthy();
    // 首响 p50/p95 同时出现在线路表与阶段表：断言至少各出现一次。
    expect(screen.getAllByRole("cell", { name: "320" }).length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByRole("cell", { name: "780" }).length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByRole("cell", { name: "8" }).length).toBeGreaterThanOrEqual(2);
    expect(screen.getByRole("cell", { name: "3" })).toBeTruthy();
  });

  it("renders — for null percentiles", async () => {
    const loadSummary = vi.fn(async (): Promise<LoadResult> => ({ ok: true, data: summary }));
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    const dashes = await screen.findAllByText("—");
    expect(dashes).toHaveLength(2);
  });

  it("renders the empty state when no routes have samples", async () => {
    const loadSummary = vi.fn(async (): Promise<LoadResult> => ({
      ok: true,
      data: { sessionsScanned: 0, routes: [], recentTurns: [] },
    }));
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    expect(await screen.findByText("暂无线路延迟样本。")).toBeTruthy();
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("renders load failures with the shared code：message format", async () => {
    const loadSummary = vi.fn(async (): Promise<LoadResult> => ({
      ok: false,
      error: { code: "DIAGNOSTICS_FAILED", message: "原始信息" },
    }));
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toBe("DIAGNOSTICS_FAILED：原始信息"),
    );
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("disables 刷新 while loading and re-loads on click", async () => {
    const first = deferred<Extract<LoadResult, { ok: true }>>();
    const loadSummary = vi.fn(() => first.promise);
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    await waitFor(() =>
      expect(screen.getByRole("status").textContent).toContain("正在读取延迟统计"),
    );
    const refresh = screen.getByRole("button", { name: "刷新" }) as HTMLButtonElement;
    expect(refresh.disabled).toBe(true);

    first.resolve({ ok: true, data: summary });
    await waitFor(() => expect(refresh.disabled).toBe(false));
    expect(screen.getByText("阿里云实时")).toBeTruthy();

    const second = deferred<Extract<LoadResult, { ok: true }>>();
    loadSummary.mockReturnValue(second.promise);
    fireEvent.click(refresh);
    expect(loadSummary).toHaveBeenCalledTimes(2);
    expect((screen.getByRole("button", { name: "刷新" }) as HTMLButtonElement).disabled).toBe(true);
    second.resolve({ ok: true, data: summary });
    await waitFor(() => expect((screen.getByRole("button", { name: "刷新" }) as HTMLButtonElement).disabled).toBe(false));
  });
});

describe("DiagnosticsPanel 瀑布条开关", () => {
  afterEach(cleanup);

  it("勾选开关写入显示偏好", async () => {
    const loadSummary = vi.fn(async (): Promise<LoadResult> => ({ ok: true, data: summary }));
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    const toggle = await screen.findByRole("checkbox", { name: "在对话中显示每轮延迟瀑布条" });
    expect((toggle as HTMLInputElement).checked).toBe(true);
    fireEvent.click(toggle);
    expect((toggle as HTMLInputElement).checked).toBe(false);
    expect(window.localStorage.getItem(LATENCY_WATERFALL_STORAGE_KEY)).toBe("off");
  });
});

describe("DiagnosticsPanel 性能面板", () => {
  afterEach(cleanup);

  it("默认展示第一条线路的分阶段百分位与最近轮折线", async () => {
    const loadSummary = vi.fn(async (): Promise<LoadResult> => ({ ok: true, data: summary }));
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    expect(await screen.findByText("分阶段延迟（阿里云实时 · 实时）")).toBeTruthy();
    expect(screen.getByRole("table", { name: "分阶段延迟百分位" })).toBeTruthy();
    expect(screen.getByRole("cell", { name: "responseDoneMs" })).toBeTruthy();
    const sparkline = screen.getByRole("img", { name: /最近 3 轮总延迟折线/ });
    expect(sparkline.querySelector("polyline")).toBeTruthy();
    expect(sparkline.getAttribute("aria-label")).toContain("640～780");
  });

  it("切换线路后阶段表与折线跟随所选线路", async () => {
    const loadSummary = vi.fn(async (): Promise<LoadResult> => ({ ok: true, data: summary }));
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    await screen.findByText("分阶段延迟（阿里云实时 · 实时）");
    fireEvent.click(screen.getByRole("button", { name: "查看" }));
    // 级联线路没有分阶段数据与足够样本：显示占位说明。
    expect(screen.getByText("分阶段延迟（级联线路 · 级联）")).toBeTruthy();
    expect(screen.getByText(/该线路暂无分阶段时间线数据/)).toBeTruthy();
    expect(screen.getByText(/暂无足够的最近轮样本/)).toBeTruthy();
    expect(screen.queryByRole("img", { name: /总延迟折线/ })).toBeNull();
  });

  it("样本不足 2 轮时不画折线", async () => {
    const thin: DiagnosticsLatencySummary = {
      ...summary,
      recentTurns: [{ routeId: "route-1", mode: "realtime", totalMs: 500, createdAt: "t1" }],
    };
    const loadSummary = vi.fn(async (): Promise<LoadResult> => ({ ok: true, data: thin }));
    render(<DiagnosticsPanel loadSummary={loadSummary} />);
    await screen.findByText("分阶段延迟（阿里云实时 · 实时）");
    expect(screen.getByText(/暂无足够的最近轮样本/)).toBeTruthy();
  });
});
