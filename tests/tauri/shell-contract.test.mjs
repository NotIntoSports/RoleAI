import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("root scripts are Tauri-only", async () => {
  const pkg = JSON.parse(await readFile("package.json", "utf8"));
  const vite = await readFile("vite.config.ts", "utf8");
  const tauri = JSON.parse(await readFile("src-tauri/tauri.conf.json", "utf8"));

  assert.match(vite, /outDir:\s*["']dist-tauri-ui["']/);
  assert.match(vite, /host:\s*["']127\.0\.0\.1["']/);
  assert.match(vite, /port:\s*1420/);
  assert.deepEqual(vite.match(/envPrefix:\s*\[[^\]]+\]/)?.[0], 'envPrefix: ["TAURI_ENV_"]');
  assert.equal(tauri.build.frontendDist, "../dist-tauri-ui");
  assert.equal(tauri.build.devUrl, "http://127.0.0.1:1420");
  assert.equal(tauri.identifier, "com.aivirtualassistant.desktop");
  const capability = JSON.parse(await readFile("src-tauri/capabilities/main.json", "utf8"));
  // (5) window scope pinned to exactly ["main"].
  assert.deepEqual(capability.windows, ["main"]);
  const permissions = capability.permissions;
  assert.ok(Array.isArray(permissions), "capability.permissions must be an array");
  // Phase 0-1 accepted baseline. Phase 3+ may ADD command permissions but must
  // never lose these, so we assert a superset (containment) rather than equality.
  const baselinePermissions = [
    "core:default",
    "allow-foundation-get-status",
    "allow-diagnostics-export",
    "allow-config-get-startup-state",
    "allow-config-restore-last-good",
    "allow-config-restore-defaults",
    "allow-open-app-directory",
  ];
  // (1) always includes core:default.
  assert.ok(permissions.includes("core:default"), "capability must include core:default");
  // (3) superset of the Phase 0-1 baseline.
  for (const required of baselinePermissions) {
    assert.ok(
      permissions.includes(required),
      `capability must retain baseline permission: ${required}`,
    );
  }
  // (2) every entry matches an explicit allow-list shape: core 默认、命令 allow-*，
  // 或官方插件命名空间权限（如 updater:default）。插件名逐一枚举，新增命名空间
  // 必须显式更新本契约（与安全面基线同一提交）。
  const shape = /^(core:default|allow-[a-z0-9-]+|(updater):[a-z0-9-]+)$/;
  for (const permission of permissions) {
    assert.match(permission, shape, `unexpected capability permission shape: ${permission}`);
  }
  // (4) no duplicate grants.
  assert.equal(
    new Set(permissions).size,
    permissions.length,
    "capability.permissions must not contain duplicates",
  );
  assert.doesNotMatch(JSON.stringify(capability), /shell:allow-(?:execute|spawn)|fs:|https?:\/\/|"\*"/i);
  assert.equal(pkg.main, undefined);
  assert.equal(pkg.scripts["dev:tauri-ui"], "vite --config vite.config.ts");
  assert.equal(pkg.scripts["build:tauri-ui"], "tsc -p tsconfig.tauri.json && vite build --config vite.config.ts");
  assert.equal(pkg.scripts["test:tauri-ui"], "vitest run --config vite.config.ts");
  assert.equal(pkg.scripts["tauri:dev"], "tauri dev");
  assert.equal(pkg.scripts["tauri:build"], "tauri build");
  assert.equal(pkg.scripts.dev, undefined);
  assert.equal(pkg.scripts.build, undefined);
  assert.equal(pkg.scripts.start, undefined);
  assert.equal(pkg.scripts["build:desktop"], undefined);
  assert.equal(pkg.scripts["make:windows"], undefined);
  assert.equal(pkg.scripts["start:windows"], undefined);
  assert.equal(pkg.scripts["test:desktop-shell"], undefined);
  assert.equal(pkg.scripts["test:obs"], undefined);
  assert.equal(pkg.dependencies?.next, undefined);
  assert.equal(pkg.dependencies?.["obs-websocket-js"], undefined);
  assert.equal(pkg.dependencies?.["@ricky0123/vad-web"], undefined);
  assert.equal(pkg.devDependencies?.electron, undefined);
  assert.equal(pkg.devDependencies?.["electron-builder"], undefined);
});
