// 在线演示入口（浏览器运行，不进桌面正式包）。
// 做法：先用官方 @tauri-apps/api/mocks 的 mockIPC 拦截 IPC 与事件总线，
// 再渲染原桌面 App，外面包一层演示横幅/引导（不改业务代码）。
// 正式包入口是 index.html → src/main.tsx，永远不会引用本文件（见 B10 隔离测试）。
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC } from "@tauri-apps/api/mocks";

import { App } from "../app/app";
import { initializeTheme } from "../features/appearance/theme";
import "../styles/foundation.css";
import "../styles/shell.css";

import { handleDemoInvoke, installDemoResetHook } from "./backend";
import { DemoOverlay } from "./overlay";

mockIPC(handleDemoInvoke, { shouldMockEvents: true });
installDemoResetHook();

const root = document.getElementById("root");
if (!root) {
  throw new Error("Demo application root is missing");
}
initializeTheme();

createRoot(root).render(
  <StrictMode>
    <DemoOverlay>
      <App />
    </DemoOverlay>
  </StrictMode>,
);
