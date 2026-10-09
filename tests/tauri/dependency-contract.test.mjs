import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("frontend and Rust foundation dependencies are locked", async () => {
  const pkg = JSON.parse(await readFile("package.json", "utf8"));
  const cargo = await readFile("src-tauri/Cargo.toml", "utf8");

  assert.equal(pkg.dependencies["@tauri-apps/api"], "2.12.1");
  assert.equal(pkg.devDependencies["@tauri-apps/cli"], "2.12.1");
  assert.equal(pkg.devDependencies.vite, "8.3.2");
  assert.equal(pkg.dependencies.react, "19.3.0");
  assert.equal(pkg.dependencies["react-dom"], "19.3.0");
  assert.match(cargo, /tauri\s*=\s*\{\s*version\s*=\s*"=?2/);
  assert.match(cargo, /keyring/);
  assert.match(cargo, /rusqlite/);
  assert.match(cargo, /ts-rs/);
});

test("page-shell routing dependency is locked and forbidden libraries are absent", async () => {
  const pkg = JSON.parse(await readFile("package.json", "utf8"));

  // wouter must be a production dependency at an exact version
  assert.ok(pkg.dependencies.wouter, "wouter must be in dependencies");
  assert.doesNotMatch(pkg.dependencies.wouter, /[\^~><]/, "wouter version must be exact");
  assert.equal(pkg.devDependencies?.wouter, undefined, "wouter must not be in devDependencies");

  // test:tauri must include vitest (test:tauri-ui)
  assert.ok(pkg.scripts["test:tauri"].includes("test:tauri-ui"), "test:tauri must run vitest");

  // Forbidden routing/state libraries
  const forbidden = ["react-router", "react-router-dom", "@tanstack/react-router", "zustand", "jotai", "redux", "@reduxjs/toolkit", "mobx", "valtio", "recoil"];
  for (const lib of forbidden) {
    assert.equal(pkg.dependencies?.[lib], undefined, `${lib} must not be in dependencies`);
    assert.equal(pkg.devDependencies?.[lib], undefined, `${lib} must not be in devDependencies`);
  }
});
