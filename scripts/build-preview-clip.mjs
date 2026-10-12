// 演示口型生成驱动：Node 直跑 TS 会因无扩展名导入失败，先用 Vite SSR 打包再执行。
// 入口：scripts/build-preview-clip.ts。会议助手画面走 Tavus 视频接口，不在本机跑模型。
import { build } from "vite";
import path from "node:path";
import { readdir } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.join(here, "..", ".codex-tmp", "preview-clip-gen");
process.env.ROLEAI_ROOT = path.resolve(here, "..");

await build({
  configFile: false,
  logLevel: "error",
  build: {
    ssr: path.join(here, "build-preview-clip.ts"),
    outDir,
    emptyOutDir: true,
    rollupOptions: { external: [/^node:/] },
  },
});

const entry = (await readdir(outDir)).find((name) => /^build-preview-clip\.(m?js)$/.test(name));
if (!entry) throw new Error(`preview clip generator bundle not found in ${outDir}`);
await import(pathToFileURL(path.join(outDir, entry)).href);
