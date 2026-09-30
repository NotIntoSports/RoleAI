/**
 * Route table for the Tauri page shell.
 * Single source of truth — all navigation, pages, and tests derive from this module.
 * Design spec: docs/superpowers/specs/2026-09-04-tauri-local-monolith-design.md §6.1–6.5
 */

import { t, tList } from "../i18n";

export const routeIds = ["workspace", "livestream", "practice", "materials", "records", "services", "settings"] as const;

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

/** Display label for each route, in the active interface language. */
export function routeLabel(id: RouteId): string {
  return t(`app.routes.labels.${id}`);
}

/** Design spec section reference for each route. */
export function routeDesignRef(id: RouteId): string {
  const refs: Record<RouteId, string> = {
    workspace: "6.1",
    livestream: "6.1",
    // 6.6：lane-E 模拟面试训练设计（docs/superpowers/specs/2026-10-mock-interview-training.md，不入库）。
    practice: "6.6",
    materials: "6.2",
    records: "6.3",
    services: "6.4",
    settings: "6.5",
  };
  return refs[id];
}

/** Future capabilities to be migrated into each page (from design spec §6), in the active interface language. */
export function routeCapabilities(id: RouteId): readonly string[] {
  return tList(`app.routes.capabilities.${id}`);
}

/** Type guard for RouteId. */
export function isRouteId(value: unknown): value is RouteId {
  return typeof value === "string" && (routeIds as readonly string[]).includes(value);
}
