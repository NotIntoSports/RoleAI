import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it } from "vitest";

import type { TurnLatencyView } from "../../generated/bindings";
import {
  LatencyWaterfall,
  buildWaterfallSegments,
  formatLatencyTotal,
} from "./latency-waterfall";

function cascadeLatency(overrides: Partial<TurnLatencyView["timeline"]> = {}): TurnLatencyView {
  return {
    routeId: "route-1",
    mode: "cascade",
    interrupted: false,
    timeline: {
      speechStartedMs: null,
      speechStoppedMs: null,
      transcriptDoneMs: null,
      responseCreatedMs: null,
      firstAudioMs: null,
      responseDoneMs: null,
      asrDoneMs: 100,
      retrievalDoneMs: 150,
      llmFirstTokenMs: 300,
      llmDoneMs: 500,
      ttsDoneMs: 700,
      playbackStartedMs: null,
      playbackDoneMs: null,
      ...overrides,
    },
  };
}

describe("buildWaterfallSegments", () => {
  it("级联时间线按相邻锚点差值切段", () => {
    const segments = buildWaterfallSegments(cascadeLatency());
    expect(segments.map((segment) => segment.label)).toEqual([
      "资料检索",
      "首词生成",
      "回答生成",
      "语音合成",
    ]);
    expect(segments.map((segment) => segment.ms)).toEqual([50, 150, 200, 200]);
  });

  it("实时时间线只统计有值的锚点", () => {
    const segments = buildWaterfallSegments({
      routeId: "route-2",
      mode: "realtime",
      interrupted: true,
      timeline: {
        speechStartedMs: 12_000,
        speechStoppedMs: 13_000,
        transcriptDoneMs: null,
        responseCreatedMs: 13_050,
        firstAudioMs: 13_450,
        responseDoneMs: 14_200,
        asrDoneMs: null,
        retrievalDoneMs: null,
        llmFirstTokenMs: null,
        llmDoneMs: null,
        ttsDoneMs: null,
        playbackStartedMs: null,
        playbackDoneMs: null,
      },
    });
    // speechStarted→speechStopped（1000）为句中停顿前的说话时段，同样计入；
    // transcriptDone 缺失，speechStopped→responseCreated 直连。
    expect(segments.map((segment) => segment.ms)).toEqual([1000, 50, 400, 750]);
  });

  it("时间线全空返回空段列表", () => {
    expect(buildWaterfallSegments(cascadeLatency({
      asrDoneMs: null,
      retrievalDoneMs: null,
      llmFirstTokenMs: null,
      llmDoneMs: null,
      ttsDoneMs: null,
    }))).toEqual([]);
  });
});

describe("formatLatencyTotal", () => {
  it("毫秒与秒的显示切换", () => {
    expect(formatLatencyTotal(950)).toBe("950 毫秒");
    expect(formatLatencyTotal(1_200)).toBe("1.2 秒");
  });
});

describe("LatencyWaterfall 组件", () => {
  afterEach(cleanup);

  it("默认只渲染堆叠条与总耗时，点击展开明细表", () => {
    const view = render(<LatencyWaterfall latency={cascadeLatency()} turnIndex={2} />);
    expect(screen.queryByRole("table")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /第 3 轮延迟 600 毫秒/ }));
    expect(screen.getByRole("table")).toBeTruthy();
    expect(screen.getByRole("cell", { name: "语音合成" })).toBeTruthy();
    expect(screen.getAllByRole("cell", { name: "200 毫秒" }).length).toBe(2);
    expect(view.container.querySelector("caption")).toBeTruthy();
  });

  it("被打断的轮次展示打断标注", () => {
    render(
      <LatencyWaterfall
        latency={{ ...cascadeLatency(), interrupted: true }}
        turnIndex={0}
      />,
    );
    expect(screen.getByText("被打断")).toBeTruthy();
  });

  it("无可显示段时不渲染任何内容", () => {
    const view = render(
      <LatencyWaterfall
        latency={cascadeLatency({
          asrDoneMs: null,
          retrievalDoneMs: null,
          llmFirstTokenMs: null,
          llmDoneMs: null,
          ttsDoneMs: null,
        })}
        turnIndex={0}
      />,
    );
    expect(view.container.querySelector(".latency-waterfall")).toBeNull();
  });
});
