import { Fragment, useEffect, useRef, type FormEvent } from "react";
import { Bot, Copy, MessageSquare, Volume2 } from "lucide-react";

import { clockOf } from "./workspace-format";
import type { SessionTurnView } from "../../generated/bindings";

interface TranscriptPanelProps {
  transcript: string;
  reply: string;
  turns: SessionTurnView[];
  historyTurns: SessionTurnView[];
  roleName: string;
  welcomeRoleName: string;
  active: boolean;
  pendingConfirmation: boolean;
  confirmationText: string;
  onConfirmationTextChange: (value: string) => void;
  onConfirmCandidate: (event: FormEvent) => void;
  busy: boolean;
  unusedMaterials: boolean;
  onCopy: (text: string) => void;
  onAdjustConfiguration: () => void;
}

export function TranscriptPanel({
  transcript,
  reply,
  turns,
  historyTurns,
  roleName,
  welcomeRoleName,
  active,
  pendingConfirmation,
  confirmationText,
  onConfirmationTextChange,
  onConfirmCandidate,
  busy,
  unusedMaterials,
  onCopy,
  onAdjustConfiguration,
}: TranscriptPanelProps) {
  const conversationRef = useRef<HTMLDivElement | null>(null);
  const conversationBottomRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const container = conversationRef.current;
    const bottom = conversationBottomRef.current;
    if (!container || !bottom) return;
    const nearBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 400;
    if (nearBottom && typeof bottom.scrollIntoView === "function") bottom.scrollIntoView({ block: "end" });
  }, [turns.length, transcript, reply]);

  return (
    <div className="session-conversation" role="region" aria-label="会话对话" tabIndex={0} ref={conversationRef}>
      {!transcript && !reply && turns.length === 0 ? (
        <div className="session-welcome">
          <span className="session-welcome-icon"><MessageSquare size={25} strokeWidth={1.5} aria-hidden="true" /></span>
          <span className="session-welcome-eyebrow">ROLEAI · 你的对话助手</span>
          <h3>{active ? "正在等待你的输入" : "开始一段新对话"}</h3>
          <p>{active ? "开口说出问题，停顿后自动提交。" : "点击「开始会话」，与 RoleAI 交流。"}</p>
          <p>{welcomeRoleName}</p>
          {!active && <button className="button-ghost" type="button" onClick={onAdjustConfiguration}>调整会话配置</button>}
        </div>
      ) : (
        <div className="session-turn">
          {historyTurns.map((item) => (
            <Fragment key={item.id}>
              {item.userText && (
                <article className="session-bubble session-bubble-user" aria-label={`用户转写 · 第 ${item.turnIndex + 1} 轮`}>
                  <p>{item.userText}</p>
                </article>
              )}
              {item.assistantText && (
                <article className="session-bubble session-bubble-assistant" aria-label={`AI 回复 · 第 ${item.turnIndex + 1} 轮`}>
                  <header className="bubble-head">
                    <span className="bubble-avatar" aria-hidden="true"><Bot size={15} /></span>
                    <h3>{roleName}</h3>
                    {clockOf(item.createdAt) && <time className="bubble-time">{clockOf(item.createdAt)}</time>}
                    <button type="button" className="bubble-copy" aria-label={`复制第 ${item.turnIndex + 1} 轮回复`} onClick={() => onCopy(item.assistantText)}>
                      <Copy size={13} aria-hidden="true" />
                    </button>
                  </header>
                  <p>{item.assistantText}</p>
                </article>
              )}
            </Fragment>
          ))}
          {historyTurns.length > 0 && <p className="session-turn-label">当前轮</p>}
          {transcript && (
            <article className="session-bubble session-bubble-user session-bubble-live" aria-label="用户转写">
              <p>{transcript}</p>
            </article>
          )}
          {reply && (
            <article className="session-bubble session-bubble-assistant session-bubble-live" aria-label="AI 回复">
              <header className="bubble-head">
                <span className="bubble-avatar" aria-hidden="true"><Bot size={15} /></span>
                <h3>{roleName}</h3>
              </header>
              <p>{reply}</p>
            </article>
          )}
          {pendingConfirmation && (
            <form className="candidate-confirmation" onSubmit={onConfirmCandidate}>
              <label htmlFor="candidate-confirmation-text">确认播报内容</label>
              <textarea
                id="candidate-confirmation-text"
                value={confirmationText}
                onChange={(event) => onConfirmationTextChange(event.target.value)}
              />
              <p>求职者模式不会自动播报。请核对或编辑后再确认。</p>
              <button className="button-primary" disabled={busy || !active || !confirmationText.trim()} type="submit">
                <Volume2 size={15} aria-hidden="true" />确认并播报
              </button>
            </form>
          )}
        </div>
      )}
      {unusedMaterials && <p className="session-materials-note">本轮未使用资料</p>}
      <div ref={conversationBottomRef} aria-hidden="true" />
    </div>
  );
}
