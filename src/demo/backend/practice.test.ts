import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  AnswerMetrics,
  CommandResult,
  PracticePlan,
  PracticePlanGenerateInput,
  PracticePlanSummary,
  PracticeProgress,
  PracticeReport,
  PracticeReportSummary,
} from "../../generated/bindings";

import { handleDemoInvoke } from "./index";

vi.mock("@tauri-apps/api/event", () => ({
  emit: vi.fn().mockResolvedValue(undefined),
  listen: vi.fn().mockResolvedValue(() => {}),
}));

async function invoke<T>(cmd: string, payload?: Record<string, unknown>): Promise<CommandResult<T>> {
  return (await handleDemoInvoke(cmd, payload)) as CommandResult<T>;
}

function expectOk<T>(result: CommandResult<T>): T {
  if (!result.ok) throw new Error(`demo invoke failed: ${result.error.code} ${result.error.message}`);
  return result.data;
}

function expectErr(result: CommandResult<unknown>): void {
  if (result.ok) throw new Error("expected the demo invoke to fail");
}

describe("demo backend: practice", () => {
  afterEach(() => {
    window.localStorage.clear();
  });

  it("seeds one plan and two historical reports with growth dimensions", async () => {
    const plans = expectOk(await invoke<PracticePlanSummary[]>("practice_plan_list"));
    expect(plans).toHaveLength(1);
    expect(plans[0].position).toBe("后端开发工程师");
    expect(plans[0].questionCount).toBe(5);

    const reports = expectOk(await invoke<PracticeReportSummary[]>("practice_report_list"));
    expect(reports).toHaveLength(2);
    // 列表按 created_at 倒序；携带成长曲线所需的维度与时长。
    expect(reports[0].sessionId).toBe("demo-practice-report-2");
    expect(reports[0].createdAt > reports[1].createdAt).toBe(true);
    expect(reports[1].totalScore).toBeLessThan(reports[0].totalScore);
    expect(reports[1].dimensions.contentDepth).toBeGreaterThan(0);
    expect(reports[1].durationSeconds).not.toBeNull();
  });

  it("generates a plan honouring the requested question count", async () => {
    const input: PracticePlanGenerateInput = {
      position: "前端开发工程师",
      interviewerStyle: "HR",
      questionCount: 3,
      difficulty: "进阶",
    };
    const plan = expectOk(await invoke<PracticePlan>("practice_plan_generate", { input }));
    expect(plan.questions).toHaveLength(3);
    expect(plan.questions.every((question) => question.prompt.length > 0)).toBe(true);
    expect(plan.title).toContain("前端开发工程师");

    const saved = expectOk(await invoke<PracticePlan>("practice_plan_save", { input: plan }));
    expect(saved.id).toBe(plan.id);
    const plans = expectOk(await invoke<PracticePlanSummary[]>("practice_plan_list"));
    expect(plans.map((summary) => summary.id)).toContain(plan.id);

    expectOk(await invoke("practice_plan_delete", { planId: plan.id }));
    const afterDelete = expectOk(await invoke<PracticePlanSummary[]>("practice_plan_list"));
    expect(afterDelete.map((summary) => summary.id)).not.toContain(plan.id);
  });

  it("rejects practice session commands before a training starts", async () => {
    expectErr(await invoke("practice_session_progress", { sessionId: "sess-demo-1" }));
    expectErr(await invoke("practice_session_skip", { sessionId: "sess-demo-1" }));
    expectErr(await invoke("practice_session_turn_metrics", { sessionId: "sess-demo-1" }));
    expectErr(await invoke("practice_report_generate", { sessionId: "sess-demo-1" }));
    expectErr(await invoke("practice_report_get", { sessionId: "sess-demo-1" }));
  });

  it("starts a scripted training session and derives progress and metrics", async () => {
    const plans = expectOk(await invoke<PracticePlanSummary[]>("practice_plan_list"));
    const started = expectOk(
      await invoke<SessionStartResultLike>("practice_session_start", { input: { planId: plans[0].id } }),
    );
    if (started.kind !== "started") throw new Error("expected the demo training session to start");
    const sessionId = started.session.id;

    expectErr(await invoke("practice_session_progress", { sessionId: "sess-other" }));

    const progress = expectOk(await invoke<PracticeProgress>("practice_session_progress", { sessionId }));
    expect(progress.planId).toBe(plans[0].id);
    expect(progress.totalQuestions).toBe(5);
    expect(progress.finished).toBe(false);

    const skipped = expectOk(await invoke<PracticeProgress>("practice_session_skip", { sessionId }));
    expect(skipped.questionIndex).toBeGreaterThan(progress.questionIndex);

    const metrics = expectOk(await invoke("practice_session_turn_metrics", { sessionId }));
    expect(metrics).toBeNull(); // 脚本尚未产出候选人回答时无指标。

    // 伪造一条候选人回答：逐字流式落库后，进度推进、指标可算。
    expectOk(await invoke("session_finalize_utterance", { text: "嗯，那个项目就是支付网关，我负责限流。" }));
    await vi.waitFor(
      async () => {
        const advanced = expectOk(await invoke<PracticeProgress>("practice_session_progress", { sessionId }));
        expect(advanced.questionIndex).toBeGreaterThanOrEqual(progress.questionIndex);
        const answerMetrics = expectOk(await invoke<AnswerMetrics>("practice_session_turn_metrics", { sessionId }));
        expect(answerMetrics).not.toBeNull();
        expect(answerMetrics.chineseChars).toBeGreaterThan(0);
        expect(answerMetrics.fillers.some((hit) => hit.word === "那个")).toBe(true);
      },
      { timeout: 10_000 },
    );

    expectOk(await invoke("session_stop"));
    // 结束后进度仍可读（与真实后端一致：会话记录仍在）。
    const afterStop = expectOk(await invoke<PracticeProgress>("practice_session_progress", { sessionId }));
    expect(afterStop.finished).toBe(false);
  });

  it("generates a report from a training session and exports markdown", async () => {
    const plans = expectOk(await invoke<PracticePlanSummary[]>("practice_plan_list"));
    const started = expectOk(
      await invoke<SessionStartResultLike>("practice_session_start", { input: { planId: plans[0].id } }),
    );
    if (started.kind !== "started") throw new Error("expected the demo training session to start");
    const sessionId = started.session.id;
    expectOk(await invoke("session_finalize_utterance", { text: "我负责支付网关的限流模块，p99 下降 30%。" }));
    await vi.waitFor(
      async () => {
        const metrics = expectOk(await invoke<AnswerMetrics | null>("practice_session_turn_metrics", { sessionId }));
        expect(metrics).not.toBeNull();
      },
      { timeout: 10_000 },
    );

    const report = expectOk(await invoke<PracticeReport>("practice_report_generate", { sessionId }));
    expect(report.sessionId).toBe(sessionId);
    expect(report.llmAvailable).toBe(true);
    expect(report.perQuestion.length).toBeGreaterThan(0);
    expect(report.objective.answers.length).toBeGreaterThan(0);

    const fetched = expectOk(await invoke<PracticeReport>("practice_report_get", { sessionId }));
    expect(fetched.sessionId).toBe(sessionId);

    expectErr(await invoke("practice_report_export", { sessionId, format: "json" }));
    const exported = expectOk(await invoke<{ path: string }>("practice_report_export", { sessionId, format: "markdown" }));
    expect(exported.path).toContain(".md");

    expectOk(await invoke("session_stop"));
  });
});

type SessionStartResultLike = { kind: "started"; session: { id: string } } | { kind: "blocked"; issues: unknown[] };
