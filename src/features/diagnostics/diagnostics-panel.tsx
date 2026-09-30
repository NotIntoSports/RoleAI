import { useCallback, useEffect, useMemo, useRef, useState } from "react";

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

const MODE_LABELS: Record<string, string> = {
  realtime: "实时",
  cascade: "级联",
};

function modeLabel(mode: string): string {
  return MODE_LABELS[mode] ?? mode;
}

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

interface SparklineProps {
  samples: Array<{ totalMs: number }>;
}

/** 最近 N 轮总延迟折线：纯 SVG polyline，与瀑布条同一「不引入图表库」方案。 */
export function LatencySparkline({ samples }: SparklineProps) {
  if (samples.length < 2) return null;
  const width = 240;
  const height = 48;
  const max = Math.max(...samples.map((sample) => sample.totalMs));
  const min = Math.min(...samples.map((sample) => sample.totalMs));
  const span = Math.max(max - min, 1);
  const points = samples
    .map((sample, index) => {
      const x = (index / (samples.length - 1)) * width;
      const y = height - 2 - ((sample.totalMs - min) / span) * (height - 4);
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
  return (
    <svg
      className="latency-sparkline"
      width={width}
      height={height}
      role="img"
      aria-label={`最近 ${samples.length} 轮总延迟折线，范围 ${Math.round(min)}～${Math.round(max)} 毫秒`}
      viewBox={`0 0 ${width} ${height}`}
    >
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

export function DiagnosticsPanel({ loadSummary }: DiagnosticsPanelProps) {
  const [state, setState] = useState<PanelState>({ kind: "loading" });
  const [selectedEntry, setSelectedEntry] = useState(0);
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

  const routes = state.kind === "ready" ? state.data.routes : [];
  const entry = routes[selectedEntry] ?? routes[0];
  // 选中线路的最近轮总延迟样本（折线从旧到新：接口最新在前，展示时反转）。
  const recentSamples = useMemo(() => {
    if (state.kind !== "ready" || !entry) return [];
    return state.data.recentTurns
      .filter((sample) => sample.routeId === entry.routeId && sample.mode === entry.mode)
      .filter((sample) => sample.totalMs !== null)
      .slice(0, 50)
      .map((sample) => ({ totalMs: sample.totalMs as number }))
      .reverse();
  }, [state, entry]);

  const busy = state.kind === "loading";
  return (
    <section className="service-panel diagnostics-panel" aria-labelledby="diagnostics-panel-heading">
      <h3 className="section-heading" id="diagnostics-panel-heading">性能面板</h3>
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
      {state.kind === "ready" && state.data.routes.length > 0 && entry && (
        <>
          <table className="diagnostics-table">
            <thead>
              <tr>
                <th scope="col">线路</th>
                <th scope="col">模式</th>
                <th scope="col">样本数</th>
                <th scope="col">首响 P50（毫秒）</th>
                <th scope="col">首响 P95（毫秒）</th>
                <th scope="col">累计丢帧</th>
              </tr>
            </thead>
            <tbody>
              {state.data.routes.map((route, index) => (
                <tr key={`${route.routeId}-${route.mode}`}>
                  <td>{route.routeLabel}</td>
                  <td>{modeLabel(route.mode)}</td>
                  <td>{route.samples}</td>
                  <td>{route.p50Ms ?? "—"}</td>
                  <td>{route.p95Ms ?? "—"}</td>
                  <td>{route.ingressDroppedTotal}</td>
                  <td>
                    <button
                      type="button"
                      className="button-ghost"
                      aria-pressed={index === selectedEntry}
                      onClick={() => setSelectedEntry(index)}
                    >
                      {index === selectedEntry ? "查看中" : "查看"}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <h4 className="section-subheading">分阶段延迟（{entry.routeLabel} · {modeLabel(entry.mode)}）</h4>
          {entry.stages.length === 0 ? (
            <p className="muted">该线路暂无分阶段时间线数据（旧记录或纯文本轮）。</p>
          ) : (
            <table className="diagnostics-table" aria-label="分阶段延迟百分位">
              <thead>
                <tr>
                  <th scope="col">阶段</th>
                  <th scope="col">样本数</th>
                  <th scope="col">P50（毫秒）</th>
                  <th scope="col">P95（毫秒）</th>
                </tr>
              </thead>
              <tbody>
                {entry.stages.map((stage) => (
                  <tr key={stage.stage}>
                    <td>{stage.stage}</td>
                    <td>{stage.samples}</td>
                    <td>{stage.p50Ms ?? "—"}</td>
                    <td>{stage.p95Ms ?? "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <h4 className="section-subheading">最近 50 轮总延迟（{entry.routeLabel} · {modeLabel(entry.mode)}）</h4>
          {recentSamples.length >= 2 ? (
            <LatencySparkline samples={recentSamples} />
          ) : (
            <p className="muted">该线路暂无足够的最近轮样本（至少 2 轮才画折线）。</p>
          )}
        </>
      )}
      <button type="button" disabled={busy} onClick={load}>刷新</button>
    </section>
  );
}
