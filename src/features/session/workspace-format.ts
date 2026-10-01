import type { PublicConfig, RoleScenario } from "../../generated/bindings";
import { t } from "../../i18n";

const PRESET_SCENARIOS: Record<string, RoleScenario> = {
  "preset-interviewer": "interviewer",
  "preset-hr": "hr",
  "preset-candidate": "candidate",
  "preset-meeting": "meetingAssistant",
  "preset-presenter": "livestreamPresenter",
};

export function roleScenario(config: PublicConfig | null, roleId: string): RoleScenario | undefined {
  return config?.roleProfiles.find((role) => role.id === roleId)?.scenario ?? PRESET_SCENARIOS[roleId];
}

const ERROR_CODES = [
  "SESSION_SIDECAR_MISSING",
  "PROVIDER_CREDENTIAL_MISSING",
  "REALTIME_UNAUTHORIZED",
  "REALTIME_DNS_FAILED",
  "REALTIME_TCP_FAILED",
  "REALTIME_TLS_FAILED",
  "REALTIME_CONNECT_FAILED",
  "REALTIME_PROTOCOL_FAILED",
  "REALTIME_CONNECTION_CLOSED",
  "REALTIME_READ_FAILED",
  "REALTIME_WRITE_FAILED",
  "REALTIME_TIMEOUT",
  "REALTIME_SESSION_UPDATE_TIMEOUT",
  "SESSION_CANCELLED",
  "NOTHING_TO_ANSWER",
  "REALTIME_NO_RESPONSE",
  "MEETING_PROCESS_NOT_AVAILABLE",
  "SESSION_SIDECAR_INVALID_PID",
  "SESSION_SIDECAR_SPAWN_FAILED",
  "PLAYBACK_FAILED",
  "PLAYBACK_START_FAILED",
  "PLAYBACK_TIMEOUT",
  "PLAYBACK_CANCELLED",
  "PLAYBACK_NOT_CONFIRMED",
  "PLATFORM_UNSUPPORTED",
] as const;

type KnownErrorCode = (typeof ERROR_CODES)[number];

export const errorText = (error: { code: string; message: string; field?: string | null }) => {
  const localized = (ERROR_CODES as readonly string[]).includes(error.code)
    ? t(`session.errors.${error.code as KnownErrorCode}`)
    : undefined;
  return localized ?? `${error.field ? error.field + "：" : ""}${error.code}：${error.message}`;
};

// 供应商透传的原始错误码（出现在 message 前缀里）翻译成可行动的提示。
const REMOTE_ERROR_HINTS: Record<string, "session.remoteHints.downstream_reconnect_exceeded" | "session.remoteHints.1113"> = {
  downstream_reconnect_exceeded: "session.remoteHints.downstream_reconnect_exceeded",
  "1113": "session.remoteHints.1113",
};

// 连续快速失败的常见原因是供应商限流（如阿里云实时模型对高频请求限流）：
// 追加可行动的提示，避免用户在限流窗口内反复重试加深限流。
export function rateLimitHint(): string {
  return t("session.rateLimitHint");
}

export function humanizeRemoteError(message: string): string {
  for (const [code, key] of Object.entries(REMOTE_ERROR_HINTS)) {
    if (message.startsWith(code)) return t(key);
  }
  return message;
}

export const ACTIVE_PHASES = new Set([
  "preparing",
  "listening",
  "thinking",
  "speaking",
  "stopping",
  "recovering",
  "blocked",
]);

// 音量条四根 bar 的灵敏度系数：同样的电平下制造出高低起伏，避免齐刷刷等高。
export const BAR_FACTORS = [0.45, 0.7, 0.6, 0.85] as const;

export function formatDuration(totalSeconds: number): string {
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

export function clockOf(iso: string | undefined): string {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
