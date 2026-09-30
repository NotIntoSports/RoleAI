// 轮次延迟瀑布条：一条水平堆叠条显示各阶段耗时占比，点击展开明细表。
// 纯 CSS/flex 实现，不引入图表库（与设置页性能面板同一方案）。
import { useState } from "react";

import type { TurnLatencyView } from "../../generated/bindings";

interface StageAnchor {
  field: string;
  label: string;
  color: string;
}

// 各模式的锚点顺序（时间线字段声明序）。条形图显示相邻锚点之间的耗时段。
const REALTIME_ANCHORS: StageAnchor[] = [
  { field: "speechStartedMs", label: "听到说话", color: "#8fb4d9" },
  { field: "speechStoppedMs", label: "说完（断句）", color: "#5f8fc4" },
  { field: "transcriptDoneMs", label: "转写完成", color: "#7fbf8e" },
  { field: "responseCreatedMs", label: "请求发出", color: "#d9b25f" },
  { field: "firstAudioMs", label: "首包音频", color: "#d98a5f" },
  { field: "responseDoneMs", label: "回答完成", color: "#b78fd9" },
];

const CASCADE_ANCHORS: StageAnchor[] = [
  { field: "asrDoneMs", label: "转写完成", color: "#5f8fc4" },
  { field: "retrievalDoneMs", label: "资料检索", color: "#8fceb0" },
  { field: "llmFirstTokenMs", label: "首词生成", color: "#d9b25f" },
  { field: "llmDoneMs", label: "回答生成", color: "#e0a04f" },
  { field: "ttsDoneMs", label: "语音合成", color: "#d98a5f" },
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
      segments.push({ label: present[index].anchor.label, ms, color: present[index].anchor.color });
    }
  }
  return segments;
}

export function formatLatencyTotal(ms: number): string {
  if (ms >= 1_000) return `${(ms / 1_000).toFixed(1)} 秒`;
  return `${Math.round(ms)} 毫秒`;
}

interface LatencyWaterfallProps {
  latency: TurnLatencyView;
  turnIndex: number;
}

export function LatencyWaterfall({ latency, turnIndex }: LatencyWaterfallProps) {
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
        aria-label={`第 ${turnIndex + 1} 轮延迟 ${formatLatencyTotal(total)}，点击${open ? "收起" : "展开"}明细`}
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
      {latency.interrupted && <span className="latency-waterfall-flag">被打断</span>}
      {open && (
        <table className="latency-waterfall-details">
          <caption className="sr-only">第 {turnIndex + 1} 轮分阶段耗时</caption>
          <thead>
            <tr>
              <th scope="col">阶段</th>
              <th scope="col">耗时</th>
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
