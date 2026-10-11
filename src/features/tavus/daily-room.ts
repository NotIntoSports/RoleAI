import Daily, {
  type DailyCall,
  type DailyEventObjectAppMessage,
  type DailyEventObjectTrack,
} from "@daily-co/daily-js";

import { t } from "../../i18n";

export type DailyRoom = {
  leave: () => Promise<void>;
  setLocalAudio: (enabled: boolean) => void;
};

export type TavusCaptionRole = "replica" | "user";
export type TavusSpeakingStatus = "listening" | "replica-speaking" | "user-speaking";

export type DailyRoomEventHandlers = {
  /** Tavus conversation.utterance 的单句发言（说话方随 role 给出）。 */
  onUtterance?: (text: string, role: TavusCaptionRole) => void;
  /** 对方/用户开口、闭口带来的说话状态变化。 */
  onSpeakingChange?: (status: TavusSpeakingStatus) => void;
};

// Tavus 事件在 Daily app-message 里的字段位置有多个版本：事件名可能在顶层
// event_type，也可能整个事件包在 event 里（数据在 event.data）；文本与说话方
// 可能在顶层、properties 或 event.data 上。按候选位置依次取值。
function tavusField(message: Record<string, unknown>, key: string): unknown {
  const nestedEvent = message.event;
  const candidates: unknown[] = [
    message,
    nestedEvent,
    message.data,
    nestedEvent && typeof nestedEvent === "object"
      ? (nestedEvent as Record<string, unknown>).data
      : undefined,
  ];
  for (const candidate of candidates) {
    if (!candidate || typeof candidate !== "object") continue;
    const holder = candidate as Record<string, unknown>;
    if (holder[key] !== undefined) return holder[key];
    const properties = holder.properties;
    if (
      properties
      && typeof properties === "object"
      && (properties as Record<string, unknown>)[key] !== undefined
    ) {
      return (properties as Record<string, unknown>)[key];
    }
  }
  return undefined;
}

function tavusEventType(message: Record<string, unknown>): string | null {
  if (typeof message.event_type === "string") return message.event_type;
  const nestedEvent = message.event;
  if (nestedEvent && typeof nestedEvent === "object") {
    const nestedType = (nestedEvent as Record<string, unknown>).event_type;
    if (typeof nestedType === "string") return nestedType;
  }
  return null;
}

export async function joinDailyRoom(
  url: string,
  video: HTMLVideoElement,
  audio: HTMLAudioElement,
  handlers: DailyRoomEventHandlers = {},
): Promise<DailyRoom> {
  // CSP 不放开 'unsafe-eval'：通话引擎只能用 <script> 从 *.daily.co 加载（tauri.conf.json 的 script-src）。
  const call: DailyCall = Daily.createCallObject({ dailyConfig: { avoidEval: true } });
  const boundVideoTrack = { current: null as MediaStreamTrack | null };
  const boundAudioTrack = { current: null as MediaStreamTrack | null };

  const bindTrack = (element: HTMLVideoElement | HTMLAudioElement, track: MediaStreamTrack) => {
    element.srcObject = new MediaStream([track]);
    void element.play().catch(() => undefined);
  };

  // 只消费远端轨道：视频只绑 camera 人像轨（屏幕共享 screenVideo 不上舞台），
  // 音频只绑人声轨；同类轨道后到直接替换旧的。
  call.on("track-started", (event: DailyEventObjectTrack) => {
    if (!event.participant || event.participant.local) return;
    if (event.type === "video") {
      boundVideoTrack.current = event.track;
      bindTrack(video, event.track);
    } else if (event.type === "audio") {
      boundAudioTrack.current = event.track;
      bindTrack(audio, event.track);
    }
  });
  call.on("track-stopped", (event: DailyEventObjectTrack) => {
    if (event.type === "video" && boundVideoTrack.current === event.track) {
      boundVideoTrack.current = null;
      video.srcObject = null;
    } else if (event.type === "audio" && boundAudioTrack.current === event.track) {
      boundAudioTrack.current = null;
      audio.srcObject = null;
    }
  });

  // Tavus 的字幕与说话状态通过房间 app-message 下发，不接入现有语音泵。
  call.on("app-message", (event: DailyEventObjectAppMessage) => {
    let payload: unknown = event.data;
    if (typeof payload === "string") {
      try {
        payload = JSON.parse(payload);
      } catch {
        return;
      }
    }
    if (!payload || typeof payload !== "object") return;
    const message = payload as Record<string, unknown>;
    switch (tavusEventType(message)) {
      case "conversation.utterance": {
        const text = tavusField(message, "speech");
        const role = tavusField(message, "role");
        if (typeof text === "string" && text && (role === "replica" || role === "user")) {
          handlers.onUtterance?.(text, role);
        }
        return;
      }
      case "conversation.replica.started_speaking":
        handlers.onSpeakingChange?.("replica-speaking");
        return;
      case "conversation.replica.stopped_speaking":
      case "conversation.user.stopped_speaking":
        handlers.onSpeakingChange?.("listening");
        return;
      case "conversation.user.started_speaking":
        handlers.onSpeakingChange?.("user-speaking");
        return;
      default:
        return;
    }
  });

  await call.join({
    url,
    subscribeToTracksAutomatically: true,
    startVideoOff: true,
    startAudioOff: false,
  });

  // 麦克风必须真正发布，否则对方听不见；打不开时释放房间，把原因抛给调用方，
  // 调用方据此保持「未接通」状态。
  try {
    call.setLocalAudio(true);
    if (!call.localAudio()) throw new Error("microphone unavailable");
  } catch {
    try {
      await call.leave();
    } finally {
      await call.destroy();
    }
    throw new Error(t("tavus.call.micFailed"));
  }

  return {
    async leave() {
      try {
        await call.leave();
      } finally {
        await call.destroy();
      }
    },
    setLocalAudio(enabled: boolean) {
      call.setLocalAudio(enabled);
    },
  };
}
