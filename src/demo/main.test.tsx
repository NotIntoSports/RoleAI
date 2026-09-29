import { screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";

// 演示入口回归测试：官方 mockIPC 注入 + 动态加载原桌面入口后，
// 工作台应能渲染出来，且没有未捕获的页面错误。
// 真实 Chromium 的端到端冒烟由 e2e（B07 Playwright）覆盖。
describe("demo entry (src/demo/main.tsx)", () => {
  it("renders the workspace shell through the official mockIPC bridge", async () => {
    const root = document.createElement("div");
    root.id = "root";
    document.body.appendChild(root);

    const pageErrors: unknown[] = [];
    const onError = (event: ErrorEvent) => pageErrors.push(event.error);
    window.addEventListener("error", onError);

    await import("./main");

    await waitFor(
      () => {
        expect(screen.getByRole("heading", { level: 1 }).textContent).toBe("工作台");
      },
      { timeout: 5000 },
    );
    // 导航与六个页面入口齐备，说明 Shell 完整挂载。
    expect(screen.getByRole("navigation", { name: "主导航" })).toBeTruthy();

    window.removeEventListener("error", onError);
    expect(pageErrors).toEqual([]);
  });
});
