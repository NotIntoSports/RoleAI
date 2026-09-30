import { useCallback, useEffect, useRef, useState } from "react";

import { EmptyState } from "../../components/empty-state";
import { ErrorNotice } from "../../components/error-notice";
import {
  setLatencyWaterfallEnabled,
  useLatencyWaterfallPreference,
} from "../session/latency-preference";
import type { DiagnosticsLatencySummary, RouteLatencySummary } from "../../generated/bindings";

export type { DiagnosticsLatencySummary, RouteLatencySummary };

// 面板每次拉取的会话扫描上限；与后端 getDiagnosticsLatencySummary 的 limit 参数对应，联调时可调整。
const DEFAULT_LIMIT = 20;

type PanelState =
  | { kind: "loading" }
  | { kind: "ready"; data: DiagnosticsLatencySummary }
  | { kind: "error"; code: string; message: string };

interface DiagnosticsPanelProps {
  loadSummary: (limit: number) => Promise<
    { ok: true; data: DiagnosticsLatencySummary } | { ok: false; error: { code: string; message: string } }
  >;
  /** 后端导出命令；导出按钮待联调后再实现，本卡不渲染任何导出入口。 */
  exportDiagnostics?: (destination: string) => Promise<unknown>;
}

export function DiagnosticsPanel({ loadSummary }: DiagnosticsPanelProps) {
  const [state, setState] = useState<PanelState>({ kind: "loading" });
  const epochRef = useRef(0);
  const latencyWaterfallEnabled = useLatencyWaterfallPreference();

  const load = useCallback(() => {
    const epoch = ++epochRef.current;
    setState({ kind: "loading" });
    void loadSummary(DEFAULT_LIMIT).then((result) => {
      if (epoch !== epochRef.current) return;
      if (result.ok) setState({ kind: "ready", data: result.data });
      else setState({ kind: "error", code: result.error.code, message: result.error.message });
    });
  }, [loadSummary]);

  useEffect(() => {
    load();
  }, [load]);

  const busy = state.kind === "loading";
  return (
    <section className="service-panel diagnostics-panel" aria-labelledby="diagnostics-panel-heading">
      <h3 className="section-heading" id="diagnostics-panel-heading">诊断延迟</h3>
      <label className="diagnostics-waterfall-toggle">
        <input
          type="checkbox"
          checked={latencyWaterfallEnabled}
          onChange={(event) => setLatencyWaterfallEnabled(event.target.checked)}
        />
        在对话中显示每轮延迟瀑布条
      </label>
      {state.kind === "ready" && (
        <p className="muted">已扫描会话：{state.data.sessionsScanned}</p>
      )}
      {busy && <p className="services-message" role="status">正在读取延迟统计…</p>}
      {state.kind === "error" && <ErrorNotice error={{ code: state.code, message: state.message }} />}
      {state.kind === "ready" && state.data.routes.length === 0 && (
        <EmptyState
          title="暂无线路延迟样本。"
          hint="开始语音会话后，这里会显示各语音线路的延迟与丢帧统计。"
        />
      )}
      {state.kind === "ready" && state.data.routes.length > 0 && (
        <table className="diagnostics-table">
          <thead>
            <tr>
              <th scope="col">线路</th>
              <th scope="col">样本数</th>
              <th scope="col">P50（毫秒）</th>
              <th scope="col">P95（毫秒）</th>
              <th scope="col">累计丢帧</th>
            </tr>
          </thead>
          <tbody>
            {state.data.routes.map((route) => (
              <tr key={route.routeId}>
                <td>{route.routeLabel}</td>
                <td>{route.samples}</td>
                <td>{route.p50Ms ?? "—"}</td>
                <td>{route.p95Ms ?? "—"}</td>
                <td>{route.ingressDroppedTotal}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <button type="button" disabled={busy} onClick={load}>刷新</button>
    </section>
  );
}
