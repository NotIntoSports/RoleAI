// 轮次延迟瀑布条：一条水平堆叠条显示各阶段耗时占比，点击展开明细表。
// 纯 CSS/flex 实现，不引入图表库（与设置页性能面板同一方案）。
import { useState } from "react";

import { t, useT, type DictionaryStringKey } from "../../i18n";
import type { TurnLatencyView } from "../../generated/bindings";

interface StageAnchor {
  field: string;
  label: DictionaryStringKey;
  color: string;
}

// 各模式的锚点顺序（时间线字段声明序）。条形图显示相邻锚点之间的耗时段。
const REALTIME_ANCHORS: StageAnchor[] = [
  { field: "speechStartedMs", label: "session.latency.heardSpeech", color: "#8fb4d9" },
  { field: "speechStoppedMs", label: "session.latency.speechStopped", color: "#5f8fc4" },
  { field: "transcriptDoneMs", label: "session.latency.transcriptDone", color: "#7fbf8e" },
  { field: "responseCreatedMs", label: "session.latency.requestSent", color: "#d9b25f" },
  { field: "firstAudioMs", label: "session.latency.firstAudio", color: "#d98a5f" },
  { field: "responseDoneMs", label: "session.latency.responseDone", color: "#b78fd9" },
];

const CASCADE_ANCHORS: StageAnchor[] = [
  { field: "asrDoneMs", label: "session.latency.transcriptDone", color: "#5f8fc4" },
  { field: "retrievalDoneMs", label: "session.latency.retrieval", color: "#8fceb0" },
  { field: "llmFirstTokenMs", label: "session.latency.firstToken", color: "#d9b25f" },
  { field: "llmDoneMs", label: "session.latency.llmDone", color: "#e0a04f" },
  { field: "ttsDoneMs", label: "session.latency.ttsDone", color: "#d98a5f" },
];

export interface WaterfallSegment {
  label: string;
  ms: number;
  color: string;
}

function anchorValue(latency: TurnLatencyView, field: string): number | null {
  const value = (latency.timeline as unknown as Record<string, unknown>)[field];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** 相邻锚点之间的耗时段；锚点缺失或乱序（重连/极端负载）时跳过该段。 */
export function buildWaterfallSegments(latency: TurnLatencyView): WaterfallSegment[] {
  const anchors = latency.mode === "cascade" ? CASCADE_ANCHORS : REALTIME_ANCHORS;
  const present = anchors
    .map((anchor) => ({ anchor, value: anchorValue(latency, anchor.field) }))
    .filter((item): item is { anchor: StageAnchor; value: number } => item.value !== null);
  const segments: WaterfallSegment[] = [];
  for (let index = 1; index < present.length; index += 1) {
    const ms = present[index].value - present[index - 1].value;
    if (ms > 0) {
      segments.push({ label: t(present[index].anchor.label), ms, color: present[index].anchor.color });
    }
  }
  return segments;
}

export function formatLatencyTotal(ms: number): string {
  if (ms >= 1_000) return t("session.latency.seconds", { n: (ms / 1_000).toFixed(1) });
  return t("session.latency.millis", { n: Math.round(ms) });
}

interface LatencyWaterfallProps {
  latency: TurnLatencyView;
  turnIndex: number;
}

export function LatencyWaterfall({ latency, turnIndex }: LatencyWaterfallProps) {
  useT();
  const [open, setOpen] = useState(false);
  const segments = buildWaterfallSegments(latency);
  if (segments.length === 0) return null;
  const total = segments.reduce((sum, segment) => sum + segment.ms, 0);
  return (
    <div className="latency-waterfall">
      <button
        type="button"
        className="latency-waterfall-bar"
        aria-expanded={open}
        aria-label={t("session.latency.barAria", { n: turnIndex + 1, total: formatLatencyTotal(total), state: open ? t("session.latency.collapse") : t("session.latency.expand") })}
        onClick={() => setOpen((previous) => !previous)}
      >
        <span className="latency-waterfall-track" aria-hidden="true">
          {segments.map((segment) => (
            <span
              key={segment.label}
              className="latency-waterfall-segment"
              title={`${segment.label} ${formatLatencyTotal(segment.ms)}`}
              style={{ flexGrow: segment.ms, flexBasis: 0, background: segment.color }}
            />
          ))}
        </span>
        <span className="latency-waterfall-total">{formatLatencyTotal(total)}</span>
      </button>
      {latency.interrupted && <span className="latency-waterfall-flag">{t("session.latency.interrupted")}</span>}
      {open && (
        <table className="latency-waterfall-details">
          <caption className="sr-only">{t("session.latency.detailsCaption", { n: turnIndex + 1 })}</caption>
          <thead>
            <tr>
              <th scope="col">{t("session.latency.stageCol")}</th>
              <th scope="col">{t("session.latency.durationCol")}</th>
            </tr>
          </thead>
          <tbody>
            {segments.map((segment) => (
              <tr key={segment.label}>
                <td>
                  <span className="latency-waterfall-swatch" style={{ background: segment.color }} aria-hidden="true" />
                  {segment.label}
                </td>
                <td>{formatLatencyTotal(segment.ms)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
