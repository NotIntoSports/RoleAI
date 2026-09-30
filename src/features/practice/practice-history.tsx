import { t, useT, type DictionaryStringKey } from "../../i18n";
import { formatDuration } from "../session/workspace-format";
import type { PracticeDimensions, PracticeReportSummary } from "../../generated/bindings";

/**
 * 训练历史（E09）：按时间的报告列表（岗位、风格、总分、时长、日期）
 * 与各维度成长曲线（SVG 折线，与报告页雷达图同一套手写图表方案，不引入图表库）。
 */

const DIMENSIONS: Array<{ key: keyof PracticeDimensions; labelKey: DictionaryStringKey; color: string }> = [
  { key: "contentDepth", labelKey: "practice.report.dimensions.contentDepth", color: "var(--accent-strong)" },
  { key: "structureClarity", labelKey: "practice.report.dimensions.structureClarity", color: "var(--success)" },
  { key: "fluency", labelKey: "practice.report.dimensions.fluency", color: "var(--danger)" },
  { key: "jobFit", labelKey: "practice.report.dimensions.jobFit", color: "var(--text-muted)" },
];

function clampScore(value: number): number {
  if (!Number.isFinite(value) || value < 0) return 0;
  return Math.min(5, value);
}

function formatScore(value: number): string {
  const rounded = Math.round(clampScore(value) * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

/** 时间升序（最旧在左）；只有一条记录时只画数据点。 */
export function PracticeGrowthChart({ reports }: { reports: PracticeReportSummary[] }) {
  useT();
  const ordered = [...reports].sort((a, b) => a.createdAt.localeCompare(b.createdAt));
  const width = 340;
  const height = 170;
  const pad = 28;
  const count = ordered.length;
  const point = (index: number, score: number): [number, number] => {
    const x = count <= 1 ? width / 2 : pad + (index * (width - pad * 2)) / (count - 1);
    const y = height - pad - (clampScore(score) / 5) * (height - pad * 2);
    return [x, y];
  };
  const describe = DIMENSIONS.map(({ labelKey, key }) => {
    const values = ordered.map((report) => formatScore(report.dimensions[key]));
    return `${t(labelKey)}（${values.join("→")}）`;
  }).join("，");
  return (
    <figure className="practice-growth">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width={width}
        height={height}
        role="img"
        aria-label={t("practice.history.growthAria", { items: describe })}
      >
        {/* 5 分与 0 分参考线 */}
        <line x1={pad} y1={pad} x2={width - pad} y2={pad} stroke="var(--border)" strokeDasharray="4 3" />
        <line x1={pad} y1={height - pad} x2={width - pad} y2={height - pad} stroke="var(--border)" />
        <text x={4} y={pad + 4} fontSize={10} fill="var(--text-muted)">5</text>
        <text x={4} y={height - pad + 4} fontSize={10} fill="var(--text-muted)">0</text>
        {DIMENSIONS.map(({ key, color }) => {
          const points = ordered
            .map((report, index) => point(index, report.dimensions[key]).map((v) => v.toFixed(1)).join(","))
            .join(" ");
          if (count > 1) {
            return <polyline key={key} points={points} fill="none" stroke={color} strokeWidth={1.6} />;
          }
          return null;
        })}
        {ordered.map((report, index) =>
          DIMENSIONS.map(({ key, color }) => {
            const [x, y] = point(index, report.dimensions[key]);
            return <circle key={`${report.sessionId}-${key}`} cx={x} cy={y} r={2.4} fill={color} />;
          }),
        )}
        <text x={pad} y={height - 8} fontSize={10} fill="var(--text-muted)">
          {ordered[0]?.createdAt.slice(0, 10) ?? ""}
        </text>
        <text x={width - pad} y={height - 8} fontSize={10} fill="var(--text-muted)" textAnchor="end">
          {ordered[count - 1]?.createdAt.slice(0, 10) ?? ""}
        </text>
      </svg>
      <figcaption className="practice-growth-legend">
        {DIMENSIONS.map(({ labelKey, color }) => (
          <span key={labelKey}>
            <span className="practice-growth-swatch" style={{ background: color }} aria-hidden="true" />
            {t(labelKey)}
          </span>
        ))}
      </figcaption>
    </figure>
  );
}

export interface PracticeHistorySectionProps {
  reports: PracticeReportSummary[];
  selectedSessionId: string | null;
  onSelect: (sessionId: string) => void;
}

export function PracticeHistorySection({ reports, selectedSessionId, onSelect }: PracticeHistorySectionProps) {
  useT();
  return (
    <section className="practice-history" aria-label={t("practice.history.regionAria")}>
      <PracticeGrowthChart reports={reports} />
      <ul className="practice-history-list">
        {reports.map((report) => (
          <li key={report.sessionId}>
            <button
              type="button"
              className="practice-history-item"
              data-selected={report.sessionId === selectedSessionId ? "true" : undefined}
              aria-pressed={report.sessionId === selectedSessionId}
              onClick={() => onSelect(report.sessionId)}
            >
              <span className="practice-history-position">{report.position || t("practice.report.untitledPosition")}</span>
              <span className="status-badge">{report.interviewerStyle}</span>
              <span className="practice-history-score">{formatScore(report.totalScore)} / 5</span>
              <span className="muted practice-history-meta">
                {report.durationSeconds !== null ? formatDuration(Math.round(report.durationSeconds)) : t("practice.history.durationUnavailable")}
                {" · "}
                {report.createdAt.slice(0, 10)}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
