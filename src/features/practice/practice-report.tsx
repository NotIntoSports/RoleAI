import { useCallback, useEffect, useState } from "react";
import { Download, RefreshCw, Sparkles } from "lucide-react";

import * as api from "../../api/commands";
import { errorNoticeText as errorText } from "../../components/error-notice";
import { t, useT, type DictionaryStringKey } from "../../i18n";
import type { PracticeDimensions, PracticeReport } from "../../generated/bindings";
import { formatDuration } from "../session/workspace-format";

/**
 * 训练报告页（E08）：总分 + 维度雷达图（手写 SVG，不引入图表库）、
 * 逐题卡片（可展开原始转写）、客观指标卡、导出 Markdown，
 * 底部固定声明“评分由 AI 生成，仅供练习参考”。
 * 报告不存在时提供“生成报告”；定性点评失败（llmAvailable=false）时
 * 显示客观指标并允许重试（后端按同 sessionId 覆盖保存）。
 */

const DIMENSION_LABELS: Array<{ key: keyof PracticeDimensions; labelKey: DictionaryStringKey }> = [
  { key: "contentDepth", labelKey: "practice.report.dimensions.contentDepth" },
  { key: "structureClarity", labelKey: "practice.report.dimensions.structureClarity" },
  { key: "fluency", labelKey: "practice.report.dimensions.fluency" },
  { key: "jobFit", labelKey: "practice.report.dimensions.jobFit" },
];

function clampScore(value: number): number {
  if (!Number.isFinite(value) || value < 0) return 0;
  return Math.min(5, value);
}

function formatScore(value: number): string {
  const rounded = Math.round(clampScore(value) * 10) / 10;
  return Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
}

/** 四维雷达图：手写 SVG（约百行），分数 0~5 映射到半径。 */
export function PracticeRadarChart({ dimensions }: { dimensions: PracticeDimensions }) {
  useT();
  const width = 320;
  const height = 220;
  const center: [number, number] = [160, 104];
  const radius = 72;
  const values = DIMENSION_LABELS.map(({ key }) => clampScore(dimensions[key]));
  // 轴序：上、右、下、左（数学角度 90° → 0° → 270° → 180°）。
  const point = (axis: number, ratio: number): [number, number] => {
    const angle = (90 - axis * 90) * (Math.PI / 180);
    return [center[0] + radius * ratio * Math.cos(angle), center[1] - radius * ratio * Math.sin(angle)];
  };
  const ring = (ratio: number) =>
    [0, 1, 2, 3].map((axis) => point(axis, ratio).map((v) => v.toFixed(1)).join(",")).join(" ");
  const dataPolygon = values
    .map((value, axis) => point(axis, value / 5).map((v) => v.toFixed(1)).join(","))
    .join(" ");
  const labelOffset: Array<[number, number]> = [
    [0, -12],
    [14, 4],
    [0, 20],
    [-14, 4],
  ];
  const dimensionTexts = DIMENSION_LABELS.map(({ labelKey }, index) => `${t(labelKey)} ${formatScore(values[index])}`);
  return (
    <svg
      className="practice-radar"
      viewBox={`0 0 ${width} ${height}`}
      width={width}
      height={height}
      role="img"
      aria-label={t("practice.report.radarAria", { items: dimensionTexts.join("，") })}
    >
      {[1, 2, 3, 4, 5].map((level) => (
        <polygon key={level} points={ring(level / 5)} fill="none" stroke="var(--border)" strokeWidth={level === 5 ? 1.5 : 1} />
      ))}
      {[0, 1, 2, 3].map((axis) => {
        const [x, y] = point(axis, 1);
        return <line key={axis} x1={center[0]} y1={center[1]} x2={x} y2={y} stroke="var(--border)" />;
      })}
      <polygon points={dataPolygon} fill="var(--accent-strong-soft)" stroke="var(--accent-strong)" strokeWidth={1.5} />
      {values.map((value, axis) => {
        const [x, y] = point(axis, value / 5);
        return <circle key={axis} cx={x} cy={y} r={2.5} fill="var(--accent-strong)" />;
      })}
      {DIMENSION_LABELS.map(({ labelKey }, axis) => {
        const [x, y] = point(axis, 1);
        const [dx, dy] = labelOffset[axis];
        const anchor = axis === 0 || axis === 2 ? "middle" : axis === 1 ? "start" : "end";
        return (
          <text key={labelKey} x={x + dx} y={y + dy} textAnchor={anchor} fontSize={11} fill="var(--text-muted)">
            {t(labelKey)} {formatScore(values[axis])}
          </text>
        );
      })}
    </svg>
  );
}

interface Aggregate {
  chinesePerMinute: number | null;
  englishPerMinute: number | null;
}

function aggregateSpeechRate(report: PracticeReport): Aggregate {
  const rates = report.objective.answers
    .map((answer) => answer.speechRate)
    .filter((rate): rate is NonNullable<typeof rate> => rate !== null);
  if (rates.length === 0) return { chinesePerMinute: null, englishPerMinute: null };
  const chinesePerMinute = rates.reduce((sum, rate) => sum + rate.chinesePerMinute, 0) / rates.length;
  const englishPerMinute = rates.reduce((sum, rate) => sum + rate.englishPerMinute, 0) / rates.length;
  return { chinesePerMinute, englishPerMinute };
}

function ObjectiveCard({ report }: { report: PracticeReport }) {
  useT();
  const { objective } = report;
  const rate = aggregateSpeechRate(report);
  const fillers = objective.topFillers.slice(0, 5);
  return (
    <div className="practice-objective" aria-label={t("practice.report.objectiveAria")}>
      <h4>{t("practice.report.objectiveHeading")}</h4>
      <ul className="practice-objective-list">
        <li>
          {t("practice.report.speechRateLabel")}
          {rate.chinesePerMinute !== null
            ? t("practice.report.speechRateChars", { n: Math.round(rate.chinesePerMinute) })
            : rate.englishPerMinute !== null
              ? t("practice.report.speechRateWords", { n: Math.round(rate.englishPerMinute) })
              : t("practice.report.timeUnavailable")}
        </li>
        <li>
          {t("practice.report.fillersTop5")}
          {fillers.length > 0
            ? fillers.map((hit) => `${hit.word} ×${hit.count}`).join("、")
            : t("practice.report.noFillers")}
        </li>
        <li>
          {t("practice.report.durationSummaryLabel")}
          {objective.totalDurationSeconds !== null && objective.averageAnswerSeconds !== null
            ? t("practice.report.durationSummary", { total: formatDuration(Math.round(objective.totalDurationSeconds)), average: formatDuration(Math.round(objective.averageAnswerSeconds)) })
            : t("practice.report.timeUnavailable")}
        </li>
        {objective.longPauses !== null && <li>{t("practice.report.longPauses", { n: objective.longPauses })}</li>}
      </ul>
      <p className="muted practice-objective-note">{t("practice.report.objectiveNote")}</p>
    </div>
  );
}

function QuestionCard({ review }: { review: PracticeReport["perQuestion"][number] }) {
  useT();
  const score = clampScore(review.score);
  return (
    <li className="practice-review-card">
      <div className="practice-review-head">
        <span className="practice-review-index">{t("practice.report.questionIndex", { n: review.index })}</span>
        <span className="practice-review-question">{review.question}</span>
        <span className="status-badge" data-tone={score > 0 ? "success" : undefined}>
          {score > 0 ? `${formatScore(score)} / 5` : t("practice.report.notScored")}
        </span>
      </div>
      <details className="practice-review-answer">
        <summary>{t("practice.report.myAnswerSummary")}</summary>
        <p>{review.answer || t("practice.report.noAnswer")}</p>
      </details>
      {score > 0 && (
        <div className="practice-review-body">
          {review.strengths.length > 0 && (
            <p><strong>{t("practice.report.strengths")}</strong>{review.strengths.join("；")}</p>
          )}
          {review.issues.length > 0 && (
            <p><strong>{t("practice.report.issues")}</strong>{review.issues.join("；")}</p>
          )}
          {review.modelAnswer && (
            <p><strong>{t("practice.report.modelAnswer")}</strong>{review.modelAnswer}</p>
          )}
        </div>
      )}
    </li>
  );
}

export interface PracticeReportViewProps {
  sessionId: string;
}

export function PracticeReportView({ sessionId }: PracticeReportViewProps) {
  useT();
  const [report, setReport] = useState<PracticeReport | null>(null);
  const [phase, setPhase] = useState<"loading" | "ready" | "missing" | "error">("loading");
  const [message, setMessage] = useState("");
  const [busy, setBusy] = useState(false);

  const load = useCallback(async (id: string) => {
    setPhase("loading");
    setMessage("");
    try {
      const result = await api.getPracticeReport(id);
      if (result.ok) {
        setReport(result.data);
        setPhase("ready");
      } else if (result.error.code === "PRACTICE_REPORT_NOT_FOUND") {
        setReport(null);
        setPhase("missing");
      } else {
        setReport(null);
        setPhase("error");
        setMessage(errorText(result.error));
      }
    } catch {
      setReport(null);
      setPhase("error");
      setMessage(t("practice.report.ipcReadFailed"));
    }
  }, []);

  useEffect(() => {
    void load(sessionId);
  }, [sessionId, load]);

  // 生成（或重试）报告：后端按 sessionId 覆盖保存；LLM 不可用时返回
  // llmAvailable=false 的客观指标报告，界面固定显示重试入口。
  async function handleGenerate() {
    setBusy(true);
    setMessage("");
    try {
      const result = await api.generatePracticeReport(sessionId);
      if (result.ok) {
        setReport(result.data);
        setPhase("ready");
      } else {
        setMessage(errorText(result.error));
      }
    } catch {
      setMessage(t("practice.report.ipcGenerateFailed"));
    } finally {
      setBusy(false);
    }
  }

  async function handleExport() {
    setBusy(true);
    setMessage("");
    try {
      const result = await api.exportPracticeReport(sessionId, "markdown");
      if (result.ok) {
        setMessage(t("practice.report.exported", { path: result.data.path }));
      } else {
        setMessage(errorText(result.error));
      }
    } catch {
      setMessage(t("practice.report.ipcExportFailed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="practice-report-view">
      {message && (
        <p className="services-message" role="status">
          {message}
        </p>
      )}
      {phase === "loading" && <p className="muted" role="status">{t("practice.report.reading")}</p>}
      {phase === "error" && <p className="muted">{t("practice.report.loadFailed")}</p>}
      {phase === "missing" && (
        <div className="practice-report-missing">
          <p className="muted">{t("practice.report.missingTitle")}</p>
          <button className="button-primary" type="button" disabled={busy} onClick={() => void handleGenerate()}>
            <Sparkles size={15} aria-hidden="true" />
            {busy ? t("practice.report.generating") : t("practice.report.generate")}
          </button>
          <p className="muted">{t("practice.report.generateNote")}</p>
        </div>
      )}
      {phase === "ready" && report && (
        <article className="practice-report" aria-label={t("practice.report.detailsAria")}>
          <header className="practice-report-head">
            <div>
              <h4 className="practice-report-title">
                {report.position || t("practice.report.untitledPosition")} · {report.interviewerStyle}
              </h4>
              <p className="muted">
                {t("practice.report.totalScore", { score: formatScore(report.totalScore) })} · {report.createdAt.slice(0, 10)}
              </p>
            </div>
            <div className="practice-report-actions">
              {!report.llmAvailable && (
                <button className="button-ghost" type="button" disabled={busy} onClick={() => void handleGenerate()}>
                  <RefreshCw size={14} aria-hidden="true" />
                  {t("practice.report.retryQualitative")}
                </button>
              )}
              <button className="button-ghost" type="button" disabled={busy} onClick={() => void handleExport()}>
                <Download size={14} aria-hidden="true" />
                {t("practice.report.exportMarkdown")}
              </button>
            </div>
          </header>
          {!report.llmAvailable && (
            <p className="services-message" role="alert">
              {t("practice.report.qualitativeFailed")}
            </p>
          )}
          {report.topSuggestions.length > 0 && (
            <section className="practice-report-suggestions" aria-label={t("practice.report.suggestionsAria")}>
              <h4>{t("practice.report.suggestionsHeading")}</h4>
              <ol>
                {report.topSuggestions.map((suggestion) => (
                  <li key={suggestion}>{suggestion}</li>
                ))}
              </ol>
            </section>
          )}
          <div className="practice-report-grid">
            <PracticeRadarChart dimensions={report.dimensions} />
            <ObjectiveCard report={report} />
          </div>
          <section className="practice-report-questions" aria-label={t("practice.report.perQuestionAria")}>
            <h4>{t("practice.report.perQuestionHeading")}</h4>
            <ul>
              {report.perQuestion.map((review) => (
                <QuestionCard key={review.index} review={review} />
              ))}
            </ul>
          </section>
          <footer className="practice-report-disclaimer">{t("practice.report.disclaimer")}</footer>
        </article>
      )}
    </div>
  );
}
