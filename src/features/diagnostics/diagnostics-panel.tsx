import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { EmptyState } from "../../components/empty-state";
import { ErrorNotice } from "../../components/error-notice";
import { t, useT } from "../../i18n";
import {
  setLatencyWaterfallEnabled,
  useLatencyWaterfallPreference,
} from "../session/latency-preference";
import type { DiagnosticsLatencySummary, RouteLatencySummary } from "../../generated/bindings";

export type { DiagnosticsLatencySummary, RouteLatencySummary };

// 面板每次拉取的会话扫描上限；与后端 getDiagnosticsLatencySummary 的 limit 参数对应，联调时可调整。
const DEFAULT_LIMIT = 20;

const MODE_LABEL_KEYS: Record<string, "diagnostics.modeRealtime" | "diagnostics.modeCascade"> = {
  realtime: "diagnostics.modeRealtime",
  cascade: "diagnostics.modeCascade",
};

function modeLabel(mode: string): string {
  const key = MODE_LABEL_KEYS[mode];
  return key ? t(key) : mode;
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
      aria-label={t("diagnostics.sparklineAria", { n: samples.length, min: Math.round(min), max: Math.round(max) })}
      viewBox={`0 0 ${width} ${height}`}
    >
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

export function DiagnosticsPanel({ loadSummary }: DiagnosticsPanelProps) {
  useT();
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
      <h3 className="section-heading" id="diagnostics-panel-heading">{t("diagnostics.heading")}</h3>
      <label className="diagnostics-waterfall-toggle">
        <input
          type="checkbox"
          checked={latencyWaterfallEnabled}
          onChange={(event) => setLatencyWaterfallEnabled(event.target.checked)}
        />
        {t("diagnostics.waterfallToggle")}
      </label>
      {state.kind === "ready" && (
        <p className="muted">{t("diagnostics.scannedSessions", { n: state.data.sessionsScanned })}</p>
      )}
      {busy && <p className="services-message" role="status">{t("diagnostics.reading")}</p>}
      {state.kind === "error" && <ErrorNotice error={{ code: state.code, message: state.message }} />}
      {state.kind === "ready" && state.data.routes.length === 0 && (
        <EmptyState
          title={t("diagnostics.emptyTitle")}
          hint={t("diagnostics.emptyHint")}
        />
      )}
      {state.kind === "ready" && state.data.routes.length > 0 && entry && (
        <>
          <table className="diagnostics-table">
            <thead>
              <tr>
                <th scope="col">{t("diagnostics.colRoute")}</th>
                <th scope="col">{t("diagnostics.colMode")}</th>
                <th scope="col">{t("diagnostics.colSamples")}</th>
                <th scope="col">{t("diagnostics.colFirstP50")}</th>
                <th scope="col">{t("diagnostics.colFirstP95")}</th>
                <th scope="col">{t("diagnostics.colDropped")}</th>
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
                      {index === selectedEntry ? t("diagnostics.viewing") : t("diagnostics.view")}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <h4 className="section-subheading">{t("diagnostics.stagesHeading", { route: entry.routeLabel, mode: modeLabel(entry.mode) })}</h4>
          {entry.stages.length === 0 ? (
            <p className="muted">{t("diagnostics.stagesEmpty")}</p>
          ) : (
            <table className="diagnostics-table" aria-label={t("diagnostics.stagesTableAria")}>
              <thead>
                <tr>
                  <th scope="col">{t("diagnostics.colStage")}</th>
                  <th scope="col">{t("diagnostics.colSamples")}</th>
                  <th scope="col">{t("diagnostics.colP50")}</th>
                  <th scope="col">{t("diagnostics.colP95")}</th>
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
          <h4 className="section-subheading">{t("diagnostics.recentHeading", { route: entry.routeLabel, mode: modeLabel(entry.mode) })}</h4>
          {recentSamples.length >= 2 ? (
            <LatencySparkline samples={recentSamples} />
          ) : (
            <p className="muted">{t("diagnostics.recentEmpty")}</p>
          )}
        </>
      )}
      <button type="button" disabled={busy} onClick={load}>{t("diagnostics.refresh")}</button>
    </section>
  );
}
