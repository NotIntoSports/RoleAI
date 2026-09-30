import { act, cleanup, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it } from "vitest";

import {
  LATENCY_WATERFALL_STORAGE_KEY,
  setLatencyWaterfallEnabled,
  useLatencyWaterfallPreference,
} from "./latency-preference";

function Probe() {
  const enabled = useLatencyWaterfallPreference();
  return <output data-enabled={enabled ? "on" : "off"}>pref</output>;
}

describe("延迟瀑布条显示偏好", () => {
  afterEach(() => {
    cleanup();
    window.localStorage.clear();
  });

  it("默认开启（未写入任何偏好）", () => {
    render(<Probe />);
    expect(screen.getByRole("status").dataset.enabled).toBe("on");
  });

  it("关闭后写入 off，重新挂载仍保持关闭", () => {
    render(<Probe />);
    act(() => setLatencyWaterfallEnabled(false));
    expect(screen.getByRole("status").dataset.enabled).toBe("off");
    expect(window.localStorage.getItem(LATENCY_WATERFALL_STORAGE_KEY)).toBe("off");
    cleanup();
    render(<Probe />);
    expect(screen.getByRole("status").dataset.enabled).toBe("off");
  });

  it("重新开启会移除 off 标记并通知订阅者", () => {
    act(() => setLatencyWaterfallEnabled(false));
    render(<Probe />);
    act(() => setLatencyWaterfallEnabled(true));
    expect(screen.getByRole("status").dataset.enabled).toBe("on");
    expect(window.localStorage.getItem(LATENCY_WATERFALL_STORAGE_KEY)).toBeNull();
  });
});
