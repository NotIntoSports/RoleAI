// 一条命令生成 README 需要的全部界面截图（Playwright 库 API，不引入新依赖）。
// 用法：先 `npm run dev:demo`（脚本检测不到服务时会自动拉起并在结束时关闭），
// 然后 `npm run screenshots`。截图会隐藏演示横幅（?capture=1）。
import { spawn, execSync } from "node:child_process";
import { mkdirSync, statSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = path.join(repoRoot, ".github", "assets", "screenshots");
const BASE = process.env.DEMO_URL ?? "http://127.0.0.1:1421/RoleAI/demo/";
const VIEWPORT = { width: 1440, height: 900 };
const WARN_BYTES = 1.5 * 1024 * 1024;

let serverProcess = null;

async function waitForServer(timeoutMs = 45000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(BASE);
      if (response.ok) return;
    } catch {
      // 服务还没就绪。
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error(`演示 dev server 未在 ${timeoutMs}ms 内就绪（${BASE}）`);
}

async function ensureServer() {
  try {
    const response = await fetch(BASE);
    if (response.ok) return;
  } catch {
    // 未运行，自动拉起。
  }
  serverProcess = spawn(
    process.platform === "win32" ? "npm.cmd" : "npm",
    ["run", "dev:demo"],
    { stdio: "ignore", detached: true, cwd: repoRoot },
  );
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

function shotPath(name) {
  return path.join(OUT_DIR, name);
}

function reportSize(name) {
  const size = statSync(shotPath(name)).size;
  const kb = Math.round(size / 1024);
  console.log(`  ✓ ${name} (${kb} KB${size > WARN_BYTES ? " —— 超过 1.5MB，考虑压缩" : ""})`);
}

async function gotoApp(page, hash) {
  await page.goto(`${BASE}?capture=1${hash}`);
  await page.getByRole("navigation", { name: "主导航" }).waitFor();
  await page.waitForTimeout(400);
}

async function main() {
  await mkdir(OUT_DIR, { recursive: true });
  await ensureServer();
  const browser = await chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });

  try {
    // —— 会话进行中的截图（深色 / 浅色）——
    for (const theme of ["dark", "light"]) {
      const context = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor: 2 });
      await context.addInitScript((t) => {
        window.localStorage.setItem("ai-assistant.theme", t);
      }, theme);
      const page = await context.newPage();

      await gotoApp(page, "#/workspace");
      await page.getByRole("button", { name: "开始语音会话" }).click();
      // 面试官追问瞬间（第 2 轮回答流式输出中）。
      await page.getByText("接下来看缓存", { exact: false }).first().waitFor({ timeout: 60_000 });
      await page.waitForTimeout(theme === "dark" ? 700 : 0);
      if (theme === "dark") {
        await page.screenshot({ path: shotPath("interview-live.png") });
        reportSize("interview-live.png");
      }
      // 第 4 轮回答（脚本收尾）——头图。
      await page.getByText("主动打断并给出自己的论据", { exact: false }).first().waitFor({ timeout: 60_000 });
      await page.waitForTimeout(1200);
      const workspaceShot = theme === "dark" ? "workspace-dark.png" : "workspace-light.png";
      await page.screenshot({ path: shotPath(workspaceShot) });
      reportSize(workspaceShot);
      await page.getByRole("button", { name: "结束通话" }).click();
      await context.close();
    }

    // —— 静态页面截图（深色）——
    const context = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor: 2 });
    await context.addInitScript(() => {
      window.localStorage.setItem("ai-assistant.theme", "dark");
    });
    const page = await context.newPage();

    // 角色编辑。
    await gotoApp(page, "&category=roles#/settings");
    await page.getByText("严苛面试官").first().waitFor();
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("roles.png") });
    reportSize("roles.png");

    // 资料库（带检索结果）。
    await gotoApp(page, "#/materials");
    await page.getByText(/份资料/).waitFor();
    await page.getByPlaceholder("搜索资料内容…").fill("缓存击穿");
    await page.getByRole("button", { name: "搜索" }).click();
    await page.getByText("检索结果").waitFor();
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("materials.png") });
    reportSize("materials.png");

    // 记录列表 + 详情。
    await gotoApp(page, "#/records");
    await page.getByText(/次会话/).waitFor();
    await page.getByRole("button", { name: "查看" }).first().click();
    await page.getByText(/轮/).first().waitFor({ timeout: 15_000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("records.png") });
    reportSize("records.png");

    // 服务配置（Key 为空的脱敏态）。
    await gotoApp(page, "#/services");
    await page.getByRole("heading", { name: "演示智能云（虚构）" }).waitFor();
    await page.getByRole("button", { name: /^测试 演示智能云/ }).click();
    await page.getByText("连接测试通过").first().waitFor({ timeout: 10_000 });
    await page.screenshot({ path: shotPath("services.png") });
    reportSize("services.png");

    // 虚拟直播讲稿（生成 → 确认 → 播报中）。
    await gotoApp(page, "#/livestream");
    await page.getByText("演示讲稿").waitFor({ timeout: 15_000 });
    await page.getByText("直播配置与产品资料").click();
    // check() 自带可操作性等待：配置区可能被挂载期重渲染暂时收起。
    await page.getByLabel("云帆协同办公平台产品手册 v2.3.pdf").check({ timeout: 20000 });
    await page.getByLabel("产品标题").fill("云帆协同办公平台");
    await page.getByRole("button", { name: "生成分段讲稿" }).click();
    await page.getByRole("button", { name: "确认讲稿" }).waitFor({ timeout: 20_000 });
    await page.getByRole("button", { name: "确认讲稿" }).click();
    await page.getByRole("button", { name: "开始播报" }).click();
    try {
      await page.getByText('段落标题').first().waitFor({ timeout: 15_000 });
    } catch (e) {
      console.error('[debug]', JSON.stringify(await page.evaluate(() => ({
        cards: document.querySelectorAll('.livestream-script .preflight-card').length,
        msgs: [...document.querySelectorAll('p[role=status]')].map((p) => p.textContent),
        checkbox: [...document.querySelectorAll('.livestream-configuration input[type=checkbox]')].map((b) => b.checked),
        detailsOpen: document.querySelector('.livestream-configuration')?.open,
        title: document.querySelector('input[value*="云帆"]')?.value ?? null,
      }))));
      throw e;
    }
    await page.getByText("正在实际播放").waitFor({ timeout: 15_000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("livestream.png") });
    reportSize("livestream.png");

    // 外观设置。
    await gotoApp(page, "&category=appearance#/settings");
    await page.getByText("选择适合你的工作环境").waitFor();
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("settings-appearance.png") });
    reportSize("settings-appearance.png");

    // 延迟诊断。
    await gotoApp(page, "&category=diagnostics#/settings");
    await page.getByText("诊断延迟").waitFor();
    await page.getByText("演示实时线路（端到端）").waitFor({ timeout: 15_000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("diagnostics.png") });
    reportSize("diagnostics.png");

    await context.close();
  } finally {
    await browser.close();
    stopServer();
  }
  console.log(`\n全部截图已生成到 ${OUT_DIR}`);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
