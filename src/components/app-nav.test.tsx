import { cleanup, render, screen, fireEvent } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppNav } from "./app-nav";

describe("AppNav", () => {
  afterEach(cleanup);

  it("renders a nav element with aria-label", () => {
    render(<AppNav current="workspace" onNavigate={() => {}} />);
    const nav = screen.getByRole("navigation", { name: "主导航" });
    expect(nav).toBeTruthy();
  });

  it("renders 6 navigation buttons with Chinese labels", () => {
    render(<AppNav current="workspace" onNavigate={() => {}} />);
    const buttons = screen.getAllByRole("button").filter((button) => button.classList.contains("app-nav-item"));
    expect(buttons.length).toBe(6);
    expect(buttons[0].textContent).toBe("工作台");
    expect(buttons[1].textContent).toBe("虚拟直播");
    expect(buttons[2].textContent).toBe("资料");
    expect(buttons[3].textContent).toBe("记录");
    expect(buttons[4].textContent).toBe("服务");
    expect(buttons[5].textContent).toBe("设置");
  });

  it("marks the current route button with aria-current and data-active", () => {
    render(<AppNav current="records" onNavigate={() => {}} />);
    const buttons = screen.getAllByRole("button");
    const activeButton = buttons.find((b) => b.getAttribute("aria-label") === "记录");
    expect(activeButton?.getAttribute("aria-current")).toBe("page");
    expect(activeButton?.getAttribute("data-active")).toBe("true");
  });

  it("other buttons do not have aria-current", () => {
    render(<AppNav current="records" onNavigate={() => {}} />);
    const buttons = screen.getAllByRole("button");
    const inactiveButtons = buttons.filter((b) => b.textContent !== "记录");
    for (const btn of inactiveButtons) {
      expect(btn.getAttribute("aria-current")).toBeNull();
      expect(btn.getAttribute("data-active")).not.toBe("true");
    }
  });

  it("clicking an inactive button calls onNavigate with the correct route id", () => {
    const onNavigate = vi.fn();
    render(<AppNav current="workspace" onNavigate={onNavigate} />);
    const servicesButton = screen.getByRole("button", { name: "服务" });
    fireEvent.click(servicesButton);
    expect(onNavigate).toHaveBeenCalledWith("services");
    expect(onNavigate).toHaveBeenCalledTimes(1);
  });

  it("clicking the active button does NOT call onNavigate", () => {
    const onNavigate = vi.fn();
    render(<AppNav current="workspace" onNavigate={onNavigate} />);
    const workspaceButton = screen.getByRole("button", { name: "工作台" });
    fireEvent.click(workspaceButton);
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it("closes expanded navigation after selecting a destination", () => {
    const onNavigate = vi.fn();
    render(<AppNav current="workspace" onNavigate={onNavigate} />);
    const toggle = screen.getByRole("button", { name: "切换导航" });
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "资料" }));
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(onNavigate).toHaveBeenCalledWith("materials");
  });

  it("does not render any anchor tags", () => {
    const { container } = render(<AppNav current="workspace" onNavigate={() => {}} />);
    expect(container.querySelectorAll("a").length).toBe(0);
  });

  it("does not contain login/logout text or target=_blank", () => {
    const { container } = render(<AppNav current="workspace" onNavigate={() => {}} />);
    expect(container.innerHTML).not.toMatch(/登录|退出登录/);
    expect(container.innerHTML).not.toMatch(/target=["']_blank["']/);
  });

  it("buttons are keyboard accessible (tabIndex is not -1)", () => {
    render(<AppNav current="workspace" onNavigate={() => {}} />);
    const buttons = screen.getAllByRole("button");
    for (const btn of buttons) {
      expect(btn.tabIndex).not.toBe(-1);
    }
  });
});
