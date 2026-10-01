/**
 * 真实桌面应用 WebDriver 端到端最小验证（lane-I I02）。
 *
 * 官方方案（https://v2.tauri.app/develop/tests/webdriver/）：
 *   tauri-driver（官方，MIT/Apache-2.0）作为 WebDriver 前端，Windows 下把
 *   msedgedriver 当原生驱动拉起（--native-driver），应用二进制通过
 *   `tauri:options.application` 交给它启动；客户端用官方 selenium-webdriver。
 *   msedgedriver 版本必须与本机 WebView2 Runtime 一致，否则会话建立会挂起。
 *
 * 隔离方式复用 scripts/test-tauri-package.mjs 的既有约定：
 *   应用以 `--isolated-root <已存在的空目录>` 启动（startup.rs 校验），
 *   配置 / 数据 / 日志 / WebView 数据全部落在该目录，退出后清理。
 *
 * 前置条件（不在脚本内构建，构建由 CI/门禁编排）：
 *   1. debug 产物：npm run build:tauri-ui && npx tauri build --debug --no-bundle
 *      产物 src-tauri/target/debug/role-ai-desktop.exe（可用 TAURI_E2E_EXECUTABLE 覆盖）。
 *   2. tauri-driver：cargo install tauri-driver --locked（~/.cargo/bin）。
 *   3. msedgedriver：.tools/msedgedriver/msedgedriver.exe（可用 TAURI_E2E_MSEDGEDRIVER 覆盖），
 *      版本与本机 WebView2 Runtime 一致（查询见 docs/dependency-decisions.md I02 条目）。
 *
 * 断言：主窗口渲染出工作台页头 → 导航到全部 6 个页面且各自页头 h1 渲染
 * 且各自页头 h1 渲染。麦克风相关流程不在 e2e 范围内（CI 无音频设备）。
 */
import { spawn } from "node:child_process";
import { access, mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { constants } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Builder, By, until } from "selenium-webdriver";

const REPOSITORY_ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const SESSION_TIMEOUT_MS = 60_000;
const ELEMENT_TIMEOUT_MS = 20_000;
/** 与 routes.ts 的 routeIds / routeLabel 对齐；页面 h1 由 page-shell.tsx 渲染。 */
const ROUTES = [
  ["workspace", "工作台"],
  ["livestream", "虚拟直播"],
  ["materials", "资料"],
  ["records", "记录"],
  ["services", "服务"],
  ["settings", "设置"],
];

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exitCode = 1;
}

async function requireFile(path, hint) {
  const absolute = resolve(path);
  try {
    await access(absolute, constants.F_OK);
  } catch {
    throw new Error(`${absolute} 不存在。${hint}`);
  }
  return absolute;
}

/** 找一个空闲 TCP 端口（关闭后立刻用有小概率被抢，e2e 环境可接受）。 */
function findFreePort() {
  return new Promise((resolvePort, rejectPort) => {
    const server = createServer();
    server.once("error", rejectPort);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolvePort(port));
    });
  });
}

/** 复用 test-tauri-package.mjs 的隔离启动环境：剥离宿主环境里会影响隔离的变量。 */
function isolatedLaunchEnvironment(sourceEnvironment, webviewDataFolder) {
  const environment = Object.fromEntries(
    Object.entries(sourceEnvironment).filter(([key]) => !new Set([
      "AI_VIRTUAL_ASSISTANT_CONFIG",
    ]).has(key.toUpperCase()) && !key.toUpperCase().startsWith("WEBVIEW2_")),
  );
  environment.WEBVIEW2_USER_DATA_FOLDER = webviewDataFolder;
  return environment;
}

function waitForHttpStatus(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  return (async function poll() {
    while (Date.now() < deadline) {
      try {
        const response = await fetch(url);
        if (response.ok) return;
      } catch {
        // driver 尚未监听，继续等
      }
      await new Promise((resolveWait) => setTimeout(resolveWait, 250));
    }
    throw new Error(`WebDriver 服务 ${url} 在 ${timeoutMs / 1000}s 内未就绪（多为 msedgedriver 与 WebView2 Runtime 版本不匹配导致的挂起）`);
  })();
}

/** taskkill /T 只作用于本脚本拉起的进程树，不会碰到用户自己开的应用。 */
function killProcessTree(pid) {
  return new Promise((resolveKill) => {
    const child = spawn("taskkill.exe", ["/PID", String(pid), "/T", "/F"], {
      stdio: "ignore",
      windowsHide: true,
    });
    child.once("error", () => resolveKill());
    child.once("exit", () => resolveKill());
  });
}

async function withRetries(action, attempts, delayMs) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      await action();
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolveRetry) => setTimeout(resolveRetry, delayMs));
    }
  }
  throw lastError;
}

async function runSmoke() {
  if (process.platform !== "win32") throw new Error("桌面端 WebDriver e2e 目前只支持 Windows（WebView2 + msedgedriver）");

  const executable = await requireFile(
    process.env.TAURI_E2E_EXECUTABLE ?? join(REPOSITORY_ROOT, "src-tauri", "target", "debug", "role-ai-desktop.exe"),
    "先跑 npm run build:tauri-ui && npx tauri build --debug --no-bundle",
  );
  const tauriDriverBinary = await requireFile(
    process.env.TAURI_E2E_TAURI_DRIVER ?? join(homedir(), ".cargo", "bin", "tauri-driver.exe"),
    "先跑 cargo install tauri-driver --locked",
  );
  const msedgedriverBinary = await requireFile(
    process.env.TAURI_E2E_MSEDGEDRIVER ?? join(REPOSITORY_ROOT, ".tools", "msedgedriver", "msedgedriver.exe"),
    "下载与本机 WebView2 Runtime 版本一致的 edgedriver_win64.zip 并解压到 .tools/msedgedriver/",
  );

  const isolatedRoot = await mkdtemp(join(tmpdir(), "roleai-e2e-"));
  const driverPort = await findFreePort();
  const nativePort = await findFreePort();
  const tauriDriver = spawn(
    tauriDriverBinary,
    ["--port", String(driverPort), "--native-port", String(nativePort), "--native-driver", msedgedriverBinary],
    { stdio: ["ignore", "inherit", "inherit"], windowsHide: true },
  );
  let driver;
  try {
    await waitForHttpStatus(`http://127.0.0.1:${driverPort}/status`, SESSION_TIMEOUT_MS);

    driver = await Promise.race([
      new Builder()
        .usingServer(`http://127.0.0.1:${driverPort}`)
        .withCapabilities({
          browserName: "wry",
          "tauri:options": {
            application: executable,
            // 等号单段形式：msedgedriver 只原样保留 `--flag=value`；
            // 两段式的松散路径会被 Chromium 命令行解析改写（见 startup.rs 测试）。
            args: [`--isolated-root=${isolatedRoot}`],
          },
        })
        .build(),
      new Promise((_, reject) => setTimeout(
        () => reject(new Error(`会话在 ${SESSION_TIMEOUT_MS / 1000}s 内未建立（msedgedriver 版本与 WebView2 Runtime 不匹配时官方文档描述为“挂起”）`)),
        SESSION_TIMEOUT_MS,
      )),
    ]);

    // 主窗口出现：工作台页头渲染（page-shell.tsx 的 h1#page-heading-workspace）。
    const workspaceHeading = await driver.wait(
      until.elementLocated(By.css("#page-heading-workspace")),
      ELEMENT_TIMEOUT_MS,
    );
    if ((await workspaceHeading.getText()) !== "工作台") {
      throw new Error("主窗口已渲染，但工作台页头文本不符");
    }

    // 导航到全部页面并断言各自页头渲染（app-nav 的按钮 aria-label = 页面名）。
    for (const [route, label] of ROUTES.slice(1)) {
      const navButton = await driver.wait(
        until.elementLocated(By.css(`.app-nav-item[aria-label="${label}"]`)),
        ELEMENT_TIMEOUT_MS,
      );
      await navButton.click();
      const heading = await driver.wait(
        until.elementLocated(By.css(`#page-heading-${route}`)),
        ELEMENT_TIMEOUT_MS,
      );
      await driver.wait(until.elementTextIs(heading, label), ELEMENT_TIMEOUT_MS);
    }
    console.log(`PASS: ${executable} 在隔离目录中启动，主窗口渲染工作台，全部 ${ROUTES.length} 个页面导航渲染正常。`);
  } finally {
    if (driver) {
      await driver.quit().catch(() => {});
    }
    if (tauriDriver.exitCode === null) {
      // driver.quit() 正常时会话内的应用已被 msedgedriver 关闭；这里兜底清树
      //（tauri-driver 在 Windows 不处理信号，msedgedriver 与应用可能残留）。
      await killProcessTree(tauriDriver.pid);
    }
    // WebView2 可能在宿主退出后才释放缓存句柄，带重试删除（同 test-tauri-package.mjs）。
    await withRetries(
      () => rm(isolatedRoot, { recursive: true, force: true, maxRetries: 1 }),
      20,
      100,
    ).catch((error) => fail(`隔离目录 ${isolatedRoot} 清理失败：${error.message}`));
  }
}

runSmoke().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
});
