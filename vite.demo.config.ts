// 演示版独立 Vite 配置：入口 demo.html → src/demo/main.tsx。
// 不影响正式构建（vite.config.ts → dist-tauri-ui）。
// base：GitHub Pages 下站点挂在 <repo>/demo/ 子路径；可用 DEMO_BASE 环境变量覆盖。
import { fileURLToPath } from "node:url";

import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const demoBase = process.env.DEMO_BASE ?? "/RoleAI/demo/";

export default defineConfig({
  base: demoBase,
  plugins: [react()],
  clearScreen: false,
  publicDir: false,
  server: {
    host: "127.0.0.1",
    port: 1421,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/native/**", "**/.codex-tmp/**", "**/target/**"],
    },
  },
  build: {
    outDir: "dist-demo",
    emptyOutDir: true,
    target: "chrome105",
    rollupOptions: {
      input: {
        demo: fileURLToPath(new URL("./demo.html", import.meta.url)),
      },
    },
  },
});
