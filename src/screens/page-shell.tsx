import { routeLabel, type RouteId } from "../app/routes";

const descriptions: Record<RouteId, string> = {
  workspace: "让对话自然进行，重要时刻由你掌控。",
  livestream: "根据产品资料准备讲稿，并通过 OBS 输出可控的虚拟直播。",
  practice: "按题单与 AI 面试官对练，结束后生成评分报告。",
  materials: "整理参考资料，为每一次回答提供上下文。",
  records: "回顾对话，留存值得继续跟进的内容。",
  services: "连接模型与语音服务，配置你的 AI 能力。",
  settings: "调整工作空间，管理角色与本地数据。",
};

export interface PageShellProps {
  id: RouteId;
}

export function PageShell({ id }: PageShellProps) {
  const label = routeLabel(id);
  const headingId = `page-heading-${id}`;

  return (
    <header className="page-header" role="region" aria-labelledby={headingId}>
      <div><h1 id={headingId}>{label}</h1><p>{descriptions[id]}</p></div>
    </header>
  );
}
