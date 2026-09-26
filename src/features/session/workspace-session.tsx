import { Fragment, FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { Bot, ChevronDown, FileText, Hand, MessageSquare, MicOff, Play, RotateCcw, Square, Volume2, Wrench } from "lucide-react";

import * as api from "../../api/commands";
import "../../styles/workspace.css";
import { connectLiveKitRoom, disconnectLiveKitRoom } from "./livekit-room";
import { MicStreamer, type MicStreamCallbacks, type MicStreamController } from "./mic-recorder";
import { PreflightIssues } from "./preflight-issues";
import type {
  AgentCommandInput,
  CommandResult,
  RuntimeStatus,
  SessionReplyEvent,
  SessionTranscriptEvent,
  PreflightIssue,
  PublicConfig,
  SessionTurnView,
  WebSource,
  MeetingProcess,
  AudioOutputDevice,
  VirtualAudioPreparation,
  RoleScenario,
} from "../../generated/bindings";

const PRESET_SCENARIOS: Record<string, RoleScenario> = {
  "preset-interviewer": "interviewer",
  "preset-hr": "hr",
  "preset-candidate": "candidate",
  "preset-meeting": "meetingAssistant",
  "preset-presenter": "livestreamPresenter",
};

const MEETING_NAMES: Record<string, string> = {
  "teams.exe": "Microsoft Teams", "ms-teams.exe": "Microsoft Teams",
  "wemeetapp.exe": "腾讯会议", "feishu.exe": "飞书", "lark.exe": "Lark",
  "dingtalk.exe": "钉钉", "zoom.exe": "Zoom",
};

const PREPARATION_PHASES: Record<string, string> = {
  checking: "检测安装环境", downloading: "下载安装包", verifying: "校验安装包和签名",
  authorizing: "等待 Windows 管理员授权", installing: "安装驱动", rechecking: "重新检测音频端点",
};

function roleScenario(config: PublicConfig | null, roleId: string): RoleScenario | undefined {
  return config?.roleProfiles.find((role) => role.id === roleId)?.scenario ?? PRESET_SCENARIOS[roleId];
}

const errorText = (error: { code: string; message: string; field?: string | null }) =>
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

function humanizeRemoteError(message: string): string {
  for (const [code, hint] of Object.entries(REMOTE_ERROR_HINTS)) {
    if (message.startsWith(code)) return hint;
  }
  return message;
}

const ACTIVE_PHASES = new Set([
  "preparing",
  "listening",
  "thinking",
  "speaking",
  "stopping",
  "recovering",
  "blocked",
]);

const PHASE_LABELS: Record<string, string> = {
  idle: "未开始",
  preparing: "准备中",
  listening: "聆听中",
  thinking: "思考中",
  speaking: "回复中",
  stopping: "停止中",
  recovering: "恢复中",
  blocked: "需要处理",
  completed: "已结束",
  failed: "会话异常",
};

const MODE_LABELS: Record<string, string> = {
  ai_active: "AI 应答",
  operator_speaking: "人工接管",
  paused: "已暂停",
  muted: "已静音",
};

export type SessionListen = <T>(
  event: string,
  handler: (payload: T) => void,
) => Promise<() => void> | (() => void);

export interface WorkspaceSessionProps {
  finalizeUtterance?: (text: string) => Promise<void>;
  listen?: SessionListen;
  createMicStreamer?: (callbacks: MicStreamCallbacks) => MicStreamController;
}

function defaultCreateMicStreamer(callbacks: MicStreamCallbacks): MicStreamController {
  return new MicStreamer(callbacks);
}

async function defaultFinalizeUtterance(text: string) {
  const result = await api.finalizeSessionUtterance(text);
  if (!result.ok) {
    throw new Error(errorText(result.error));
  }
}

async function defaultListen<T>(event: string, handler: (payload: T) => void) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return () => {};
  }
  try {
    const mod = await import("@tauri-apps/api/event");
    return mod.listen<T>(event, (envelope) => handler(envelope.payload));
  } catch {
    return () => {};
  }
}

export function WorkspaceSession({
  finalizeUtterance = defaultFinalizeUtterance,
  listen = defaultListen,
  createMicStreamer = defaultCreateMicStreamer,
}: WorkspaceSessionProps) {
  const [phase, setPhase] = useState("idle");
  const [mode, setMode] = useState("ai_active");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [transcript, setTranscript] = useState("");
  const [reply, setReply] = useState("");
  const [turns, setTurns] = useState<SessionTurnView[]>([]);
  const [unusedMaterials, setUnusedMaterials] = useState(false);
  const [message, setMessage] = useState("正在读取会话状态…");
  const [issues, setIssues] = useState<PreflightIssue[]>([]);
  const [configurationOpen, setConfigurationOpen] = useState(true);
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [roleProfileId, setRoleProfileId] = useState("");
  const [voiceRouteId, setVoiceRouteId] = useState("");
  const [allowWebSearch, setAllowWebSearch] = useState(false);
  const [webSources, setWebSources] = useState<WebSource[]>([]);
  const [webDegraded, setWebDegraded] = useState(false);
  const [pendingConfirmation, setPendingConfirmation] = useState(false);
  const [confirmationText, setConfirmationText] = useState("");
  const [inputSource, setInputSource] = useState("mic");
  const [meetingProcesses, setMeetingProcesses] = useState<MeetingProcess[]>([]);
  const [meetingPid, setMeetingPid] = useState("");
  const [audioOutputs, setAudioOutputs] = useState<AudioOutputDevice[]>([]);
  const [outputDeviceId, setOutputDeviceId] = useState("");
  const [virtualAudio, setVirtualAudio] = useState<VirtualAudioPreparation | null>(null);
  const [installingAudio, setInstallingAudio] = useState(false);
  const [audioPreparationPhase, setAudioPreparationPhase] = useState("checking");
  const [audioRetryBlocked, setAudioRetryBlocked] = useState(false);
  const [audioAttempted, setAudioAttempted] = useState(false);
  useEffect(() => {
    let disposed = false;
    let unlisten = () => {};
    Promise.resolve(listen<string>("virtual_audio.preparation.v1", (phase) => {
      if (!disposed && phase in PREPARATION_PHASES) setAudioPreparationPhase(phase);
    })).then((cleanup) => { if (disposed) cleanup(); else unlisten = cleanup; }).catch(() => {});
    return () => { disposed = true; unlisten(); };
  }, [listen]);
  async function refreshVirtualAudio() {
    try {
      const result = await api.getVirtualAudioStatus();
      if (!result.ok) { setVirtualAudio(null); setMessage(errorText(result.error)); return; }
      setVirtualAudio(result.data);
      setAudioRetryBlocked(result.data.state === "installing");
      setOutputDeviceId(result.data.renderEndpointId ?? "");
    } catch { setVirtualAudio(null); setMessage("无法检测虚拟声卡。"); }
  }
  async function installVirtualAudio() {
    if (installingAudio || audioRetryBlocked) return;
    setInstallingAudio(true); setAudioAttempted(true); setAudioPreparationPhase("checking"); setMessage("");
    try {
      const result = await api.installVirtualAudio();
      if (!result.ok) {
        setAudioRetryBlocked(["PREREQUISITE_TIMEOUT", "PREREQUISITE_INSTALL_BUSY"].includes(result.error.code));
        setMessage(result.error.message);
        return;
      }
      setVirtualAudio(result.data); setOutputDeviceId(result.data.renderEndpointId ?? "");
      setAudioRetryBlocked(result.data.diagnostic?.retryAllowed === false);
      setMessage(result.data.detail);
    } catch { setMessage("虚拟声卡安装失败，请稍后重试。"); }
    finally { setInstallingAudio(false); }
  }
  async function refreshAudioOutputs() {
    setOutputDeviceId("");
    try {
      const result = await api.listAudioOutputs();
      if (result.ok) setAudioOutputs(result.data);
      else { setAudioOutputs([]); setMessage(errorText(result.error)); }
    } catch { setAudioOutputs([]); setMessage("无法读取音频输出设备。"); }
  }
  async function refreshMeetings() {
    setMeetingPid("");
    try {
      const result = await api.listMeetingProcesses();
      if (!result.ok) { setMeetingProcesses([]); setMessage(result.error.message); return; }
      setMeetingProcesses(result.data);
      setMeetingPid(result.data.length === 1 ? String(result.data[0].pid) : "");
      if (!result.data.length) setMessage("未检测到会议窗口，请打开 Teams、腾讯会议、飞书、钉钉或 Zoom 后刷新。");
    } catch { setMeetingProcesses([]); setMessage("无法检测会议进程，请稍后重试。"); }
  }
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const result = await api.getConfigPublic();
        if (!cancelled && result.ok) {
          setConfig(result.data);
          const role = result.data.roleProfiles.find((item) => item.id === result.data.activeRoleProfileId && item.configVersion > 0);
          const route = result.data.speech.voiceRoutes.find((item) => item.id === result.data.speech.activeVoiceRouteId && item.configVersion > 0);
          setConfigurationOpen(!role || !route?.ready);
          setRoleProfileId(result.data.activeRoleProfileId ?? "");
          setVoiceRouteId(result.data.speech.activeVoiceRouteId ?? "");
          setAllowWebSearch(roleScenario(result.data, result.data.activeRoleProfileId ?? "") === "meetingAssistant");
          // 语音输出设备列表用于手动文字/本机麦克风模式播报 AI 回复；
          // 进入页面即加载，避免用户必须手点"刷新音频设备"。
          void refreshAudioOutputs();
        }
      } catch { /* The runtime preflight supplies actionable configuration errors. */ }
    })();
    return () => { cancelled = true; };
  }, []);
  const [operationBusy, setBusy] = useState(false);
  const busy = operationBusy;
  const requestEpoch = useRef(0);
  useEffect(() => () => { requestEpoch.current += 1; }, []);
  const [sayText, setSayText] = useState("");
  const [correctText, setCorrectText] = useState("");
  const [revision, setRevision] = useState(0);
  const [reportSummary, setReportSummary] = useState("");
  const [reportDetail, setReportDetail] = useState("");
  const [transport, setTransport] = useState<"direct" | "livekit">("direct");
  const [livekitState, setLivekitState] = useState("idle");
  const livekitRoom = useRef<Awaited<ReturnType<typeof connectLiveKitRoom>> | null>(null);
  const statusSeq = useRef(0);
  const transcriptSeq = useRef(0);
  const replySeq = useRef(0);
  const sessionIdRef = useRef<string | null>(null);
  const refreshRef = useRef<() => Promise<void>>(async () => {});

  const applyStatus = useCallback((next: RuntimeStatus) => {
    if (next.seq <= statusSeq.current) return;
    statusSeq.current = next.seq;
    setPhase(next.phase);
    setMode(next.mode);
    setUnusedMaterials(next.unusedMaterials);
    setRevision(next.revision);
    if (next.mode !== "ai_active") {
      setPendingConfirmation(false);
    }
    if (next.lastErrorCode) {
      setMessage(errorText({ code: next.lastErrorCode, message: "会话运行时错误" }));
    }
  }, []);

  const applyTranscript = useCallback((payload: SessionTranscriptEvent) => {
    if (payload.seq <= transcriptSeq.current) return;
    transcriptSeq.current = payload.seq;
    setTranscript(payload.text);
    void refreshRef.current();
  }, []);

  const applyReply = useCallback((payload: SessionReplyEvent) => {
    if (payload.seq <= replySeq.current) return;
    replySeq.current = payload.seq;
    setReply(payload.text);
    void refreshRef.current();
  }, []);

  const refresh = useCallback(
    async (id?: string | null) => {
      const epoch = requestEpoch.current;
      const target = id ?? sessionIdRef.current;
      try {
        const statusResult = await api.getRuntimeStatus();
        if (epoch !== requestEpoch.current) return;
        if (statusResult.ok) {
          applyStatus(statusResult.data);
        } else {
          setMessage(errorText(statusResult.error));
        }
        if (target) {
          const detail = await api.getSession(target);
          if (epoch !== requestEpoch.current) return;
          if (detail.ok) {
            setTurns(detail.data.turns);
            const last = detail.data.turns.at(-1);
            if (last) {
              setTranscript(last.userText);
              setReply(last.assistantText);
              setUnusedMaterials(!last.materialsUsed);
              setWebSources(last.webSources ?? []);
              setWebDegraded(last.webDegraded ?? false);
              const pending = last.playbackStatus === "pending_confirmation" && last.userConfirmed !== true
                && (!statusResult.ok || statusResult.data.mode === "ai_active");
              setPendingConfirmation(pending);
              if (pending) setConfirmationText(last.assistantText);
            }
          } else {
            setMessage(errorText(detail.error));
          }
        }
      } catch {
        if (epoch === requestEpoch.current) setMessage("IPC_UNAVAILABLE：无法读取会话状态");
      }
    },
    [applyStatus],
  );

  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  useEffect(() => { refreshRef.current = refresh; }, [refresh]);

  const conversationRef = useRef<HTMLDivElement | null>(null);
  const conversationBottomRef = useRef<HTMLDivElement | null>(null);
  // 事件文本最多 160 字符而库里是全文，因此用前缀比较识别“列表最后一轮就是当前显示轮”，
  // 避免刷新落地前的窗口期里同一轮出现两次。
  const lastTurn = turns.at(-1);
  const lastTurnIsLive = !!lastTurn && (transcript !== "" || reply !== "")
    && (transcript === "" || lastTurn.userText.startsWith(transcript))
    && (reply === "" || lastTurn.assistantText.startsWith(reply));
  const historyTurns = lastTurnIsLive ? turns.slice(0, -1) : turns;
  useEffect(() => {
    const container = conversationRef.current;
    const bottom = conversationBottomRef.current;
    if (!container || !bottom) return;
    const nearBottom = container.scrollHeight - container.scrollTop - container.clientHeight < 400;
    if (nearBottom && typeof bottom.scrollIntoView === "function") bottom.scrollIntoView({ block: "end" });
  }, [turns.length, transcript, reply]);

  useEffect(() => {
    void (async () => {
      try {
        const result = await api.getRuntimeStatus();
        if (result.ok) {
          applyStatus(result.data);
          setMessage("");
        } else {
          setMessage(errorText(result.error));
        }
      } catch {
        setMessage("IPC_UNAVAILABLE：无法读取会话状态");
      }
    })();
  }, [applyStatus]);

  useEffect(() => {
    let cancelled = false;
    const unlisteners: Array<() => void> = [];
    void (async () => {
      const topics: Array<[string, (payload: never) => void]> = [
        ["runtime.status.v1", applyStatus as (payload: never) => void],
        ["session.transcript.v1", applyTranscript as (payload: never) => void],
        ["session.reply.v1", applyReply as (payload: never) => void],
      ];
      for (const [event, handler] of topics) {
        try {
          const unlisten = await Promise.resolve(listen(event, handler));
          if (cancelled) {
            unlisten();
            return;
          }
          unlisteners.push(unlisten);
        } catch {
          // Event bus is optional when IPC is unavailable.
        }
      }
    })();
    return () => {
      cancelled = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [listen, applyStatus, applyTranscript, applyReply]);

  async function run(action: () => Promise<CommandResult<unknown>>, success = "") {
    setBusy(true);
    try {
      const result = await action();
      if (!result.ok) {
        setMessage(errorText(result.error));
        return false;
      }
      if (success) setMessage(success);
      else setMessage("");
      return true;
    } catch {
      setMessage("IPC_UNAVAILABLE：本地操作失败");
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function start() {
    requestEpoch.current += 1;
    setBusy(true);
    setIssues([]);
    if (config && (!roleProfileId || !voiceRouteId)) {
      setConfigurationOpen(true);
      setIssues([
        ...(!roleProfileId ? [{ code: "SESSION_ROLE_REQUIRED", area: "role", action: "open_services" }] : []),
        ...(!voiceRouteId ? [{ code: "SESSION_ROUTE_REQUIRED", area: "speech", action: "open_services" }] : []),
      ]);
      setMessage("");
      setBusy(false);
      return;
    }
    try {
      if (inputSource === "meeting" && !meetingPid) { setConfigurationOpen(true); setMessage("请刷新并选择会议进程。"); return; }
      if (inputSource === "meeting" && !virtualAudio?.installed) { setConfigurationOpen(true); setMessage(virtualAudio?.rebootRequired ? "请重启 Windows，使虚拟声卡生效后再开始会议。" : "请先安装并自动配置虚拟声卡。"); return; }
      const result = roleProfileId && voiceRouteId
        ? await api.startSession(transport, { roleProfileId, voiceRouteId, allowWebSearch: allowWebSearch && canSearch, ...(inputSource === "meeting" ? { meetingPid: Number(meetingPid) } : {}), ...(outputDeviceId ? { outputDeviceId } : {}) })
        : await api.startSession(transport);
      if (!result.ok) {
        setConfigurationOpen(true);
        setMessage(errorText(result.error));
        return;
      }
      if (result.data.kind === "blocked") {
        setConfigurationOpen(true);
        setIssues(result.data.issues);
        setMessage("");
        return;
      }
      setConfigurationOpen(false);
      setSessionId(result.data.session.id);
      sessionIdRef.current = result.data.session.id;
      setPhase(result.data.session.status);
      setTranscript("");
      setReply("");
      setTurns([]);
      setWebSources([]);
      setWebDegraded(false);
      setPendingConfirmation(false);
      setConfirmationText("");
      setUnusedMaterials(false);
      setSayText("");
      setCorrectText("");
      setReportSummary("");
      setReportDetail("");
      setMessage("");
      setLivekitState("idle");
      if (result.data.livekit) {
        try {
          livekitRoom.current = await connectLiveKitRoom(result.data.livekit);
          setLivekitState("connected");
        } catch {
          setConfigurationOpen(true);
          setLivekitState("error");
          setMessage("LIVEKIT_CONNECT_FAILED：无法进入房间");
        }
      }
      await refresh(result.data.session.id);
    } catch {
      setConfigurationOpen(true);
      setMessage("IPC_UNAVAILABLE：本地操作失败");
    } finally {
      setBusy(false);
    }
  }

  async function stop() {
    requestEpoch.current += 1;
    await disconnectLiveKitRoom(livekitRoom.current);
    livekitRoom.current = null;
    setLivekitState("idle");
    const ok = await run(() => api.stopSession());
    if (ok) {
      setPhase("completed");
      setPendingConfirmation(false);
      await refresh();
    }
  }

  async function setModeName(next: "ai_active" | "operator_speaking" | "paused" | "muted") {
    const ok = await run(() => api.setSessionMode(next));
    if (ok) {
      setMode(next);
      if (next !== "ai_active") setPendingConfirmation(false);
      await refresh();
    }
  }

  function commandId() {
    return crypto.randomUUID();
  }

  function reportLines(result: Record<string, unknown>) {
    const report = result.report;
    if (!report || typeof report !== "object") return "";
    const parts: string[] = [];
    const record = report as Record<string, unknown>;
    for (const key of ["strengths", "followUps", "limitations"] as const) {
      const value = record[key];
      if (Array.isArray(value)) {
        for (const item of value) {
          if (typeof item === "string" && item.trim()) parts.push(item);
        }
      }
    }
    return parts.join("；");
  }

  async function runAgentCommand(input: AgentCommandInput) {
    setBusy(true);
    try {
      const result = await api.sessionAgentCommand(input);
      if (!result.ok) {
        setMessage(errorText(result.error));
        return;
      }
      if (!result.data.ok) {
        setMessage(result.data.error);
        return;
      }
      setMessage("");
      if (input.action === "report") {
        const summary =
          typeof result.data.result.summary === "string" ? result.data.result.summary : "";
        setReportSummary(summary);
        setReportDetail(reportLines(result.data.result));
      }
      await refresh();
    } catch {
      setMessage("IPC_UNAVAILABLE：本地操作失败");
    } finally {
      setBusy(false);
    }
  }

  async function submitSay(event: FormEvent) {
    event.preventDefault();
    await runAgentCommand({
      id: commandId(),
      action: "say",
      text: sayText.trim() || null,
      answer: null,
      mode: null,
      expectedRevision: revision,
    });
  }

  async function confirmCandidateAnswer(event: FormEvent) {
    event.preventDefault();
    const text = confirmationText.trim();
    if (!text) return;
    await runAgentCommand({
      id: commandId(),
      action: "confirm_candidate",
      text,
      answer: null,
      mode: null,
      expectedRevision: revision,
    });
  }

  async function submitCorrect() {
    await runAgentCommand({
      id: commandId(),
      action: "correct",
      text: null,
      answer: correctText.trim() || null,
      mode: null,
      expectedRevision: revision,
    });
  }

  async function submitRetry() {
    await runAgentCommand({
      id: commandId(),
      action: "retry",
      text: null,
      answer: null,
      mode: null,
      expectedRevision: revision,
    });
  }

  async function submitReport() {
    await runAgentCommand({
      id: commandId(),
      action: "report",
      text: null,
      answer: null,
      mode: null,
      expectedRevision: revision,
    });
  }

  const active = ACTIVE_PHASES.has(phase);
  const selectedRoleScenario = roleScenario(config, roleProfileId);
  const hotkeyInFlight = useRef(false);
  useEffect(() => {
    if (!active || inputSource !== "meeting" || selectedRoleScenario !== "meetingAssistant") return;
    let disposed = false;
    let unlisten = () => {};
    void (async () => {
      try {
        unlisten = await Promise.resolve(listen("session.assistant_hotkey.v1", () => {
          if (disposed || hotkeyInFlight.current) return;
          hotkeyInFlight.current = true;
          void api.triggerMeetingAssistant()
            .then((result) => {
              if (!result.ok) setMessage(errorText(result.error));
              else return refresh();
            })
            .catch(() => setMessage("快捷提问失败，请回到工作台重试。"))
            .finally(() => { hotkeyInFlight.current = false; });
        }));
      } catch {
        if (!disposed) setMessage("全局快捷键事件不可用；仍可在工作台点击提问。");
      }
    })();
    return () => {
      disposed = true;
      unlisten();
    };
  }, [active, inputSource, selectedRoleScenario, refresh, listen]);
  const modeRef = useRef(mode);
  useEffect(() => { modeRef.current = mode; }, [mode]);
  const [micActive, setMicActive] = useState(false);
  useEffect(() => {
    if (!active || inputSource !== "mic") return;
    let disposed = false;
    const streamer = createMicStreamer({
      onChunk: (pcm, sampleRate) => {
        // 接管/静音期间不推流，避免人工发言被当作对练内容转写。
        if (disposed || modeRef.current !== "ai_active") return;
        void api.pushMicPcm(pcm, sampleRate).then((result) => {
          if (!disposed && !result.ok) setMessage(errorText(result.error));
        }).catch(() => { if (!disposed) setMessage("IPC_UNAVAILABLE：麦克风数据发送失败"); });
      },
      onError: (micError) => { if (!disposed) setMessage(micError); },
    });
    setMicActive(true);
    streamer.start().catch(() => {
      if (disposed) return;
      setMicActive(false);
      setMessage("无法访问麦克风，请检查系统麦克风权限后重试。");
    });
    return () => { disposed = true; streamer.stop(); setMicActive(false); };
  }, [active, inputSource, createMicStreamer]);
  const audioFinalizePending = useRef(false);
  useEffect(() => {
    if (!active || phase !== "listening" || busy) return;
    if (inputSource === "meeting" && !new Set<RoleScenario>(["interviewer", "hr", "candidate", "meetingAssistant"]).has(selectedRoleScenario as RoleScenario)) return;
    const timer = window.setInterval(() => {
      if (audioFinalizePending.current) return;
      // 接管/静音期间后端不会应答，此时 finalize 只会产生无意义的报错。
      if (modeRef.current !== "ai_active") return;
      void (async () => {
        try {
          const ready = await api.isSessionAudioReady();
          if (!ready.ok || !ready.data.ready || audioFinalizePending.current) return;
          audioFinalizePending.current = true;
          await finalizeUtterance("");
          await refresh();
        } catch (error) {
          // 透出真实错误码（如 REALTIME_REMOTE_ERROR），否则用户只能看到笼统提示。
          setMessage(
            humanizeRemoteError(
              error instanceof Error && error.message ? error.message : "",
            ) || "自动转写失败，已保留会话，可重试或人工接管。",
          );
        } finally { audioFinalizePending.current = false; }
      })();
    }, 250);
    return () => window.clearInterval(timer);
  }, [active, inputSource, phase, busy, selectedRoleScenario, finalizeUtterance, refresh]);
  const selectedRoute = config?.speech.voiceRoutes.find((route) => route.id === voiceRouteId);
  const searchProtocol = config?.models.providers.find((provider) => provider.id === selectedRoute?.llmProviderId)?.webCapability;
  const canSearch = selectedRoute?.mode === "cascaded" && !!searchProtocol && searchProtocol !== "none";

  return (
    <section className="workspace-session" aria-labelledby="workspace-session-heading">
      <header className="session-toolbar">
        <div className="session-toolbar-meta">
          <h2 id="workspace-session-heading">当前会话</h2>
          {config && <label className="session-role">角色<select disabled={busy || active} value={roleProfileId} onChange={(event) => { const next = event.target.value; setRoleProfileId(next); setAllowWebSearch(roleScenario(config, next) === "meetingAssistant"); }}>
              <option value="">请选择角色</option>
              {config.roleProfiles.filter((role) => role.configVersion > 0).map((role) => <option key={role.id} value={role.id}>{role.name}</option>)}
            </select></label>}
          <span className="status-badge" data-active={active}>
            {PHASE_LABELS[phase] ?? phase}
          </span>
          <span className="session-mode">{MODE_LABELS[mode] ?? mode}</span>
          {livekitState !== "idle" && (
            <span className="session-mode">LiveKit {livekitState === "connected" ? "已连接" : "连接失败"}</span>
          )}
        </div>
        <div className="session-config-heading">
          <span className="session-config-summary">{inputSource === "meeting" ? "会议音频" : "本机麦克风"} · {config?.speech.voiceRoutes.find((route) => route.id === voiceRouteId)?.name ?? "尚未选择语音线路"}</span>
          <button type="button" className="button-ghost" aria-expanded={configurationOpen} aria-controls="session-configuration" onClick={() => setConfigurationOpen((open) => !open)}><Wrench size={15} aria-hidden="true" />会话配置<ChevronDown size={14} aria-hidden="true" /></button>
        </div>
        <div id="session-configuration" className="session-configuration" hidden={!configurationOpen}>
          {!config && <p className="muted">尚未读取到会话配置，请到“服务”和“设置”检查线路与角色。</p>}
          {config && <fieldset disabled={busy || active} className="session-selection">
            <legend>本场会话配置</legend>
            <label>输入来源<select value={inputSource} onChange={(event) => { setInputSource(event.target.value); if (event.target.value === "meeting") { void refreshMeetings(); void refreshVirtualAudio(); } }}>
              <option value="mic">本机麦克风</option><option value="meeting">会议音频</option>
            </select></label>
            {inputSource === "mic" && <small>不用会议或直播：直接对麦克风说话，检测到停顿自动提交给角色；AI 播报时自动抑制回声。{micActive ? "麦克风已开启。" : ""}</small>}
            {inputSource === "meeting" && <>
              <label>会议进程<select value={meetingPid} onChange={(event) => setMeetingPid(event.target.value)}>
                <option value="">请选择会议进程</option>
                {meetingProcesses.map((process) => <option key={process.pid} value={process.pid}>{MEETING_NAMES[process.name.toLowerCase()] ?? process.name} · {process.title} · {process.pid}</option>)}
              </select></label>
              <button type="button" onClick={() => void refreshMeetings()}>刷新会议进程</button>
              <small>仅采集所选会议的音频，不采集屏幕。请告知参会者 AI 参与和转写；检测停顿后自动提交完整语句。</small>
              {selectedRoleScenario === "meetingAssistant" && <small>会议助手普通讨论只转写；被点名，或按 Ctrl+Alt+A 时才回答。</small>}
              {virtualAudio?.state === "missing" && <div className="preflight-card" role="alert">
                <span>检测到缺少虚拟声卡，是否安装并自动配置？</span>
                <button type="button" disabled={installingAudio || audioRetryBlocked} onClick={() => void installVirtualAudio()}>{installingAudio ? "正在安装…" : "是，自动安装"}</button>
              </div>}
              {installingAudio && <p role="status">{PREPARATION_PHASES[audioPreparationPhase]}… 请勿重复启动安装。</p>}
              {audioAttempted && !installingAudio && !virtualAudio?.installed && <small>最近安装步骤：{PREPARATION_PHASES[audioPreparationPhase]}。{audioRetryBlocked ? "请先重新检测，确认没有仍在运行的安装任务。" : "失败说明见页面提示；再次安装前会重新检查驱动状态。"}</small>}
              {!installingAudio && virtualAudio && !virtualAudio.installed && !["missing", "reboot_required"].includes(virtualAudio.state) && <div className="preflight-card" role="alert">{virtualAudio.detail}</div>}
              <button type="button" disabled={installingAudio} onClick={() => void refreshVirtualAudio()}>重新检测虚拟声卡</button>
              {virtualAudio?.rebootRequired && <div className="preflight-card" role="alert">虚拟声卡驱动已安装，需要重启 Windows 后继续。软件不会自动重启电脑。</div>}
              {virtualAudio?.installed && <small>虚拟声卡端点已就绪，将自动绑定音频线路；尚不代表会议对方已能听到声音。</small>}
            </>}
            {inputSource !== "meeting" && <><label>语音输出<select value={outputDeviceId} onChange={(event) => setOutputDeviceId(event.target.value)}>
              <option value="">仅文字，不播放</option>
              {audioOutputs.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
            </select></label>
            <button type="button" onClick={() => void refreshAudioOutputs()}>刷新音频设备</button>
            <small>{outputDeviceId ? "只向所选设备播放。会议需选择虚拟声卡的输入端，并在会议软件选择对应麦克风。" : "当前仅显示文字，AI 语音不会进入会议。"}</small>
            </>}

            <label>语音线路<select value={voiceRouteId} onChange={(event) => setVoiceRouteId(event.target.value)}>
              <option value="">请选择语音线路</option>
              {config.speech.voiceRoutes.filter((route) => route.configVersion > 0).map((route) => <option key={route.id} value={route.id}>{route.name} · {route.llmModelId ?? route.e2eModelId}</option>)}
            </select></label>
            <label><input type="checkbox" disabled={!canSearch} checked={allowWebSearch && canSearch} onChange={(event) => setAllowWebSearch(event.target.checked)} />允许本场联网搜索（可能产生费用）</label>
            {!canSearch && <small>联网问答需要级联语音线路，并在模型供应商设置中选择支持的搜索协议。</small>}
          </fieldset>}
          <fieldset className="session-transport">
            <legend>传输方式</legend>
            <label>
              <input
                type="radio"
                name="transport"
                value="direct"
                checked={transport === "direct"}
                disabled={busy || active}
                onChange={() => setTransport("direct")}
              />
              <span>本机直连</span>
            </label>
            <label>
              <input
                type="radio"
                name="transport"
                value="livekit"
                checked={transport === "livekit"}
                disabled={busy || active}
                onChange={() => setTransport("livekit")}
              />
              <span>LiveKit</span>
            </label>
          </fieldset>
        </div>
        <div className="session-toolbar-controls">
          <div className="service-actions session-controls">
            <button className="button-primary" disabled={busy || active} type="button" onClick={() => void start()}>
              <Play size={14} aria-hidden="true" />开始会话
            </button>
            <button disabled={!active} type="button" onClick={() => void stop()}>
              <Square size={14} aria-hidden="true" />停止
            </button>
            <button disabled={!active} type="button" onClick={() => void setModeName("operator_speaking")}>
              <Hand size={14} aria-hidden="true" />接管
            </button>
            <button
              disabled={!active}
              type="button"
              onPointerDown={() => void setModeName("operator_speaking")}
              onPointerUp={() => void setModeName("ai_active")}
              onPointerCancel={() => void setModeName("ai_active")}
              onKeyDown={(event) => { if (event.key === " " || event.key === "Enter") void setModeName("operator_speaking"); }}
              onKeyUp={(event) => { if (event.key === " " || event.key === "Enter") void setModeName("ai_active"); }}
            >
              <Volume2 size={14} aria-hidden="true" />按住人工发言
            </button>
            <button disabled={busy || !active} type="button" onClick={() => void setModeName("ai_active")}>
              <Bot size={14} aria-hidden="true" />恢复 AI
            </button>
            <button disabled={busy || !active} type="button" onClick={() => void setModeName("muted")}>
              <MicOff size={14} aria-hidden="true" />静音
            </button>
          </div>
        </div>
      </header>
      <PreflightIssues issues={issues} />
      {webDegraded && <p className="services-message">联网搜索未成功，本次回答未联网，请勿作为最新信息使用。</p>}
      {webSources.length > 0 && <aside className="services-message" aria-label="联网来源">
        <span>已联网 · 来源：</span>
        {webSources.map((source) => <button key={source.url} title={source.url} type="button" onClick={() => void run(() => api.openWebSource(source.url))}>{source.title}</button>)}
      </aside>}
      {message && (
        <p className="services-message session-message" role="status">
          {message}
        </p>
      )}
      <div className="session-conversation" role="region" aria-label="会话对话" tabIndex={0} ref={conversationRef}>
        {!transcript && !reply && turns.length === 0 ? (
          <div className="session-welcome">
            <span className="session-welcome-icon"><MessageSquare size={25} strokeWidth={1.5} aria-hidden="true" /></span>
            <span className="session-welcome-eyebrow">ROLEAI · 你的对话助手</span>
            <h3>{active ? "正在等待你的输入" : "开始一段新对话"}</h3>
            <p>{active ? "开口说出问题，停顿后自动提交。" : "点击「开始会话」，与 RoleAI 交流。"}</p>
            <p>{config?.roleProfiles.find((role) => role.id === roleProfileId)?.name ?? "选择一个角色，让对话从这里开始"}</p>
            {!active && <button className="button-ghost" type="button" onClick={() => {
              setConfigurationOpen(true);
              requestAnimationFrame(() => document.getElementById("session-configuration")?.querySelector<HTMLElement>("select, input, button")?.focus());
            }}>调整会话配置</button>}
          </div>
        ) : (
          <div className="session-turn">
            {historyTurns.map((item) => (
              <Fragment key={item.id}>
                {item.userText && (
                  <article className="session-bubble session-bubble-user" aria-label={`用户转写 · 第 ${item.turnIndex + 1} 轮`}>
                    <h3>你 <span>· 转写</span></h3>
                    <p>{item.userText}</p>
                  </article>
                )}
                {item.assistantText && (
                  <article className="session-bubble session-bubble-assistant" aria-label={`AI 回复 · 第 ${item.turnIndex + 1} 轮`}>
                    <h3><Bot size={16} aria-hidden="true" />RoleAI</h3>
                    <p>{item.assistantText}</p>
                  </article>
                )}
              </Fragment>
            ))}
            {historyTurns.length > 0 && <p className="session-turn-label">当前轮</p>}
            {transcript && (
              <article className="session-bubble session-bubble-user" aria-label="用户转写">
                <h3>你 <span>· 转写</span></h3>
                <p>{transcript}</p>
              </article>
            )}
            {reply && (
              <article className="session-bubble session-bubble-assistant" aria-label="AI 回复">
                <h3><Bot size={16} aria-hidden="true" />RoleAI</h3>
                <p>{reply}</p>
              </article>
            )}
            {pendingConfirmation && (
              <form className="candidate-confirmation" onSubmit={confirmCandidateAnswer}>
                <label htmlFor="candidate-confirmation-text">确认播报内容</label>
                <textarea
                  id="candidate-confirmation-text"
                  value={confirmationText}
                  onChange={(event) => setConfirmationText(event.target.value)}
                />
                <p>求职者模式不会自动播报。请核对或编辑后再确认。</p>
                <button className="button-primary" disabled={busy || !active || !confirmationText.trim()} type="submit">
                  <Volume2 size={15} aria-hidden="true" />确认并播报
                </button>
              </form>
            )}
          </div>
        )}
        {unusedMaterials && <p className="session-materials-note">本轮未使用资料</p>}
        <div ref={conversationBottomRef} aria-hidden="true" />
      </div>
      <details className="session-tools">
        <summary><Wrench size={15} aria-hidden="true" />会话工具<ChevronDown size={15} className="session-tools-chevron" aria-hidden="true" /></summary>
        <div className="session-tools-body" role="region" aria-label="会话工具">
          <form className="service-form session-tool-form" onSubmit={submitSay}>
            <label htmlFor="session-say">朗读文本</label>
            <div className="session-tool-row">
              <input id="session-say" value={sayText} onChange={(event) => setSayText(event.target.value)} placeholder="输入需要 AI 朗读的文本" />
              <button disabled={busy || !active} type="submit"><Volume2 size={15} aria-hidden="true" />朗读</button>
            </div>
          </form>
          <form className="service-form session-tool-form" onSubmit={(event) => { event.preventDefault(); void submitCorrect(); }}>
            <label htmlFor="session-correct">纠正内容</label>
            <div className="session-tool-row">
              <input id="session-correct" value={correctText} onChange={(event) => setCorrectText(event.target.value)} placeholder="输入修正后的回答" />
              <button disabled={busy || !active} type="submit">纠正</button>
            </div>
          </form>
          <div className="service-actions">
            <button disabled={busy || !active} type="button" onClick={() => void submitRetry()}><RotateCcw size={15} aria-hidden="true" />重试</button>
            <button disabled={busy || !active} type="button" onClick={() => void submitReport()}><FileText size={15} aria-hidden="true" />报告</button>
          </div>
          {(reportSummary || reportDetail) && (
            <section className="session-report" aria-labelledby="session-report-heading">
              <h3 id="session-report-heading">会话纪要</h3>
              {reportSummary && <p>{reportSummary}</p>}
              {reportDetail && <p className="muted">{reportDetail}</p>}
            </section>
          )}
        </div>
      </details>
    </section>
  );
}
