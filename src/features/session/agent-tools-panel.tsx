import type { Dispatch, FormEvent, SetStateAction } from "react";
import { Bot, FileText, Globe, Hand, MicOff, Play, RotateCcw, Square, Volume2 } from "lucide-react";

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
  return (
    <div className="composer-more-panel" role="region" aria-label="会话工具">
      <p className="composer-more-title">工具调用</p>
      <label className="composer-switch">
        <span><Globe size={14} aria-hidden="true" />联网搜索</span>
        <input
          type="checkbox"
          role="switch"
          aria-label="联网搜索"
          disabled={active || !canSearch}
          checked={allowWebSearch && canSearch}
          onChange={(event) => setAllowWebSearch(event.target.checked)}
        />
      </label>
      <p className="composer-more-title">通话控制</p>
      <div className="composer-more-actions">
        {active && (
          <button className="button-primary" disabled type="button">
            <Play size={14} aria-hidden="true" />开始会话
          </button>
        )}
        <button disabled={!active} type="button" onClick={onStop}>
          <Square size={14} aria-hidden="true" />停止
        </button>
        <button disabled={!active} type="button" onClick={() => onSetMode("operator_speaking")}>
          <Hand size={14} aria-hidden="true" />接管
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
          <Volume2 size={14} aria-hidden="true" />按住人工发言
        </button>
        <button disabled={busy || !active} type="button" onClick={() => onSetMode("ai_active")}>
          <Bot size={14} aria-hidden="true" />恢复 AI
        </button>
        <button disabled={busy || !active} type="button" onClick={() => onSetMode("muted")}>
          <MicOff size={14} aria-hidden="true" />静音
        </button>
      </div>
      <p className="composer-more-title">内容工具</p>
      <form className="service-form session-tool-form" onSubmit={onSaySubmit}>
        <label htmlFor="session-say">朗读文本</label>
        <div className="session-tool-row">
          <input id="session-say" value={sayText} onChange={(event) => setSayText(event.target.value)} placeholder="输入需要 AI 朗读的文本" />
          <button disabled={busy || !active} type="submit"><Volume2 size={15} aria-hidden="true" />朗读</button>
        </div>
      </form>
      <form className="service-form session-tool-form" onSubmit={(event) => { event.preventDefault(); onCorrectSubmit(); }}>
        <label htmlFor="session-correct">纠正内容</label>
        <div className="session-tool-row">
          <input id="session-correct" value={correctText} onChange={(event) => setCorrectText(event.target.value)} placeholder="输入修正后的回答" />
          <button disabled={busy || !active} type="submit">纠正</button>
        </div>
      </form>
      <div className="composer-more-actions">
        <button disabled={busy || !active} type="button" onClick={onRetry}><RotateCcw size={15} aria-hidden="true" />重试</button>
        <button disabled={busy || !active} type="button" onClick={onReport}><FileText size={15} aria-hidden="true" />报告</button>
      </div>
    </div>
  );
}
