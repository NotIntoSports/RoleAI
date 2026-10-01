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
import { DEMO_SCRIPT_IDS, demoScriptById, scriptForRole } from "./scripts";
import { getState } from "./state";
import { latency, ok, err } from "./util";

import { demoT } from "./demo-text";

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
  // 映射集中在 scriptForRole（按角色 id / 场景，语言无关）；
  // 白名单校验返回的脚本 id 确实是已注册的演示脚本，否则回退面试官脚本。
  const mapped = scriptForRole(roleProfileId, getState().roleProfiles.find((role) => role.id === roleProfileId)?.scenario);
  return DEMO_SCRIPT_IDS.has(mapped.id) ? mapped : demoScriptById("interview-strict");
}

function transportOf(scriptId: string): string {
  return scriptId === "meeting" ? "meeting-bridge" : "realtime-e2e";
}

const persistence: LivePersistence = {
  appendTurn: (sessionId, userText, assistantText) => {
    appendLiveTurn(sessionId, userText, assistantText);
  },
  updateTurnAssistant: (sessionId, userText, assistantText, extras) => {
    updateLiveTurnAssistant(sessionId, userText, assistantText, extras);
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
  const text = demoT().live;
  const roleName =
    s.roleProfiles.find((role) => role.id === session?.roleProfileId)?.name ?? text.reportFallbackRole;
  return {
    report: {
      summary: text.reportSummary(turns.length, roleName),
      strengths: text.reportStrengths,
      followUps: text.reportFollowUps,
      limitations: text.reportLimitations,
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
        return err("SESSION_STATE_INVALID", demoT().live.noActiveSession);
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
    case "session_agent_command": {
      const input = payload.input as AgentCommandInput;
      const live = requireActive();
      if (!live) {
        return ok<AgentCommandResult>({
          commandId: input.id,
          action: input.action,
          ok: false,
          result: {},
          error: `SESSION_STATE_INVALID：${demoT().live.noActiveSession}`,
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
