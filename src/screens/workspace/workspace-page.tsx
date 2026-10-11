import { TavusCallProvider } from "../../features/tavus/call-lock";
import { WorkspaceSession } from "../../features/session/workspace-session";
import { PageShell } from "../page-shell";

export function WorkspacePage() {
  return (
    <div className="workspace-page">
      <PageShell id="workspace" />
      {/* 通话舞台由 WorkspaceSession 渲染在会话主区域里；provider 提供媒体 refs 与通话状态。 */}
      <TavusCallProvider>
        <WorkspaceSession />
      </TavusCallProvider>
    </div>
  );
}
