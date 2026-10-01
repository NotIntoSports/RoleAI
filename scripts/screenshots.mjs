// 一条命令生成 README 需要的全部界面截图（Playwright 库 API，不引入新依赖）。
// 用法：先 `npm run dev:demo`（脚本检测不到服务时会自动拉起并在结束时关闭），
// 然后 `npm run screenshots`。截图会隐藏演示横幅（?capture=1）。
// `node scripts/screenshots.mjs --lang en`（lane-H H12 / B21）：界面切到 English，
// 截图输出到 .github/assets/screenshots/en/；默认（无参数）中文行为保持不变。
import { spawn, execSync } from "node:child_process";
import { mkdirSync, statSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = path.join(repoRoot, ".github", "assets", "screenshots");
const WEB_OUT_DIR = path.join(repoRoot, "website", "assets", "screenshots");
const BASE = process.env.DEMO_URL ?? "http://127.0.0.1:1421/RoleAI/demo/";
const VIEWPORT = { width: 1440, height: 900 };
const WARN_BYTES = 1.5 * 1024 * 1024;

// —— 界面语言（B21）：选择器随语言切换；键名与下方用法一一对应。 ——
const langIndex = process.argv.indexOf("--lang");
const LANG = langIndex !== -1 ? (process.argv[langIndex + 1] ?? "zh-CN") : "zh-CN";
if (LANG !== "zh-CN" && LANG !== "en") {
  console.error(`不支持的语言：${LANG}（可选 zh-CN / en）`);
  process.exitCode = 1;
  process.exit(1);
}
const COPY = {
  "zh-CN": {
    nav: "主导航",
    micStart: "开始语音会话",
    endCall: "结束通话",
    turn2Reply: "接下来看缓存",
    turn4ReplyEnd: "我按你的案例继续追问",
    latencyBar: /轮延迟/,
    latencyDetails: "分阶段耗时",
    strictInterviewer: "严苛面试官",
    materialsCount: /份资料/,
    searchPlaceholder: "搜索资料内容…",
    searchAction: "搜索",
    searchResults: "检索结果",
    sessionsCount: /次会话/,
    view: "查看",
    turnLabel: /轮/,
    providerName: "演示智能云（虚构）",
    testProvider: /^测试 演示智能云/,
    testPassed: "连接测试通过",
    positionLabel: "岗位方向",
    demoScript: "演示讲稿",
    liveConfig: "直播配置与产品资料",
    productTitle: "产品标题",
    generateScript: "生成分段讲稿",
    confirmScript: "确认讲稿",
    startSpeaking: "开始播报",
    segmentTitle: "段落标题",
    playingLive: "正在实际播放",
    appearanceDescription: "选择适合你的工作环境",
    performancePanel: "性能面板",
    demoRouteName: "演示实时线路（端到端）",
    stageLatencies: "分阶段延迟",
  },
  en: {
    nav: "Primary navigation",
    micStart: "Start voice session",
    endCall: "End call",
    turn2Reply: "Now caching",
    turn4ReplyEnd: "keep pressing on your case",
    latencyBar: /latency/i,
    latencyDetails: "stage durations",
    strictInterviewer: "Strict interviewer",
    materialsCount: /\d+ materials/,
    searchPlaceholder: "Search material content…",
    searchAction: "Search",
    searchResults: "Search results",
    sessionsCount: /sessions/,
    view: "View",
    turnLabel: /Turn/,
    providerName: "Demo Cloud (fictional)",
    testProvider: /^Test Demo Cloud/,
    testPassed: "Connection test passed",
    positionLabel: "Target role",
    demoScript: "Demo script",
    liveConfig: "Live setup and product materials",
    productTitle: "Product title",
    generateScript: "Generate segmented script",
    confirmScript: "Confirm script",
    startSpeaking: "Start speaking",
    segmentTitle: "Segment title",
    playingLive: "Playing live",
    appearanceDescription: "Pick the work environment",
    performancePanel: "Performance panel",
    demoRouteName: "Demo realtime route (end-to-end)",
    stageLatencies: "Stage latencies",
  },
};
const copy = COPY[LANG];
// 英文种子与英文界面都依赖 i18n 偏好：在页面脚本运行前写入 localStorage。
const LANGUAGE_INIT = LANG === "en" ? (l) => window.localStorage.setItem("ai-assistant.language", l) : null;

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

async function gotoApp(page, hash) {
  await page.goto(`${BASE}?capture=1${hash}`);
  await page.getByRole("navigation", { name: copy.nav }).waitFor();
  await page.waitForTimeout(400);
}

async function captureAll(outDir, deviceScaleFactor) {
  const shotPath = (name) => path.join(outDir, name);
  const reportSize = (name) => {
    const size = statSync(shotPath(name)).size;
    const kb = Math.round(size / 1024);
    console.log(`  ✓ ${name} (${kb} KB${size > WARN_BYTES ? " —— 超过 1.5MB，考虑压缩" : ""})`);
  };
  await mkdir(outDir, { recursive: true });
  await ensureServer();
  const browser = await chromium.launch({
    args: ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream"],
  });

  try {
    // —— 会话进行中的截图（深色 / 浅色）——
    for (const theme of ["dark", "light"]) {
      const context = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor, locale: "zh-CN" });
      await context.addInitScript((t) => {
        window.localStorage.setItem("ai-assistant.theme", t);
      }, theme);
      if (LANGUAGE_INIT) await context.addInitScript(LANGUAGE_INIT, LANG);
      const page = await context.newPage();

      await gotoApp(page, "#/workspace");
      await page.getByRole("button", { name: copy.micStart }).click();
      // 面试官追问瞬间（第 2 轮回答流式输出中）。
      await page.getByText(copy.turn2Reply, { exact: false }).first().waitFor({ timeout: 60_000 });
      await page.waitForTimeout(theme === "dark" ? 700 : 0);
      if (theme === "dark") {
        await page.screenshot({ path: shotPath("interview-live.png") });
        reportSize("interview-live.png");
      }
      // 第 4 轮回答（脚本收尾）——头图。
      await page.getByText(copy.turn4ReplyEnd, { exact: false }).first().waitFor({ timeout: 60_000 });
      await page.waitForTimeout(800);
      // The panel stops auto-scrolling mid-stream; align the finished reply to the bottom edge.
      await page.evaluate(() => {
        const replies = document.querySelectorAll(".session-conversation .session-bubble-assistant");
        replies[replies.length - 1]?.scrollIntoView({ block: "end" });
      });
      await page.waitForTimeout(300);
      const workspaceShot = theme === "dark" ? "workspace-dark.png" : "workspace-light.png";
      await page.screenshot({ path: shotPath(workspaceShot) });
      reportSize(workspaceShot);
      await page.getByRole("button", { name: copy.endCall }).click();
      await context.close();
    }

    // —— 延迟瀑布条（lane-F F07 追加，只加不改）——
    {
      const context = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor, locale: "zh-CN" });
      await context.addInitScript(() => {
        window.localStorage.setItem("ai-assistant.theme", "dark");
      });
      if (LANGUAGE_INIT) await context.addInitScript(LANGUAGE_INIT, LANG);
      const page = await context.newPage();
      await gotoApp(page, "#/workspace");
      await page.getByRole("button", { name: copy.micStart }).click();
      await page.getByText(copy.turn2Reply, { exact: false }).first().waitFor({ timeout: 60_000 });
      await page.waitForTimeout(800);
      // 展开首个有数据的轮次瀑布条明细表。
      // （第 1 轮是开场白，不产生时间线数据；从第 2 轮起才有延迟瀑布条，
      //   合并修正 M：原选择器 /第 1 轮延迟/ 永远匹配不到，见波次 2 报告。）
      await page.getByRole("button", { name: copy.latencyBar }).first().click();
      await page.getByText(copy.latencyDetails).first().waitFor({ timeout: 5_000 });
      await page.waitForTimeout(300);
      await page.screenshot({ path: shotPath("latency-waterfall.png") });
      reportSize("latency-waterfall.png");
      await page.getByRole("button", { name: copy.endCall }).click();
      await context.close();
    }

    // —— 静态页面截图（深色）——
    const context = await browser.newContext({ viewport: VIEWPORT, deviceScaleFactor, locale: "zh-CN" });
    await context.addInitScript(() => {
      window.localStorage.setItem("ai-assistant.theme", "dark");
    });
    if (LANGUAGE_INIT) await context.addInitScript(LANGUAGE_INIT, LANG);
    const page = await context.newPage();

    // 角色编辑。
    await gotoApp(page, "&category=roles#/settings");
    await page.getByText(copy.strictInterviewer).first().waitFor();
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("roles.png") });
    reportSize("roles.png");

    // 资料库（带检索结果）。
    await gotoApp(page, "#/materials");
    await page.getByText(copy.materialsCount).waitFor();
    await page.getByPlaceholder(copy.searchPlaceholder).fill(LANG === "en" ? "cache breakdown" : "缓存击穿");
    await page.getByRole("button", { name: copy.searchAction }).click();
    await page.getByText(copy.searchResults).waitFor();
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("materials.png") });
    reportSize("materials.png");

    // 记录列表 + 详情。
    await gotoApp(page, "#/records");
    await page.getByText(copy.sessionsCount).waitFor();
    await page.getByRole("button", { name: copy.view, exact: true }).first().click();
    await page.getByText(copy.turnLabel).first().waitFor({ timeout: 15_000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("records.png") });
    reportSize("records.png");

    // 服务配置（Key 为空的脱敏态）。
    await gotoApp(page, "#/services");
    await page.getByRole("heading", { name: copy.providerName }).waitFor();
    await page.getByRole("button", { name: copy.testProvider }).click();
    await page.getByText(copy.testPassed).first().waitFor({ timeout: 10_000 });
    await page.screenshot({ path: shotPath("services.png") });
    reportSize("services.png");

    // 虚拟直播讲稿（生成 → 确认 → 播报中）。
    await gotoApp(page, "#/livestream");
    await page.getByText(copy.demoScript).waitFor({ timeout: 15_000 });
    await page.getByText(copy.liveConfig).click();
    // check() 自带可操作性等待：配置区可能被挂载期重渲染暂时收起。
    await page.getByLabel(LANG === "en" ? "Yunfan Collaboration Suite Product Handbook v2.3.pdf" : "云帆协同办公平台产品手册 v2.3.pdf").check({ timeout: 20000 });
    await page.getByLabel(copy.productTitle).fill(LANG === "en" ? "Yunfan Collaboration Suite" : "云帆协同办公平台");
    await page.getByRole("button", { name: copy.generateScript }).click();
    await page.getByRole("button", { name: copy.confirmScript }).waitFor({ timeout: 20_000 });
    await page.getByRole("button", { name: copy.confirmScript }).click();
    await page.getByRole("button", { name: copy.startSpeaking }).click();
    try {
      await page.getByText(copy.segmentTitle).first().waitFor({ timeout: 15_000 });
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
    await page.getByText(copy.playingLive).waitFor({ timeout: 15_000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("livestream.png") });
    reportSize("livestream.png");

    // 外观设置。
    await gotoApp(page, "&category=appearance#/settings");
    await page.getByText(copy.appearanceDescription).waitFor();
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("settings-appearance.png") });
    reportSize("settings-appearance.png");

    // 延迟诊断（lane-F F05：诊断区升级为性能面板）。
    await gotoApp(page, "&category=diagnostics#/settings");
    await page.getByText(copy.performancePanel).waitFor();
    // 性能面板上线路名会出现多次（线路区/分阶段区/最近轮区），取首个即可。
    await page.getByText(copy.demoRouteName).first().waitFor({ timeout: 15_000 });
    await page.waitForTimeout(400);
    await page.screenshot({ path: shotPath("diagnostics.png") });
    reportSize("diagnostics.png");

    // 性能面板（分阶段百分位 + 最近轮折线，lane-F F07 追加）。
    await page.getByText(copy.stageLatencies).first().waitFor({ timeout: 10_000 });
    await page.waitForTimeout(300);
    await page.screenshot({ path: shotPath("performance-panel.png") });
    reportSize("performance-panel.png");

    await context.close();
  } finally {
    await browser.close();
    stopServer();
  }
  console.log(`\n截图已生成到 ${outDir}`);
}

async function main() {
  // B21/H12：--lang en 只产出 .github/assets/screenshots/en/；中文流程保持原样（含 website 资产）。
  if (LANG === "en") {
    await captureAll(path.join(OUT_DIR, "en"), 2);
    return;
  }
  await captureAll(OUT_DIR, 2);
  await captureAll(WEB_OUT_DIR, 1);
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
