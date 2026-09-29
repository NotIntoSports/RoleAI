// 实时会话命令（session_start/stop/set_mode/finalize/agent_command/audio/push）
// 以及活跃会话管理。事件通过官方 @tauri-apps/api/event 的 emit 发送，
// 由 mockIPC(shouldMockEvents) 分发给页面里的 listen 订阅。
import { emit } from "@tauri-apps/api/event";
import type {
  AgentCommandInput,
  AgentCommandResult,
  RuntimeStatus,
  SessionStartResult,
  SessionSummary,
  SessionTurnView,
} from "../../generated/bindings";

import type { LivePersistence } from "./live-session";
import { ScriptedLiveSession } from "./live-session";
import { appendLiveTurn, createLiveSession, finishLiveSession, updateLiveTurnAssistant } from "./records";
import { COACH_SCRIPT, INTERVIEW_SCRIPT, MEETING_SCRIPT, scriptForRole } from "./scripts";
import { getState } from "./state";
import { latency, ok, err } from "./util";

const EMPTY_TURN: SessionTurnView = {
  id: "demo-empty-turn",
  turnIndex: 0,
  userText: "",
  assistantText: "",
  materialsUsed: false,
  citations: [],
  createdAt: new Date(0).toISOString(),
};

function scriptFor(roleProfileId: string | null) {
  // 直播讲解员复用会议脚本（讲稿口播场景）；其余按角色/场景映射。
  if (roleProfileId === "preset-presenter") return MEETING_SCRIPT;
  const mapped = scriptForRole(roleProfileId, getState().roleProfiles.find((role) => role.id === roleProfileId)?.scenario);
  return mapped === COACH_SCRIPT || mapped === MEETING_SCRIPT || mapped === INTERVIEW_SCRIPT
    ? mapped
    : INTERVIEW_SCRIPT;
}

function transportOf(scriptId: string): string {
  return scriptId === "meeting" ? "meeting-bridge" : "realtime-e2e";
}

const persistence: LivePersistence = {
  appendTurn: (sessionId, userText, assistantText) => {
    appendLiveTurn(sessionId, userText, assistantText);
  },
  updateTurnAssistant: (sessionId, userText, assistantText) => {
    updateLiveTurnAssistant(sessionId, userText, assistantText);
  },
  finish: (sessionId) => {
    finishLiveSession(sessionId);
  },
};

let active: ScriptedLiveSession | null = null;
let activeSession: SessionSummary | null = null;

function lastTurnView(): SessionTurnView {
  if (!activeSession) return EMPTY_TURN;
  const turns = getState().sessionTurns[activeSession.id] ?? [];
  return turns.at(-1) ?? EMPTY_TURN;
}

function buildReport(): Record<string, unknown> {
  const s = getState();
  const session = activeSession;
  const turns = session ? s.sessionTurns[session.id] ?? [] : [];
  const roleName =
    s.roleProfiles.find((role) => role.id === session?.roleProfileId)?.name ?? "演示角色";
  return {
    report: {
      summary: `本场景共进行 ${turns.length} 轮对话（角色：${roleName}）。在线演示评分为固定脚本演示，不代表模型真实评价；桌面版会基于真实对话生成评分报告。`,
      strengths: [
        "核心概念边界清楚，能落到组件级方案",
        "面对追问能给出取舍理由，而不是只报结论",
      ],
      followUps: [
        "建议为高频追问准备量化数据（容量、延迟、成本）",
        "建议把一次线上问题的复盘整理成自己的方法论",
      ],
      limitations: [
        "演示环境未连接真实模型，评语为脚本内容，仅供参考",
      ],
    },
  };
}

function agentResult(input: AgentCommandInput, result: Record<string, unknown> = {}): AgentCommandResult {
  return {
    commandId: input.id,
    action: input.action,
    ok: true,
    result,
    error: "",
  };
}

/** 活跃会话的运行状态；无活跃会话时返回 null（调用方回退到 idle）。 */
export function liveRuntimeStatus(): RuntimeStatus | null {
  if (active) return active.getStatus();
  return null;
}

function requireActive(): ScriptedLiveSession | null {
  return active;
}

export function handleLiveSessionCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "session_start": {
      const roleProfileId = (payload.roleProfileId as string | undefined) ?? getState().activeRoleProfileId;
      const script = scriptFor(roleProfileId ?? null);
      // 重复点击“开始会话”时先收掉上一场。
      if (active) active.stop();
      const session = createLiveSession(roleProfileId ?? "preset-strict-interviewer", transportOf(script.id));
      active = new ScriptedLiveSession(session.id, script, { emitEvent: emit }, persistence);
      activeSession = { ...session };
      active.begin();
      return latency(120, 320).then(() =>
        ok({ kind: "started", session } satisfies SessionStartResult),
      );
    }
    case "session_stop": {
      if (!active || !activeSession) {
        return err("SESSION_STATE_INVALID", "当前没有进行中的会话。");
      }
      active.stop();
      const summary = finishLiveSession(activeSession.id);
      active = null;
      activeSession = null;
      return latency(80, 200).then(() => ok(summary));
    }
    case "session_set_mode": {
      const mode = payload.mode as string;
      const live = requireActive();
      live?.setMode(mode);
      if (activeSession) activeSession = { ...activeSession, status: mode === "ai_active" ? "listening" : "paused" };
      return latency(40, 120).then(() =>
        ok(live ? live.getStatus() : ({
          phase: "idle",
          mode,
          seq: 0,
          unusedMaterials: false,
          lastErrorCode: null,
          revision: 0,
          realtimeStatus: "idle",
        } satisfies RuntimeStatus)),
      );
    }
    case "session_finalize_utterance": {
      const text = String(payload.text ?? "").trim();
      const live = requireActive();
      if (!text || !live) return latency(20, 60).then(() => ok(lastTurnView()));
      // 文字输入与语音同一管线：插话进入串行队列，后台驱动回答流式上屏。
      live.submitUserText(text);
      return latency(30, 90).then(() => ok(lastTurnView()));
    }
    case "session_trigger_assistant": {
      const live = requireActive();
      if (!live) return err("SESSION_STATE_INVALID", "当前没有进行中的会话。");
      live.submitUserText("@会议助手 请继续");
      return latency(30, 90).then(() => ok(lastTurnView()));
    }
    case "session_agent_command": {
      const input = payload.input as AgentCommandInput;
      const live = requireActive();
      if (!live) {
        return ok<AgentCommandResult>({
          commandId: input.id,
          action: input.action,
          ok: false,
          result: {},
          error: "SESSION_STATE_INVALID：当前没有进行中的会话",
        });
      }
      switch (input.action) {
        case "report":
          return latency(200, 500).then(() => ok(agentResult(input, buildReport())));
        case "retry":
          live.retryLast();
          return latency(40, 100).then(() => ok(agentResult(input)));
        case "say":
          if (input.text) live.submitUserText(input.text);
          return latency(40, 100).then(() => ok(agentResult(input)));
        default:
          return latency(40, 100).then(() => ok(agentResult(input)));
      }
    }
    case "session_audio_ready":
      return ok({ ready: true });
    case "session_push_mic_pcm":
      return ok({ accepted: true });
    case "session_push_video_frame":
      return ok({ accepted: true });
    default:
      return undefined;
  }
}
