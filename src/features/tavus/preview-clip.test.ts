import { describe, expect, it } from "vitest";

import { zhCN } from "../../locales/zh-CN";
import {
  PREVIEW_ASK_AT,
  PREVIEW_DURATION,
  PREVIEW_LISTEN_AT,
  PREVIEW_READY_AT,
  PREVIEW_SPEAKING_FRAME_AT,
  PREVIEW_SUMMARY_AT,
  previewCue,
} from "./preview-clip";
import { PREVIEW_BEATS, previewBeatSpeech } from "./preview-script";

describe("previewCue", () => {
  it("follows the meeting beats", () => {
    expect(previewCue(0)).toEqual({ caption: null, role: "replica", status: "listening" });
    expect(previewCue(PREVIEW_READY_AT).caption).toBe("tavus.call.previewCaption1");
    expect(previewCue(PREVIEW_READY_AT).status).toBe("replica-speaking");
    expect(previewCue(PREVIEW_ASK_AT)).toMatchObject({
      caption: "tavus.call.previewCaption2",
      role: "user",
      status: "user-speaking",
    });
    expect(previewCue(PREVIEW_SUMMARY_AT).caption).toBe("tavus.call.previewCaption3");
    expect(previewCue(PREVIEW_SUMMARY_AT).status).toBe("replica-speaking");
    expect(previewCue(PREVIEW_LISTEN_AT).caption).toBeNull();
    expect(previewCue(PREVIEW_LISTEN_AT).status).toBe("listening");
    expect(previewCue(Number.NaN).status).toBe("listening");
  });

  it("stays within the 35 second loop and parks the screenshot on her first line", () => {
    expect(PREVIEW_READY_AT).toBeLessThan(PREVIEW_ASK_AT);
    expect(PREVIEW_ASK_AT).toBeLessThan(PREVIEW_SUMMARY_AT);
    expect(PREVIEW_SUMMARY_AT).toBeLessThan(PREVIEW_LISTEN_AT);
    expect(PREVIEW_LISTEN_AT).toBeLessThan(PREVIEW_DURATION);
    expect(PREVIEW_DURATION).toBeLessThanOrEqual(35);
    expect(PREVIEW_SPEAKING_FRAME_AT).toBeGreaterThanOrEqual(PREVIEW_READY_AT);
    expect(PREVIEW_SPEAKING_FRAME_AT).toBeLessThan(PREVIEW_ASK_AT);
  });

  it("speaks the chinese captions for each beat", () => {
    const call = zhCN.tavus.call;
    expect(PREVIEW_BEATS.map((beat) => beat.captionKey)).toEqual([
      "tavus.call.previewCaption1",
      "tavus.call.previewCaption2",
      "tavus.call.previewCaption3",
    ]);
    expect(previewBeatSpeech(PREVIEW_BEATS[0]!)).toBe(call.previewCaption1);
    expect(previewBeatSpeech(PREVIEW_BEATS[1]!)).toBe(call.previewCaption2);
    expect(previewBeatSpeech(PREVIEW_BEATS[2]!)).toBe(call.previewCaption3);
  });
});
