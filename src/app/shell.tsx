import { useEffect, useRef, type ComponentType } from "react";
import { Router } from "wouter";
import { useHashLocation } from "wouter/use-hash-location";

import { AppNav } from "../components/app-nav";
import { MaterialsPage } from "../screens/materials/materials-page";
import { LivestreamPage } from "../screens/livestream/livestream-page";
import { RecordsPage } from "../screens/records/records-page";
import { SettingsPage } from "../screens/settings/settings-page";
import { WorkspacePage } from "../screens/workspace/workspace-page";
import { parseHash, type RouteId } from "./routes";

const pages: Record<RouteId, ComponentType> = {
  workspace: WorkspacePage,
  livestream: LivestreamPage,
  materials: MaterialsPage,
  records: RecordsPage,
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
  // 页面内的 <Link> 默认走 pushState 改路径，而本壳只监听 hash；统一交给 hash 路由，深链才能切到目标页。
  return (
    <Router hook={useHashLocation}>
      <div className="app-shell">
        {/* 末尾的空 "?" 让 wouter 清空 location.search，避免深链留下的 ?category= 带到下一次进入的页面。 */}
        <AppNav current={current} onNavigate={(id) => setLocation(`/${id}?`)} />
        <main className="app-main" ref={mainRef}>
          <div className={`app-content app-content-${current}`}><Page /></div>
        </main>
      </div>
    </Router>
  );
}
