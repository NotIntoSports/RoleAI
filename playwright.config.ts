// 演示版 Playwright 配置：webServer 起演示 dev server，只测浏览器演示版。
// 真实桌面 exe 的端到端测试归 I 线（tauri-driver），不在这里。
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "e2e/demo",
  timeout: 120_000,
  expect: { timeout: 15_000 },
  fullyParallel: false,
  workers: 1,
  reporter: [["list"]],
  use: {
    baseURL: "http://127.0.0.1:1421",
    viewport: { width: 1440, height: 900 },
    // 失败时保留截图与 trace 到 test-results/（已 gitignore），
    // 供 CI 的 e2e-demo job 作为 artifact 上传（I01）。
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
    launchOptions: {
      // 演示默认输入源是本机麦克风：无头模式用假设备自动授权，避免权限弹窗卡住。
      args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
    },
  },
  webServer: {
    command: "npm run dev:demo",
    url: "http://127.0.0.1:1421/RoleAI/demo/",
    reuseExistingServer: true,
    timeout: 60_000,
  },
});
