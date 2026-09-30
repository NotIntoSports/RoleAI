import { useCallback, useEffect, useState } from "react";
import { Download, RefreshCw, Sparkles } from "lucide-react";

import * as api from "../../api/commands";
import { errorNoticeText as errorText } from "../../components/error-notice";
import type { PracticeDimensions, PracticeReport } from "../../generated/bindings";
import { formatDuration } from "../session/workspace-format";

/**
 * 训练报告页（E08）：总分 + 维度雷达图（手写 SVG，不引入图表库）、
 * 逐题卡片（可展开原始转写）、客观指标卡、导出 Markdown，
 * 底部固定声明“评分由 AI 生成，仅供练习参考”。
 * 报告不存在时提供“生成报告”；定性点评失败（llmAvailable=false）时
 * 显示客观指标并允许重试（后端按同 sessionId 覆盖保存）。
 */

const DIMENSION_LABELS: Array<{ key: keyof PracticeDimensions; label: string }> = [
  { key: "contentDepth", label: "内容深度" },
  { key: "structureClarity", label: "结构清晰" },
  { key: "fluency", label: "表达流畅" },
  { key: "jobFit", label: "岗位匹配" },
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
  const size = 200;
  const center = size / 2;
  const radius = 70;
  const values = DIMENSION_LABELS.map(({ key }) => clampScore(dimensions[key]));
  // 轴序：上、右、下、左（数学角度 90° → 0° → 270° → 180°）。
  const point = (axis: number, ratio: number): [number, number] => {
    const angle = (90 - axis * 90) * (Math.PI / 180);
    return [center + radius * ratio * Math.cos(angle), center - radius * ratio * Math.sin(angle)];
  };
  const ring = (ratio: number) =>
    [0, 1, 2, 3].map((axis) => point(axis, ratio).map((v) => v.toFixed(1)).join(",")).join(" ");
  const dataPolygon = values
    .map((value, axis) => point(axis, value / 5).map((v) => v.toFixed(1)).join(","))
    .join(" ");
  const labelOffset: Array<[number, number]> = [
    [0, -10],
    [10, 4],
    [0, 16],
    [-10, 4],
  ];
  return (
    <svg
      className="practice-radar"
      viewBox={`0 0 ${size} ${size}`}
      width={size}
      height={size}
      role="img"
      aria-label={`维度雷达图：${DIMENSION_LABELS.map(({ label }, index) => `${label} ${formatScore(values[index])}`).join("，")}`}
    >
      {[1, 2, 3, 4, 5].map((level) => (
        <polygon key={level} points={ring(level / 5)} fill="none" stroke="var(--border)" strokeWidth={level === 5 ? 1.5 : 1} />
      ))}
      {[0, 1, 2, 3].map((axis) => {
        const [x, y] = point(axis, 1);
        return <line key={axis} x1={center} y1={center} x2={x} y2={y} stroke="var(--border)" />;
      })}
      <polygon points={dataPolygon} fill="var(--accent-strong-soft)" stroke="var(--accent-strong)" strokeWidth={1.5} />
      {values.map((value, axis) => {
        const [x, y] = point(axis, value / 5);
        return <circle key={axis} cx={x} cy={y} r={2.5} fill="var(--accent-strong)" />;
      })}
      {DIMENSION_LABELS.map(({ label }, axis) => {
        const [x, y] = point(axis, 1);
        const [dx, dy] = labelOffset[axis];
        const anchor = axis === 0 || axis === 2 ? "middle" : axis === 1 ? "start" : "end";
        return (
          <text key={label} x={x + dx} y={y + dy} textAnchor={anchor} fontSize={11} fill="var(--text-muted)">
            {label} {formatScore(values[axis])}
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
  const { objective } = report;
  const rate = aggregateSpeechRate(report);
  const fillers = objective.topFillers.slice(0, 5);
  return (
    <div className="practice-objective" aria-label="客观指标">
      <h4>客观指标（本地计算）</h4>
      <ul className="practice-objective-list">
        <li>
          语速：
          {rate.chinesePerMinute !== null
            ? `约 ${Math.round(rate.chinesePerMinute)} 字/分钟（各次回答平均）`
            : rate.englishPerMinute !== null
              ? `约 ${Math.round(rate.englishPerMinute)} 词/分钟（各次回答平均）`
              : "时间信息不足，暂不可用"}
        </li>
        <li>
          口头禅 Top5：
          {fillers.length > 0
            ? fillers.map((hit) => `${hit.word} ×${hit.count}`).join("、")
            : "未检测到口头禅"}
        </li>
        <li>
          时长分布：
          {objective.totalDurationSeconds !== null && objective.averageAnswerSeconds !== null
            ? `总 ${formatDuration(Math.round(objective.totalDurationSeconds))} · 平均每次 ${formatDuration(Math.round(objective.averageAnswerSeconds))}`
            : "时间信息不足，暂不可用"}
        </li>
        {objective.longPauses !== null && <li>回答间长停顿：{objective.longPauses} 次</li>}
      </ul>
      <p className="muted practice-objective-note">客观指标由本地转写启发式计算，仅供练习参考。</p>
    </div>
  );
}

function QuestionCard({ review }: { review: PracticeReport["perQuestion"][number] }) {
  const score = clampScore(review.score);
  return (
    <li className="practice-review-card">
      <div className="practice-review-head">
        <span className="practice-review-index">第 {review.index} 题</span>
        <span className="practice-review-question">{review.question}</span>
        <span className="status-badge" data-tone={score > 0 ? "success" : undefined}>
          {score > 0 ? `${formatScore(score)} / 5` : "未评"}
        </span>
      </div>
      <details className="practice-review-answer">
        <summary>我的回答（原始转写）</summary>
        <p>{review.answer || "（本轮没有候选人回答）"}</p>
      </details>
      {score > 0 && (
        <div className="practice-review-body">
          {review.strengths.length > 0 && (
            <p><strong>优点：</strong>{review.strengths.join("；")}</p>
          )}
          {review.issues.length > 0 && (
            <p><strong>问题：</strong>{review.issues.join("；")}</p>
          )}
          {review.modelAnswer && (
            <p><strong>改进示范：</strong>{review.modelAnswer}</p>
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
      setMessage("IPC_UNAVAILABLE：无法读取训练报告");
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
      setMessage("IPC_UNAVAILABLE：报告生成失败");
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
        setMessage(`报告已导出：${result.data.path}`);
      } else {
        setMessage(errorText(result.error));
      }
    } catch {
      setMessage("IPC_UNAVAILABLE：报告导出失败");
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
      {phase === "loading" && <p className="muted" role="status">正在读取训练报告…</p>}
      {phase === "error" && <p className="muted">报告读取失败，可稍后重试。</p>}
      {phase === "missing" && (
        <div className="practice-report-missing">
          <p className="muted">这次训练还没有报告。</p>
          <button className="button-primary" type="button" disabled={busy} onClick={() => void handleGenerate()}>
            <Sparkles size={15} aria-hidden="true" />
            {busy ? "正在生成报告…" : "生成训练报告"}
          </button>
          <p className="muted">生成调用你配置的模型服务；报告也可以稍后在历史中重新生成。</p>
        </div>
      )}
      {phase === "ready" && report && (
        <article className="practice-report" aria-label="训练报告详情">
          <header className="practice-report-head">
            <div>
              <h4 className="practice-report-title">
                {report.position || "模拟面试训练"} · {report.interviewerStyle}
              </h4>
              <p className="muted">
                总分 {formatScore(report.totalScore)} / 5 · {report.createdAt.slice(0, 10)}
              </p>
            </div>
            <div className="practice-report-actions">
              {!report.llmAvailable && (
                <button className="button-ghost" type="button" disabled={busy} onClick={() => void handleGenerate()}>
                  <RefreshCw size={14} aria-hidden="true" />
                  重试生成定性点评
                </button>
              )}
              <button className="button-ghost" type="button" disabled={busy} onClick={() => void handleExport()}>
                <Download size={14} aria-hidden="true" />
                导出 Markdown
              </button>
            </div>
          </header>
          {!report.llmAvailable && (
            <p className="services-message" role="alert">
              定性点评生成失败，可重试。以下为本地客观指标。
            </p>
          )}
          {report.topSuggestions.length > 0 && (
            <section className="practice-report-suggestions" aria-label="改进建议">
              <h4>最重要的三条改进建议</h4>
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
          <section className="practice-report-questions" aria-label="逐题点评">
            <h4>逐题点评</h4>
            <ul>
              {report.perQuestion.map((review) => (
                <QuestionCard key={review.index} review={review} />
              ))}
            </ul>
          </section>
          <footer className="practice-report-disclaimer">评分由 AI 生成，仅供练习参考</footer>
        </article>
      )}
    </div>
  );
}
