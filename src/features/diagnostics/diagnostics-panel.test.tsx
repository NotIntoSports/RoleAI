import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it, vi } from "vitest";

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
  routes: [
    { routeId: "route-1", routeLabel: "阿里云实时", samples: 8, p50Ms: 320, p95Ms: 780, ingressDroppedTotal: 3 },
    { routeId: "route-2", routeLabel: "级联线路", samples: 4, p50Ms: null, p95Ms: null, ingressDroppedTotal: 0 },
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
    expect(screen.getByRole("cell", { name: "320" })).toBeTruthy();
    expect(screen.getByRole("cell", { name: "780" })).toBeTruthy();
    expect(screen.getByRole("cell", { name: "8" })).toBeTruthy();
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
      data: { sessionsScanned: 0, routes: [] },
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
