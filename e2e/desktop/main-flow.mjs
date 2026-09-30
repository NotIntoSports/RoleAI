/**
 * 真实桌面应用主流程端到端测试（lane-I I03）。
 *
 * 复用 I02 验证过的 WebDriver 管线（tauri-driver + msedgedriver + selenium-webdriver，
 * 见 e2e/desktop/wd-smoke.mjs 顶部注释与 docs/dependency-decisions.md 的 I02 条目），
 * 在隔离目录中启动 debug 构建驱动界面走完主流程；模型相关交互全部指向本目录的
 * mock-openai-server.mjs（OpenAI 兼容最小子集），不连任何真实 AI 服务。
 *
 * 用例（对应 I03 卡片）：
 *   1. 首次启动 → 服务页配置 mock 供应商 → 连接测试通过；
 *   2. 新建角色 → 保存 → 落盘配置包含该角色（重启语义见下方“已知限制”），
 *      并断言隔离目录的单次性契约（同根重启被 startup.rs 拒绝，exit 2）；
 *   3. 导入一份 txt 资料 → FTS 搜索命中；
 *   4. 文本输入完成一轮对话 → 记录页出现该会话 → 导出 Markdown（校验文件内容）；
 *   5. 模拟面试：生成题单（mock 固定 JSON）→ 开始训练 → 作答/跳过 → 结束
 *      → 生成报告 → 模拟面试页出现报告（总分 + 雷达图 + 免责声明）。
 *
 * 已知限制（记账本）：
 *   - --isolated-root 按设计只接受空目录（startup.rs），同一隔离目录无法二次启动；
 *     “重启后仍在”以「保存后配置文件落盘 + 同根重启被拒（单次性契约）」断言，
 *     配置从该文件加载的行为由 src-tauri 配置层单测覆盖。
 *   - 麦克风语音流程不在 e2e 范围内（CI 无音频设备）：会话以本机麦克风输入源启动
 *     仅用于激活文本输入通道（WebView 自动授予权限），不依赖真实收音。
 *
 * 前置条件与 wd-smoke.mjs 相同（不在此脚本内构建）：
 *   debug 产物、tauri-driver、msedgedriver；可用 TAURI_E2E_EXECUTABLE /
 *   TAURI_E2E_TAURI_DRIVER / TAURI_E2E_MSEDGEDRIVER 覆盖。
 * 运行：npm run test:e2e:desktop:main
 */
import { spawn } from "node:child_process";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { Builder, By, until } from "selenium-webdriver";
import { MOCK_REPLY_TEXT, startMockOpenAiServer } from "./mock-openai-server.mjs";

const REPOSITORY_ROOT = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const SESSION_TIMEOUT_MS = 90_000;
const ELEMENT_TIMEOUT_MS = 30_000;
const STEP_TIMEOUT_MS = 60_000;
/** 失败现场截图目录（best-effort，供 I04 CI artifact 与本地排查）。 */
const ARTIFACT_DIR = process.env.TAURI_E2E_ARTIFACT_DIR
  ?? join(REPOSITORY_ROOT, ".codex-tmp", "e2e-desktop");

const PROVIDER_NAME = "E2E Mock 供应商";
const ROUTE_NAME = "E2E 级联线路";
const ROLE_NAME = "E2E 角色甲";
const MATERIAL_FILE_NAME = "e2e-interview-jd.txt";
const MATERIAL_KEYWORD = "量子缓存一致性";
const CHAT_USER_TEXT = "你好，请用一句话介绍模拟面试训练。";
const PRACTICE_ANSWER_TEXT = "E2E 练习回答：我会先说结论，再分三点展开。";
const PRACTICE_POSITION = "后端开发工程师";

let stepCounter = 0;
let lastScreenshotDriver = null;

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

function waitForHttpStatus(url, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  return (async function poll() {
    while (Date.now() < deadline) {
      try {
        const response = await fetch(url);
        if (response.ok) return;
      } catch {
        // 驱动尚未监听，继续等
      }
      await new Promise((resolveWait) => setTimeout(resolveWait, 250));
    }
    throw new Error(`WebDriver 服务 ${url} 在 ${timeoutMs / 1000}s 内未就绪`);
  })();
}

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

async function step(name, action) {
  stepCounter += 1;
  console.log(`\n[I03 ${stepCounter}] ${name}`);
  try {
    await action();
  } catch (error) {
    await captureFailureScreenshot(name);
    const reason = error instanceof Error ? error.message : String(error);
    throw new Error(`步骤「${name}」失败：${reason}`);
  }
}

async function captureFailureScreenshot(name) {
  const driver = lastScreenshotDriver;
  if (!driver) return;
  try {
    const { mkdir, writeFile: writeFileArtifact } = await import("node:fs/promises");
    const encoded = await driver.takeScreenshot();
    await mkdir(ARTIFACT_DIR, { recursive: true });
    const safeName = name.replace(/[\\/:*?"<>|\s]+/g, "-").slice(0, 60);
    const path = join(ARTIFACT_DIR, `i03-step${stepCounter}-${safeName}.png`);
    await writeFileArtifact(path, Buffer.from(encoded, "base64"));
    console.error(`[I03] 失败现场截图已保存：${path}`);
  } catch {
    // 截图是 best-effort，不掩盖原始失败
  }
}

/** React 受控输入不能用 executeScript 直接赋值（不触发 onChange）；用原生 setter + input 事件。 */
async function fillInput(driver, element, value) {
  await driver.executeScript(
    `
    const [el, next] = arguments;
    const proto = el.tagName === "TEXTAREA" ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(proto, "value").set;
    setter.call(el, next);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    `,
    element,
    value,
  );
}

async function fillByXpath(driver, xpath, value) {
  const element = await driver.wait(until.elementLocated(By.xpath(xpath)), ELEMENT_TIMEOUT_MS);
  await fillInput(driver, element, value);
  return element;
}

async function clickByCss(driver, css) {
  const element = await driver.wait(until.elementLocated(By.css(css)), ELEMENT_TIMEOUT_MS);
  await element.click();
  return element;
}

async function clickByXpath(driver, xpath) {
  const element = await driver.wait(until.elementLocated(By.xpath(xpath)), ELEMENT_TIMEOUT_MS);
  await element.click();
  return element;
}

/** 等待某个 CSS 元素的文本包含目标子串（每轮重新定位，避免 React 重渲染导致的 stale element）。 */
async function waitTextContains(driver, css, substring, timeoutMs = ELEMENT_TIMEOUT_MS) {
  const deadline = Date.now() + timeoutMs;
  let lastText = "";
  while (Date.now() < deadline) {
    const elements = await driver.findElements(By.css(css));
    for (const element of elements) {
      let text = "";
      try {
        text = await element.getText();
      } catch {
        continue; // 元素刚好被重渲染替换，下一轮重新定位
      }
      lastText = text;
      if (text.includes(substring)) return element;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 250));
  }
  throw new Error(`等待 ${css} 出现文本「${substring}」超时；最后读到的文本：${JSON.stringify(lastText.slice(0, 200))}`);
}

/** 通过左侧导航切换页面，并等待目标页头渲染。 */
async function navigateTo(driver, route, label) {
  await clickByCss(driver, `.app-nav-item[aria-label="${label}"]`);
  await driver.wait(until.elementLocated(By.css(`#page-heading-${route}`)), ELEMENT_TIMEOUT_MS);
  await waitTextContains(driver, `#page-heading-${route}`, label);
  const href = await driver.executeScript("return window.location.href").catch(() => "?");
  console.log(`[I03] 导航到「${label}」完成，URL=${href}`);
}

async function mockRequestsWithRetries(mockServer, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${mockServer.port}/__requests`);
      if (response.ok) return (await response.json()).requests;
    } catch {
      // 重试
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 250));
  }
  throw new Error("读取 mock 请求记录超时");
}

async function assertMockCalled(mockServer, kind, substring) {
  const requests = await mockRequestsWithRetries(mockServer);
  const hit = requests.find(
    (request) => request.kind === kind
      && (substring ? request.userContent.includes(substring) : true),
  );
  if (!hit) {
    throw new Error(
      `mock 未收到预期的 ${kind} 请求（${substring ? `包含「${substring}」` : "任意"}）；实际：${JSON.stringify(requests)}`,
    );
  }
}

/** 等待会话进入「聆听中」再提交文本，避开开启回合（题面播报）仍在进行时的输入竞争。 */
async function waitPhaseListening(driver, timeoutMs = STEP_TIMEOUT_MS) {
  await waitTextContains(driver, ".session-toolbar .status-badge", "聆听中", timeoutMs);
}

async function submitChatText(driver, text) {
  await waitPhaseListening(driver);
  const input = await driver.wait(
    until.elementLocated(By.css('input[aria-label="输入内容"]')),
    ELEMENT_TIMEOUT_MS,
  );
  await driver.wait(until.elementIsEnabled(input), ELEMENT_TIMEOUT_MS, "聊天输入框未启用");
  await fillInput(driver, input, text);
  await clickByCss(driver, 'button[aria-label="发送"]');
  await waitTextContains(driver, ".session-conversation", text);
  await waitTextContains(driver, ".session-conversation", MOCK_REPLY_TEXT, STEP_TIMEOUT_MS);
}

/** 结束会话并等待阶段徽标回到「已结束」（stopSession 完成的界面信号）。 */
async function stopSessionAndWait(driver) {
  await clickByCss(driver, 'button[aria-label="结束通话"]');
  await waitTextContains(driver, ".session-toolbar .status-badge", "已结束", STEP_TIMEOUT_MS);
}

/** 挂起现场诊断：从 webview 内做带超时的 invoke 探测并导出文档状态（区分后端卡死与路由/前端问题）。 */
async function dumpIpcDiagnostics(driver) {
  try {
    const report = await driver.executeAsyncScript(`
      const done = arguments[arguments.length - 1];
      const probe = (name, args) => Promise.race([
        window.__TAURI_INTERNALS__.invoke(name, args).then(
          (value) => name + ': OK ' + JSON.stringify(value).slice(0, 120),
          (error) => name + ': ERR ' + String(error).slice(0, 120),
        ),
        new Promise((resolve) => setTimeout(() => resolve(name + ': TIMEOUT(2.5s)'), 2500)),
      ]);
      Promise.all([probe('practice_plan_list'), probe('runtime_get_status')]).then(done);
    `);
    console.error(`[I03] IPC 诊断：${report}`);
  } catch (error) {
    console.error(`[I03] IPC 诊断执行失败：${error.message}`);
  }
  try {
    const state = await driver.executeScript(`
      return {
        href: window.location.href,
        title: document.title,
        visibility: document.visibilityState,
        hasWizard: !!document.querySelector(".practice-wizard"),
        hasWorkspace: !!document.querySelector(".workspace-session"),
        activeNav: document.querySelector(".app-nav-item[data-active='true']")?.getAttribute("aria-label") ?? "(none)",
      };
    `);
    console.error(
      `[I03] 文档状态：href=${state.href} wizard=${state.hasWizard} workspace=${state.hasWorkspace} 当前导航=${state.activeNav}`,
    );
  } catch (error) {
    console.error(`[I03] 文档状态读取失败：${error.message}`);
  }
}

async function runMainFlow() {
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

  const mockServer = await startMockOpenAiServer();
  activeMockServer = mockServer;
  const fixturesRoot = await mkdtemp(join(tmpdir(), "roleai-e2e-fixtures-"));
  const materialPath = join(fixturesRoot, MATERIAL_FILE_NAME);
  await writeFile(materialPath, [
    "岗位 JD：后端开发工程师（E2E 模拟面试示例资料）",
    "",
    "职责：",
    `- 负责${MATERIAL_KEYWORD}服务的设计与实现。`,
    "- 参与流式语音管线的低延迟优化。",
    "",
    "要求：",
    "- 熟悉分布式系统基础，理解幂等与背压。",
    "- 有 Rust 或 Go 生产经验。",
  ].join("\n"), "utf8");

  const isolatedRoot = await mkdtemp(join(tmpdir(), "roleai-e2e-flow-"));
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
            args: [`--isolated-root=${isolatedRoot}`],
          },
        })
        .build(),
      new Promise((_, reject) => setTimeout(
        () => reject(new Error(`会话在 ${SESSION_TIMEOUT_MS / 1000}s 内未建立`)),
        SESSION_TIMEOUT_MS,
      )),
    ]);
    lastScreenshotDriver = driver;

    await step("应用启动：主窗口渲染工作台", async () => {
      await waitTextContains(driver, "#page-heading-workspace", "工作台");
    });

    // ── 用例 1：配置服务（指向 mock）→ 测试连接成功 ──
    await step("服务页：添加 mock 供应商并保存", async () => {
      await navigateTo(driver, "services", "服务");
      await fillByXpath(driver, "//label[contains(., '显示名称')]/input", PROVIDER_NAME);
      await fillByXpath(driver, "//label[contains(., '接入地址')]/input", mockServer.baseUrl);
      await clickByXpath(driver, "//button[normalize-space()='保存供应商']");
      await waitTextContains(driver, "p.services-message[data-tone='success']", "供应商已保存");
    });

    await step("服务页：连接测试通过（发现 mock 模型）", async () => {
      await clickByXpath(driver, `//button[@aria-label='测试 ${PROVIDER_NAME}']`);
      await waitTextContains(
        driver,
        "#services-panel-providers .service-test-result[data-tone='success']",
        "连接测试通过",
      );
      const requests = await mockRequestsWithRetries(mockServer);
      if (!requests.some((request) => request.kind === "models")) {
        throw new Error("mock 未收到 /models 请求，连接测试可能未真正发起");
      }
    });

    await step("服务页：配置级联语音线路并测试", async () => {
      await clickByCss(driver, "#services-category-routes");
      await fillByXpath(driver, "//label[contains(., '线路名称')]/input", ROUTE_NAME);
      for (const prefix of ["ASR", "LLM", "TTS"]) {
        const providerSelect = await driver.wait(
          until.elementLocated(By.xpath(`//label[contains(., '${prefix} 供应商')]/select`)),
          ELEMENT_TIMEOUT_MS,
        );
        await providerSelect.findElement(By.xpath(`./option[normalize-space()='${PROVIDER_NAME}']`)).click();
        await fillByXpath(driver, `//label[contains(., '${prefix} 模型')]/input`, `e2e-mock-${prefix.toLowerCase()}`);
      }
      await clickByXpath(driver, "//button[normalize-space()='保存语音线路']");
      await waitTextContains(driver, "p.services-message[data-tone='success']", "语音线路已保存");
    });

    await step("服务页：线路测试通过并启用", async () => {
      await clickByXpath(
        driver,
        `//section[@id='services-panel-routes']//article[contains(., '${ROUTE_NAME}')]//button[normalize-space()='测试']`,
      );
      await waitTextContains(driver, "p.services-message[data-tone='success']", "线路测试通过", STEP_TIMEOUT_MS);
      await assertMockCalled(mockServer, "tts");
      await clickByXpath(
        driver,
        `//section[@id='services-panel-routes']//article[contains(., '${ROUTE_NAME}')]//button[normalize-space()='启用']`,
      );
      await waitTextContains(driver, "p.services-message[data-tone='success']", "语音线路已启用");
    });

    // ── 用例 2：新建角色 → 保存 → 持久化 ──
    await step("设置页：新建角色并设为默认", async () => {
      await navigateTo(driver, "settings", "设置");
      await clickByCss(driver, "#settings-category-roles");
      await fillByXpath(
        driver,
        "//section[contains(@class,'role-editor')]//label[contains(., '显示名称')]/input",
        ROLE_NAME,
      );
      await fillByXpath(
        driver,
        "//section[contains(@class,'role-editor')]//label[contains(., '系统提示')]/textarea",
        "你是 E2E 测试用的面试助手，回答保持简短。",
      );
      await clickByXpath(driver, "//section[contains(@class,'role-editor')]//button[normalize-space()='保存角色']");
      await waitTextContains(
        driver,
        "section.role-editor p.services-message",
        "角色已保存",
      );
      await clickByXpath(
        driver,
        `//section[contains(@class,'role-editor')]//article[contains(., '${ROLE_NAME}')]//button[normalize-space()='设为默认']`,
      );
      await waitTextContains(
        driver,
        "section.role-editor p.services-message",
        "默认角色已更新",
      );
    });

    await step("持久化：角色写入隔离目录配置文件，且同根重启被单次性契约拒绝", async () => {
      const configPath = join(isolatedRoot, "config", "local.json");
      const configText = await readFile(configPath, "utf8");
      if (!configText.includes(ROLE_NAME)) {
        throw new Error(`配置文件 ${configPath} 中未找到角色「${ROLE_NAME}」，持久化失败`);
      }
      if (!configText.includes(ROUTE_NAME)) {
        throw new Error(`配置文件 ${configPath} 中未找到语音线路「${ROUTE_NAME}」`);
      }
      // isolated root 只接受空目录（startup.rs）：同根二启必须 exit 2。
      // 这是“重启持久化”的平台契约边界：隔离目录单次有效，正式持久化走配置文件。
      const relaunch = spawn(executable, [`--isolated-root=${isolatedRoot}`], {
        stdio: ["ignore", "ignore", "pipe"],
        windowsHide: true,
      });
      let stderr = "";
      relaunch.stderr.on("data", (chunk) => { stderr += String(chunk); });
      const exitCode = await new Promise((resolveExit, rejectExit) => {
        const timer = setTimeout(() => rejectExit(new Error("同根重启探针 15s 内未退出")), 15_000);
        relaunch.once("error", rejectExit);
        relaunch.once("exit", (code) => { clearTimeout(timer); resolveExit(code); });
      });
      if (exitCode !== 2) {
        throw new Error(`同根重启预期 exit 2（隔离目录单次性），实际 ${exitCode}；stderr=${stderr.trim()}`);
      }
    });

    // ── 用例 3：导入 txt 资料 → 搜索命中 ──
    await step("资料页：导入 txt 并在列表出现", async () => {
      await navigateTo(driver, "materials", "资料");
      await clickByCss(driver, "details.library-import > summary");
      await fillByXpath(driver, "//label[contains(., '文件路径')]/input", materialPath);
      await clickByXpath(driver, "//button[normalize-space()='导入']");
      await waitTextContains(driver, "section.materials-library p.services-message", "资料已导入");
      await driver.wait(
        until.elementLocated(By.css(`article[aria-label="${MATERIAL_FILE_NAME}"]`)),
        ELEMENT_TIMEOUT_MS,
        "已导入资料列表未出现该文件",
      );
    });

    await step("资料页：FTS 搜索命中导入内容", async () => {
      await fillByXpath(driver, "//form[@aria-label='搜索资料']//label/input", MATERIAL_KEYWORD);
      await clickByXpath(driver, "//form[@aria-label='搜索资料']//button[normalize-space()='搜索']");
      await waitTextContains(driver, ".library-search-hit", MATERIAL_KEYWORD, STEP_TIMEOUT_MS);
    });

    // ── 用例 4：文本输入一轮对话 → 记录页出现会话 → 导出 Markdown ──
    await step("工作台：开始会话并用文本输入完成一轮对话", async () => {
      await navigateTo(driver, "workspace", "工作台");
      await clickByCss(driver, "button.composer-start");
      await submitChatText(driver, CHAT_USER_TEXT);
      await assertMockCalled(mockServer, "turn", CHAT_USER_TEXT);
    });

    await step("工作台：结束会话", async () => {
      await stopSessionAndWait(driver);
    });

    await step("记录页：会话记录出现且导出 Markdown 落盘", async () => {
      await navigateTo(driver, "records", "记录");
      await clickByXpath(driver, "//button[normalize-space()='查看']");
      await waitTextContains(driver, ".record-detail", CHAT_USER_TEXT);
      await waitTextContains(driver, ".record-detail", MOCK_REPLY_TEXT);
      await clickByXpath(driver, "//button[normalize-space()='导出 Markdown']");
      const pathMessage = await waitTextContains(
        driver,
        "section.records-list p.services-message",
        "exports",
        STEP_TIMEOUT_MS,
      );
      const exportedPath = (await pathMessage.getText()).trim();
      const markdown = await readFile(exportedPath, "utf8");
      if (!markdown.includes(CHAT_USER_TEXT) || !markdown.includes(MOCK_REPLY_TEXT)) {
        throw new Error(`导出的 Markdown 内容不完整：${exportedPath}`);
      }
      console.log(`[I03] 已校验导出文件：${exportedPath}`);
    });

    // ── 用例 5：模拟面试闭环（mock 固定 JSON 驱动）──
    await step("模拟面试：向导生成题单", async () => {
      await navigateTo(driver, "practice", "模拟面试");
      await fillByXpath(driver, "//label[contains(., '岗位方向')]/input", PRACTICE_POSITION);
      await clickByXpath(driver, "//div[contains(@class,'practice-nav')]//button[normalize-space()='下一步']");
      await clickByXpath(driver, "//div[contains(@class,'practice-nav')]//button[normalize-space()='下一步']");
      const countInput = await driver.wait(
        until.elementLocated(By.xpath("//label[contains(., '题量')]/input")),
        ELEMENT_TIMEOUT_MS,
      );
      await fillInput(driver, countInput, "1");
      await clickByXpath(driver, "//div[contains(@class,'practice-nav')]//button[normalize-space()='下一步']");
      await clickByXpath(driver, "//button[normalize-space()='生成题单']");
      await driver.wait(until.elementLocated(By.css(".practice-plan-editor")), ELEMENT_TIMEOUT_MS, "题单编辑器未出现");
      await waitTextContains(driver, ".practice-plan-editor", "1 道题");
      await assertMockCalled(mockServer, "plan", PRACTICE_POSITION);
    });

    await step("模拟面试：开始训练并接管到工作台", async () => {
      await clickByXpath(driver, "//button[normalize-space()='开始训练']");
      const deadline = Date.now() + STEP_TIMEOUT_MS;
      let navigated = false;
      let lastHref = "";
      while (Date.now() < deadline) {
        const headings = await driver.findElements(By.css("#page-heading-workspace"));
        if (headings.length > 0) { navigated = true; break; }
        // 向导出现错误消息说明 startPracticeSession 已返回（blocked/失败），直接带着消息报错
        const messages = await driver.findElements(By.css(".practice-wizard p.services-message"));
        let messageText = "";
        for (const message of messages) {
          const text = await message.getText().catch(() => "");
          if (text.trim()) messageText = text.trim();
        }
        if (messageText) {
          throw new Error(`开始训练返回错误：${messageText}`);
        }
        // 记录 URL 变化时间线：定位 hash 何时被剥离（挂起根因证据）。
        const href = await driver.executeScript("return window.location.href").catch(() => "?");
        if (href !== lastHref) {
          console.log(`[I03] URL 变化：${lastHref || "(初始)"} → ${href}`);
          lastHref = href;
        }
        await new Promise((resolveWait) => setTimeout(resolveWait, 500));
      }
      if (!navigated) {
        await dumpIpcDiagnostics(driver);
        throw new Error("开始训练后未跳回工作台（IPC 疑似挂起）");
      }
      await waitTextContains(driver, ".practice-hud-progress", "第 1 / 1 题", STEP_TIMEOUT_MS);
    });

    await step("模拟面试：作答一轮后跳过并结束", async () => {
      await submitChatText(driver, PRACTICE_ANSWER_TEXT);
      await clickByCss(driver, ".practice-hud-skip");
      await waitTextContains(driver, ".practice-hud-progress", "全部 1 题已完成", STEP_TIMEOUT_MS);
      await stopSessionAndWait(driver);
    });

    await step("模拟面试：生成训练报告", async () => {
      await clickByCss(driver, ".practice-hud-report");
      await waitTextContains(
        driver,
        "p.services-message.session-message",
        "训练报告已生成",
        STEP_TIMEOUT_MS,
      );
      await assertMockCalled(mockServer, "review");
    });

    await step("模拟面试：报告页出现总分与雷达图", async () => {
      await navigateTo(driver, "practice", "模拟面试");
      await driver.wait(
        until.elementLocated(By.css("article.practice-report[aria-label='训练报告详情']")),
        ELEMENT_TIMEOUT_MS,
        "训练报告详情未出现",
      );
      await waitTextContains(driver, "article.practice-report", "总分");
      await waitTextContains(driver, "article.practice-report", "评分由 AI 生成，仅供练习参考");
      await driver.wait(until.elementLocated(By.css("article.practice-report svg.practice-radar")), ELEMENT_TIMEOUT_MS, "雷达图未渲染");
    });

    console.log("\nPASS: 桌面应用主流程端到端全部通过（配置/持久化/资料/对话导出/模拟面试闭环）。");
  } finally {
    if (driver) {
      await driver.quit().catch(() => {});
    }
    if (tauriDriver.exitCode === null) {
      await killProcessTree(tauriDriver.pid);
    }
    // 失败时可用 TAURI_E2E_KEEP_ROOT=1 保留隔离目录（日志/数据库）供排查。
    const keepRoot = process.env.TAURI_E2E_KEEP_ROOT === "1";
    if (keepRoot) {
      console.error(`[I03] TAURI_E2E_KEEP_ROOT=1：保留隔离目录 ${isolatedRoot} 与资料目录 ${fixturesRoot}`);
    } else {
      await withRetries(
        () => rm(isolatedRoot, { recursive: true, force: true, maxRetries: 1 }),
        20,
        100,
      ).catch((error) => fail(`隔离目录 ${isolatedRoot} 清理失败：${error.message}`));
      await withRetries(
        () => rm(fixturesRoot, { recursive: true, force: true, maxRetries: 1 }),
        20,
        100,
      ).catch((error) => fail(`资料临时目录 ${fixturesRoot} 清理失败：${error.message}`));
    }
    await mockServer.close().catch(() => {});
  }
}

/** 当前运行的 mock 服务器引用（仅用于失败时输出请求证据链）。 */
let activeMockServer = null;

runMainFlow().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
  if (activeMockServer) {
    try {
      console.error(`[I03] mock 收到的请求（${activeMockServer.requests.length} 条）：`);
      for (const request of activeMockServer.requests) {
        console.error(
          `  - ${request.method} ${request.path} kind=${request.kind}${request.userContent ? ` user="${request.userContent.slice(0, 80)}"` : ""}`,
        );
      }
    } catch {
      // 诊断输出失败不掩盖原始错误
    }
  }
});

