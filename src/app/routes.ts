/**
 * Route table for the Tauri page shell.
 * Single source of truth — all navigation, pages, and tests derive from this module.
 * Design spec: docs/superpowers/specs/2026-09-04-tauri-local-monolith-design.md §6.1–6.5
 */

export const routeIds = ["workspace", "livestream", "materials", "records", "services", "settings"] as const;

export type RouteId = typeof routeIds[number];

const DEFAULT_ROUTE: RouteId = "workspace";

/** Parse a hash string (with or without leading #) to a RouteId. Falls back to workspace. */
export function parseHash(hash: string): RouteId {
  let path = hash;
  if (path.startsWith("#")) path = path.slice(1);
  if (path.startsWith("/")) path = path.slice(1);
  const queryIndex = path.indexOf("?");
  if (queryIndex !== -1) path = path.slice(0, queryIndex);
  if (isRouteId(path)) return path;
  return DEFAULT_ROUTE;
}

/** Format a RouteId to a hash string. */
export function formatHash(id: RouteId): string {
  return `#/${id}`;
}

/** Chinese display label for each route. */
export function routeLabel(id: RouteId): string {
  const labels: Record<RouteId, string> = {
    workspace: "工作台",
    livestream: "虚拟直播",
    materials: "资料",
    records: "记录",
    services: "服务",
    settings: "设置",
  };
  return labels[id];
}

/** Design spec section reference for each route. */
export function routeDesignRef(id: RouteId): string {
  const refs: Record<RouteId, string> = {
    workspace: "6.1",
    livestream: "6.1",
    materials: "6.2",
    records: "6.3",
    services: "6.4",
    settings: "6.5",
  };
  return refs[id];
}

/** Future capabilities to be migrated into each page (from design spec §6). */
export function routeCapabilities(id: RouteId): readonly string[] {
  const capabilities: Record<RouteId, readonly string[]> = {
    workspace: [
      "当前会话状态与控制（启动/暂停/恢复/停止）",
      "AI 实时回复与字幕显示",
      "人工接管与干预控制",
      "音视频连接状态指示",
      "会议桥接卡片",
    ],
    livestream: [
      "本地产品资料生成分段讲稿",
      "图片或循环视频舞台",
      "讲稿确认与分段控制",
      "OBS Virtual Camera 输出",
    ],
    materials: [
      "简历导入与管理",
      "知识库切片与索引状态",
      "FTS 全文检索",
      "向量嵌入（sqlite-vec）",
      "资料预览与筛选",
    ],
    records: [
      "会话记录列表与分页",
      "纪要详情（摘要/优势/跟进/局限/证据）",
      "记录导出",
      "记录删除（两步确认）",
    ],
    services: [
      "模型提供方配置与状态",
      "语音路由配置与测试",
      "密钥管理（Windows Credential Manager）",
      "连接测试与健康检查",
    ],
    settings: [
      "系统诊断导出",
      "配置位置选择与显示",
      "会议画面输出模式",
      "助手声音与形象",
      "OBS 与虚拟摄像头",
      "音频路由与设备检查",
    ],
  };
  return capabilities[id];
}

/** Type guard for RouteId. */
export function isRouteId(value: unknown): value is RouteId {
  return typeof value === "string" && (routeIds as readonly string[]).includes(value);
}
