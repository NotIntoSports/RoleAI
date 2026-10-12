/** 在线演示视频通话的固定剧本。台词正文在文案表里，这里只排拍次。 */

import { zhCN } from "../../locales/zh-CN";

export const PREVIEW_USER_VOICE = "zh-CN-YunxiNeural";

export type PreviewSpeaker = "replica" | "user";
export type PreviewBeatId = "ready" | "ask" | "summary";
export type PreviewCaptionKey =
  | "tavus.call.previewCaption1"
  | "tavus.call.previewCaption2"
  | "tavus.call.previewCaption3";

export type PreviewBeat = {
  id: PreviewBeatId;
  speaker: PreviewSpeaker;
  captionKey: PreviewCaptionKey;
};

export const PREVIEW_BEATS: readonly PreviewBeat[] = [
  { id: "ready", speaker: "replica", captionKey: "tavus.call.previewCaption1" },
  { id: "ask", speaker: "user", captionKey: "tavus.call.previewCaption2" },
  { id: "summary", speaker: "replica", captionKey: "tavus.call.previewCaption3" },
];

export function previewBeatSpeech(beat: PreviewBeat): string {
  if (beat.captionKey === "tavus.call.previewCaption1") return zhCN.tavus.call.previewCaption1;
  if (beat.captionKey === "tavus.call.previewCaption2") return zhCN.tavus.call.previewCaption2;
  return zhCN.tavus.call.previewCaption3;
}
