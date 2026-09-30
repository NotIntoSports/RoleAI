// 模拟面试训练的演示后端（E10）：practice_* 命令的内存模拟实现。
// 种子数据：1 份题单、2 份历史报告（展示成长曲线）。全部内容虚构。
// 训练会话直接复用脚本化实时会话（session_start），进度按该会话轮次推导。
import type {
  AnswerMetrics,
  CommandResult,
  PracticePlan,
  PracticePlanGenerateInput,
  PracticePlanSummary,
  PracticeProgress,
  PracticeQuestion,
  PracticeReport,
  PracticeSessionStartInput,
  SessionStartResult,
  SessionTurnView,
} from "../../generated/bindings";

import { handleLiveSessionCommand } from "./live-commands";
import { getState } from "./state";
import { demoId, err, latency, ok } from "./util";

const FILLER_WORDS = ["嗯", "啊", "那个", "就是", "然后", "其实"];

const QUESTION_TEMPLATES: Array<(position: string) => PracticeQuestion> = [
  (position) => ({
    prompt: `请介绍一个你最有代表性的${position}项目，并说明你的具体职责。`,
    focus: "项目深度",
    expectedPoints: ["背景与目标", "关键决策", "量化结果"],
    followups: ["最大的技术困难是什么？", "如果重来一次会怎么改？"],
  }),
  () => ({
    prompt: "讲一次你排查线上问题的完整过程：从发现到复盘。",
    focus: "问题定位",
    expectedPoints: ["监控与假设", "验证过程", "后续预防"],
    followups: ["当时为什么不先回滚？"],
  }),
  () => ({
    prompt: "你如何判断一个技术方案半年后是否仍然成立？",
    focus: "系统设计",
    expectedPoints: ["容量与成本", "演进路径"],
    followups: ["举一个推翻重来的例子。"],
  }),
  () => ({
    prompt: "说说一次你与同事意见冲突的经历，最后怎么收场？",
    focus: "协作沟通",
    expectedPoints: ["事实与立场", "达成的共识"],
    followups: ["如果对方仍然不服呢？"],
  }),
  () => ({
    prompt: "挑一段你写过的最复杂的逻辑，讲清楚为什么必须这么复杂。",
    focus: "工程权衡",
    expectedPoints: ["约束条件", "简化尝试"],
    followups: ["有考虑过删掉这层吗？"],
  }),
  () => ({
    prompt: "你如何给一个刚上线的新服务确定告警阈值？",
    focus: "运维意识",
    expectedPoints: ["基线数据", "分级策略"],
    followups: ["误报太多怎么办？"],
  }),
  () => ({
    prompt: "讲讲你最近一次学习新技术并把它们落到项目里的经历。",
    focus: "成长性",
    expectedPoints: ["选型理由", "落地效果"],
    followups: ["踩过哪些坑？"],
  }),
  () => ({
    prompt: "如果让你重做当前项目里的一个模块，你会选哪个，为什么？",
    focus: "反思与判断",
    expectedPoints: ["现状问题", "收益估计"],
    followups: ["为什么之前没有做？"],
  }),
  () => ({
    prompt: "在工期和代码质量冲突时，你的取舍标准是什么？",
    focus: "工程权衡",
    expectedPoints: ["风险边界", "沟通方式"],
    followups: ["举一个真实的取舍案例。"],
  }),
  () => ({
    prompt: "你如何向完全不懂技术的同事解释一次故障的原因？",
    focus: "表达与抽象",
    expectedPoints: ["类比准确", "结论先行"],
    followups: ["如果对方还是不满意呢？"],
  }),
  () => ({
    prompt: "说说你做过的最有争议的技术决定，以及事后验证的结果。",
    focus: "决策与担当",
    expectedPoints: ["决策依据", "事后复盘"],
    followups: ["当时有更好的选项吗？"],
  }),
  () => ({
    prompt: "如果团队要求你把回答时间压缩到一半，你会先砍什么？",
    focus: "优先级",
    expectedPoints: ["核心诉求", "可砍项排序"],
    followups: ["被砍掉的部分怎么补？"],
  }),
];

interface ActivePractice {
  sessionId: string;
  planId: string;
  total: number;
  skipOffset: number;
}

let plans: PracticePlan[] = [];
let reports: PracticeReport[] = [];
let active: ActivePractice | null = null;
let seeded = false;

function ensureSeeded(): void {
  if (seeded) return;
  seeded = true;
  plans = [seedPlan()];
  reports = [seedReport("demo-practice-report-2", 1), seedReport("demo-practice-report-1", 0)];
}

export function resetPracticeState(): void {
  seeded = false;
  ensureSeeded();
  active = null;
}

function seedPlan(): PracticePlan {
  const templates = QUESTION_TEMPLATES.slice(0, 5);
  return {
    id: "demo-practice-plan-1",
    title: "后端开发工程师·严苛面试官",
    position: "后端开发工程师",
    interviewerStyle: "严苛面试官",
    difficulty: "标准",
    questions: templates.map((template) => template("后端")),
    createdAt: "2026-10-01T08:00:00Z",
    updatedAt: "2026-10-01T08:00:00Z",
  };
}

function dimensionsOf(contentDepth: number, structureClarity: number, fluency: number, jobFit: number) {
  return { contentDepth, structureClarity, fluency, jobFit };
}

function metricsOf(
  answers: Array<{ chars: number; words: number; duration: number | null; fillers: Array<{ word: string; count: number }> }>,
) {
  const rows = answers.map((answer, index) => ({
    answerIndex: index,
    durationSeconds: answer.duration,
    chineseChars: answer.chars,
    englishWords: answer.words,
    speechRate:
      answer.duration && answer.duration > 0
        ? {
            chinesePerMinute: Math.round((answer.chars / (answer.duration / 60)) * 10) / 10,
            englishPerMinute: Math.round((answer.words / (answer.duration / 60)) * 10) / 10,
          }
        : null,
    fillers: answer.fillers,
    structureSignals: [],
    starCoverage: [],
  }));
  const durations = rows.map((row) => row.durationSeconds);
  const total = durations.every((value) => value !== null) ? (durations as number[]).reduce((a, b) => a + b, 0) : null;
  const fillerTotals = new Map<string, number>();
  for (const answer of answers) {
    for (const hit of answer.fillers) {
      fillerTotals.set(hit.word, (fillerTotals.get(hit.word) ?? 0) + hit.count);
    }
  }
  const topFillers = [...fillerTotals.entries()]
    .map(([word, count]) => ({ word, count }))
    .sort((a, b) => b.count - a.count || a.word.localeCompare(b.word));
  return {
    answers: rows,
    totalDurationSeconds: total,
    averageAnswerSeconds: total !== null && rows.length > 0 ? Math.round((total / rows.length) * 10) / 10 : null,
    longPauses: null,
    topFillers,
  };
}

/** 2 份历史报告（虚构）：维度与总分逐次提升，用于展示成长曲线。 */
function seedReport(sessionId: string, generation: 0 | 1): PracticeReport {
  const base = generation === 0
    ? {
        createdAt: "2026-09-20T10:00:00Z",
        totalScore: 2.8,
        dimensions: dimensionsOf(2.5, 3, 3, 2.5),
        duration: 780,
      }
    : {
        createdAt: "2026-10-05T10:00:00Z",
        totalScore: 3.6,
        dimensions: dimensionsOf(3.5, 4, 3.5, 3.5),
        duration: 960,
      };
  const plan = seedPlan();
  const fillersByRound = generation === 0
    ? [[{ word: "那个", count: 4 }, { word: "嗯", count: 3 }], [{ word: "就是", count: 2 }], []]
    : [[{ word: "那个", count: 2 }], [], [{ word: "嗯", count: 1 }]];
  const answers = plan.questions.slice(0, 3).map((question, index) => ({
    // 时长按自然语速（约 200 字/分钟）与字数自洽，避免指标卡数字互相矛盾。
    chars: Math.round((base.duration / 3 / 60) * 200) + index * 10,
    words: 4 + index,
    duration: Math.round(base.duration / 3),
    fillers: fillersByRound[index],
    prompt: question.prompt,
  }));
  const scores = generation === 0 ? [3, 3, 2.5] : [4, 4, 3];
  const answersText = generation === 0
    ? "（演示转写）当时的情况比较复杂，主要是我负责的部分，具体细节就是……那个……做了一些优化。"
    : "（演示转写）项目背景是接口性能不达标，我先用压测定位瓶颈，再把缓存粒度细化，最终 p99 下降 30%。";
  const strengthsByRound = generation === 0
    ? [["态度认真"], ["思路清楚"], ["态度认真"]]
    : [["先讲结论再展开"], ["有量化结果"], ["结构完整"]];
  const issuesByRound = generation === 0
    ? [["口头禅较多"], ["缺少量化结果"], ["展开没有重点"]]
    : [[], ["时间分配可以更均衡"], []];
  return {
    sessionId,
    planId: plan.id,
    position: plan.position,
    interviewerStyle: plan.interviewerStyle,
    llmAvailable: true,
    totalScore: base.totalScore,
    dimensions: base.dimensions,
    perQuestion: answers.map((answer, index) => ({
      index: index + 1,
      question: answer.prompt,
      answer: answersText,
      score: scores[index],
      strengths: strengthsByRound[index],
      issues: issuesByRound[index],
      modelAnswer: "（演示示范）建议按“结论 → 依据 → 数字”三段式回答，并把口头禅替换成停顿。",
    })),
    topSuggestions:
      generation === 0
        ? ["回答先给结论再展开细节", "控制口头禅，用停顿代替", "每个项目准备一个量化结果"]
        : ["继续补充跨团队协作的例子", "对追问保持追问式的反问练习", "把语速稳定在 220 字/分钟以内"],
    objective: metricsOf(
      answers.map((answer) => ({
        chars: answer.chars,
        words: answer.words,
        duration: answer.duration,
        fillers: answer.fillers,
      })),
    ),
    createdAt: base.createdAt,
  };
}

function summaryOf(plan: PracticePlan): PracticePlanSummary {
  return {
    id: plan.id,
    title: plan.title,
    position: plan.position,
    interviewerStyle: plan.interviewerStyle,
    difficulty: plan.difficulty,
    questionCount: plan.questions.length,
    createdAt: plan.createdAt,
    updatedAt: plan.updatedAt,
  };
}

function planById(planId: string): PracticePlan | undefined {
  return plans.find((plan) => plan.id === planId);
}

function turnAnswers(sessionId: string): SessionTurnView[] {
  return (getState().sessionTurns[sessionId] ?? []).filter((turn) => turn.userText.trim().length > 0);
}

function progressOf(state: ActivePractice): PracticeProgress {
  const answers = turnAnswers(state.sessionId).length;
  const reached = Math.min(answers + state.skipOffset, state.total);
  return {
    planId: state.planId,
    questionIndex: Math.min(reached, Math.max(state.total - 1, 0)),
    totalQuestions: state.total,
    followupsUsed: 0,
    followupLimit: 2,
    finished: reached >= state.total,
  };
}

function lastAnswerMetrics(sessionId: string): AnswerMetrics | null {
  const answers = turnAnswers(sessionId);
  const last = answers.at(-1);
  if (!last) return null;
  const chars = (last.userText.match(/[\u4e00-\u9fff]/g) ?? []).length;
  const words = (last.userText.match(/[A-Za-z]+/g) ?? []).length;
  const fillers = FILLER_WORDS.map((word) => ({
    word,
    count: last.userText.split(word).length - 1,
  })).filter((hit) => hit.count > 0);
  const duration = 90;
  return {
    answerIndex: answers.length - 1,
    durationSeconds: duration,
    chineseChars: chars,
    englishWords: words,
    speechRate: {
      chinesePerMinute: Math.round((chars / (duration / 60)) * 10) / 10,
      englishPerMinute: Math.round((words / (duration / 60)) * 10) / 10,
    },
    fillers,
    structureSignals: [],
    starCoverage: [],
  };
}

function renderReportMarkdown(report: PracticeReport): string {
  const lines = [
    `# 模拟面试训练报告（${report.position}）`,
    "",
    `- 总分：${report.totalScore} / 5`,
    `- 维度：内容深度 ${report.dimensions.contentDepth} · 结构清晰 ${report.dimensions.structureClarity} · 表达流畅 ${report.dimensions.fluency} · 岗位匹配 ${report.dimensions.jobFit}`,
    "",
    "> 在线演示数据，全部内容均为虚构。",
    "",
  ];
  for (const review of report.perQuestion) {
    lines.push(`## 第 ${review.index} 题：${review.question}`, "", `**我的回答**：${review.answer}`, "");
    if (review.score > 0) {
      lines.push(`**评分**：${review.score} / 5`, "", `**改进示范**：${review.modelAnswer}`, "");
    }
  }
  lines.push("---", "", "评分由 AI 生成，仅供练习参考", "");
  return lines.join("\n");
}

function downloadMarkdown(fileName: string, content: string): void {
  if (typeof document === "undefined") return;
  try {
    const blob = new Blob([content], { type: "text/markdown;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = fileName;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    setTimeout(() => URL.revokeObjectURL(url), 4000);
  } catch {
    // 下载失败不影响返回值。
  }
}

/** practice 域命令分发；未识别的命令返回 undefined 交给下一个领域。 */
export function handlePracticeCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  ensureSeeded();
  switch (cmd) {
    case "practice_plan_generate": {
      const input = payload.input as PracticePlanGenerateInput;
      const position = String(input?.position ?? "后端开发工程师").trim() || "后端开发工程师";
      const count = Math.min(12, Math.max(1, Number(input?.questionCount ?? 5)));
      const style = String(input?.interviewerStyle ?? "面试官");
      const now = new Date().toISOString();
      const plan: PracticePlan = {
        id: demoId("demo-plan"),
        title: `${position}·${style}`,
        position,
        interviewerStyle: style,
        difficulty: String(input?.difficulty ?? "标准"),
        questions: Array.from({ length: count }, (_, index) => ({
          ...QUESTION_TEMPLATES[index % QUESTION_TEMPLATES.length](position),
        })),
        createdAt: now,
        updatedAt: now,
      };
      return latency(500, 1100).then(() => ok(plan));
    }
    case "practice_plan_save": {
      const plan = payload.input as PracticePlan;
      if (!plan?.id || plan.questions.length === 0) {
        return err("PRACTICE_PLAN_INVALID", "题单参数无效");
      }
      plan.updatedAt = new Date().toISOString();
      plans = [plan, ...plans.filter((item) => item.id !== plan.id)];
      return latency(80, 240).then(() => ok(plan));
    }
    case "practice_plan_list":
      return latency(40, 120).then(() => ok(plans.map(summaryOf)));
    case "practice_plan_delete": {
      const planId = String(payload.planId ?? "");
      plans = plans.filter((plan) => plan.id !== planId);
      return latency(60, 160).then(() => ok({ ready: true }));
    }
    case "practice_session_start": {
      const input = payload.input as PracticeSessionStartInput;
      const plan = planById(String(input?.planId ?? ""));
      if (!plan) return err("PRACTICE_PLAN_NOT_FOUND", "题单不存在");
      const live = handleLiveSessionCommand("session_start", {}) as Promise<
        CommandResult<SessionStartResult>
      >;
      return (async () => {
        const result = await live;
        if (!result.ok || result.data.kind !== "started") return result;
        active = {
          sessionId: result.data.session.id,
          planId: plan.id,
          total: plan.questions.length,
          skipOffset: 0,
        };
        return result;
      })();
    }
    case "practice_session_progress": {
      const sessionId = String(payload.sessionId ?? "");
      if (!active || active.sessionId !== sessionId) {
        return err("PRACTICE_SESSION_STATE_INVALID", "该会话不是模拟面试训练");
      }
      return latency(20, 60).then(() => ok(progressOf(active as ActivePractice)));
    }
    case "practice_session_skip": {
      const sessionId = String(payload.sessionId ?? "");
      const current = active;
      if (!current || current.sessionId !== sessionId) {
        return err("PRACTICE_SESSION_STATE_INVALID", "该会话不是模拟面试训练");
      }
      current.skipOffset += 1;
      return latency(60, 160).then(() => ok(progressOf(current)));
    }
    case "practice_session_turn_metrics": {
      const sessionId = String(payload.sessionId ?? "");
      if (!active || active.sessionId !== sessionId) {
        return err("PRACTICE_SESSION_STATE_INVALID", "该会话不是模拟面试训练");
      }
      return latency(20, 60).then(() => ok(lastAnswerMetrics(sessionId)));
    }
    case "practice_report_generate": {
      const sessionId = String(payload.sessionId ?? "");
      const answers = turnAnswers(sessionId);
      if (answers.length === 0) {
        return err("PRACTICE_SESSION_STATE_INVALID", "会话还没有可评价的转写");
      }
      const plan = active && active.sessionId === sessionId ? planById(active.planId) : undefined;
      const count = Math.min(3, answers.length);
      const now = new Date().toISOString();
      const report: PracticeReport = {
        sessionId,
        planId: plan?.id ?? plans[0]?.id ?? "",
        position: plan?.position ?? "后端开发工程师",
        interviewerStyle: plan?.interviewerStyle ?? "严苛面试官",
        llmAvailable: true,
        totalScore: 3.8,
        dimensions: dimensionsOf(4, 3.5, 4, 3.5),
        perQuestion: Array.from({ length: count }, (_, index) => ({
          index: index + 1,
          question:
            plan?.questions[index]?.prompt ?? plans[0]?.questions[index]?.prompt ?? `第 ${index + 1} 题`,
          answer: answers[index].userText,
          score: [4, 4, 3.5][index],
          strengths: index === 0 ? ["结构清晰", "有具体例子"] : ["先讲结论再展开"],
          issues: index === 0 ? ["结尾缺少量化结果"] : [],
          modelAnswer: "（演示示范）按“结论 → 依据 → 数字”三段式重组回答。",
        })),
        topSuggestions: ["回答先给结论再展开细节", "每个项目准备一个量化结果", "用停顿代替口头禅"],
        objective: metricsOf(
          answers.slice(0, 3).map((turn, index) => ({
            chars: (turn.userText.match(/[\u4e00-\u9fff]/g) ?? []).length,
            words: (turn.userText.match(/[A-Za-z]+/g) ?? []).length,
            duration: 90 + index * 15,
            fillers: FILLER_WORDS.map((word) => ({ word, count: turn.userText.split(word).length - 1 }))
              .filter((hit) => hit.count > 0),
          })),
        ),
        createdAt: now,
      };
      reports = [report, ...reports.filter((item) => item.sessionId !== sessionId)];
      return latency(600, 1200).then(() => ok(report));
    }
    case "practice_report_list":
      return latency(40, 120).then(() =>
        ok(
          [...reports]
            .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
            .map((report) => ({
              sessionId: report.sessionId,
              planId: report.planId,
              position: report.position,
              interviewerStyle: report.interviewerStyle,
              totalScore: report.totalScore,
              dimensions: report.dimensions,
              durationSeconds: report.objective.totalDurationSeconds,
              createdAt: report.createdAt,
            })),
        ),
      );
    case "practice_report_get": {
      const sessionId = String(payload.sessionId ?? "");
      const report = reports.find((item) => item.sessionId === sessionId);
      if (!report) return err("PRACTICE_REPORT_NOT_FOUND", "训练报告不存在");
      return latency(40, 120).then(() => ok(report));
    }
    case "practice_report_export": {
      const sessionId = String(payload.sessionId ?? "");
      const format = String(payload.format ?? "");
      if (format !== "markdown") {
        return err("SESSION_EXPORT_FORMAT_INVALID", "Unsupported export format");
      }
      const report = reports.find((item) => item.sessionId === sessionId);
      if (!report) return err("PRACTICE_REPORT_NOT_FOUND", "训练报告不存在");
      const fileName = `roleai-practice-${sessionId}.md`;
      downloadMarkdown(fileName, renderReportMarkdown(report));
      return latency(120, 400).then(() =>
        ok({ path: `已通过浏览器下载：${fileName}（在线演示不写入本地磁盘）` }),
      );
    }
    default:
      return undefined;
  }
}
