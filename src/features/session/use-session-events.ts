import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";

import * as api from "../../api/commands";
import { audioDiagnostics, decodePcm16Base64, type WebAudioPlayer } from "./web-audio-player";
import { errorText } from "./workspace-format";
import type { RoleScenario, RuntimeStatus, SessionReplyEvent, SessionTranscriptEvent } from "../../generated/bindings";

type SessionEventListen = <T>(
  event: string,
  handler: (payload: T) => void,
) => Promise<() => void> | (() => void);

interface LocalSessionAudioEvent {
  seq: number;
  pcmBase64: string;
  sampleRate: number;
}

interface LocalSessionPlaybackControlEvent {
  seq: number;
  action: "clear";
}

export interface SessionEventsParams {
  listen: SessionEventListen;
  refresh: () => Promise<void>;
  applyStatus: (next: RuntimeStatus) => void;
  setTranscript: Dispatch<SetStateAction<string>>;
  setReply: Dispatch<SetStateAction<string>>;
  setMessage: Dispatch<SetStateAction<string>>;
  webAudioPlayerRef: { current: WebAudioPlayer | null };
  active: boolean;
  inputSource: string;
  selectedRoleScenario: RoleScenario | undefined;
}

export function useSessionEvents({
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
}: SessionEventsParams) {
  const transcriptSeq = useRef(0);
  const replySeq = useRef(0);
  const playbackSeq = useRef(0);
  const refreshRef = useRef<() => Promise<void>>(async () => {});
  const hotkeyInFlight = useRef(false);
  const [realtimeStatus, setRealtimeStatus] = useState("idle");

  const applyTranscript = useCallback((payload: SessionTranscriptEvent) => {
    if (payload.seq > transcriptSeq.current) {
      transcriptSeq.current = payload.seq;
      setTranscript(payload.text);
    }
    // done=true 表示已落库：流式快照用 2^32 起步的大序号，落库事件用小序号，
    // 按序号门控会被当旧事件丢掉，所以落库收尾不看序号、总是刷新，文本以库为准。
    if (payload.done) void refreshRef.current();
  }, [setTranscript]);

  const applyReply = useCallback((payload: SessionReplyEvent) => {
    if (payload.seq > replySeq.current) {
      replySeq.current = payload.seq;
      setReply(payload.text);
    }
    if (payload.done) void refreshRef.current();
  }, [setReply]);

  const applyPlaybackAudio = useCallback((payload: LocalSessionAudioEvent) => {
    audioDiagnostics.eventsReceived += 1;
    audioDiagnostics.bytesReceived += payload.pcmBase64.length;
    if (payload.seq <= playbackSeq.current) return;
    playbackSeq.current = payload.seq;
    const player = webAudioPlayerRef.current;
    if (!player) return;
    void player.resume().catch(() => undefined);
    player.appendPcm16(decodePcm16Base64(payload.pcmBase64));
  }, [webAudioPlayerRef]);

  const applyPlaybackControl = useCallback((payload: LocalSessionPlaybackControlEvent) => {
    if (payload.seq <= playbackSeq.current) return;
    playbackSeq.current = payload.seq;
    webAudioPlayerRef.current?.clear();
  }, [webAudioPlayerRef]);

  useEffect(() => {
    refreshRef.current = refresh;
  }, [refresh]);

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
  }, [active, inputSource, selectedRoleScenario, refresh, listen, setMessage]);

  // 新会话从零计流式字幕 seq：上一会话的 partial 用过大号 seq，
  // 不重置会把本会话开头的字幕事件整体门控丢弃。
  const resetStreamSeq = useCallback(() => {
    transcriptSeq.current = 0;
    replySeq.current = 0;
    playbackSeq.current = 0;
  }, []);

  return { realtimeStatus, setRealtimeStatus, resetStreamSeq };
}
