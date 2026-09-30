import { useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";

import * as api from "../../api/commands";
import { t } from "../../i18n";
import type { MicStreamCallbacks, MicStreamController } from "./mic-recorder";
import {
  WebAudioPlayer,
  audioDiagnostics,
  createAudioContextForOutput,
  resolveWebAudioSinkId,
} from "./web-audio-player";
import { errorText } from "./workspace-format";
import type { AudioOutputDevice } from "../../generated/bindings";

export interface SessionMediaParams {
  active: boolean;
  inputSource: string;
  outputDeviceId: string;
  audioOutputs: AudioOutputDevice[];
  modeRef: { current: string };
  createMicStreamer: (
    callbacks: MicStreamCallbacks,
    sharedContext?: AudioContext,
  ) => MicStreamController;
  setMessage: Dispatch<SetStateAction<string>>;
}

// 麦克风推流、音量条电平与网页音频播放（WebAudioPlayer/输出设备切换）的共享载体。
// webAudioPlayerRef/playbackContextRef 同时被事件订阅、会话控制与工具条清除按钮使用，
// 因此由本 hook 持有并返回，避免多处各建引用导致播放断链。
export function useSessionMedia({
  active,
  inputSource,
  outputDeviceId,
  audioOutputs,
  modeRef,
  createMicStreamer,
  setMessage,
}: SessionMediaParams) {
  const [micLevel, setMicLevel] = useState(0);
  const [micActive, setMicActive] = useState(false);
  const webAudioPlayerRef = useRef<WebAudioPlayer | null>(null);
  // 在「开始会话」点击手势内创建并 resume，避免自动播放策略让上下文一直挂起。
  const playbackContextRef = useRef<AudioContext | null>(null);
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
            setMessage(t("session.controls.outputNotFound"));
            return;
          }
          return target.setSinkId!(sinkId).then(() => {
            audioDiagnostics.sinkId = sinkId;
          });
        }).catch(() => {
          if (!disposed) setMessage(t("session.controls.outputSwitchFailed"));
        });
      }
    }
    const streamer = createMicStreamer({
      onChunk: (pcm, sampleRate) => {
        // 接管/静音期间不推流，避免人工发言被当作对练内容转写。
        if (disposed || modeRef.current !== "ai_active") return;
        void api.pushMicPcm(pcm, sampleRate).then((result) => {
          if (!disposed && !result.ok) setMessage(errorText(result.error));
        }).catch(() => { if (!disposed) setMessage(t("session.ipc.micPushFailed")); });
      },
      onError: (micError) => { if (!disposed) setMessage(micError); },
      onLevel: (level) => { if (!disposed) setMicLevel(level); },
    }, context ?? undefined);
    setMicActive(true);
    streamer.start().catch(() => {
      if (disposed) return;
      setMicActive(false);
      setMessage(t("session.controls.micUnavailable"));
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
  return { micActive, micLevel, playbackContextRef, webAudioPlayerRef };
}
