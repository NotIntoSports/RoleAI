/** 在线演示视频通话的固定剧本。生成脚本和播放器都读这里。 */

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
  /** 同一拍的台词。会议助手的几句会拼成一次 Tavus 视频请求。参会者只出声。 */
  lines: readonly string[];
};

export const PREVIEW_BEATS: readonly PreviewBeat[] = [
  {
    id: "ready",
    speaker: "replica",
    captionKey: "tavus.call.previewCaption1",
    lines: ["我在。需要纪要的时候，点我的名字。"],
  },
  {
    id: "ask",
    speaker: "user",
    captionKey: "tavus.call.previewCaption2",
    lines: ["会议助手，把今天的会收成三句话。"],
  },
  {
    id: "summary",
    speaker: "replica",
    captionKey: "tavus.call.previewCaption3",
    lines: [
      "三句话。一，v2.3 按期上线，范围是项目空间和离线同步。",
      "二，崩溃率 0.12%，较上版下降约四成，这是演示数据。",
      "三，灰度和回滚还没定。",
    ],
  },
];

export function previewBeatSpeech(beat: PreviewBeat): string {
  return beat.lines.join("");
}
