// 演示语音生成驱动：Node 直跑 TS 会因无扩展名导入失败，先用 Vite SSR 打包再执行。
// 入口：scripts/generate-demo-voice.ts（台词唯一来源是 src/demo/backend 的脚本数据）。
import { build } from "vite";
import path from "node:path";
import { readdir } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.join(here, "..", ".codex-tmp", "demo-voice-gen");

await build({
  configFile: false,
  logLevel: "error",
  build: {
    ssr: path.join(here, "generate-demo-voice.ts"),
    outDir,
    emptyOutDir: true,
    rollupOptions: { external: [/^node:/] },
  },
});

// 产物扩展名随 Node/package 模式变化（.js 或 .mjs），按名字前缀找入口。
const entry = (await readdir(outDir)).find((name) => /^generate-demo-voice\.(m?js)$/.test(name));
if (!entry) throw new Error(`demo voice generator bundle not found in ${outDir}`);
await import(pathToFileURL(path.join(outDir, entry)).href);
