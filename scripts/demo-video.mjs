// 录制在线演示视频（Playwright recordVideo，webm）：
// 选角色 → 开始会话 → 两轮对话 → 打断 → 结束 → 看记录。
// 本机有 ffmpeg 时会打印转 mp4/gif 的命令；没有则仅产出 webm（零新增 npm 依赖）。
import { spawn, execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { readdir, rename, rm, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = path.join(repoRoot, ".github", "assets", "demo");
const BASE = process.env.DEMO_URL ?? "http://127.0.0.1:1421/RoleAI/demo/";

let serverProcess = null;

async function waitForServer(timeoutMs = 45000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if ((await fetch(BASE)).ok) return;
    } catch {
      // 服务还没就绪。
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`演示 dev server 未就绪（${BASE}）`);
}

async function ensureServer() {
  try {
    if ((await fetch(BASE)).ok) return;
  } catch {
    // 未运行，自动拉起。
  }
  // Node ≥ 20.12 refuses to spawn npm.cmd without a shell on Windows (EINVAL); run vite directly.
  const viteCli = path.join(repoRoot, "node_modules", "vite", "bin", "vite.js");
  serverProcess = spawn(process.execPath, [viteCli, "--config", "vite.demo.config.ts"], {
    stdio: "ignore",
    detached: true,
    cwd: repoRoot,
  });
  serverProcess.unref();
  await waitForServer();
}

function stopServer() {
  if (!serverProcess) return;
  try {
    if (process.platform === "win32") {
      execSync(`taskkill /pid ${serverProcess.pid} /T /F`, { stdio: "ignore" });
    } else {
      process.kill(-serverProcess.pid);
    }
  } catch {
    // 进程可能已退出。
  }
}

async function latestWebm(dir) {
  const entries = await readdir(dir);
  const videos = [];
  for (const entry of entries) {
    if (!entry.endsWith(".webm")) continue;
    const full = path.join(dir, entry);
    videos.push({ full, mtime: (await stat(full)).mtimeMs });
  }
  videos.sort((a, b) => b.mtime - a.mtime);
  return videos;
}

async function main() {
  mkdirSync(OUT_DIR, { recursive: true });
  await ensureServer();
  const browser = await chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    recordVideo: { dir: OUT_DIR, size: { width: 1440, height: 900 } },
  });
  const page = await context.newPage();

  // 1. 打开演示（保留横幅，展示真实入口）。
  await page.goto(BASE);
  await page.getByRole("navigation", { name: "主导航" }).waitFor();
  // 2. 首次引导快速走完（展示给观众）。
  await page.getByRole("button", { name: "下一步" }).click();
  await page.getByRole("button", { name: "下一步" }).click();
  await page.getByRole("button", { name: "开始体验" }).click();
  await page.waitForTimeout(800);
  // 3. 选角色：打开角色菜单选择严苛面试官。
  await page.getByRole("button", { name: "角色", exact: true }).click();
  await page.getByRole("option", { name: "严苛面试官" }).click();
  await page.waitForTimeout(600);
  // 4. 开始会话，播两轮对话（第 3 轮自动演示打断）。
  await page.getByRole("button", { name: "开始语音会话" }).click();
  await page.getByText("已读回执你们是怎么存储的", { exact: false }).first().waitFor({ timeout: 60_000 });
  // 5. 打断演示（第 3 轮回答被截断后用户追问）。
  await page.getByText("不好意思打断一下", { exact: false }).first().waitFor({ timeout: 60_000 });
  await page.waitForTimeout(1500);
  // 6. 结束会话。
  await page.getByRole("button", { name: "结束通话" }).click();
  await page.getByRole("button", { name: "开始语音会话" }).waitFor();
  await page.waitForTimeout(600);
  // 7. 看记录：打开记录页并查看详情。
  await page.getByRole("button", { name: "记录" }).click();
  await page.getByText(/次会话/).waitFor();
  await page.getByRole("button", { name: "查看" }).first().click();
  await page.getByText(/轮/).first().waitFor();
  await page.waitForTimeout(1500);

  await context.close();
  await browser.close();
  stopServer();

  const videos = await latestWebm(OUT_DIR);
  if (!videos.length) throw new Error("录制结束但没有找到 webm 文件");
  const target = path.join(OUT_DIR, "demo.webm");
  await rename(videos[0].full, target);
  for (const extra of videos.slice(1)) await rm(extra.full);
  const sizeMB = Math.round(((await stat(target)).size / 1024 / 1024) * 10) / 10;
  console.log(`✓ 演示视频已生成：${target}（${sizeMB} MB）`);
  const hasSystemFfmpeg = await new Promise((resolve) => {
    const check = spawn("ffmpeg", ["-version"], { stdio: "ignore", shell: true });
    check.on("error", () => resolve(false));
    check.on("exit", (code) => resolve(code === 0));
  });
  if (hasSystemFfmpeg) {
    console.log("检测到系统 ffmpeg，可手动执行：");
    console.log(`  ffmpeg -y -i "${target}" -c:v libx264 -pix_fmt yuv420p -movflags +faststart -an "${path.join(OUT_DIR, "demo.mp4")}"`);
    console.log(`  ffmpeg -y -i "${target}" -vf scale=960:-1 -an "${path.join(OUT_DIR, "demo.gif")}"`);
  } else {
    console.log("未检测到系统 ffmpeg：仅产出 webm。安装 ffmpeg 后重跑 `npm run demo:video` 可转 mp4/gif，");
    console.log("或把 webm 拖进任意 GitHub Issue 编辑框获取视频链接后贴到 README。");
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
