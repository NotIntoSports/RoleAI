import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { flushSync } from "react-dom";

import * as api from "../../api/commands";
import { errorNoticeText as errorText } from "../../components/error-notice";
import { t } from "../../i18n";
import {
  joinDailyRoom,
  type DailyRoom,
  type TavusCaptionRole,
  type TavusSpeakingStatus,
} from "./daily-room";
import { attachPreviewClip, detachPreviewClip, previewCue } from "./preview-clip";

export type TavusCallStartResult = { ok: true } | { ok: false; message: string };
export type TavusCaption = { text: string; role: TavusCaptionRole };

// 演示后端返回的预览会话 id：挂断时只清本地状态，不打结束接口。
const PREVIEW_CONVERSATION_ID = "demo-preview";

type TavusCallContextValue = {
  voiceActive: boolean;
  setVoiceActive: (active: boolean) => void;
  videoActive: boolean;
  callBusy: boolean;
  callMessage: string;
  /** 最近两条字幕（新的在后），由 Tavus app-message 或预览文案填充。 */
  captions: TavusCaption[];
  speakingStatus: TavusSpeakingStatus;
  micEnabled: boolean;
  setMicEnabled: (enabled: boolean) => void;
  startCall: (roleId: string) => Promise<TavusCallStartResult>;
  hangup: (announce?: boolean) => Promise<void>;
  videoRef: RefObject<HTMLVideoElement | null>;
  audioRef: RefObject<HTMLAudioElement | null>;
};

// 无 provider 时的兜底：WorkspaceSession 单测等场景直接渲染，入口按钮点击后静默失败。
const idleContext: TavusCallContextValue = {
  voiceActive: false,
  videoActive: false,
  setVoiceActive: () => undefined,
  callBusy: false,
  callMessage: "",
  captions: [],
  speakingStatus: "listening",
  micEnabled: false,
  setMicEnabled: () => undefined,
  startCall: async () => ({ ok: false, message: "" }),
  hangup: async () => undefined,
  videoRef: { current: null },
  audioRef: { current: null },
};

const CallContext = createContext<TavusCallContextValue>(idleContext);

// 语音会话与 Tavus 视频通话互斥的锁，同时承载视频通话的完整生命周期；
// 通话舞台（TavusCall）与开始入口（对话栏）共享这里的 refs 与状态。
export function TavusCallProvider({ children }: { children: ReactNode }) {
  const [voiceActive, setVoiceActive] = useState(false);
  const [videoActive, setVideoActive] = useState(false);
  const [callBusy, setCallBusy] = useState(false);
  const [callMessage, setCallMessage] = useState("");
  const [captions, setCaptions] = useState<TavusCaption[]>([]);
  const [speakingStatus, setSpeakingStatus] = useState<TavusSpeakingStatus>("listening");
  const [micEnabled, setMicEnabledState] = useState(false);
  const videoRef = useRef<HTMLVideoElement>(null);
  const audioRef = useRef<HTMLAudioElement>(null);
  const roomRef = useRef<DailyRoom | null>(null);
  const conversationRef = useRef<string | null>(null);
  const previewCueUnlisten = useRef<(() => void) | null>(null);

  const pushCaption = useCallback((text: string, role: TavusCaptionRole) => {
    setCaptions((current) => [...current, { text, role }].slice(-2));
  }, []);

  useEffect(() => () => {
    const conversationId = conversationRef.current;
    const room = roomRef.current;
    roomRef.current = null;
    conversationRef.current = null;
    previewCueUnlisten.current?.();
    previewCueUnlisten.current = null;
    detachPreviewClip(videoRef.current);
    void room?.leave();
    if (conversationId && conversationId !== PREVIEW_CONVERSATION_ID) {
      void api.endTavusConversation(conversationId);
    }
  }, []);

  const hangup = useCallback(async (announce = true) => {
    const room = roomRef.current;
    const conversationId = conversationRef.current;
    roomRef.current = null;
    conversationRef.current = null;
    setVideoActive(false);
    setCaptions([]);
    setSpeakingStatus("listening");
    setMicEnabledState(false);
    if (conversationId === PREVIEW_CONVERSATION_ID) {
      previewCueUnlisten.current?.();
      previewCueUnlisten.current = null;
      detachPreviewClip(videoRef.current);
    }
    if (room) {
      try {
        await room.leave();
      } catch {
        setCallMessage(t("roles.ipc.operateFailed"));
      }
    }
    // 预览通话没有真实会话，只清本地状态。
    if (conversationId && conversationId !== PREVIEW_CONVERSATION_ID) {
      try {
        const result = await api.endTavusConversation(conversationId);
        if (!result.ok) setCallMessage(errorText(result.error));
        else if (announce) setCallMessage(t("tavus.call.ended"));
      } catch {
        setCallMessage(t("roles.ipc.operateFailed"));
      }
    }
  }, []);

  const setMicEnabled = useCallback((enabled: boolean) => {
    setMicEnabledState(enabled);
    roomRef.current?.setLocalAudio(enabled);
  }, []);

  const bindPreviewCues = useCallback((video: HTMLVideoElement) => {
    previewCueUnlisten.current?.();
    let lastKey = "";
    const sync = () => {
      const cue = previewCue(video.currentTime || 0);
      const key = `${cue.caption ?? ""}:${cue.status}`;
      if (key === lastKey) return;
      lastKey = key;
      setCaptions(cue.caption ? [{ text: t(cue.caption), role: cue.role }] : []);
      setSpeakingStatus(cue.status);
    };
    video.addEventListener("timeupdate", sync);
    video.addEventListener("seeked", sync);
    previewCueUnlisten.current = () => {
      video.removeEventListener("timeupdate", sync);
      video.removeEventListener("seeked", sync);
    };
    sync();
  }, []);

  const startCall = useCallback(async (roleId: string): Promise<TavusCallStartResult> => {
    if (voiceActive) {
      return { ok: false, message: t("tavus.call.voiceBusy") };
    }
    if (!roleId) {
      return { ok: false, message: t("tavus.call.noRole") };
    }
    const video = videoRef.current;
    const audio = audioRef.current;
    if (!video || !audio) {
      return { ok: false, message: t("roles.ipc.operateFailed") };
    }
    setCallBusy(true);
    const previewPage = document.documentElement.dataset.roleaiDemo === "preview";
    // 舞台先显示出来再播放。视频还带着 hidden 时浏览器不会画出画面，舞台会一直是黑的。
    // flushSync 留在这次点击里，有声播放才不会被浏览器拦住。
    if (previewPage) {
      flushSync(() => setVideoActive(true));
      attachPreviewClip(video);
    }
    try {
      const result = await api.startTavusConversation(roleId);
      if (!result.ok) {
        if (previewPage) {
          detachPreviewClip(video);
          setVideoActive(false);
        }
        return { ok: false, message: errorText(result.error) };
      }
      conversationRef.current = result.data.conversationId;
      if (result.data.conversationUrl.startsWith("preview:")) {
        // 在线演示不进 Daily 房间，循环播放同一人物的说话片段，字幕随进度切换。
        if (!previewPage) attachPreviewClip(video);
        bindPreviewCues(video);
        setMicEnabledState(true);
        setVideoActive(true);
        return { ok: true };
      }
      if (previewPage) detachPreviewClip(video);
      try {
        roomRef.current = await joinDailyRoom(result.data.conversationUrl, video, audio, {
          onUtterance: pushCaption,
          onSpeakingChange: setSpeakingStatus,
        });
      } catch (error) {
        // 麦克风打不开等原因：joinDailyRoom 已释放房间，这里结束已创建的 Tavus 会话，
        // 并把原因透传给调用方，状态保持「未接通」。
        if (previewPage) setVideoActive(false);
        const conversationId = conversationRef.current;
        conversationRef.current = null;
        if (conversationId) void api.endTavusConversation(conversationId);
        return {
          ok: false,
          message: error instanceof Error && error.message ? error.message : t("roles.ipc.operateFailed"),
        };
      }
      setCaptions([]);
      setSpeakingStatus("listening");
      setMicEnabledState(true);
      setVideoActive(true);
      setCallMessage(t("tavus.call.joined"));
      return { ok: true };
    } catch {
      if (previewPage) {
        detachPreviewClip(video);
        setVideoActive(false);
      }
      return { ok: false, message: t("roles.ipc.operateFailed") };
    } finally {
      setCallBusy(false);
    }
  }, [voiceActive, pushCaption, bindPreviewCues]);

  const value = useMemo(
    () => ({
      voiceActive,
      setVoiceActive,
      videoActive,
      callBusy,
      callMessage,
      captions,
      speakingStatus,
      micEnabled,
      setMicEnabled,
      startCall,
      hangup,
      videoRef,
      audioRef,
    }),
    [voiceActive, videoActive, callBusy, callMessage, captions, speakingStatus, micEnabled, setMicEnabled, startCall, hangup],
  );
  return <CallContext.Provider value={value}>{children}</CallContext.Provider>;
}

export function useTavusCall() {
  return useContext(CallContext);
}
