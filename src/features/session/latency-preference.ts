// 延迟瀑布条显示偏好（设置页可关；沿用 theme.ts 的存储 + useSyncExternalStore 模式）。
import { useSyncExternalStore } from "react";

export const LATENCY_WATERFALL_STORAGE_KEY = "ai-assistant.latency-waterfall";
const listeners = new Set<() => void>();

function readPreference(): boolean {
  try {
    return window.localStorage.getItem(LATENCY_WATERFALL_STORAGE_KEY) !== "off";
  } catch {
    return true;
  }
}

let enabled = readPreference();

function notify(next: boolean) {
  enabled = next;
  listeners.forEach((listener) => listener());
}

export function setLatencyWaterfallEnabled(next: boolean) {
  // 存储不可用时仍允许本次窗口切换（与主题偏好同策略）。
  try {
    if (next) {
      window.localStorage.removeItem(LATENCY_WATERFALL_STORAGE_KEY);
    } else {
      window.localStorage.setItem(LATENCY_WATERFALL_STORAGE_KEY, "off");
    }
  } catch { /* 保留内存偏好。 */ }
  notify(next);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function getSnapshot() {
  return enabled;
}

function getServerSnapshot() {
  return true;
}

export function useLatencyWaterfallPreference() {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}
