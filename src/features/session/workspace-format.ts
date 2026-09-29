import type { PublicConfig, RoleScenario } from "../../generated/bindings";

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

export const errorText = (error: { code: string; message: string; field?: string | null }) =>
  ({ SESSION_SIDECAR_MISSING: "缺少 AudioBridge 音频组件，请安装或修复音频组件后重试。",
    PROVIDER_CREDENTIAL_MISSING: "本机未找到供应商密钥，请在供应商设置中重新保存 API Key。",
    REALTIME_UNAUTHORIZED: "实时语音鉴权失败，请检查 Token Plan API Key、套餐状态和模型权限。",
    REALTIME_DNS_FAILED: "无法解析实时语音服务地址，请检查网络和服务地址后重试。",
    REALTIME_TCP_FAILED: "无法连接实时语音服务，请检查网络后重试。",
    REALTIME_TLS_FAILED: "实时语音安全连接失败，请检查系统时间、证书或代理设置。",
    REALTIME_CONNECT_FAILED: "实时语音连接失败，请检查网络和供应商设置后重试。",
    REALTIME_PROTOCOL_FAILED: "实时语音协议协商失败，请检查服务地址是否支持 Realtime。",
    REALTIME_CONNECTION_CLOSED: "实时语音服务已断开连接，输入已保留，请重试。",
    REALTIME_READ_FAILED: "接收实时语音回复失败，输入已保留，请重试。",
    REALTIME_WRITE_FAILED: "发送至实时语音服务失败，输入已保留，请重试。",
    REALTIME_TIMEOUT: "实时语音请求超时，输入已保留，请重试。",
    REALTIME_SESSION_UPDATE_TIMEOUT: "实时语音服务未及时确认会话，输入已保留，请重试。",
    SESSION_CANCELLED: "已取消本次发送，输入已保留。",
    MEETING_PROCESS_NOT_AVAILABLE: "所选会议已退出或不再可用，请刷新会议进程。",
    SESSION_SIDECAR_INVALID_PID: "请选择有效的会议进程。",
    SESSION_SIDECAR_SPAWN_FAILED: "音频组件启动失败，请检查安装后重试。",
    PLAYBACK_FAILED: "语音未播放成功，文字回答已保留。请检查所选音频设备。",
    PLAYBACK_START_FAILED: "无法启动语音播放，请检查 AudioBridge 音频组件。",
    PLAYBACK_TIMEOUT: "语音播放超时，已停止输出。",
    PLAYBACK_CANCELLED: "语音播放已取消。",
    PLAYBACK_NOT_CONFIRMED: "音频组件未确认播放完成，不能标记为已播报。文字回答已保留。",
  }[error.code] ?? `${error.field ? error.field + "：" : ""}${error.code}：${error.message}`);

// 供应商透传的原始错误码（出现在 message 前缀里）翻译成可行动的提示。
const REMOTE_ERROR_HINTS: Record<string, string> = {
  downstream_reconnect_exceeded: "语音模型不可用：当前供应商账号可能未开通实时语音模型权限。请在服务页更换语音线路，或到供应商控制台开通后重试。",
  "1113": "供应商余额不足：账号没有可用的语音资源包，请充值或更换语音线路。",
};

// 连续快速失败的常见原因是供应商限流（如阿里云实时模型对高频请求限流）：
// 追加可行动的提示，避免用户在限流窗口内反复重试加深限流。
export const RATE_LIMIT_HINT = "若短时间内多次失败，可能是供应商限流，请等待 1–2 分钟后再说话重试。";

export function humanizeRemoteError(message: string): string {
  for (const [code, hint] of Object.entries(REMOTE_ERROR_HINTS)) {
    if (message.startsWith(code)) return hint;
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
