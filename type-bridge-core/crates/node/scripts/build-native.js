#!/usr/bin/env node
/**
 * Build the NAPI cdylib and copy it to the package root under the
 * platform-triple filename the native loader probes for.
 *
 * `napi build` is deliberately not used: this script only compiles the crate
 * and copies the artifact. The TypeScript surface (including the native loader)
 * is compiled separately via `build:types` (tsc).
 *
 * The crate lives in a cargo workspace, so the cdylib lands in the workspace
 * target dir (../../target/release), not a crate-local one.
 */

"use strict";

const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const CRATE_ROOT = path.resolve(__dirname, "..");

function platformArtifacts() {
  const arch = process.arch;
  switch (process.platform) {
    case "linux":
      if (arch === "x64") return { lib: "libtype_bridge_node.so", triple: "linux-x64-gnu" };
      if (arch === "arm64") return { lib: "libtype_bridge_node.so", triple: "linux-arm64-gnu" };
      break;
    case "darwin":
      return {
        lib: "libtype_bridge_node.dylib",
        triple: arch === "arm64" ? "darwin-arm64" : "darwin-x64",
      };
    case "win32":
      return {
        lib: "type_bridge_node.dll",
        triple: arch === "arm64" ? "win32-arm64-msvc" : "win32-x64-msvc",
      };
  }
  console.error(`[build:native] Unsupported platform/arch: ${process.platform}/${arch}`);
  process.exit(1);
}

const { lib, triple } = platformArtifacts();

const build = spawnSync(
  "cargo",
  ["build", "-p", "type-bridge-node", "--release"],
  { cwd: CRATE_ROOT, stdio: "inherit" },
);
if (build.status !== 0) {
  console.error("[build:native] cargo build failed.");
  process.exit(build.status ?? 1);
}

const targetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(process.env.CARGO_TARGET_DIR)
  : path.resolve(CRATE_ROOT, "../../target");
const source = path.join(targetDir, "release", lib);
const dest = path.join(CRATE_ROOT, `type_bridge_node.${triple}.node`);

if (!fs.existsSync(source)) {
  console.error(`[build:native] Expected artifact not found: ${source}`);
  process.exit(1);
}
fs.copyFileSync(source, dest);
console.error(`[build:native] ${path.relative(CRATE_ROOT, source)} -> ${path.basename(dest)}`);
