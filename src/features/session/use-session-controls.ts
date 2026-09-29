import type { Dispatch, FormEvent, SetStateAction } from "react";

import * as api from "../../api/commands";
import { createAudioContextForOutput, type WebAudioPlayer } from "./web-audio-player";
import type { VideoShareKind } from "./video-sharer";
import { errorText, humanizeRemoteError } from "./workspace-format";
import type {
  AgentCommandInput,
  CommandResult,
  PreflightIssue,
  PublicConfig,
  SessionTurnView,
  VirtualAudioPreparation,
  WebSource,
} from "../../generated/bindings";

export interface SessionControlsParams {
  finalizeUtterance: (text: string) => Promise<void>;
  refresh: (id?: string | null) => Promise<void>;
  config: PublicConfig | null;
  roleProfileId: string;
  voiceRouteId: string;
  allowWebSearch: boolean;
  allowBargeIn: boolean;
  inputSource: string;
  meetingPid: string;
  virtualAudio: VirtualAudioPreparation | null;
  outputDeviceId: string;
  canSearch: boolean;
  active: boolean;
  mode: string;
  busy: boolean;
  revision: number;
  sayText: string;
  correctText: string;
  chatText: string;
  confirmationText: string;
  requestEpoch: { current: number };
  sessionIdRef: { current: string | null };
  playbackContextRef: { current: AudioContext | null };
  webAudioPlayerRef: { current: WebAudioPlayer | null };
  setBusy: Dispatch<SetStateAction<boolean>>;
  setMessage: Dispatch<SetStateAction<string>>;
  setIssues: Dispatch<SetStateAction<PreflightIssue[]>>;
  setConfigurationOpen: Dispatch<SetStateAction<boolean>>;
  setSessionId: Dispatch<SetStateAction<string | null>>;
  setPhase: Dispatch<SetStateAction<string>>;
  setMode: Dispatch<SetStateAction<string>>;
  setPendingConfirmation: Dispatch<SetStateAction<boolean>>;
  setConfirmationText: Dispatch<SetStateAction<string>>;
  setTranscript: Dispatch<SetStateAction<string>>;
  setReply: Dispatch<SetStateAction<string>>;
  setTurns: Dispatch<SetStateAction<SessionTurnView[]>>;
  setWebSources: Dispatch<SetStateAction<WebSource[]>>;
  setWebDegraded: Dispatch<SetStateAction<boolean>>;
  setUnusedMaterials: Dispatch<SetStateAction<boolean>>;
  setSayText: Dispatch<SetStateAction<string>>;
  setCorrectText: Dispatch<SetStateAction<string>>;
  setChatText: Dispatch<SetStateAction<string>>;
  setVideoKind: Dispatch<SetStateAction<"off" | VideoShareKind>>;
  setReportSummary: Dispatch<SetStateAction<string>>;
  setReportDetail: Dispatch<SetStateAction<string>>;
  resetStreamSeq: () => void;
  setRealtimeStatus: Dispatch<SetStateAction<string>>;
}

export function useSessionControls({
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
}: SessionControlsParams) {
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
      resetStreamSeq();
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

  return {
    run,
    start,
    stop,
    setModeName,
    submitSay,
    submitChat,
    copyText,
    confirmCandidateAnswer,
    submitCorrect,
    submitRetry,
    submitReport,
  };
}
