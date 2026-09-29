// 演示版独立 Vite 配置：入口 demo.html → src/demo/main.tsx。
// 不影响正式构建（vite.config.ts → dist-tauri-ui）。
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
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
});
