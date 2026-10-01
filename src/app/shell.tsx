import { useEffect, useRef, type ComponentType } from "react";
import { useHashLocation } from "wouter/use-hash-location";

import { AppNav } from "../components/app-nav";
import { MaterialsPage } from "../screens/materials/materials-page";
import { LivestreamPage } from "../screens/livestream/livestream-page";
import { RecordsPage } from "../screens/records/records-page";
import { ServicesPage } from "../screens/services/services-page";
import { SettingsPage } from "../screens/settings/settings-page";
import { WorkspacePage } from "../screens/workspace/workspace-page";
import { parseHash, type RouteId } from "./routes";

const pages: Record<RouteId, ComponentType> = {
  workspace: WorkspacePage,
  livestream: LivestreamPage,
  materials: MaterialsPage,
  records: RecordsPage,
  services: ServicesPage,
  settings: SettingsPage,
};

export function Shell() {
  const [location, setLocation] = useHashLocation();
  const current = parseHash(location);
  const Page = pages[current];
  const mainRef = useRef<HTMLElement>(null);
  useEffect(() => {
    if (mainRef.current) mainRef.current.scrollTop = 0;
  }, [current]);
  return (
    <div className="app-shell">
      <AppNav current={current} onNavigate={(id) => setLocation(`/${id}`)} />
      <main className="app-main" ref={mainRef}>
        <div className={`app-content app-content-${current}`}><Page /></div>
      </main>
    </div>
  );
}
