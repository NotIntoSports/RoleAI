import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile, readdir } from "node:fs/promises";
import { join, extname } from "node:path";
import test from "node:test";

/**
 * Recursively collect all .ts/.tsx files from a directory.
 */
async function collectSourceFiles(dir) {
  const results = [];
  const entries = await readdir(dir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      results.push(...await collectSourceFiles(fullPath));
    } else if ([".ts", ".tsx"].includes(extname(entry.name)) && !entry.name.includes(".test.")) {
      results.push(fullPath);
    }
  }
  return results;
}

// Collect all shell source files (non-test .ts/.tsx in screens/, components/, and shell.tsx)
const shellFiles = [
  ...await collectSourceFiles("src/screens"),
  ...await collectSourceFiles("src/components"),
  "src/app/shell.tsx",
];

test("page shell does not directly import Tauri API", async () => {
  for (const file of shellFiles) {
    const content = await readFile(file, "utf8");
    assert.doesNotMatch(content, /@tauri-apps\/api/, `${file} must not import @tauri-apps/api`);
  }
});

test("page shell does not call fetch", async () => {
  for (const file of shellFiles) {
    const content = await readFile(file, "utf8");
    assert.doesNotMatch(content, /\bfetch\s*\(/, `${file} must not call fetch()`);
  }
});

test("page shell does not reference old API routes", async () => {
  for (const file of shellFiles) {
    const content = await readFile(file, "utf8");
    assert.doesNotMatch(content, /["'`]\/api\//, `${file} must not reference /api/ paths`);
  }
});

test("page shell does not import Next.js modules", async () => {
  for (const file of shellFiles) {
    const content = await readFile(file, "utf8");
    assert.doesNotMatch(content, /next\/(link|navigation|router|head)/, `${file} must not import next/*`);
  }
});

test("page shell does not use target=_blank", async () => {
  for (const file of shellFiles) {
    const content = await readFile(file, "utf8");
    assert.doesNotMatch(content, /target=["']_blank["']/, `${file} must not use target="_blank"`);
  }
});

test("page shell does not contain login/logout concepts", async () => {
  for (const file of shellFiles) {
    const content = await readFile(file, "utf8");
    assert.doesNotMatch(content, /登录|退出登录|control[-_]session/i, `${file} must not contain login concepts`);
  }
});

test("page shell does not use polling patterns", async () => {
  for (const file of shellFiles) {
    const content = await readFile(file, "utf8");
    assert.doesNotMatch(content, /setInterval/, `${file} must not use setInterval`);
    assert.doesNotMatch(content, /setTimeout\s*\([^)]*\d{3,}/, `${file} must not use long setTimeout`);
  }
});

test("security surface has zero drift since Phase 0-1 acceptance", async () => {
  // These 9 files must remain byte-for-byte identical to the recorded content
  // snapshot. This replaces the former `git diff --exit-code 1bc6bfb` assertion
  // with an equivalent-or-stronger mechanism: byte-level, auditable, and free of
  // any dependency on git history depth.
  const pinnedFiles = [
    "src-tauri/capabilities/main.json",
    "src-tauri/src/lib.rs",
    "src-tauri/src/contracts.rs",
    "src-tauri/src/commands/mod.rs",
    "src-tauri/src/commands/config.rs",
    "src-tauri/src/commands/materials.rs",
    "src-tauri/src/commands/sessions.rs",
    "src-tauri/src/commands/livestream.rs",
    "src-tauri/tauri.conf.json",
    "src/api/commands.ts",
    "src/generated/bindings.ts",
    "tests/tauri/shell-contract.test.mjs",
    "src-tauri/tests/security_contract.rs",
  ];
  const baseline = JSON.parse(
    await readFile("tests/tauri/security-surface-baseline.json", "utf8"),
  );
  assert.equal(baseline.algorithm, "sha256", "baseline must declare sha256");
  // The fixture must pin exactly the security-surface files — no silent removal.
  assert.deepEqual(
    Object.keys(baseline.files).sort(),
    [...pinnedFiles].sort(),
    "baseline must pin exactly the security-surface files",
  );
  const drifted = [];
  for (const rel of pinnedFiles) {
    const expected = baseline.files[rel];
    // Normalize CRLF -> LF before hashing so the check is deterministic across
    // platforms and core.autocrlf settings (mirrors the EOL-normalized semantics
    // of the `git diff` assertion this replaces). Any logical/content change is
    // still caught byte-for-byte.
    const normalized = Buffer.from(
      (await readFile(rel, "utf8")).replace(/\r\n/g, "\n"),
      "utf8",
    );
    const actual = createHash("sha256").update(normalized).digest("hex");
    if (actual !== expected) {
      drifted.push(`${rel}\n    expected ${expected}\n    actual   ${actual}`);
    }
  }
  assert.deepEqual(
    drifted,
    [],
    `security surface drift detected in ${drifted.length} file(s):\n  ${drifted.join("\n  ")}`,
  );
});

test("route ids match the design spec", async () => {
  const routesContent = await readFile("src/app/routes.ts", "utf8");
  // Extract the routeIds array
  const match = routesContent.match(/routeIds\s*=\s*\[([^\]]+)\]/);
  assert.ok(match, "routeIds array must exist in routes.ts");
  const ids = match[1].split(",").map(s => s.trim().replace(/["']/g, ""));
  assert.deepEqual(ids, [
    "workspace",
    "livestream",
    "materials",
    "records",
    "services",
    "settings",
  ]);
});

test("app.tsx retains startup dispatch structure", async () => {
  const appContent = await readFile("src/app/app.tsx", "utf8");
  assert.match(appContent, /getStartupState/, "app.tsx must call getStartupState");
  assert.match(appContent, /ConfigRepair/, "app.tsx must render ConfigRepair");
  assert.match(appContent, /Shell/, "app.tsx must render Shell");
});
