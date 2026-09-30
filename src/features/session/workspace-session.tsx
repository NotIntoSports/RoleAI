import { useCallback, useEffect, useRef, useState } from "react";
import { Camera, Check, ChevronDown, Mic, MicOff, Pause, Play, Plus, ScreenShare, Send, Square, X } from "lucide-react";

import * as api from "../../api/commands";
import "../../styles/workspace.css";
import type { MicStreamCallbacks, MicStreamController } from "./mic-recorder";
import { useSessionEvents } from "./use-session-events";
import { useSessionControls } from "./use-session-controls";
import { useSessionMedia } from "./use-session-media";
import type { VideoShareKind, VideoSharerCallbacks, VideoSharerController } from "./video-sharer";
import { defaultCreateMicStreamer, defaultCreateVideoSharer } from "./media-factories";
import { PreflightIssues } from "./preflight-issues";
import {
  ACTIVE_PHASES,
  BAR_FACTORS,
  errorText,
  formatDuration,
  humanizeRemoteError,
  rateLimitHint,
  roleScenario,
} from "./workspace-format";
import { t, useT } from "../../i18n";
import { SessionToolbar } from "./session-toolbar";
import { TranscriptPanel } from "./transcript-panel";
import { AgentToolsPanel } from "./agent-tools-panel";
import { useLatencyWaterfallPreference } from "./latency-preference";
import type {
  RuntimeStatus,
  PreflightIssue,
  PublicConfig,
  RoleScenario,
  SessionTurnView,
  WebSource,
  MeetingProcess,
  AudioOutputDevice,
  VirtualAudioPreparation,
} from "../../generated/bindings";

const PREPARATION_PHASE_KEYS = new Set(["checking", "downloading", "verifying", "authorizing", "installing", "rechecking"]);

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
  useT();
  const [phase, setPhase] = useState("idle");
  const [mode, setMode] = useState("ai_active");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [transcript, setTranscript] = useState("");
  const [reply, setReply] = useState("");
  const [turns, setTurns] = useState<SessionTurnView[]>([]);
  const [unusedMaterials, setUnusedMaterials] = useState(false);
  const [message, setMessage] = useState(() => t("session.workspace.readingStatus"));
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
  // 设置页「性能」面板可关的轮次延迟瀑布条（localStorage 偏好，默认开）。
  const latencyWaterfallEnabled = useLatencyWaterfallPreference();
  const [virtualAudio, setVirtualAudio] = useState<VirtualAudioPreparation | null>(null);
  const [installingAudio, setInstallingAudio] = useState(false);
  const [audioPreparationPhase, setAudioPreparationPhase] = useState("checking");
  const [audioRetryBlocked, setAudioRetryBlocked] = useState(false);
  const [audioAttempted, setAudioAttempted] = useState(false);
  useEffect(() => {
    let disposed = false;
    let unlisten = () => {};
    Promise.resolve(listen<string>("virtual_audio:preparation:v1", (phase) => {
      if (!disposed && PREPARATION_PHASE_KEYS.has(phase)) setAudioPreparationPhase(phase);
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
    } catch { setVirtualAudio(null); setMessage(t("session.controls.virtualAudioCheckFailed")); }
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
    } catch { setMessage(t("session.controls.virtualAudioInstallFailed")); }
    finally { setInstallingAudio(false); }
  }
  async function refreshAudioOutputs() {
    setOutputDeviceId("");
    try {
      const result = await api.listAudioOutputs();
      if (result.ok) setAudioOutputs(result.data);
      else { setAudioOutputs([]); setMessage(errorText(result.error)); }
    } catch { setAudioOutputs([]); setMessage(t("session.controls.audioOutputsReadFailed")); }
  }
  async function refreshMeetings() {
    setMeetingPid("");
    try {
      const result = await api.listMeetingProcesses();
      if (!result.ok) { setMeetingProcesses([]); setMessage(result.error.message); return; }
      setMeetingProcesses(result.data);
      setMeetingPid(result.data.length === 1 ? String(result.data[0].pid) : "");
      if (!result.data.length) setMessage(t("session.controls.noMeetingWindows"));
    } catch { setMeetingProcesses([]); setMessage(t("session.controls.meetingDetectFailed")); }
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
  const [callSeconds, setCallSeconds] = useState(0);
  const [videoKind, setVideoKind] = useState<"off" | VideoShareKind>("off");
  const callStartRef = useRef<number | null>(null);
  const pipVideoRef = useRef<HTMLVideoElement | null>(null);
  const [revision, setRevision] = useState(0);
  const [reportSummary, setReportSummary] = useState("");
  const [reportDetail, setReportDetail] = useState("");
  const statusSeq = useRef(0);
  const [moreOpen, setMoreOpen] = useState(false);
  const moreRef = useRef<HTMLDetailsElement | null>(null);
  const [roleMenuOpen, setRoleMenuOpen] = useState(false);
  const roleMenuRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!roleMenuOpen) return;
    const closeOnOutside = (event: PointerEvent) => {
      if (roleMenuRef.current && !roleMenuRef.current.contains(event.target as Node)) setRoleMenuOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === "Escape") setRoleMenuOpen(false); };
    document.addEventListener("pointerdown", closeOnOutside);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutside);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [roleMenuOpen]);
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
      setMessage(errorText({ code: next.lastErrorCode, message: t("session.ipc.runtimeError") }));
    }
  }, []);

  const refresh = useCallback(
    async (id?: string | null, restoreLive = false) => {
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
              if (restoreLive) {
                // 页面恢复/初始加载：把最后一轮回填 live 区，重建现场。
                setTranscript(last.userText);
                setReply(last.assistantText);
              } else {
                // 会话中收尾刷新：轮已进 history，live 区保持空，
                // 上一轮字幕随 history 呈现，不再挂到下一轮开始。
                setTranscript("");
                setReply("");
              }
              // 仅转写轮（有用户文本、无回答）：不算「未使用资料」。
              setUnusedMaterials(!last.materialsUsed && !(last.userText && !last.assistantText));
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
        if (epoch === requestEpoch.current) setMessage(t("session.ipc.statusUnavailable"));
      }
    },
    [applyStatus],
  );

  useEffect(() => {
    sessionIdRef.current = sessionId;
  }, [sessionId]);

  // 事件文本是本轮的前缀快照（上限 4000 字符）而库里是全文，因此用前缀比较识别
  // “列表最后一轮就是当前显示轮”，避免刷新落地前的窗口期里同一轮出现两次。
  const lastTurn = turns.at(-1);
  const lastTurnIsLive = !!lastTurn && (transcript !== "" || reply !== "")
    && (transcript === "" || lastTurn.userText.startsWith(transcript))
    && (reply === "" || lastTurn.assistantText.startsWith(reply));
  const historyTurns = lastTurnIsLive ? turns.slice(0, -1) : turns;
  // 最后一轮「有用户文本、没有回答」：发言尚未获回答（如仅转写轮），按钮高亮引导追答。
  const assistantPending = !!lastTurn?.userText && !lastTurn?.assistantText;

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
        setMessage(t("session.ipc.statusUnavailable"));
      }
    })();
  }, [applyStatus]);

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
  // modeRef 同时被媒体推流（静音门控）与自动 finalize（接管门控）读取，保持单一引用。
  const modeRef = useRef(mode);
  useEffect(() => { modeRef.current = mode; }, [mode]);
  const { micActive, micLevel, playbackContextRef, webAudioPlayerRef } = useSessionMedia({
    active,
    inputSource,
    outputDeviceId,
    audioOutputs,
    modeRef,
    createMicStreamer,
    setMessage,
  });
  const roleName = config?.roleProfiles.find((role) => role.id === roleProfileId)?.name ?? "RoleAI";
  const showBargeHint = active && phase === "speaking" && allowBargeIn && mode === "ai_active";
  const selectedRoleScenario = roleScenario(config, roleProfileId);
  const { realtimeStatus, setRealtimeStatus, resetStreamSeq, triggerAssistant } = useSessionEvents({
    listen,
    refresh,
    applyStatus,
    setTranscript,
    setReply,
    setMessage,
    webAudioPlayerRef,
    active,
    inputSource,
    selectedRoleScenario,
  });
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
          const backoff = consecutiveAutoFailures.current >= 2 ? rateLimitHint() : "";
          setMessage(
            (humanizeRemoteError(
              error instanceof Error && error.message ? error.message : "",
            ) || t("session.controls.autoTranscribeFailed")) + (backoff ? backoff : ""),
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

  const {
    run, start, stop, setModeName, submitSay, submitChat, copyText,
    confirmCandidateAnswer, submitCorrect, submitRetry, submitReport,
  } = useSessionControls({
    finalizeUtterance,
    refresh,
    config,
    roleProfileId,
    voiceRouteId,
    allowWebSearch,
    allowBargeIn,
    inputSource,
    meetingPid,
    virtualAudio,
    outputDeviceId,
    canSearch,
    active,
    mode,
    busy,
    revision,
    sayText,
    correctText,
    chatText,
    confirmationText,
    requestEpoch,
    sessionIdRef,
    playbackContextRef,
    webAudioPlayerRef,
    setBusy,
    setMessage,
    setIssues,
    setConfigurationOpen,
    setSessionId,
    setPhase,
    setMode,
    setPendingConfirmation,
    setConfirmationText,
    setTranscript,
    setReply,
    setTurns,
    setWebSources,
    setWebDegraded,
    setUnusedMaterials,
    setSayText,
    setCorrectText,
    setChatText,
    setVideoKind,
    setReportSummary,
    setReportDetail,
    resetStreamSeq,
    setRealtimeStatus,
  });

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
          if (!disposed) setMessage(t("session.ipc.videoFrameFailed"));
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
        ? t("session.controls.cameraUnavailable")
        : t("session.controls.screenShareUnavailable"));
    });
    return () => { disposed = true; sharer.stop(); };
  }, [active, canVideo, videoKind, createVideoSharer]);

  return (
    <section className="workspace-session" aria-labelledby="workspace-session-heading">
      <SessionToolbar
        config={config}
        busy={busy}
        active={active}
        phase={phase}
        mode={mode}
        realtimeStatus={realtimeStatus}
        setAllowWebSearch={setAllowWebSearch}
        inputSource={inputSource}
        setInputSource={setInputSource}
        voiceRouteId={voiceRouteId}
        setVoiceRouteId={setVoiceRouteId}
        configurationOpen={configurationOpen}
        setConfigurationOpen={setConfigurationOpen}
        refreshMeetings={refreshMeetings}
        refreshVirtualAudio={refreshVirtualAudio}
        micActive={micActive}
        meetingPid={meetingPid}
        setMeetingPid={setMeetingPid}
        meetingProcesses={meetingProcesses}
        selectedRoleScenario={selectedRoleScenario}
        virtualAudio={virtualAudio}
        installingAudio={installingAudio}
        audioRetryBlocked={audioRetryBlocked}
        audioPreparationPhase={audioPreparationPhase}
        installVirtualAudio={installVirtualAudio}
        audioAttempted={audioAttempted}
        outputDeviceId={outputDeviceId}
        setOutputDeviceId={setOutputDeviceId}
        audioOutputs={audioOutputs}
        refreshAudioOutputs={refreshAudioOutputs}
        canSearch={canSearch}
        allowWebSearch={allowWebSearch}
        allowBargeIn={allowBargeIn}
        setAllowBargeIn={setAllowBargeIn}
        onTriggerAssistant={triggerAssistant}
        assistantPending={assistantPending}
      />
      <PreflightIssues issues={issues} />
      {webDegraded && <p className="services-message">{t("session.workspace.webDegraded")}</p>}
      {webSources.length > 0 && <aside className="services-message" aria-label={t("session.workspace.webSourcesLabel")}>
        <span>{t("session.workspace.webSourcesPrefix")}</span>
        {webSources.map((source) => <button key={source.url} title={source.url} type="button" onClick={() => void run(() => api.openWebSource(source.url))}>{source.title}</button>)}
      </aside>}
      {message && (
        <p className="services-message session-message" role="status">
          {message}
        </p>
      )}
      <TranscriptPanel
        transcript={transcript}
        reply={reply}
        turns={turns}
        historyTurns={historyTurns}
        roleName={roleName}
        welcomeRoleName={config?.roleProfiles.find((role) => role.id === roleProfileId)?.name ?? t("session.workspace.welcomeFallback")}
        active={active}
        pendingConfirmation={pendingConfirmation}
        confirmationText={confirmationText}
        onConfirmationTextChange={setConfirmationText}
        onConfirmCandidate={confirmCandidateAnswer}
        busy={busy}
        unusedMaterials={unusedMaterials}
        showTranscriptOnlyNotes={inputSource === "meeting" && selectedRoleScenario === "meetingAssistant"}
        showLatency={latencyWaterfallEnabled}
        onTriggerAssistant={triggerAssistant}
        onCopy={copyText}
        onAdjustConfiguration={() => {
          setConfigurationOpen(true);
          requestAnimationFrame(() => document.getElementById("session-configuration")?.querySelector<HTMLElement>("select, input, button")?.focus());
        }}
      />
      <div className="session-footer">
        <div className="session-footer-status">
          {showBargeHint && (
            <button
              type="button"
              className="session-hint-pill"
              onClick={() => webAudioPlayerRef.current?.clear()}
            >
              <Pause size={12} aria-hidden="true" />{t("session.workspace.bargeHint")}
            </button>
          )}
        </div>
        <div className="session-compose">
          {videoKind !== "off" && (
            <div className="session-pip" role="region" aria-label={t("session.workspace.pipRegion")}>
              <video ref={pipVideoRef} autoPlay muted playsInline />
              <span className="session-pip-label">{videoKind === "camera" ? t("session.workspace.pipCamera") : t("session.workspace.pipScreen")}</span>
              <button type="button" className="session-pip-stop" aria-label={t("session.workspace.stopShareAria")} onClick={() => setVideoKind("off")}>
                <X size={13} aria-hidden="true" />
              </button>
            </div>
          )}
          {/* 官方实时通话布局：上方文本输入，下方左侧是工具与麦克风/视频，右侧是发送与通话按钮。 */}
          <form id="session-chat-form" className="composer-input" onSubmit={submitChat}>
            <input
              aria-label={t("session.workspace.inputAria")}
              value={chatText}
              onChange={(event) => setChatText(event.target.value)}
              placeholder={active ? t("session.workspace.inputActivePlaceholder") : t("session.workspace.inputIdlePlaceholder")}
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
                <summary className="composer-icon-button" aria-label={t("session.workspace.moreActions")}>
                  <Plus size={18} aria-hidden="true" />
                </summary>
                <AgentToolsPanel
                  allowWebSearch={allowWebSearch}
                  canSearch={canSearch}
                  setAllowWebSearch={setAllowWebSearch}
                  active={active}
                  busy={busy}
                  onStop={() => void stop()}
                  onSetMode={(next) => void setModeName(next)}
                  onSaySubmit={submitSay}
                  sayText={sayText}
                  setSayText={setSayText}
                  onCorrectSubmit={() => void submitCorrect()}
                  correctText={correctText}
                  setCorrectText={setCorrectText}
                  onRetry={() => void submitRetry()}
                  onReport={() => void submitReport()}
                />
              </details>
              <div className="composer-role-chip" ref={roleMenuRef}>
                <button
                  type="button"
                  className="composer-role-trigger"
                  title={t("session.workspace.switchRoleTitle")}
                  aria-label={t("session.workspace.roleAria")}
                  aria-haspopup="listbox"
                  aria-expanded={roleMenuOpen}
                  disabled={busy || active || !config}
                  onClick={() => setRoleMenuOpen(!roleMenuOpen)}
                >
                  <span className="composer-role-dot" aria-hidden="true" />
                  <span className="composer-role-name">{roleName}</span>
                  <ChevronDown size={13} aria-hidden="true" />
                </button>
                {roleMenuOpen && config && (
                  <ul className="composer-role-menu" role="listbox" aria-label={t("session.workspace.switchRoleTitle")}>
                    {config.roleProfiles.filter((role) => role.configVersion > 0).map((role) => (
                      <li key={role.id}>
                        <button
                          type="button"
                          role="option"
                          aria-selected={role.id === roleProfileId}
                          data-selected={role.id === roleProfileId}
                          onClick={() => {
                            setRoleProfileId(role.id);
                            setAllowWebSearch(roleScenario(config, role.id) === "meetingAssistant");
                            setRoleMenuOpen(false);
                          }}
                        >
                          <span className="composer-role-option-name">{role.name}</span>
                          {role.id === roleProfileId && <Check size={14} aria-hidden="true" />}
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
              <button
                type="button"
                className="composer-mic"
                aria-label={!active ? t("session.workspace.micStart") : mode === "muted" ? t("session.workspace.micUnmute") : t("session.workspace.micMute")}
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
                    aria-label={videoKind === "camera" ? t("session.workspace.cameraOff") : t("session.workspace.cameraOn")}
                    data-active={videoKind === "camera"}
                    disabled={!active || busy}
                    onClick={() => setVideoKind(videoKind === "camera" ? "off" : "camera")}
                  >
                    <Camera size={17} aria-hidden="true" />
                  </button>
                  <button
                    type="button"
                    className="composer-icon-button composer-video"
                    aria-label={videoKind === "screen" ? t("session.workspace.screenOff") : t("session.workspace.screenOn")}
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
                aria-label={t("session.workspace.sendAria")}
                disabled={busy || !active || mode !== "ai_active" || !chatText.trim()}
              >
                <Send size={15} aria-hidden="true" />
              </button>
              {/* 独立 key：开始/结束按钮占同一位置，复用同一 DOM 节点会让开始的那次点击触发结束。 */}
              {active ? (
                <button key="call-end" type="button" className="session-call-pill" aria-label={t("session.workspace.endCall")} disabled={busy} onClick={() => void stop()}>
                  <Square size={11} aria-hidden="true" />
                  <span className="session-call-time">{formatDuration(callSeconds)}</span>
                </button>
              ) : (
                <button key="call-start" className="button-primary composer-start" disabled={busy} type="button" onClick={() => void start()}>
                  <Play size={14} aria-hidden="true" />{t("session.workspace.startSession")}
                </button>
              )}
            </div>
          </div>
        </div>
        {(reportSummary || reportDetail) && (
          <section className="session-report" aria-labelledby="session-report-heading">
            <h3 id="session-report-heading">{t("session.workspace.reportHeading")}</h3>
            {reportSummary && <p>{reportSummary}</p>}
            {reportDetail && <p className="muted">{reportDetail}</p>}
          </section>
        )}
      </div>
    </section>
  );
}
