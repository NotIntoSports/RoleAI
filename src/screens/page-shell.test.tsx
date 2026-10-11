import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { PageShell } from "./page-shell";

describe("PageShell", () => {
  afterEach(cleanup);

  it("renders heading with the route label", () => {
    render(<PageShell id="workspace" />);
    expect(screen.getByRole("heading", { level: 1 }).textContent).toContain("工作台");
  });

  it("gives a short, task-oriented introduction", () => {
    render(<PageShell id="settings" />);
    expect(screen.getByText("连接模型与语音服务，管理角色、外观与本地数据。")).toBeTruthy();
  });

  it("keeps design references and capability inventories out of the page", () => {
    render(<PageShell id="materials" />);
    expect(screen.queryByRole("list")).toBeNull();
    expect(screen.queryByText(/Design §/)).toBeNull();
  });

  it("does not describe working pages as placeholders", () => {
    render(<PageShell id="records" />);
    expect(screen.queryByText(/尚未接入业务逻辑/)).toBeNull();
  });

  it("has region role with aria-labelledby", () => {
    render(<PageShell id="settings" />);
    const region = screen.getByRole("region");
    expect(region).toBeTruthy();
    expect(region.getAttribute("aria-labelledby")).toBeTruthy();
    expect(document.getElementById(region.getAttribute("aria-labelledby")!)?.textContent).toBe("设置");
  });

  it("does not render interactive elements", () => {
    const { container } = render(<PageShell id="workspace" />);
    expect(container.querySelectorAll("button").length).toBe(0);
    expect(container.querySelectorAll("input").length).toBe(0);
    expect(container.querySelectorAll("form").length).toBe(0);
    expect(container.querySelectorAll("a").length).toBe(0);
  });

  it("does not call fetch", () => {
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    render(<PageShell id="workspace" />);
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });
});
