// 会话记录命令：session_list / get / export / delete。
// 同时为 B05 的脚本化实时会话提供落库助手（createLiveSession 等）。
import type { SessionDetail, SessionSummary, SessionTurnView, TurnLatencyView } from "../../generated/bindings";

import { getState, updateState } from "./state";
import { demoId, err, latency, ok } from "./util";

function roleLabel(roleProfileId: string): string {
  return getState().roleProfiles.find((role) => role.id === roleProfileId)?.name ?? roleProfileId;
}

function sessionToMarkdown(detail: SessionDetail): string {
  const { session, turns } = detail;
  const lines: string[] = [
    `# 会话记录 ${session.id}`,
    "",
    `- 角色：${roleLabel(session.roleProfileId)}`,
    `- 状态：${session.status}`,
    `- 开始：${session.startedAt ?? "-"}`,
    `- 结束：${session.finishedAt ?? "-"}`,
    `- 传输：${session.transportMode}`,
    "",
    "> 在线演示数据，全部内容均为虚构。",
    "",
  ];
  for (const turn of turns) {
    lines.push(`## 轮 ${turn.turnIndex}`, "", `**用户**：${turn.userText}`, "", `**助手**：${turn.assistantText}`, "");
    if (turn.citations.length) {
      lines.push("引用片段：");
      for (const citation of turn.citations) {
        lines.push(`- [${citation.materialId}] ${citation.snippet}`);
      }
      lines.push("");
    }
  }
  return lines.join("\n");
}

/** 在线演示：用浏览器标准下载能力导出（桌面版写本地文件，演示不写磁盘）。 */
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
    // 下载失败不影响返回值；界面仍展示虚拟路径。
  }
}

/** B05 实时会话：创建一条进行中的会话（status 为工作台的活跃相位）。 */
export function createLiveSession(roleProfileId: string, transportMode: string): SessionSummary {
  const now = new Date().toISOString();
  const session: SessionSummary = {
    id: demoId("session"),
    status: "listening",
    roleProfileId,
    voiceRouteId: getState().activeVoiceRouteId ?? "route-demo-realtime",
    transportMode,
    startedAt: now,
    finishedAt: null,
    updatedAt: now,
  };
  updateState((s) => {
    s.sessions = [session, ...s.sessions];
    s.sessionTurns[session.id] = [];
  });
  return session;
}

/** B05 实时会话：把一轮问答落库（可带资料引用）。 */
export function appendLiveTurn(
  sessionId: string,
  userText: string,
  assistantText: string,
  extras?: { materialsUsed?: boolean; citations?: SessionTurnView["citations"]; latency?: TurnLatencyView },
): SessionTurnView {
  const now = new Date().toISOString();
  const turn: SessionTurnView = {
    id: demoId("turn"),
    turnIndex: (getState().sessionTurns[sessionId]?.length ?? 0) + 1,
    userText,
    assistantText,
    materialsUsed: extras?.materialsUsed ?? false,
    citations: extras?.citations ?? [],
    createdAt: now,
  };
  updateState((s) => {
    s.sessionTurns[sessionId] = [...(s.sessionTurns[sessionId] ?? []), turn];
    const index = s.sessions.findIndex((item) => item.id === sessionId);
    if (index !== -1) s.sessions[index] = { ...s.sessions[index], updatedAt: now };
  });
  return turn;
}

/** B05 实时会话：回填最近一条等待回答的轮的助手文本（流式 done 时调用）。 */
export function updateLiveTurnAssistant(
  sessionId: string,
  userText: string,
  assistantText: string,
  extras?: { materialsUsed?: boolean; citations?: SessionTurnView["citations"]; latency?: TurnLatencyView },
): void {
  const now = new Date().toISOString();
  updateState((s) => {
    const turns = s.sessionTurns[sessionId] ?? [];
    // 手写倒序查找（lib 是 ES2022，没有 findLastIndex）。
    let index = -1;
    for (let i = turns.length - 1; i >= 0; i -= 1) {
      if (turns[i].userText === userText) {
        index = i;
        break;
      }
    }
    if (index === -1) {
      s.sessionTurns[sessionId] = [
        ...turns,
        {
          id: demoId("turn"),
          turnIndex: turns.length + 1,
          userText,
          assistantText,
          materialsUsed: extras?.materialsUsed ?? false,
          citations: extras?.citations ?? [],
          createdAt: now,
          latency: extras?.latency,
        },
      ];
      return;
    }
    const updated: SessionTurnView = {
      ...turns[index],
      assistantText,
      materialsUsed: turns[index].materialsUsed || Boolean(extras?.materialsUsed),
      citations: turns[index].citations.length ? turns[index].citations : extras?.citations ?? [],
      latency: extras?.latency ?? turns[index].latency,
    };
    s.sessionTurns[sessionId] = turns.map((turn, i) => (i === index ? updated : turn));
  });
}

/** B05 实时会话：结束会话。 */
export function finishLiveSession(sessionId: string, status = "completed"): SessionSummary {
  const now = new Date().toISOString();
  return updateState((s) => {
    const index = s.sessions.findIndex((item) => item.id === sessionId);
    if (index === -1) return null;
    const finished: SessionSummary = {
      ...s.sessions[index],
      status,
      finishedAt: now,
      updatedAt: now,
    };
    s.sessions[index] = finished;
    return finished;
  }).sessions.find((item) => item.id === sessionId)!;
}

export function handleRecordCommand(cmd: string, payload: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "session_list":
      return latency(20, 80).then(() =>
        ok(
          [...getState().sessions].sort((a, b) =>
            (b.startedAt ?? b.updatedAt).localeCompare(a.startedAt ?? a.updatedAt),
          ),
        ),
      );
    case "session_get": {
      const sessionId = payload.sessionId as string;
      const s = getState();
      const session = s.sessions.find((item) => item.id === sessionId);
      if (!session) return err("SESSION_NOT_FOUND", "找不到该会话记录。");
      const detail: SessionDetail = { session, turns: s.sessionTurns[sessionId] ?? [] };
      return latency(30, 120).then(() => ok(detail));
    }
    case "session_export": {
      const sessionId = payload.sessionId as string;
      const format = payload.format as string;
      const s = getState();
      const session = s.sessions.find((item) => item.id === sessionId);
      if (!session) return err("SESSION_NOT_FOUND", "找不到该会话记录。");
      const detail: SessionDetail = { session, turns: s.sessionTurns[sessionId] ?? [] };
      return latency(120, 400).then(() => {
        const base = `roleai-${sessionId}`;
        if (format === "json") {
          const fileName = `${base}.json`;
          downloadMarkdown(fileName, JSON.stringify(detail, null, 2));
          return ok({ path: `已通过浏览器下载：${fileName}（在线演示不写入本地磁盘）` });
        }
        const fileName = `${base}.${format === "text" ? "txt" : "md"}`;
        const content =
          format === "text"
            ? sessionToMarkdown(detail).replaceAll("**", "").replaceAll(/^#+ /gm, "")
            : sessionToMarkdown(detail);
        downloadMarkdown(fileName, content);
        return ok({ path: `已通过浏览器下载：${fileName}（在线演示不写入本地磁盘）` });
      });
    }
    case "session_delete": {
      const sessionId = payload.sessionId as string;
      return latency(60, 160).then(() => {
        updateState((s) => {
          s.sessions = s.sessions.filter((item) => item.id !== sessionId);
          delete s.sessionTurns[sessionId];
        });
        return ok({ ready: true });
      });
    }
    default:
      return undefined;
  }
}
