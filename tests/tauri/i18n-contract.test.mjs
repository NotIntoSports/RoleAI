import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { extname, join } from "node:path";
import test from "node:test";

/**
 * i18n 防回归契约（lane-H H11）：
 * 1. 生产前端代码（排除测试、src/locales/、src/demo/ 中文数据区、注释）不得出现中文字面量；
 * 2. zh-CN 与 en 两本词典的键集合必须完全一致（含字符串叶子与数组叶子的类型一致）。
 *
 * 与 rg 相比的取舍（只严不松）：
 * - 扫描不读 .gitignore、跳过 node_modules 与点前缀目录，覆盖全部 src 下 .ts/.tsx；
 * - 注释里的中文允许存在（实现说明用中文是仓库惯例），但字符串/JSX 文本里的中文一律算违规；
 * - src/demo/** 整体豁免：演示后端按界面语言在中文/英文数据集之间切换（H09），
 *   其中文数据集本体（记录种子、脚本、练习题、横幅文案表）就是数据而非界面文案；
 * - 唯一的字面量白名单是后端契约值（见 ALLOWED_CONTRACT_LITERALS），逐文件最小化并写明出处。
 */

/** 递归收集 src 下全部 .ts/.tsx 源文件（含点前缀目录，跳过 node_modules）。 */
async function collectSourceFiles(dir) {
  const results = [];
  const entries = await readdir(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === "node_modules" || entry.name.startsWith(".")) continue;
      results.push(...(await collectSourceFiles(fullPath)));
    } else if (entry.isFile() && [".ts", ".tsx"].includes(extname(entry.name))) {
      results.push(fullPath);
    }
  }
  return results;
}

/**
 * 去掉注释（// 行注释与块注释），字符串内容保留。
 * 状态机处理引号与模板串，避免 "https://…" 里的 // 被误当注释。
 * 未知转义序列按字符原样保留，宁可多留也不误删代码。
 */
function stripComments(code) {
  let out = "";
  let state = "code"; // code | line | block | single | double | template
  for (let i = 0; i < code.length; i += 1) {
    const ch = code[i];
    const next = code[i + 1];
    if (state === "code") {
      if (ch === "/" && next === "/") { state = "line"; i += 1; continue; }
      if (ch === "/" && next === "*") { state = "block"; i += 1; continue; }
      if (ch === "'") { state = "single"; out += ch; continue; }
      if (ch === '"') { state = "double"; out += ch; continue; }
      if (ch === "`") { state = "template"; out += ch; continue; }
      out += ch;
      continue;
    }
    if (state === "line") {
      if (ch === "\n") { state = "code"; out += ch; }
      continue;
    }
    if (state === "block") {
      if (ch === "*" && next === "/") { state = "code"; i += 1; }
      continue;
    }
    // 字符串态：原样输出，遇闭引号回 code；反斜杠跳过下一字符。
    out += ch;
    if (ch === "\\") {
      if (next !== undefined) { out += next; i += 1; }
      continue;
    }
    if ((state === "single" && ch === "'") || (state === "double" && ch === '"') || (state === "template" && ch === "`")) {
      state = "code";
    }
  }
  return out;
}

// 后端契约值白名单：这些中文字面量是对外接口的取值而非界面文案。
// - practice-wizard：面试官风格与难度的 value 是后端契约（lane-H 手册 H05 卡决策，value 保持中文原值）；
// - livestream-studio：「中文」是讲稿生成语言参数的默认值，随请求发往后端（H08 卡记录）。
const ALLOWED_CONTRACT_LITERALS = new Map([
  [
    "src/features/practice/practice-wizard.tsx",
    [/"(?:面试官|严苛面试官|基础|标准|进阶)"/g, /(?:基础|标准|进阶)(?=\s*:)/g],
  ],
  ["src/features/livestream/livestream-studio.tsx", [/"中文"/g]],
]);

test("production frontend code contains no Chinese literals outside locales and demo data", async () => {
  const violations = [];
  for (const file of await collectSourceFiles("src")) {
    const normalized = file.replaceAll("\\", "/");
    if (normalized.includes(".test.")) continue;
    if (normalized.startsWith("src/locales/")) continue;
    if (normalized.startsWith("src/demo/")) continue;
    let content = stripComments(await readFile(file, "utf8"));
    const allowances = ALLOWED_CONTRACT_LITERALS.get(normalized);
    if (allowances) {
      for (const pattern of allowances) content = content.replace(pattern, "");
    }
    const match = content.match(/[\u4e00-\u9fa5]/);
    if (match) {
      const line = content.slice(0, content.indexOf(match[0])).split("\n").length;
      violations.push(`${normalized}:${line}`);
    }
  }
  assert.deepEqual(
    violations,
    [],
    `production code must move Chinese copy into src/locales dictionaries (found ${violations.length}): ${violations.join(", ")}`,
  );
});

/** 把词典对象压成「点号路径 -> 叶子类型」的映射；叶子是字符串或字符串数组。 */
function collectLeafPaths(value, prefix = "", into = new Map()) {
  if (typeof value === "string") {
    into.set(prefix, "string");
    return into;
  }
  if (Array.isArray(value)) {
    assert.ok(
      value.every((item) => typeof item === "string"),
      `dictionary leaf must be a string or a string array at: ${prefix || "(root)"}`,
    );
    into.set(prefix, "array");
    return into;
  }
  assert.ok(value !== null && typeof value === "object", `unexpected dictionary leaf at: ${prefix || "(root)"}`);
  for (const [key, child] of Object.entries(value)) {
    collectLeafPaths(child, prefix ? `${prefix}.${key}` : key, into);
  }
  return into;
}

/** 从词典 TS 源码中抠出对象字面量并求值（对象只含字符串/数组/嵌套对象，是合法 JS 表达式）。
 * endMarker 指向词典对象的收尾（含闭合大括号），切片时补回被标记吞掉的那个 "}"。 */
function evaluateDictionary(source, exportMarker, endMarker) {
  const start = source.indexOf(exportMarker);
  assert.ok(start !== -1, `dictionary export not found: ${exportMarker}`);
  const bodyStart = start + exportMarker.length;
  const end = source.indexOf(endMarker, bodyStart);
  assert.ok(end !== -1, `dictionary terminator not found: ${endMarker}`);
  const literal = source.slice(bodyStart, end);
  // 外层包一层花括号再求值：对象字面量必须以 "{" 开头，否则 "app: {" 会被当作标识符 + 标签解析；
  // 正文自带的闭合大括号与尾逗号都在 literal 内，外层花括号负责字典对象本身的开合。
  return new Function(`"use strict"; return ({ ${literal} });`)();
}

test("zh-CN and en dictionaries have identical key sets and leaf kinds", async () => {
  const zhSource = await readFile("src/locales/zh-CN.ts", "utf8");
  const enSource = await readFile("src/locales/en.ts", "utf8");
  const zh = evaluateDictionary(zhSource, "export const zhCN = {", "} as const;");
  const en = evaluateDictionary(enSource, "export const en: Dictionary = {", "\n};");

  const zhPaths = collectLeafPaths(zh);
  const enPaths = collectLeafPaths(en);

  const missingInEn = [...zhPaths.keys()].filter((key) => !enPaths.has(key));
  const missingInZh = [...enPaths.keys()].filter((key) => !zhPaths.has(key));
  const kindMismatches = [...zhPaths.entries()]
    .filter(([key, kind]) => enPaths.has(key) && enPaths.get(key) !== kind)
    .map(([key, kind]) => `${key} (${kind} vs ${enPaths.get(key)})`);

  assert.deepEqual(missingInEn, [], "keys present in zh-CN but missing in en");
  assert.deepEqual(missingInZh, [], "keys present in en but missing in zh-CN");
  assert.deepEqual(kindMismatches, [], "leaf kind (string vs array) must match between zh-CN and en");
  // 基本规模底线：词典被意外清空时立刻暴露。
  assert.ok(zhPaths.size > 500, `zh-CN dictionary unexpectedly small: ${zhPaths.size}`);
  assert.equal(zhPaths.size, enPaths.size);
});
