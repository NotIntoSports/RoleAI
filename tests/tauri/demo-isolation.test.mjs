import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join, extname } from "node:path";
import test from "node:test";

/**
 * 演示隔离契约：在线演示的代码（mockIPC 桥、脚本会话、演示横幅文案）绝不进入
 * 桌面正式包产物 dist-tauri-ui/。产物不存在时提示先运行构建（不判失败）。
 *
 * 标记选取原则（均在 dist-demo 里做过阳性对照，确保“真泄漏必被抓”）：
 * - minifier 会重命名 mockIPC 一类的函数名，不能用作标记；
 * - JSX 会把 “在线演示：所有数据均为虚构” 拆成两个子串，故用更短的稳定字串；
 * - 字符串字面量与对象属性名不会被压缩掉。
 */
const DIST = "dist-tauri-ui";
const MARKERS = [
  "所有数据均为虚构", // 演示横幅文案
  "在线演示尚未模拟该能力", // 模拟后端兜底错误
  "__roleaiDemoReset", // 一键重置钩子（属性名，不会被改名）
  "roleai.demo.onboarded", // 引导持久化 key
  "Couldn't find callback id", // 官方 @tauri-apps/api/mocks 模块的告警串
];

async function collectFiles(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await collectFiles(fullPath)));
    } else if (entry.isFile() && [".html", ".js", ".css"].includes(extname(entry.name))) {
      files.push(fullPath);
    }
  }
  return files;
}

test("demo code never ships in the desktop bundle (dist-tauri-ui)", async (t) => {
  let files;
  try {
    files = await collectFiles(DIST);
  } catch {
    t.skip("dist-tauri-ui 不存在：先运行 `npm run build:tauri-ui` 再跑本契约测试");
    return;
  }
  assert.ok(files.length > 0, "dist-tauri-ui 为空：先运行 `npm run build:tauri-ui`");
  const offenders = [];
  for (const file of files) {
    const content = await readFile(file, "utf8");
    for (const marker of MARKERS) {
      if (content.includes(marker)) {
        offenders.push(`${file} 含演示标记 "${marker}"`);
      }
    }
  }
  assert.deepEqual(
    offenders,
    [],
    `演示代码泄漏进正式包（演示入口只允许进 dist-demo）：
${offenders.join("\n")}`,
  );
});
