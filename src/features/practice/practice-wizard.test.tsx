import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as commands from "../../api/commands";
import type { MaterialSummary, PracticePlan, SessionSummary } from "../../generated/bindings";
import { PracticeWizard } from "./practice-wizard";

vi.mock("../../api/commands", () => ({
  listMaterials: vi.fn(),
  generatePracticePlan: vi.fn(),
  savePracticePlan: vi.fn(),
  startPracticeSession: vi.fn(),
}));

function material(overrides: Partial<MaterialSummary> = {}): MaterialSummary {
  return {
    id: "mat-jd",
    fileName: "jd-backend.md",
    contentSha256: "sha-jd",
    mediaType: "text/markdown",
    byteSize: 256,
    status: "vector_ready",
    chunkCount: 4,
    ...overrides,
  };
}

function plan(overrides: Partial<PracticePlan> = {}): PracticePlan {
  return {
    id: "plan-1",
    title: "后端开发工程师·面试官",
    position: "后端开发工程师",
    interviewerStyle: "面试官",
    difficulty: "标准",
    questions: [
      { prompt: "介绍一个你负责过的项目。", focus: "项目经验", expectedPoints: ["背景", "职责"], followups: ["最大的困难是什么？"] },
      { prompt: "如何设计一个高并发订单服务？", focus: "系统设计", expectedPoints: [], followups: [] },
    ],
    createdAt: "2026-10-01T00:00:00Z",
    updatedAt: "2026-10-01T00:00:00Z",
    ...overrides,
  };
}

const PANEL_LABELS = ["岗位与资料", "面试官风格", "题量与时长", "预览并编辑题单"] as const;

function summarySession(): SessionSummary {
  return {
    id: "sess-practice-1",
    status: "listening",
    roleProfileId: "role-1",
    voiceRouteId: "route-1",
    transportMode: "direct",
    startedAt: "2026-10-01T00:00:00Z",
    finishedAt: null,
    updatedAt: "2026-10-01T00:00:00Z",
  };
}

async function reachStep(index: number) {
  if (index >= 1) {
    fireEvent.input(screen.getByLabelText("岗位方向"), { target: { value: "后端开发工程师" } });
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
  }
  if (index >= 2) {
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
  }
  if (index >= 3) {
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
  }
  await waitFor(() => expect(screen.getByLabelText(PANEL_LABELS[index])).toBeTruthy());
}

describe("PracticeWizard", () => {
  beforeEach(() => {
    vi.mocked(commands.listMaterials).mockResolvedValue({ ok: true, data: [] });
  });
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("loads materials and blocks 下一步 until position is filled", async () => {
    render(<PracticeWizard />);
    expect(screen.getByRole("status").textContent).toContain("正在读取本地资料");
    expect(await screen.findByText("还没有资料。")).toBeTruthy();
    const next = screen.getByRole("button", { name: "下一步" });
    expect(next).toBeDisabled();
    fireEvent.input(screen.getByLabelText("岗位方向"), { target: { value: "后端开发工程师" } });
    expect(next).not.toBeDisabled();
  });

  it("guides to the materials page when no materials exist", async () => {
    render(<PracticeWizard materialsLink="/materials" />);
    await screen.findByText("还没有资料。");
    const link = screen.getByRole("link", { name: "去资料页导入 JD 或简历" });
    expect(link.getAttribute("href")).toBe("/materials");
  });

  it("lists loaded materials in the JD and resume selects", async () => {
    vi.mocked(commands.listMaterials).mockResolvedValue({
      ok: true,
      data: [material(), material({ id: "mat-resume", fileName: "resume.md" })],
    });
    render(<PracticeWizard />);
    expect(await screen.findByLabelText("岗位 JD（可选）")).toBeTruthy();
    const jd = screen.getByLabelText("岗位 JD（可选）") as HTMLSelectElement;
    const resume = screen.getByLabelText("个人简历（可选）") as HTMLSelectElement;
    expect(jd.textContent).toContain("jd-backend.md");
    expect(resume.textContent).toContain("resume.md");
  });

  it("surfaces a materials load error but still allows proceeding", async () => {
    vi.mocked(commands.listMaterials).mockResolvedValue({
      ok: false,
      error: { code: "DATABASE_OPERATION_FAILED", message: "数据库不可用", requestId: "r1", field: "database", retryable: true },
    });
    render(<PracticeWizard />);
    expect(await screen.findByText(/DATABASE_OPERATION_FAILED/)).toBeTruthy();
    fireEvent.input(screen.getByLabelText("岗位方向"), { target: { value: "后端" } });
    expect(screen.getByRole("button", { name: "下一步" })).not.toBeDisabled();
  });

  it("walks all four steps with style and count selections", async () => {
    render(<PracticeWizard />);
    await screen.findByText("还没有资料。");
    fireEvent.input(screen.getByLabelText("岗位方向"), { target: { value: "后端开发工程师" } });
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));

    expect(screen.queryByLabelText("岗位与资料")).toBeNull();
    const stylePanel = screen.getByLabelText("面试官风格");
    expect(stylePanel.textContent).toContain("面试官");
    expect(stylePanel.textContent).toContain("HR");
    expect(stylePanel.textContent).toContain("严苛面试官");
    const strict = screen.getByLabelText(/严苛面试官/);
    fireEvent.click(strict);
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));

    const countPanel = screen.getByLabelText("题量与时长");
    expect(countPanel.textContent).toContain("15~25 分钟");
    const count = screen.getByLabelText("题量（1~12 题）") as HTMLInputElement;
    fireEvent.input(count, { target: { value: "8" } });
    expect(countPanel.textContent).toContain("24~40 分钟");
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));

    const editPanel = screen.getByLabelText("预览并编辑题单");
    expect(editPanel.textContent).toContain("后端开发工程师");
    expect(editPanel.textContent).toContain("严苛面试官");
    expect(editPanel.textContent).toContain("8 题");
  });

  it("returns to a previous step via 上一步", async () => {
    render(<PracticeWizard />);
    await reachStep(2);
    fireEvent.click(screen.getByRole("button", { name: "上一步" }));
    expect(screen.getByLabelText("面试官风格")).toBeTruthy();
  });

  it("generates a plan with the wizard selections", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({ ok: true, data: plan() });
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    await waitFor(() => expect(commands.generatePracticePlan).toHaveBeenCalledTimes(1));
    expect(commands.generatePracticePlan).toHaveBeenCalledWith({
      position: "后端开发工程师",
      jdMaterialId: undefined,
      resumeMaterialId: undefined,
      interviewerStyle: "面试官",
      questionCount: 5,
      difficulty: "标准",
    });
    expect(await screen.findByText("介绍一个你负责过的项目。")).toBeTruthy();
    expect(screen.getByText("2 道题")).toBeTruthy();
  });

  it("passes selected material ids to generation", async () => {
    vi.mocked(commands.listMaterials).mockResolvedValue({
      ok: true,
      data: [material(), material({ id: "mat-resume", fileName: "resume.md" })],
    });
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({ ok: true, data: plan() });
    render(<PracticeWizard />);
    await screen.findByLabelText("岗位 JD（可选）");
    fireEvent.change(screen.getByLabelText("岗位 JD（可选）"), { target: { value: "mat-jd" } });
    fireEvent.change(screen.getByLabelText("个人简历（可选）"), { target: { value: "mat-resume" } });
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    await waitFor(() => expect(commands.generatePracticePlan).toHaveBeenCalled());
    expect(commands.generatePracticePlan).toHaveBeenCalledWith(
      expect.objectContaining({ jdMaterialId: "mat-jd", resumeMaterialId: "mat-resume" }),
    );
  });

  it("surfaces a generation error", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({
      ok: false,
      error: { code: "PRACTICE_MODEL_RESPONSE_INVALID", message: "题单生成失败，可稍后重试", requestId: "r1", retryable: true },
    });
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    expect((await screen.findByRole("status")).textContent).toContain("PRACTICE_MODEL_RESPONSE_INVALID");
  });

  it("edits a prompt and saves the plan", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({ ok: true, data: plan() });
    vi.mocked(commands.savePracticePlan).mockImplementation(async (input) => ({ ok: true, data: input }));
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    const textarea = await screen.findByLabelText("第 1 题题面");
    fireEvent.input(textarea, { target: { value: "介绍一个你主导过的项目。" } });
    fireEvent.click(screen.getByRole("button", { name: "保存题单" }));
    await waitFor(() => expect(commands.savePracticePlan).toHaveBeenCalledTimes(1));
    const saved = vi.mocked(commands.savePracticePlan).mock.calls[0][0];
    expect(saved.questions[0].prompt).toBe("介绍一个你主导过的项目。");
    expect(await screen.findByText("题单已保存，可直接开始训练。")).toBeTruthy();
  });

  it("starts training: saves the unsaved plan first, then starts a practice session", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({ ok: true, data: plan() });
    vi.mocked(commands.savePracticePlan).mockImplementation(async (input) => ({ ok: true, data: input }));
    vi.mocked(commands.startPracticeSession).mockResolvedValue({
      ok: true,
      data: { kind: "started", session: summarySession() },
    });
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    await screen.findByText("介绍一个你负责过的项目。");
    fireEvent.click(screen.getByRole("button", { name: "开始训练" }));
    await waitFor(() => expect(commands.startPracticeSession).toHaveBeenCalledWith({ planId: "plan-1" }));
    // 先保存题单（训练按已保存的题单 id 启动）。
    expect(commands.savePracticePlan).toHaveBeenCalledTimes(1);
  });

  it("starts training directly when the plan is already saved", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({ ok: true, data: plan() });
    vi.mocked(commands.savePracticePlan).mockImplementation(async (input) => ({ ok: true, data: input }));
    vi.mocked(commands.startPracticeSession).mockResolvedValue({
      ok: true,
      data: { kind: "started", session: summarySession() },
    });
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    await screen.findByText("介绍一个你负责过的项目。");
    fireEvent.click(screen.getByRole("button", { name: "保存题单" }));
    await screen.findByText("题单已保存，可直接开始训练。");
    fireEvent.click(screen.getByRole("button", { name: "开始训练" }));
    await waitFor(() => expect(commands.startPracticeSession).toHaveBeenCalledWith({ planId: "plan-1" }));
    expect(commands.savePracticePlan).toHaveBeenCalledTimes(1);
  });

  it("surfaces a session-start failure instead of navigating", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({ ok: true, data: plan() });
    vi.mocked(commands.savePracticePlan).mockImplementation(async (input) => ({ ok: true, data: input }));
    vi.mocked(commands.startPracticeSession).mockResolvedValue({
      ok: false,
      error: { code: "SESSION_ROLE_REQUIRED", message: "请选择有效的会话角色", requestId: "r", retryable: false },
    });
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    await screen.findByText("介绍一个你负责过的项目。");
    fireEvent.click(screen.getByRole("button", { name: "开始训练" }));
    const notice = await screen.findByText(/SESSION_ROLE_REQUIRED/);
    expect(notice.className).toContain("services-message");
  });

  it("blocks saving when every question is removed", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({
      ok: true,
      data: plan({ questions: [plan().questions[0]] }),
    });
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    await screen.findByText("介绍一个你负责过的项目。");
    fireEvent.click(screen.getByRole("button", { name: "删除第 1 题" }));
    const save = screen.getByRole("button", { name: "保存题单" });
    expect(save).toBeDisabled();
    expect(commands.savePracticePlan).not.toHaveBeenCalled();
  });

  it("blocks saving when a prompt is cleared", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({ ok: true, data: plan() });
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    const textarea = await screen.findByLabelText("第 1 题题面");
    fireEvent.input(textarea, { target: { value: "   " } });
    const save = screen.getByRole("button", { name: "保存题单" });
    expect(save).toBeDisabled();
  });

  it("keeps 重新生成 working after an edit", async () => {
    vi.mocked(commands.generatePracticePlan).mockResolvedValue({ ok: true, data: plan() });
    render(<PracticeWizard />);
    await reachStep(3);
    fireEvent.click(screen.getByRole("button", { name: "生成题单" }));
    await screen.findByText("介绍一个你负责过的项目。");
    fireEvent.click(screen.getByRole("button", { name: "重新生成" }));
    await waitFor(() => expect(commands.generatePracticePlan).toHaveBeenCalledTimes(2));
  });
});
