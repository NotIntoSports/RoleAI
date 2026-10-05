// 输入来源（本机麦克风/会议音频）持久化：应用重启后恢复上次选择，
// 避免重启后静默回退到默认的本机麦克风，导致会议模式失联（听不到会议内容）。
export const INPUT_SOURCE_STORAGE_KEY = "ai-assistant.input-source";

export function readInputSourcePreference(): string {
  try {
    return window.localStorage.getItem(INPUT_SOURCE_STORAGE_KEY) === "meeting" ? "meeting" : "mic";
  } catch {
    return "mic";
  }
}

export function writeInputSourcePreference(source: string) {
  try {
    if (source === "meeting") {
      window.localStorage.setItem(INPUT_SOURCE_STORAGE_KEY, "meeting");
    } else {
      window.localStorage.removeItem(INPUT_SOURCE_STORAGE_KEY);
    }
  } catch { /* 存储不可用时仅保留内存状态。 */ }
}
