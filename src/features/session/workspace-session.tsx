import { Fragment, FormEvent, useCallback, useEffect, useRef, useState } from "react";
import { Bot, Camera, ChevronDown, Copy, FileText, Globe, Hand, MessageSquare, Mic, MicOff, Pause, Play, Plus, RotateCcw, ScreenShare, Send, Square, Volume2, Wrench, X } from "lucide-react";

import * as api from "../../api/commands";
import "../../styles/workspace.css";
import { MicStreamer, type MicStreamCallbacks, type MicStreamController } from "./mic-recorder";
import {
  WebAudioPlayer,
  audioDiagnostics,
  createAudioContextForOutput,
  decodePcm16Base64,
  resolveWebAudioSinkId,
} from "./web-audio-player";
import { VideoSharer, type VideoShareKind, type VideoSharerCallbacks, type VideoSharerController } from "./video-sharer";
import { PreflightIssues } from "./preflight-issues";
import {
  ACTIVE_PHASES,
  BAR_FACTORS,
  RATE_LIMIT_HINT,
  clockOf,
  errorText,
  formatDuration,
  humanizeRemoteError,
  roleScenario,
} from "./workspace-format";
import type {
  AgentCommandInput,
  CommandResult,
  RuntimeStatus,
  SessionReplyEvent,
  SessionTranscriptEvent,
  PreflightIssue,
  PublicConfig,
  RoleScenario,
  SessionTurnView,
  WebSource,
  MeetingProcess,
  AudioOutputDevice,
  VirtualAudioPreparation,
} from "../../generated/bindings";

const MEETING_NAMES: Record<string, string> = {
  "teams.exe": "Microsoft Teams", "ms-teams.exe": "Microsoft Teams",
  "wemeetapp.exe": "腾讯会议", "feishu.exe": "飞书", "lark.exe": "Lark",
  "dingtalk.exe": "钉钉", "zoom.exe": "Zoom",
};

const PREPARATION_PHASES: Record<string, string> = {
  checking: "检测安装环境", downloading: "下载安装包", verifying: "校验安装包和签名",
  authorizing: "等待 Windows 管理员授权", installing: "安装驱动", rechecking: "重新检测音频端点",
};

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
  createMicStreamer?: (
    callbacks: MicStreamCallbacks,
    sharedContext?: AudioContext,
  ) => MicStreamController;
  createVideoSharer?: (
    kind: VideoShareKind,
    callbacks: VideoSharerCallbacks,
  ) => VideoSharerController;
}

function defaultCreateMicStreamer(
  callbacks: MicStreamCallbacks,
  sharedContext?: AudioContext,
): MicStreamController {
  return new MicStreamer(callbacks, sharedContext);
}

interface LocalSessionAudioEvent {
  seq: number;
  pcmBase64: string;
  sampleRate: number;
}

interface LocalSessionPlaybackControlEvent {
  seq: number;
  action: "clear";
}

function defaultCreateVideoSharer(
  kind: VideoShareKind,
  callbacks: VideoSharerCallbacks,
): VideoSharerController {
  return new VideoSharer(kind, callbacks);
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
  createVideoSharer = defaultCreateVideoSharer,
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
  const [allowBargeIn, setAllowBargeIn] = useState(api.DEFAULT_BARGE_IN);
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
    Promise.resolve(listen<string>("virtual_audio:preparation:v1", (phase) => {
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
  const [chatText, setChatText] = useState("");
  const [micLevel, setMicLevel] = useState(0);
  const [callSeconds, setCallSeconds] = useState(0);
  const [videoKind, setVideoKind] = useState<"off" | VideoShareKind>("off");
  const callStartRef = useRef<number | null>(null);
  const pipVideoRef = useRef<HTMLVideoElement | null>(null);
  const [revision, setRevision] = useState(0);
  const [reportSummary, setReportSummary] = useState("");
  const [reportDetail, setReportDetail] = useState("");
  const statusSeq = useRef(0);
  const transcriptSeq = useRef(0);
  const replySeq = useRef(0);
  const playbackSeq = useRef(0);
  const webAudioPlayerRef = useRef<WebAudioPlayer | null>(null);
  // 在「开始会话」点击手势内创建并 resume，避免自动播放策略让上下文一直挂起。
  const playbackContextRef = useRef<AudioContext | null>(null);
  const [moreOpen, setMoreOpen] = useState(false);
  const moreRef = useRef<HTMLDetailsElement | null>(null);
  useEffect(() => {
    if (!moreOpen) return;
    const closeOnOutside = (event: PointerEvent) => {
      if (moreRef.current && !moreRef.current.contains(event.target as Node)) setMoreOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === "Escape") setMoreOpen(false); };
    document.addEventListener("pointerdown", closeOnOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [moreOpen]);
  const sessionIdRef = useRef<string | null>(null);
  const refreshRef = useRef<() => Promise<void>>(async () => {});
  const [realtimeStatus, setRealtimeStatus] = useState("idle");

  const applyStatus = useCallback((next: RuntimeStatus) => {
    if (next.seq <= statusSeq.current) return;
    statusSeq.current = next.seq;
    setPhase(next.phase);
    setMode(next.mode);
    setUnusedMaterials(next.unusedMaterials);
    setRevision(next.revision);
    if (next.realtimeStatus && next.realtimeStatus !== "unknown") {
      setRealtimeStatus(next.realtimeStatus);
    }
    if (next.mode !== "ai_active") {
      setPendingConfirmation(false);
    }
    if (next.lastErrorCode) {
      setMessage(errorText({ code: next.lastErrorCode, message: "会话运行时错误" }));
    }
  }, []);

  const applyTranscript = useCallback((payload: SessionTranscriptEvent) => {
    if (payload.seq > transcriptSeq.current) {
      transcriptSeq.current = payload.seq;
      setTranscript(payload.text);
    }
    // done=true 表示已落库：流式快照用 2^32 起步的大序号，落库事件用小序号，
    // 按序号门控会被当旧事件丢掉，所以落库收尾不看序号、总是刷新，文本以库为准。
    if (payload.done) void refreshRef.current();
  }, []);

  const applyReply = useCallback((payload: SessionReplyEvent) => {
    if (payload.seq > replySeq.current) {
      replySeq.current = payload.seq;
      setReply(payload.text);
    }
    if (payload.done) void refreshRef.current();
  }, []);
  const applyPlaybackAudio = useCallback((payload: LocalSessionAudioEvent) => {
    audioDiagnostics.eventsReceived += 1;
    audioDiagnostics.bytesReceived += payload.pcmBase64.length;
    if (payload.seq <= playbackSeq.current) return;
    playbackSeq.current = payload.seq;
    const player = webAudioPlayerRef.current;
    if (!player) return;
    void player.resume().catch(() => undefined);
    player.appendPcm16(decodePcm16Base64(payload.pcmBase64));
  }, []);
  const applyPlaybackControl = useCallback((payload: LocalSessionPlaybackControlEvent) => {
    if (payload.seq <= playbackSeq.current) return;
    playbackSeq.current = payload.seq;
    webAudioPlayerRef.current?.clear();
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
  // 事件文本是本轮的前缀快照（上限 4000 字符）而库里是全文，因此用前缀比较识别
  // “列表最后一轮就是当前显示轮”，避免刷新落地前的窗口期里同一轮出现两次。
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
        ["runtime:status:v1", applyStatus as (payload: never) => void],
        ["session:transcript:v1", applyTranscript as (payload: never) => void],
        ["session:reply:v1", applyReply as (payload: never) => void],
        ["session:audio:v1", applyPlaybackAudio as (payload: never) => void],
        ["session:playback-control:v1", applyPlaybackControl as (payload: never) => void],
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
  }, [listen, applyStatus, applyTranscript, applyReply, applyPlaybackAudio, applyPlaybackControl]);

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
    if (inputSource === "mic" && !playbackContextRef.current && typeof AudioContext === "function") {
      // 必须在点击手势的同步阶段创建并 resume，await 之后手势已失效。
      const { context } = createAudioContextForOutput("");
      void context.resume().catch(() => undefined);
      playbackContextRef.current = context;
    }
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
        ? await api.startSession({ roleProfileId, voiceRouteId, allowWebSearch: allowWebSearch && canSearch, allowBargeIn, ...(inputSource === "meeting" ? { meetingPid: Number(meetingPid) } : {}), ...(outputDeviceId ? { outputDeviceId } : {}) })
        : await api.startSession();
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
      // 新会话从零计流式字幕 seq：上一会话的 partial 用过大号 seq，
      // 不重置会把本会话开头的字幕事件整体门控丢弃。
      transcriptSeq.current = 0;
      replySeq.current = 0;
      playbackSeq.current = 0;
      webAudioPlayerRef.current?.clear();
      setRealtimeStatus("connected");
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
      setVideoKind("off");
      setReportSummary("");
      setReportDetail("");
      setMessage("");
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
    const ok = await run(() => api.stopSession());
    if (ok) {
      setPhase("completed");
      setPendingConfirmation(false);
      setVideoKind("off");
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

  // Composer 文本发送：与语音共用 finalize 管线（triggerSource=manual），
  // 文字进入对话历史并由角色生成语音回复。
  async function submitChat(event: FormEvent) {
    event.preventDefault();
    const text = chatText.trim();
    if (!text || busy || !active || mode !== "ai_active") return;
    setChatText("");
    // 先把文字即时上屏为本轮转写；阻塞的轮次请求返回后由刷新接管。
    setTranscript(text);
    try {
      await finalizeUtterance(text);
      setMessage("");
    } catch (error) {
      setMessage(
        error instanceof Error && error.message
          ? humanizeRemoteError(error.message)
          : "发送失败，请重试。",
      );
    }
    await refresh();
  }

  function copyText(text: string) {
    try {
      void navigator.clipboard?.writeText(text).catch(() => {});
    } catch {
      // WebView2 剪贴板不可用时静默忽略，不影响会话。
    }
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
  // 通话计时：以会话在本界面内首次进入活跃态的时刻为锚点。
  useEffect(() => {
    if (!active) {
      callStartRef.current = null;
      setCallSeconds(0);
      return;
    }
    if (callStartRef.current === null) callStartRef.current = Date.now();
    const timer = window.setInterval(() => {
      if (callStartRef.current !== null) {
        setCallSeconds(Math.floor((Date.now() - callStartRef.current) / 1000));
      }
    }, 1000);
    return () => window.clearInterval(timer);
  }, [active]);
  const roleName = config?.roleProfiles.find((role) => role.id === roleProfileId)?.name ?? "RoleAI";
  const showBargeHint = active && phase === "speaking" && allowBargeIn && mode === "ai_active";
  const selectedRoleScenario = roleScenario(config, roleProfileId);
  const hotkeyInFlight = useRef(false);
  useEffect(() => {
    if (!active || inputSource !== "meeting" || selectedRoleScenario !== "meetingAssistant") return;
    let disposed = false;
    let unlisten = () => {};
    void (async () => {
      try {
        unlisten = await Promise.resolve(listen("session:assistant_hotkey:v1", () => {
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
    let context: AudioContext | null = playbackContextRef.current;
    playbackContextRef.current = null;
    let player: WebAudioPlayer | null = null;
    if (!context && typeof AudioContext === "function") {
      context = createAudioContextForOutput("").context;
    }
    if (context) {
      player = new WebAudioPlayer(context);
      webAudioPlayerRef.current = player;
      void player.resume().catch(() => undefined);
      const outputName = audioOutputs.find((device) => device.id === outputDeviceId)?.name;
      const target = context as AudioContext & { setSinkId?: (id: string) => Promise<void> };
      if (outputDeviceId && typeof target.setSinkId === "function") {
        void resolveWebAudioSinkId(outputName).then((sinkId) => {
          if (disposed) return;
          if (!sinkId) {
            setMessage("未在浏览器中找到所选输出设备，已使用系统默认输出。");
            return;
          }
          return target.setSinkId!(sinkId).then(() => {
            audioDiagnostics.sinkId = sinkId;
          });
        }).catch(() => {
          if (!disposed) setMessage("切换输出设备失败，已使用系统默认输出。");
        });
      }
    }
    const streamer = createMicStreamer({
      onChunk: (pcm, sampleRate) => {
        // 接管/静音期间不推流，避免人工发言被当作对练内容转写。
        if (disposed || modeRef.current !== "ai_active") return;
        void api.pushMicPcm(pcm, sampleRate).then((result) => {
          if (!disposed && !result.ok) setMessage(errorText(result.error));
        }).catch(() => { if (!disposed) setMessage("IPC_UNAVAILABLE：麦克风数据发送失败"); });
      },
      onError: (micError) => { if (!disposed) setMessage(micError); },
      onLevel: (level) => { if (!disposed) setMicLevel(level); },
    }, context ?? undefined);
    setMicActive(true);
    streamer.start().catch(() => {
      if (disposed) return;
      setMicActive(false);
      setMessage("无法访问麦克风，请检查系统麦克风权限后重试。");
    });
    return () => {
      disposed = true;
      streamer.stop();
      player?.clear();
      if (context) void context.close().catch(() => undefined);
      webAudioPlayerRef.current = null;
      setMicActive(false);
      setMicLevel(0);
    };
    // audioOutputs 只用于按名称映射 sinkId，不应因列表刷新而重建麦克风与播放。
  }, [active, inputSource, outputDeviceId, createMicStreamer]);
  const audioFinalizePending = useRef(false);
  const consecutiveAutoFailures = useRef(0);
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
          consecutiveAutoFailures.current = 0;
          await refresh();
        } catch (error) {
          // 透出真实错误码（如 REALTIME_REMOTE_ERROR），否则用户只能看到笼统提示。
          // 连续快速失败多为供应商限流：追加退避提示，避免用户在限流窗口内反复重试。
          consecutiveAutoFailures.current += 1;
          const backoff = consecutiveAutoFailures.current >= 2 ? RATE_LIMIT_HINT : "";
          setMessage(
            (humanizeRemoteError(
              error instanceof Error && error.message ? error.message : "",
            ) || "自动转写失败，已保留会话，可重试或人工接管。") + (backoff ? backoff : ""),
          );
        } finally { audioFinalizePending.current = false; }
      })();
    }, 250);
    return () => window.clearInterval(timer);
  }, [active, inputSource, phase, busy, selectedRoleScenario, finalizeUtterance, refresh]);
  const selectedRoute = config?.speech.voiceRoutes.find((route) => route.id === voiceRouteId);
  const searchProtocol = config?.models.providers.find((provider) => provider.id === selectedRoute?.llmProviderId)?.webCapability;
  // 端到端线路由 DashScope session.update enable_search 开启（Qwen3.8-Omni-Realtime 系
  // 服务端搜索）；级联线路仍要求已选择支持的搜索协议。
  const canSearch = selectedRoute?.mode === "e2e"
    || (selectedRoute?.mode === "cascaded" && !!searchProtocol && searchProtocol !== "none");

  // 视频帧上行（摄像头/桌面共享）仅端到端 Qwen-Omni 系线路支持
  // （input_image_buffer.append 为 DashScope 方言能力，与联网搜索同判定模式）。
  const canVideo = selectedRoute?.mode === "e2e";
  useEffect(() => {
    if (!active || !canVideo || videoKind === "off") return;
    let disposed = false;
    const sharer = createVideoSharer(videoKind, {
      onFrame: (jpeg) => {
        if (disposed) return;
        void api.pushVideoFrame(jpeg).then((result) => {
          if (disposed || result.ok) return;
          setMessage(errorText(result.error));
        }).catch(() => {
          if (!disposed) setMessage("IPC_UNAVAILABLE：视频帧发送失败");
        });
      },
      onError: (videoError) => { if (!disposed) setMessage(videoError); },
      // 系统 UI 点"停止共享"/摄像头拔出：复位按钮与预览。
      onEnded: () => { if (!disposed) setVideoKind("off"); },
    });
    sharer.start().then(() => {
      if (disposed) return;
      if (pipVideoRef.current && sharer.stream) pipVideoRef.current.srcObject = sharer.stream;
    }).catch(() => {
      if (disposed) return;
      setVideoKind("off");
      setMessage(videoKind === "camera"
        ? "无法访问摄像头，请检查系统权限后重试。"
        : "无法开始桌面共享，请重试。");
    });
    return () => { disposed = true; sharer.stop(); };
  }, [active, canVideo, videoKind, createVideoSharer]);

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
          {realtimeStatus === "reconnecting" && <span className="status-badge" data-active={active}>语音重连中…</span>}
          {realtimeStatus === "failed" && <span className="status-badge" data-active={false}>语音连接失败</span>}
          <span className="session-mode">{MODE_LABELS[mode] ?? mode}</span>
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
              <option value="">系统默认输出</option>
              {audioOutputs.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}
            </select></label>
            <button type="button" onClick={() => void refreshAudioOutputs()}>刷新音频设备</button>
              <small>本机麦克风使用 WebView 全双工播放和浏览器回声消除；所选输出同时作为原生兜底设备。</small>
            </>}

            <label>语音线路<select value={voiceRouteId} onChange={(event) => setVoiceRouteId(event.target.value)}>
              <option value="">请选择语音线路</option>
              {config.speech.voiceRoutes.filter((route) => route.configVersion > 0).map((route) => <option key={route.id} value={route.id}>{route.name} · {route.llmModelId ?? route.e2eModelId}</option>)}
            </select></label>
            <label><input type="checkbox" disabled={!canSearch} checked={allowWebSearch && canSearch} onChange={(event) => setAllowWebSearch(event.target.checked)} />允许本场联网搜索（可能产生费用）</label>
            {!canSearch && <small>联网问答：端到端线路需 DashScope Qwen3.8-Omni 系模型；级联线路需在模型供应商设置中选择支持的搜索协议。</small>}
            <label><input type="checkbox" disabled={busy || active} checked={allowBargeIn} onChange={(event) => setAllowBargeIn(event.target.checked)} />允许语音打断（说话即可停止 AI 播报）</label>
            <small>采集会议音频的会话会自动关闭打断；本机麦克风会话随时生效。</small>
          </fieldset>}
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
                    <p>{item.userText}</p>
                  </article>
                )}
                {item.assistantText && (
                  <article className="session-bubble session-bubble-assistant" aria-label={`AI 回复 · 第 ${item.turnIndex + 1} 轮`}>
                    <header className="bubble-head">
                      <span className="bubble-avatar" aria-hidden="true"><Bot size={15} /></span>
                      <h3>{roleName}</h3>
                      {clockOf(item.createdAt) && <time className="bubble-time">{clockOf(item.createdAt)}</time>}
                      <button type="button" className="bubble-copy" aria-label={`复制第 ${item.turnIndex + 1} 轮回复`} onClick={() => copyText(item.assistantText)}>
                        <Copy size={13} aria-hidden="true" />
                      </button>
                    </header>
                    <p>{item.assistantText}</p>
                  </article>
                )}
              </Fragment>
            ))}
            {historyTurns.length > 0 && <p className="session-turn-label">当前轮</p>}
            {transcript && (
              <article className="session-bubble session-bubble-user session-bubble-live" aria-label="用户转写">
                <p>{transcript}</p>
              </article>
            )}
            {reply && (
              <article className="session-bubble session-bubble-assistant session-bubble-live" aria-label="AI 回复">
                <header className="bubble-head">
                  <span className="bubble-avatar" aria-hidden="true"><Bot size={15} /></span>
                  <h3>{roleName}</h3>
                </header>
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
      <div className="session-footer">
        <div className="session-footer-status">
          {showBargeHint && (
            <button
              type="button"
              className="session-hint-pill"
              onClick={() => webAudioPlayerRef.current?.clear()}
            >
              <Pause size={12} aria-hidden="true" />说话、输入或点击打断
            </button>
          )}
        </div>
        <div className="session-compose">
          {videoKind !== "off" && (
            <div className="session-pip" role="region" aria-label="视频画面预览">
              <video ref={pipVideoRef} autoPlay muted playsInline />
              <span className="session-pip-label">{videoKind === "camera" ? "摄像头" : "共享桌面"}</span>
              <button type="button" className="session-pip-stop" aria-label="停止视频共享" onClick={() => setVideoKind("off")}>
                <X size={13} aria-hidden="true" />
              </button>
            </div>
          )}
          {/* 官方实时通话布局：上方文本输入，下方左侧是工具与麦克风/视频，右侧是发送与通话按钮。 */}
          <form id="session-chat-form" className="composer-input" onSubmit={submitChat}>
            <input
              aria-label="输入内容"
              value={chatText}
              onChange={(event) => setChatText(event.target.value)}
              placeholder={active ? "说话，或输入文字继续对话" : "开始会话后即可说话或输入文字"}
              disabled={!active}
            />
          </form>
          <div className="composer-toolbar">
            <div className="composer-toolbar-group">
              <details
                className="composer-more"
                open={moreOpen}
                onToggle={(event) => setMoreOpen((event.currentTarget as HTMLDetailsElement).open)}
                ref={moreRef}
              >
                <summary className="composer-icon-button" aria-label="更多操作">
                  <Plus size={18} aria-hidden="true" />
                </summary>
                <div className="composer-more-panel" role="region" aria-label="会话工具">
                  <p className="composer-more-title">工具调用</p>
                  <label className="composer-switch">
                    <span><Globe size={14} aria-hidden="true" />联网搜索</span>
                    <input
                      type="checkbox"
                      role="switch"
                      aria-label="联网搜索"
                      disabled={active || !canSearch}
                      checked={allowWebSearch && canSearch}
                      onChange={(event) => setAllowWebSearch(event.target.checked)}
                    />
                  </label>
                  <p className="composer-more-title">通话控制</p>
                  <div className="composer-more-actions">
                    {active && (
                      <button className="button-primary" disabled type="button">
                        <Play size={14} aria-hidden="true" />开始会话
                      </button>
                    )}
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
                  <p className="composer-more-title">内容工具</p>
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
                  <div className="composer-more-actions">
                    <button disabled={busy || !active} type="button" onClick={() => void submitRetry()}><RotateCcw size={15} aria-hidden="true" />重试</button>
                    <button disabled={busy || !active} type="button" onClick={() => void submitReport()}><FileText size={15} aria-hidden="true" />报告</button>
                  </div>
                </div>
              </details>
              <span className="composer-role-chip" title={`当前角色：${roleName}`}>
                <span className="composer-role-dot" aria-hidden="true" />{roleName}
              </span>
              <button
                type="button"
                className="composer-mic"
                aria-label={!active ? "开始语音会话" : mode === "muted" ? "取消静音" : "静音麦克风"}
                data-live={active && inputSource === "mic" && mode === "ai_active"}
                disabled={busy}
                onClick={() => {
                  if (!active) void start();
                  else void setModeName(mode === "muted" ? "ai_active" : "muted");
                }}
              >
                {mode === "muted" && active ? (
                  <MicOff size={16} aria-hidden="true" />
                ) : (
                  <Mic size={16} aria-hidden="true" />
                )}
                {active && inputSource === "mic" && mode !== "muted" && (
                  <span className="composer-bars" aria-hidden="true">
                    {BAR_FACTORS.map((factor) => (
                      <span key={factor} className="composer-bar" style={{ height: `${Math.max(15, Math.round(micLevel * factor * 100))}%` }} />
                    ))}
                  </span>
                )}
              </button>
              {canVideo && (
                <>
                  <button
                    type="button"
                    className="composer-icon-button composer-video"
                    aria-label={videoKind === "camera" ? "关闭摄像头共享" : "共享摄像头"}
                    data-active={videoKind === "camera"}
                    disabled={!active || busy}
                    onClick={() => setVideoKind(videoKind === "camera" ? "off" : "camera")}
                  >
                    <Camera size={17} aria-hidden="true" />
                  </button>
                  <button
                    type="button"
                    className="composer-icon-button composer-video"
                    aria-label={videoKind === "screen" ? "关闭桌面共享" : "共享桌面"}
                    data-active={videoKind === "screen"}
                    disabled={!active || busy}
                    onClick={() => setVideoKind(videoKind === "screen" ? "off" : "screen")}
                  >
                    <ScreenShare size={17} aria-hidden="true" />
                  </button>
                </>
              )}
            </div>
            <div className="composer-toolbar-group">
              <button
                type="submit"
                form="session-chat-form"
                className="composer-send"
                aria-label="发送"
                disabled={busy || !active || mode !== "ai_active" || !chatText.trim()}
              >
                <Send size={15} aria-hidden="true" />
              </button>
              {/* 独立 key：开始/结束按钮占同一位置，复用同一 DOM 节点会让开始的那次点击触发结束。 */}
              {active ? (
                <button key="call-end" type="button" className="session-call-pill" aria-label="结束通话" disabled={busy} onClick={() => void stop()}>
                  <Square size={11} aria-hidden="true" />
                  <span className="session-call-time">{formatDuration(callSeconds)}</span>
                </button>
              ) : (
                <button key="call-start" className="button-primary composer-start" disabled={busy} type="button" onClick={() => void start()}>
                  <Play size={14} aria-hidden="true" />开始会话
                </button>
              )}
            </div>
          </div>
        </div>
        {(reportSummary || reportDetail) && (
          <section className="session-report" aria-labelledby="session-report-heading">
            <h3 id="session-report-heading">会话纪要</h3>
            {reportSummary && <p>{reportSummary}</p>}
            {reportDetail && <p className="muted">{reportDetail}</p>}
          </section>
        )}
      </div>
    </section>
  );
}
