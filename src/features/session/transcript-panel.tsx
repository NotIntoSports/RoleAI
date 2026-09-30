import { Fragment, useEffect, useRef, type FormEvent } from "react";
import { Bot, Copy, MessageSquare, Volume2 } from "lucide-react";

import { t, useT } from "../../i18n";
import { clockOf } from "./workspace-format";
import { LatencyWaterfall } from "./latency-waterfall";
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
  /** 会议助手 + 会议音频：有用户文本、无回答的历史轮标注「未点名，仅转写」。 */
  showTranscriptOnlyNotes: boolean;
  /** 设置里可关的轮次延迟瀑布条（无 timeline 数据的旧轮次不显示）。 */
  showLatency: boolean;
  /** 最后一轮仅转写标注旁的追答入口（与工具栏「让助手回答」同一路径）。 */
  onTriggerAssistant: () => void;
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
  showTranscriptOnlyNotes,
  showLatency,
  onTriggerAssistant,
  onCopy,
  onAdjustConfiguration,
}: TranscriptPanelProps) {
  useT();
  const conversationRef = useRef<HTMLDivElement | null>(null);
  const conversationBottomRef = useRef<HTMLDivElement | null>(null);
  // 仅最后一轮仅转写轮旁提供追答：更早的转写轮的应答素材已被后续轮消费，
  // 后端会返回 NOTHING_TO_ANSWER，按钮只挂在仍然可追答的那一轮上。
  const lastTurn = turns[turns.length - 1];
  useEffect(() => {
    const container = conversationRef.current;
    const bottom = conversationBottomRef.current;
    if (!container || !bottom) return;
    const nearBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 400;
    if (nearBottom && typeof bottom.scrollIntoView === "function") bottom.scrollIntoView({ block: "end" });
  }, [turns.length, transcript, reply]);

  return (
    <div className="session-conversation" role="region" aria-label={t("session.transcript.region")} tabIndex={0} ref={conversationRef}>
      {!transcript && !reply && turns.length === 0 ? (
        <div className="session-welcome">
          <span className="session-welcome-icon"><MessageSquare size={25} strokeWidth={1.5} aria-hidden="true" /></span>
          <span className="session-welcome-eyebrow">{t("session.transcript.eyebrow")}</span>
          <h3>{active ? t("session.transcript.waitingTitle") : t("session.transcript.newTitle")}</h3>
          <p>{active ? t("session.transcript.waitingHint") : t("session.transcript.newHint")}</p>
          <p>{welcomeRoleName}</p>
          {!active && <button className="button-ghost" type="button" onClick={onAdjustConfiguration}>{t("session.transcript.adjustConfig")}</button>}
        </div>
      ) : (
        <div className="session-turn">
          {historyTurns.map((item) => (
            <Fragment key={item.id}>
              {item.userText && (
                <article className="session-bubble session-bubble-user" aria-label={t("session.transcript.userTurnAria", { n: item.turnIndex + 1 })}>
                  <p>{item.userText}</p>
                </article>
              )}
              {item.userText && !item.assistantText && showTranscriptOnlyNotes && (
                <p className="session-transcript-note" aria-label={t("session.transcript.transcriptOnlyAria", { n: item.turnIndex + 1 })}>
                  {t("session.transcript.transcriptOnlyNote")}
                  {active && item === lastTurn && (
                    <button type="button" className="session-transcript-followup" disabled={busy} onClick={onTriggerAssistant}>{t("session.transcript.followUp")}</button>
                  )}
                </p>
              )}
              {item.assistantText && (
                <article className="session-bubble session-bubble-assistant" aria-label={t("session.transcript.assistantTurnAria", { n: item.turnIndex + 1 })}>
                  <header className="bubble-head">
                    <span className="bubble-avatar" aria-hidden="true"><Bot size={15} /></span>
                    <h3>{roleName}</h3>
                    {clockOf(item.createdAt) && <time className="bubble-time">{clockOf(item.createdAt)}</time>}
                    <button type="button" className="bubble-copy" aria-label={t("session.transcript.copyTurnAria", { n: item.turnIndex + 1 })} onClick={() => onCopy(item.assistantText)}>
                      <Copy size={13} aria-hidden="true" />
                    </button>
                  </header>
                  <p>{item.assistantText}</p>
                  {showLatency && item.latency && (
                    <LatencyWaterfall latency={item.latency} turnIndex={item.turnIndex} />
                  )}
                </article>
              )}
            </Fragment>
          ))}
          {historyTurns.length > 0 && <p className="session-turn-label">{t("session.transcript.currentTurn")}</p>}
          {transcript && (
            <article className="session-bubble session-bubble-user session-bubble-live" aria-label={t("session.transcript.liveUserAria")}>
              <p>{transcript}</p>
            </article>
          )}
          {reply && (
            <article className="session-bubble session-bubble-assistant session-bubble-live" aria-label={t("session.transcript.liveReplyAria")}>
              <header className="bubble-head">
                <span className="bubble-avatar" aria-hidden="true"><Bot size={15} /></span>
                <h3>{roleName}</h3>
              </header>
              <p>{reply}</p>
            </article>
          )}
          {pendingConfirmation && (
            <form className="candidate-confirmation" onSubmit={onConfirmCandidate}>
              <label htmlFor="candidate-confirmation-text">{t("session.transcript.confirmLabel")}</label>
              <textarea
                id="candidate-confirmation-text"
                value={confirmationText}
                onChange={(event) => onConfirmationTextChange(event.target.value)}
              />
              <p>{t("session.transcript.confirmHint")}</p>
              <button className="button-primary" disabled={busy || !active || !confirmationText.trim()} type="submit">
                <Volume2 size={15} aria-hidden="true" />{t("session.transcript.confirmAction")}
              </button>
            </form>
          )}
        </div>
      )}
      {unusedMaterials && <p className="session-materials-note">{t("session.transcript.unusedMaterials")}</p>}
      <div ref={conversationBottomRef} aria-hidden="true" />
    </div>
  );
}
