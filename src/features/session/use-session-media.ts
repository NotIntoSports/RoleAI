import { useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";

import * as api from "../../api/commands";
import { currentLanguage, t } from "../../i18n";
import type { AudioPathEvidence } from "../../generated/bindings";
import type { MicStreamCallbacks, MicStreamController } from "./mic-recorder";
import { MIC_SIGNAL_EPSILON, chooseBoundDevice, type MicDeviceEvidence } from "./mic-signal-probe";
import {
  WebAudioPlayer,
  audioDiagnostics,
  createAudioContextForOutput,
  resolveWebAudioSinkId,
} from "./web-audio-player";
import { errorText } from "./workspace-format";
import type { AudioOutputDevice } from "../../generated/bindings";

// 看门狗判定：持续这么久没有一次有效电平（rms×4 归一化后的 0..1 值）即提示。
// 真实麦克风在安静房间里噪声底也会让 rms 远大于 0，纯静音才是异常。
const MIC_SILENCE_WATCHDOG_MS = 4000;
const MIC_SILENCE_LEVEL_EPSILON = 0.001;

/**
 * LLM 复核（异步，不阻塞会话）：把探针证据交激活线路的大模型诊断；
 * 建议改绑且目标确有信号时无缝重绑；诊断文本上屏。任何失败静默降级
 * （确定性快绑已生效，静音看门狗仍在）。
 */
async function runAudioReview(
  streamer: MicStreamController,
  evidence: MicDeviceEvidence[],
  boundDeviceId: string,
  setMessage: Dispatch<SetStateAction<string>>,
  isDisposed: () => boolean,
  onDiagnosed: () => void,
): Promise<void> {
  const payload: AudioPathEvidence = {
    inputs: evidence.map((device) => ({
      deviceId: device.deviceId,
      label: device.label,
      virtualEndpoint: device.virtual,
      rmsPeak: device.rmsPeak,
    })),
    boundDeviceId,
    osDefaultLabel: evidence.find((device) => device.deviceId === "default")?.label ?? null,
    locale: currentLanguage(),
  };
  let review: Awaited<ReturnType<typeof api.reviewAudioPath>>;
  try {
    review = await api.reviewAudioPath(payload);
  } catch {
    return;
  }
  if (isDisposed() || !review.ok) return;
  const { switchDeviceId, diagnosis, advice } = review.data;
  if (switchDeviceId && switchDeviceId !== boundDeviceId && streamer.switchDevice) {
    const target = evidence.find((device) => device.deviceId === switchDeviceId);
    if (!target || target.rmsPeak <= MIC_SIGNAL_EPSILON) return;
    try {
      await streamer.switchDevice(switchDeviceId);
    } catch {
      return;
    }
    if (isDisposed()) return;
    onDiagnosed();
    setMessage(t("session.controls.micSwitched", { name: target.label || switchDeviceId }));
    return;
  }
  if (diagnosis || advice) {
    onDiagnosed();
    setMessage([diagnosis, advice].filter(Boolean).join(" "));
  }
}

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
  const lastSignalAtRef = useRef(0);
  const silenceHintRef = useRef(false);
  const silenceWatchdogRef = useRef<number | null>(null);
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
      onLevel: (level) => {
        if (disposed) return;
        if (level > MIC_SILENCE_LEVEL_EPSILON) {
          lastSignalAtRef.current = Date.now();
          // 信号恢复后撤掉看门狗提示：消息条是全局单行，陈旧的"无信号"
          // 提示会误导用户以为还在静音。
          if (silenceHintRef.current) {
            silenceHintRef.current = false;
            setMessage("");
          }
        }
        setMicLevel(level);
      },
    }, context ?? undefined);
    setMicActive(true);
    lastSignalAtRef.current = Date.now();
    silenceHintRef.current = false;
    streamer.start().then((evidence) => {
      if (disposed) return;
      // 静音看门狗：采到纯静音（默认输入被虚拟声卡占用/麦克风静音）时界面只会
      // 停在"等待输入"，用户无从判断原因；每 4 秒无有效电平重发一次提示，
      // 有声即静默。权限弹窗等启动耗时不算在内，采集真正跑起来后才开始计时。
      silenceWatchdogRef.current = window.setInterval(() => {
        if (disposed) return;
        if (Date.now() - lastSignalAtRef.current < MIC_SILENCE_WATCHDOG_MS) return;
        lastSignalAtRef.current = Date.now();
        silenceHintRef.current = true;
        setMessage(t("session.controls.micSilenceHint"));
      }, 1000);
      // LLM 复核（每次会话）：有探针证据才发起；结论给出更优绑定或诊断后，
      // 看门狗即被撤下（它的职责已被更具体的诊断接管）。
      if (evidence && evidence.length) {
        void runAudioReview(
          streamer,
          evidence,
          chooseBoundDevice(evidence) ?? "",
          setMessage,
          () => disposed,
          () => {
            if (silenceWatchdogRef.current !== null) {
              window.clearInterval(silenceWatchdogRef.current);
              silenceWatchdogRef.current = null;
            }
          },
        );
      }
    }).catch(() => {
      if (disposed) return;
      setMicActive(false);
      setMessage(t("session.controls.micUnavailable"));
    });
    return () => {
      disposed = true;
      streamer.stop();
      player?.clear();
      if (context) void context.close().catch(() => undefined);
      if (silenceWatchdogRef.current !== null) {
        window.clearInterval(silenceWatchdogRef.current);
        silenceWatchdogRef.current = null;
      }
      webAudioPlayerRef.current = null;
      setMicActive(false);
      setMicLevel(0);
    };
    // audioOutputs 只用于按名称映射 sinkId，不应因列表刷新而重建麦克风与播放。
  }, [active, inputSource, outputDeviceId, createMicStreamer]);
  return { micActive, micLevel, playbackContextRef, webAudioPlayerRef };
}
