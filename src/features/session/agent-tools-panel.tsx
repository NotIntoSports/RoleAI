import type { Dispatch, FormEvent, SetStateAction } from "react";
import { Bot, FileText, Globe, Hand, MicOff, Play, RotateCcw, Square, Volume2 } from "lucide-react";

import { t, useT } from "../../i18n";

interface AgentToolsPanelProps {
  allowWebSearch: boolean;
  canSearch: boolean;
  setAllowWebSearch: Dispatch<SetStateAction<boolean>>;
  active: boolean;
  busy: boolean;
  onStop: () => void;
  onSetMode: (next: "ai_active" | "operator_speaking" | "paused" | "muted") => void;
  onSaySubmit: (event: FormEvent) => void;
  sayText: string;
  setSayText: Dispatch<SetStateAction<string>>;
  onCorrectSubmit: () => void;
  correctText: string;
  setCorrectText: Dispatch<SetStateAction<string>>;
  onRetry: () => void;
  onReport: () => void;
}

export function AgentToolsPanel({
  allowWebSearch,
  canSearch,
  setAllowWebSearch,
  active,
  busy,
  onStop,
  onSetMode,
  onSaySubmit,
  sayText,
  setSayText,
  onCorrectSubmit,
  correctText,
  setCorrectText,
  onRetry,
  onReport,
}: AgentToolsPanelProps) {
  useT();
  return (
    <div className="composer-more-panel" role="region" aria-label={t("session.tools.region")}>
      <p className="composer-more-title">{t("session.tools.toolsTitle")}</p>
      <label className="composer-switch">
        <span><Globe size={14} aria-hidden="true" />{t("session.tools.webSearch")}</span>
        <input
          type="checkbox"
          role="switch"
          aria-label={t("session.tools.webSearch")}
          disabled={active || !canSearch}
          checked={allowWebSearch && canSearch}
          onChange={(event) => setAllowWebSearch(event.target.checked)}
        />
      </label>
      <p className="composer-more-title">{t("session.tools.callTitle")}</p>
      <div className="composer-more-actions">
        {active && (
          <button className="button-primary" disabled type="button">
            <Play size={14} aria-hidden="true" />{t("session.tools.startSession")}
          </button>
        )}
        <button disabled={!active} type="button" onClick={onStop}>
          <Square size={14} aria-hidden="true" />{t("session.tools.stop")}
        </button>
        <button disabled={!active} type="button" onClick={() => onSetMode("operator_speaking")}>
          <Hand size={14} aria-hidden="true" />{t("session.tools.takeover")}
        </button>
        <button
          disabled={!active}
          type="button"
          onPointerDown={() => onSetMode("operator_speaking")}
          onPointerUp={() => onSetMode("ai_active")}
          onPointerCancel={() => onSetMode("ai_active")}
          onKeyDown={(event) => { if (event.key === " " || event.key === "Enter") onSetMode("operator_speaking"); }}
          onKeyUp={(event) => { if (event.key === " " || event.key === "Enter") onSetMode("ai_active"); }}
        >
          <Volume2 size={14} aria-hidden="true" />{t("session.tools.holdToTalk")}
        </button>
        <button disabled={busy || !active} type="button" onClick={() => onSetMode("ai_active")}>
          <Bot size={14} aria-hidden="true" />{t("session.tools.resumeAI")}
        </button>
        <button disabled={busy || !active} type="button" onClick={() => onSetMode("muted")}>
          <MicOff size={14} aria-hidden="true" />{t("session.tools.mute")}
        </button>
      </div>
      <p className="composer-more-title">{t("session.tools.contentTitle")}</p>
      <form className="service-form session-tool-form" onSubmit={onSaySubmit}>
        <label htmlFor="session-say">{t("session.tools.sayLabel")}</label>
        <div className="session-tool-row">
          <input id="session-say" value={sayText} onChange={(event) => setSayText(event.target.value)} placeholder={t("session.tools.sayPlaceholder")} />
          <button disabled={busy || !active} type="submit"><Volume2 size={15} aria-hidden="true" />{t("session.tools.sayAction")}</button>
        </div>
      </form>
      <form className="service-form session-tool-form" onSubmit={(event) => { event.preventDefault(); onCorrectSubmit(); }}>
        <label htmlFor="session-correct">{t("session.tools.correctLabel")}</label>
        <div className="session-tool-row">
          <input id="session-correct" value={correctText} onChange={(event) => setCorrectText(event.target.value)} placeholder={t("session.tools.correctPlaceholder")} />
          <button disabled={busy || !active} type="submit">{t("session.tools.correctAction")}</button>
        </div>
      </form>
      <div className="composer-more-actions">
        <button disabled={busy || !active} type="button" onClick={onRetry}><RotateCcw size={15} aria-hidden="true" />{t("session.tools.retry")}</button>
        <button disabled={busy || !active} type="button" onClick={onReport}><FileText size={15} aria-hidden="true" />{t("session.tools.report")}</button>
      </div>
    </div>
  );
}
