import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { CommandResult, SessionStartResult } from "../../generated/bindings";

import { chunkText, ScriptedLiveSession, type LivePersistence } from "./live-session";
import { handleDemoInvoke } from "./index";
import {
  appendLiveTurn,
  createLiveSession,
  finishLiveSession,
  updateLiveTurnAssistant,
} from "./records";
import { COACH_SCRIPT, INTERVIEW_SCRIPT, MEETING_SCRIPT, scriptForRole } from "./scripts";
import { getState, resetDemoState } from "./state";

const NO_PERSISTENCE: LivePersistence = {
  appendTurn: () => undefined,
  updateTurnAssistant: () => undefined,
  finish: () => undefined,
};

const REAL_PERSISTENCE: LivePersistence = {
  appendTurn: (sessionId, userText) => {
    appendLiveTurn(sessionId, userText, "");
  },
  updateTurnAssistant: (sessionId, userText, assistantText, extras) => {
    updateLiveTurnAssistant(sessionId, userText, assistantText, extras);
  },
  finish: (sessionId) => {
    finishLiveSession(sessionId);
  },
};

interface DemoEvent {
  event: string;
  payload: Record<string, unknown>;
}

function collectedEmitter(events: DemoEvent[]) {
  return (event: string, payload: unknown) => {
    events.push({ event, payload: payload as Record<string, unknown> });
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  window.localStorage.clear();
  resetDemoState();
});

afterEach(() => {
  vi.useRealTimers();
});

async function driveUntil(condition: () => boolean, maxMs = 60000) {
  let elapsed = 0;
  while (!condition() && elapsed < maxMs) {
    await vi.advanceTimersByTimeAsync(120);
    elapsed += 120;
  }
  expect(condition()).toBe(true);
}

describe("chunkText", () => {
  it("groups CJK characters and keeps latin words whole", () => {
    expect(chunkText("你好世界")).toEqual(["你好", "世界"]);
    expect(chunkText("用 Java 加 Redis 实现")).toEqual(["用 ", "Java ", "加 ", "Redis ", "实现"]);
    expect(chunkText("")).toEqual([]);
  });
});

describe("ScriptedLiveSession", () => {
  it("streams a scripted turn: status → transcript partial/done → thinking → speaking → reply done, persisting the turn", async () => {
    const events: DemoEvent[] = [];
    const session = createLiveSession("preset-strict-interviewer", "realtime-e2e");
    const live = new ScriptedLiveSession(
      session.id,
      { id: "test", turns: [{ userText: "你好世界", replyText: "好的收到" }] },
      { emitEvent: collectedEmitter(events), random: () => 0.5 },
      REAL_PERSISTENCE,
    );
    live.begin();
    await driveUntil(() => {
      const turns = getState().sessionTurns[session.id] ?? [];
      return turns.length === 1 && turns[0].assistantText === "好的收到";
    });

    const names = events.map((e) => e.event);
    expect(names[0]).toBe("runtime:status:v1");
    expect(names).toContain("session:transcript:v1");
    expect(names).toContain("session:reply:v1");
    const statusPhases = events
      .filter((e) => e.event === "runtime:status:v1")
      .map((e) => (e.payload as { phase: string }).phase);
    expect(statusPhases).toEqual(["listening", "listening", "thinking", "speaking", "listening"]);
    const transcriptDone = events.find(
      (e) => e.event === "session:transcript:v1" && (e.payload as { done: boolean }).done,
    );
    expect((transcriptDone!.payload as { text: string }).text).toBe("你好世界");
    const replyDone = events.find(
      (e) => e.event === "session:reply:v1" && (e.payload as { done: boolean }).done,
    );
    expect((replyDone!.payload as { text: string }).text).toBe("好的收到");
    // 事件序号单调递增（前端的 seq 门控依赖这一点）。
    const replySeqs = events
      .filter((e) => e.event === "session:reply:v1")
      .map((e) => (e.payload as { seq: number }).seq);
    expect([...replySeqs].sort((a, b) => a - b)).toEqual(replySeqs);
    expect(live.getStatus().phase).toBe("listening");
    expect(live.getStatus().realtimeStatus).toBe("connected");
  });

  it("cuts the reply and clears playback when the turn demonstrates a barge-in", async () => {
    const events: DemoEvent[] = [];
    const session = createLiveSession("preset-strict-interviewer", "realtime-e2e");
    const live = new ScriptedLiveSession(
      session.id,
      {
        id: "test",
        turns: [
          {
            userText: "请问缓存击穿怎么处理？",
            replyText: "这是一个用来演示打断逻辑的比较长的回答内容",
            interruptAfterChars: 4,
          },
        ],
      },
      { emitEvent: collectedEmitter(events), random: () => 0.5 },
      NO_PERSISTENCE,
    );
    live.begin();
    await driveUntil(() => {
      const clears = events.filter(
        (e) => e.event === "session:playback-control:v1" && (e.payload as { action: string }).action === "clear",
      );
      return clears.length === 1;
    });

    const lastReply = [...events]
      .reverse()
      .find((e) => e.event === "session:reply:v1")!;
    expect((lastReply.payload as { text: string; done: boolean }).done).toBe(true);
    expect((lastReply.payload as { text: string }).text.length).toBe(4);
    const phaseAfter = live.getStatus().phase;
    expect(phaseAfter).toBe("listening");
  });

  it("pauses the timeline on non-ai_active modes and resumes on ai_active", async () => {
    const events: DemoEvent[] = [];
    const live = new ScriptedLiveSession(
      "session-test-pause",
      { id: "test", turns: [{ userText: "你好世界", replyText: "好的收到" }] },
      { emitEvent: collectedEmitter(events), random: () => 0.5 },
      NO_PERSISTENCE,
    );
    live.begin();
    await vi.advanceTimersByTimeAsync(300);
    live.setMode("operator_speaking");
    const countWhenPaused = events.length;
    await vi.advanceTimersByTimeAsync(5000);
    expect(events.length).toBe(countWhenPaused);
    live.setMode("ai_active");
    await driveUntil(() => events.some((e) => e.event === "session:reply:v1" && (e.payload as { done: boolean }).done));
  });

  it("stops cleanly: pending timers cancelled, no further events, session finished", async () => {
    const events: DemoEvent[] = [];
    const finished: string[] = [];
    const live = new ScriptedLiveSession(
      "session-test-stop",
      {
        id: "test",
        turns: INTERVIEW_SCRIPT.turns,
      },
      { emitEvent: collectedEmitter(events), random: () => 0.5 },
      { ...NO_PERSISTENCE, finish: (sessionId) => finished.push(sessionId) },
    );
    live.begin();
    await vi.advanceTimersByTimeAsync(1500);
    live.stop();
    const countAtStop = events.length;
    expect(finished).toEqual(["session-test-stop"]);
    await vi.advanceTimersByTimeAsync(10000);
    expect(events.length).toBe(countAtStop);
    expect(live.getStatus().phase).toBe("idle");
  });

  it("answers ad-hoc user text via submitUserText and retries the last turn", async () => {
    const events: DemoEvent[] = [];
    const live = new ScriptedLiveSession(
      "session-test-chat",
      { id: "test", turns: [] },
      { emitEvent: collectedEmitter(events), random: () => 0.5 },
      NO_PERSISTENCE,
    );
    live.begin();
    await vi.advanceTimersByTimeAsync(800);
    live.submitUserText("我自己输入的问题");
    await driveUntil(() =>
      events.some((e) => e.event === "session:reply:v1" && (e.payload as { done: boolean }).done),
    );
    const replyCount = events.filter((e) => e.event === "session:reply:v1").length;
    live.retryLast();
    await driveUntil(() => events.filter((e) => e.event === "session:reply:v1").length > replyCount);
  });
});

describe("script selection", () => {
  it("maps roles to the three scripts", () => {
    expect(scriptForRole("preset-strict-interviewer", undefined)).toBe(INTERVIEW_SCRIPT);
    expect(scriptForRole("preset-expression-coach", undefined)).toBe(COACH_SCRIPT);
    expect(scriptForRole("preset-meeting", "meetingAssistant")).toBe(MEETING_SCRIPT);
    expect(scriptForRole(null, undefined)).toBe(INTERVIEW_SCRIPT);
  });
});

describe("live session commands", () => {
  beforeEach(() => {
    vi.useRealTimers();
    window.localStorage.clear();
    resetDemoState();
    // 会话命令用官方 mockIPC 提供事件总线（与演示入口同一条路径）。
    mockIPC(() => ({}), { shouldMockEvents: true });
  });

  afterEach(() => {
    clearMocks();
  });

  it("session_start returns a started session in the listening phase", async () => {
    const result = (await handleDemoInvoke("session_start", {
      roleProfileId: "preset-strict-interviewer",
      voiceRouteId: "route-demo-realtime",
    })) as CommandResult<SessionStartResult>;
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.data.kind).toBe("started");
    if (result.data.kind !== "started") return;
    expect(result.data.session.status).toBe("listening");
  });

  it("finalize with empty text is a harmless no-op (auto-finalize loop)", async () => {
    const result = (await handleDemoInvoke("session_finalize_utterance", { text: "" })) as CommandResult<unknown>;
    expect(result.ok).toBe(true);
  });

  it("agent command report returns a fictional structured report", async () => {
    (await handleDemoInvoke("session_start", { roleProfileId: "preset-strict-interviewer" })) as CommandResult<unknown>;
    const result = (await handleDemoInvoke("session_agent_command", {
      input: { id: "cmd-1", action: "report", text: null, answer: null, mode: null, expectedRevision: 0 },
    })) as CommandResult<{ ok: boolean; result: { report: Record<string, unknown> } }>;
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.data.ok).toBe(true);
    const report = result.data.result.report;
    expect(Array.isArray(report.strengths)).toBe(true);
    expect(String(report.summary)).toContain("演示");
  });

  it("stop without an active session reports a stable error", async () => {
    // 前面的用例可能留下了活跃会话（模块级状态），先收掉再断言空态报错。
    await handleDemoInvoke("session_stop", {});
    const result = (await handleDemoInvoke("session_stop", {})) as CommandResult<never>;
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error.code).toBe("SESSION_STATE_INVALID");
  });
});

describe("ScriptedLiveSession 延迟时间线（lane-F F07）", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    window.localStorage.clear();
    resetDemoState();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("脚本轮落库时带上单调的时间线与模式标注", async () => {
    const session = createLiveSession("preset-strict-interviewer", "realtime-e2e");
    const live = new ScriptedLiveSession(
      session.id,
      { id: "test", turns: [{ userText: "你好世界", replyText: "好的收到" }] },
      { emitEvent: collectedEmitter([]), random: () => 0.5 },
      REAL_PERSISTENCE,
    );
    live.begin();
    await driveUntil(() => {
      const turns = getState().sessionTurns[session.id] ?? [];
      return turns.length === 1 && turns[0].latency != null;
    });

    const latency = getState().sessionTurns[session.id]![0].latency!;
    expect(latency.mode).toBe("realtime");
    expect(latency.routeId).toBe("route-demo-realtime");
    expect(latency.interrupted).toBe(false);
    const timeline = latency.timeline;
    for (const anchor of [
      timeline.speechStartedMs,
      timeline.speechStoppedMs,
      timeline.transcriptDoneMs,
      timeline.responseCreatedMs,
      timeline.firstAudioMs,
      timeline.responseDoneMs,
    ]) {
      expect(anchor).toBeTypeOf("number");
    }
    const values = [
      timeline.speechStartedMs!,
      timeline.speechStoppedMs!,
      timeline.transcriptDoneMs!,
      timeline.responseCreatedMs!,
      timeline.firstAudioMs!,
      timeline.responseDoneMs!,
    ];
    expect([...values].sort((a, b) => a - b)).toEqual(values);
    // 演示「首响延迟」= 请求发出 → 首包音频，与桌面实时线路口径一致（0.4～0.9s 量级）。
    const firstResponse = timeline.firstAudioMs! - timeline.responseCreatedMs!;
    expect(firstResponse).toBeGreaterThanOrEqual(400);
    expect(firstResponse).toBeLessThan(1_200);
  });

  it("被打断的演示轮在延迟视图上标注 interrupted", async () => {
    const session = createLiveSession("preset-strict-interviewer", "realtime-e2e");
    const live = new ScriptedLiveSession(
      session.id,
      {
        id: "test",
        turns: [
          {
            userText: "请问缓存击穿怎么处理？",
            replyText: "这是一个用来演示打断逻辑的比较长的回答内容",
            interruptAfterChars: 4,
          },
        ],
      },
      { emitEvent: collectedEmitter([]), random: () => 0.5 },
      REAL_PERSISTENCE,
    );
    live.begin();
    await driveUntil(() => {
      const turns = getState().sessionTurns[session.id] ?? [];
      return turns.length === 1 && turns[0].latency != null;
    });
    expect(getState().sessionTurns[session.id]![0].latency!.interrupted).toBe(true);
  });
});
