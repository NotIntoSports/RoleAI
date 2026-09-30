import { useEffect, useRef, useState } from "react";

import * as api from "../../api/commands";
import { t, useT } from "../../i18n";
import type { AnswerMetrics, PracticeProgress, SessionTurnView } from "../../generated/bindings";
import { formatDuration } from "../session/workspace-format";
import "../../styles/practice.css";

/**
 * 训练信息条（E07）：叠加在工作台会话界面顶部，只在模拟面试训练会话中渲染。
 * 展示当前题号、本题计时、跳过按钮，以及基于 E02 客观指标的逐轮表达提示
 * （每轮结束后经 practice_session_turn_metrics 更新，不做逐字实时计算）。
 * 非训练会话探针失败即整体不渲染，不影响原有会话界面。
 */

// 表达提示阈值（启发式，仅供参考，不参与评分）：
// 中文日常对话约 200~300 字/分钟、英文约 120~175 词/分钟，超过视为偏快；
// 单条回答口头禅合计 ≥5 次（“嗯、啊、那个、就是、然后、其实”等，词表与后端一致）提示留意。
const FAST_CHINESE_PER_MINUTE = 300;
const FAST_ENGLISH_PER_MINUTE = 175;
const MANY_FILLERS_PER_ANSWER = 5;

export interface PracticeHudProps {
  /** 当前会话 id；为空（会话未开始）时不渲染。 */
  sessionId: string | null;
  /** 会话是否处于活跃阶段（计时有意义）。 */
  active: boolean;
  /** 会话轮次：每轮结束（数组更新）后刷新进度与表达提示。 */
  turns: SessionTurnView[];
  /** 跳过等操作失败时上抛提示（工作台的 message 栏）。 */
  onNotify?: (message: string) => void;
}

function expressionHints(metrics: AnswerMetrics | null): string[] {
  if (!metrics) return [];
  const hints: string[] = [];
  const rate = metrics.speechRate;
  if (rate && rate.chinesePerMinute > FAST_CHINESE_PER_MINUTE) {
    hints.push(t("practice.hud.rateFastChars", { n: Math.round(rate.chinesePerMinute) }));
  } else if (rate && rate.englishPerMinute > FAST_ENGLISH_PER_MINUTE) {
    hints.push(t("practice.hud.rateFastWords", { n: Math.round(rate.englishPerMinute) }));
  }
  const fillerTotal = metrics.fillers.reduce((sum, hit) => sum + hit.count, 0);
  if (fillerTotal >= MANY_FILLERS_PER_ANSWER) {
    hints.push(t("practice.hud.fillersMany", { n: fillerTotal }));
  }
  return hints;
}

export function PracticeHud({ sessionId, active, turns, onNotify }: PracticeHudProps) {
  useT();
  const [progress, setProgress] = useState<PracticeProgress | null>(null);
  const [turnMetrics, setTurnMetrics] = useState<AnswerMetrics | null>(null);
  const [skipBusy, setSkipBusy] = useState(false);
  const [reportBusy, setReportBusy] = useState(false);
  const [questionSeconds, setQuestionSeconds] = useState(0);
  const questionStartRef = useRef<number | null>(null);

  // 会话切换或每轮转写更新后：拉取训练进度与最近一条回答的客观指标。
  // 非训练会话（progress 拉取失败）保持不渲染。
  useEffect(() => {
    if (!sessionId) {
      setProgress(null);
      setTurnMetrics(null);
      return;
    }
    let cancelled = false;
    void (async () => {
      try {
        const result = await api.getPracticeSessionProgress(sessionId);
        if (cancelled) return;
        if (!result.ok) {
          setProgress(null);
          setTurnMetrics(null);
          return;
        }
        setProgress(result.data);
        const metrics = await api.getPracticeSessionTurnMetrics(sessionId);
        if (!cancelled) setTurnMetrics(metrics.ok ? metrics.data : null);
      } catch {
        if (!cancelled) {
          setProgress(null);
          setTurnMetrics(null);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [sessionId, turns]);

  const questionIndex = progress?.questionIndex ?? -1;
  // 本题计时：从已知题号变化的时刻起算。页面在题目中途加载时（如从向导跳回），
  // 后端没有记录题目开始时间，计时从接管时刻开始，属已知近似。
  useEffect(() => {
    if (questionIndex < 0) return;
    questionStartRef.current = Date.now();
    setQuestionSeconds(0);
  }, [sessionId, questionIndex]);
  useEffect(() => {
    if (!active || !progress || progress.finished) return;
    const timer = window.setInterval(() => {
      if (questionStartRef.current !== null) {
        setQuestionSeconds(Math.floor((Date.now() - questionStartRef.current) / 1000));
      }
    }, 1000);
    return () => window.clearInterval(timer);
  }, [active, progress, questionIndex]);

  async function handleSkip() {
    if (!sessionId || !progress || progress.finished || skipBusy) return;
    setSkipBusy(true);
    try {
      const result = await api.skipPracticeQuestion(sessionId);
      if (result.ok) {
        setProgress(result.data);
      } else {
        onNotify?.(t("practice.hud.skipFailed", { code: result.error.code }));
      }
    } catch {
      onNotify?.(t("practice.hud.skipFailedLocal"));
    } finally {
      setSkipBusy(false);
    }
  }

  // 全部题目完成后的报告入口：生成成功后到「模拟面试」页查看报告详情。
  async function handleGenerateReport() {
    if (!sessionId || reportBusy) return;
    setReportBusy(true);
    try {
      const result = await api.generatePracticeReport(sessionId);
      if (result.ok) {
        onNotify?.(t("practice.hud.reportGenerated"));
      } else {
        onNotify?.(t("practice.hud.reportFailed", { code: result.error.code }));
      }
    } catch {
      onNotify?.(t("practice.hud.reportFailedLocal"));
    } finally {
      setReportBusy(false);
    }
  }

  if (!progress) return null;
  const hints = expressionHints(turnMetrics);
  return (
    <section className="practice-hud" aria-label={t("practice.hud.regionAria")} data-finished={progress.finished ? "true" : undefined}>
      <span className="practice-hud-progress">
        {progress.finished
          ? t("practice.hud.finishedAll", { n: progress.totalQuestions })
          : t("practice.hud.questionProgress", { n: progress.questionIndex + 1, total: progress.totalQuestions })}
      </span>
      {!progress.finished && (
        <span className="practice-hud-timer" role="timer" aria-label={t("practice.hud.timerAria")}>
          {t("practice.hud.timer", { time: formatDuration(questionSeconds) })}
        </span>
      )}
      {!progress.finished && (
        <button
          type="button"
          className="button-ghost practice-hud-skip"
          disabled={skipBusy || !active}
          onClick={() => void handleSkip()}
        >
          {skipBusy ? t("practice.hud.skipping") : t("practice.hud.skip")}
        </button>
      )}
      {progress.finished && (
        <button
          type="button"
          className="button-ghost practice-hud-report"
          disabled={reportBusy}
          onClick={() => void handleGenerateReport()}
        >
          {reportBusy ? t("practice.hud.generatingReport") : t("practice.hud.generateReport")}
        </button>
      )}
      {hints.length > 0 && (
        <ul className="practice-hud-hints">
          {hints.map((hint) => (
            <li key={hint}>{hint}</li>
          ))}
        </ul>
      )}
    </section>
  );
}
