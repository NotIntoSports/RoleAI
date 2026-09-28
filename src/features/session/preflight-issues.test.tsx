import { cleanup, render, screen } from "@testing-library/react";
import "@testing-library/jest-dom/vitest";
import { afterEach, describe, expect, it } from "vitest";

import type { PreflightIssue } from "../../generated/bindings";
import { PreflightIssues, describeIssue } from "./preflight-issues";

function issue(code: string): PreflightIssue {
  return { code, area: "test", action: "open_settings" };
}

describe("describeIssue", () => {
  it("maps every known code to actionable copy", () => {
    expect(describeIssue(issue("SESSION_ROLE_REQUIRED")).text).toBe("请选择一个会话角色。");
    expect(describeIssue(issue("SESSION_ROUTE_REQUIRED")).text).toBe("请先配置并启用语音线路。");
    expect(describeIssue(issue("SESSION_STAGE_INCOMPLETE")).text).toBe("语音线路的模型配置不完整。");
    expect(describeIssue(issue("SESSION_CREDENTIAL_MISSING")).text).toBe("模型服务缺少有效的 API Key。");
    expect(describeIssue(issue("SESSION_DATABASE_UNAVAILABLE")).text).toBe(
      "本地资料库暂时不可用，请修复配置或重启应用。",
    );
  });

  it("falls back to generic copy for unknown codes", () => {
    expect(describeIssue(issue("SOMETHING_NEW")).text).toBe("会话启动遇到问题，请检查相关配置后重试。");
  });
});

describe("PreflightIssues", () => {
  afterEach(cleanup);

  it("renders nothing for an empty list", () => {
    const { container } = render(<PreflightIssues issues={[]} />);
    expect(container.innerHTML).toBe("");
  });

  it("renders each issue with its guided link", () => {
    render(<PreflightIssues issues={[issue("SESSION_ROLE_REQUIRED"), issue("SESSION_ROUTE_REQUIRED")]} />);
    const region = screen.getByRole("status", { name: "会话启动检查" });
    expect(region.textContent).toContain("请选择一个会话角色。");
    expect(region.textContent).toContain("请先配置并启用语音线路。");
    expect(screen.getByRole("link", { name: "选择或创建角色" }).getAttribute("href")).toBe(
      "/settings?category=roles",
    );
    expect(screen.getByRole("link", { name: "配置语音线路" }).getAttribute("href")).toBe(
      "/services?category=routes",
    );
  });

  it("renders the generic guidance for an unknown code", () => {
    render(<PreflightIssues issues={[issue("SOMETHING_NEW")]} />);
    expect(screen.getByRole("status", { name: "会话启动检查" }).textContent).toContain(
      "会话启动遇到问题，请检查相关配置后重试。",
    );
    expect(screen.getByRole("link", { name: "打开设置" }).getAttribute("href")).toBe("/settings");
  });
});
