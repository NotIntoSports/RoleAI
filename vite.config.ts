import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  publicDir: false,
  envPrefix: ["TAURI_ENV_"],
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    watch: {
      // Rust 编译时会锁定 PDB 文件，由 Tauri 负责监听后台目录；
      // AudioBridge 的 .NET 构建输出（obj/bin）会被写入方锁定，watch 到即 EBUSY 崩溃。
      // 其他工具（如 Codex）在 .codex-tmp 等目录并行 cargo 构建时同样会锁定产物。
      ignored: ["**/src-tauri/**", "**/native/**", "**/.codex-tmp/**", "**/target/**"],
    },
  },
  build: {
    outDir: "dist-tauri-ui",
    emptyOutDir: true,
    target: "chrome105",
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    passWithNoTests: true,
    // 覆盖率（lane-I I05）：排除纯样式与 ts-rs 生成代码，HTML 报告供 CI artifact。
    coverage: {
      provider: "v8",
      include: ["src/**"],
      exclude: ["src/**/*.css", "src/generated/**", "src/**/*.test.*"],
      reporter: ["text", "html"],
      reportsDirectory: "coverage/ui",
    },
  },
});
