import { useState } from "react";
import { routeIds, routeLabel, type RouteId } from "../app/routes";
import { t, useT } from "../i18n";
import { FolderOpen, History, Menu, MessageSquare, Radio, Settings2, SlidersHorizontal, Sparkles, X } from "lucide-react";

const icons: Record<RouteId, typeof MessageSquare> = { workspace: MessageSquare, livestream: Radio, materials: FolderOpen, records: History, services: SlidersHorizontal, settings: Settings2 };

export interface AppNavProps {
  current: RouteId;
  onNavigate: (id: RouteId) => void;
}

export function AppNav({ current, onNavigate }: AppNavProps) {
  useT();
  const [expanded, setExpanded] = useState(false);
  function item(id: RouteId) {
    const isActive = id === current;
    const Icon = icons[id];
    return (
      <button key={id} type="button" className="app-nav-item" aria-label={routeLabel(id)} title={routeLabel(id)}
        aria-current={isActive ? "page" : undefined} data-active={isActive ? "true" : undefined}
        onClick={() => { setExpanded(false); if (!isActive) onNavigate(id); }}>
        <Icon size={18} strokeWidth={1.7} aria-hidden="true" />
        <span>{routeLabel(id)}</span>
      </button>
    );
  }
  return (
    <nav className="app-nav" aria-label={t("app.nav.primary")} data-expanded={expanded}>
      <div className="app-brand"><span className="brand-mark"><Sparkles size={19} aria-hidden="true" /></span><span>RoleAI</span></div>
      <button className="app-nav-toggle button-ghost" type="button" aria-label={t("app.nav.toggle")} aria-expanded={expanded} aria-controls="app-nav-links" onClick={() => setExpanded(!expanded)}>{expanded ? <X size={20} /> : <Menu size={20} />}<span>{routeLabel(current)}</span></button>
      <div className="app-nav-links" id="app-nav-links">
        <div className="app-nav-primary">{routeIds.filter((id) => id !== "settings").map(item)}</div>
        <div className="app-nav-footer">{item("settings")}<p><span className="local-status-dot" />{t("app.nav.localWorkspace")}</p></div>
      </div>
    </nav>
  );
}
