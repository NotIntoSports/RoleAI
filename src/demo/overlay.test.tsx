import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { DemoOverlay } from "./overlay";

describe("DemoOverlay", () => {
  afterEach(() => {
    cleanup();
    window.localStorage.clear();
    window.history.replaceState(null, "", "/");
  });

  it("shows the fictional-data banner with a desktop download link by default", () => {
    render(<DemoOverlay><div>app</div></DemoOverlay>);
    expect(screen.getByRole("note", { name: "演示模式说明" }).textContent).toContain("所有数据均为虚构");
    expect(screen.getByRole("link", { name: /下载桌面版/ }).getAttribute("href")).toContain("github.com");
  });

  it("hides banner and onboarding in capture mode (?capture=1)", () => {
    window.history.replaceState(null, "", "/?capture=1");
    render(<DemoOverlay><div>app</div></DemoOverlay>);
    expect(screen.queryByRole("note", { name: "演示模式说明" })).toBeNull();
    expect(screen.queryByRole("dialog", { name: "在线演示引导" })).toBeNull();
  });

  it("shows a skippable 3-step onboarding on first visit and remembers the dismissal", () => {
    render(<DemoOverlay><div>app</div></DemoOverlay>);
    expect(screen.getByRole("dialog", { name: "在线演示引导" }).textContent).toContain("选一个角色");
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    expect(screen.getByRole("dialog", { name: "在线演示引导" }).textContent).toContain("开始会话");
    fireEvent.click(screen.getByRole("button", { name: "跳过引导" }));
    expect(screen.queryByRole("dialog", { name: "在线演示引导" })).toBeNull();
    cleanup();
    // 二次进入不再出现引导。
    render(<DemoOverlay><div>app</div></DemoOverlay>);
    expect(screen.queryByRole("dialog", { name: "在线演示引导" })).toBeNull();
  });

  it("completes all three steps and closes via 开始体验", () => {
    render(<DemoOverlay><div>app</div></DemoOverlay>);
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    expect(screen.getByRole("dialog", { name: "在线演示引导" }).textContent).toContain("回看记录");
    fireEvent.click(screen.getByRole("button", { name: "开始体验" }));
    expect(screen.queryByRole("dialog", { name: "在线演示引导" })).toBeNull();
    expect(window.localStorage.getItem("roleai.demo.onboarded")).toBe("1");
  });

  it("persists the browser-speech toggle through the demo-voice storage", () => {
    render(<DemoOverlay><div>app</div></DemoOverlay>);
    const toggle = screen.getByRole("checkbox", { name: /朗读 AI 回答/ }) as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    fireEvent.click(toggle);
    expect(window.localStorage.getItem("roleai.demo.voice")).toBe("1");
    fireEvent.click(toggle);
    expect(window.localStorage.getItem("roleai.demo.voice")).toBeNull();
  });
});
