// 在线演示入口（浏览器运行，不进桌面正式包）。
// 做法：先用官方 @tauri-apps/api/mocks 的 mockIPC 拦截 IPC 与事件总线，
// 再动态加载原桌面入口 src/main.tsx —— 业务代码零改动。
// 正式包入口是 index.html → src/main.tsx，永远不会引用本文件（见 B10 隔离测试）。
import { mockIPC } from "@tauri-apps/api/mocks";

import { handleDemoInvoke, installDemoResetHook } from "./backend";

mockIPC(handleDemoInvoke, { shouldMockEvents: true });
installDemoResetHook();

void import("../main");
