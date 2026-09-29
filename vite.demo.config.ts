// 演示版独立 Vite 配置：入口 demo.html → src/demo/main.tsx。
// 不影响正式构建（vite.config.ts → dist-tauri-ui）。
// base：GitHub Pages 下站点挂在 <repo>/demo/ 子路径；可用 DEMO_BASE 环境变量覆盖。
import { fileURLToPath } from "node:url";

import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

const demoBase = process.env.DEMO_BASE ?? "/RoleAI/demo/";

/** dev 模式把文档请求改写到 demo.html（vite dev 默认只会回 index.html）。 */
function demoHtmlEntry(): Plugin {
  return {
    name: "demo-html-entry",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use((req, _res, next) => {
        const raw = (req.url ?? "/").split("?")[0];
        const withoutBase = raw.startsWith(demoBase) ? raw.slice(demoBase.length - 1) : raw;
        if (withoutBase === "/" || withoutBase === "/index.html") {
          // vite 的 base 中间件在后面运行：这里必须保留 base 前缀。
          req.url = `${demoBase}demo.html`;
        }
        next();
      });
    },
  };
}

export default defineConfig({
  base: demoBase,
  plugins: [react(), demoHtmlEntry()],
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
