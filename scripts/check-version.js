#!/usr/bin/env node
/**
 * T6.6 版本一致性校验脚本。
 * 检查 Cargo.toml、tauri.conf.json、package.json 三处版本号一致。
 * 用法：node scripts/check-version.js
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function readJSON(p) {
  return JSON.parse(fs.readFileSync(path.join(root, p), "utf-8"));
}

function readTomlVersion(p) {
  const txt = fs.readFileSync(path.join(root, p), "utf-8");
  const m = txt.match(/^version\s*=\s*"([^"]+)"/m);
  if (m) return m[1];
  // version.workspace = true → 读 workspace 版本
  if (/^version\.workspace\s*=\s*true/m.test(txt)) {
    const wsTxt = fs.readFileSync(path.join(root, "Cargo.toml"), "utf-8");
    const wsM = wsTxt.match(/^version\s*=\s*"([^"]+)"/m);
    return wsM ? wsM[1] + " (workspace)" : null;
  }
  return null;
}

const results = [];

const pkg = readJSON("package.json");
results.push(["package.json", pkg.version]);

const tauri = readJSON("src-tauri/tauri.conf.json");
results.push(["tauri.conf.json", tauri.version]);

const wsVersion = readTomlVersion("Cargo.toml");
results.push(["Cargo.toml (workspace)", wsVersion]);

const tauriCargo = readTomlVersion("src-tauri/Cargo.toml");
results.push(["src-tauri/Cargo.toml", tauriCargo]);

const versions = new Set(results.map(([, v]) => v.replace(" (workspace)", "")));
if (versions.size > 1) {
  console.error("版本不一致：");
  for (const [file, ver] of results) {
    console.error(`  ${file}: ${ver}`);
  }
  process.exit(1);
} else {
  console.log(`版本一致: ${results[0][1]}`);
  for (const [file, ver] of results) {
    console.log(`  ${file}: ${ver}`);
  }
}
