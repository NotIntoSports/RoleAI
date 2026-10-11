// 虚拟形象开关持久化：开启时工作台的「开始」改走 Tavus 视频通话，关闭时走已配置的语音线路。
// 默认关闭：Tavus 会把人设发往第三方并消耗通话时长，必须由用户明确开启。
export const AVATAR_STORAGE_KEY = "ai-assistant.avatar-enabled";

export function readAvatarPreference(): boolean {
  try {
    return window.localStorage.getItem(AVATAR_STORAGE_KEY) === "on";
  } catch {
    return false;
  }
}

export function writeAvatarPreference(enabled: boolean) {
  try {
    if (enabled) {
      window.localStorage.setItem(AVATAR_STORAGE_KEY, "on");
    } else {
      window.localStorage.removeItem(AVATAR_STORAGE_KEY);
    }
  } catch { /* 存储不可用时仅保留内存状态。 */ }
}
