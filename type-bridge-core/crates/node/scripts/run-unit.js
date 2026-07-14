#!/usr/bin/env node
/** Run compiled Node unit tests against the package's freshly built addon. */

"use strict";

const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "..");
const TEST_ROOT = path.resolve(ROOT, "../../../tmp/node-unit/tests/unit");

function platformTriple() {
  const arch = process.arch;
  if (process.platform === "linux" && arch === "x64") return "linux-x64-gnu";
  if (process.platform === "linux" && arch === "arm64") return "linux-arm64-gnu";
  if (process.platform === "darwin" && arch === "x64") return "darwin-x64";
  if (process.platform === "darwin" && arch === "arm64") return "darwin-arm64";
  if (process.platform === "win32" && arch === "x64") return "win32-x64-msvc";
  if (process.platform === "win32" && arch === "arm64") return "win32-arm64-msvc";
  throw new Error(`Unsupported platform/arch: ${process.platform}/${arch}`);
}

function nativeArtifact() {
  const explicit = process.env.TYPE_BRIDGE_NODE_NATIVE_PATH;
  const candidate = explicit
    ? path.resolve(explicit)
    : path.join(ROOT, `type_bridge_node.${platformTriple()}.node`);
  if (!fs.existsSync(candidate) || !fs.statSync(candidate).isFile()) {
    throw new Error(
      `Native unit-test artifact does not exist: ${candidate}. Run npm run build:native first.`,
    );
  }
  return candidate;
}

function unitTests() {
  if (!fs.existsSync(TEST_ROOT) || !fs.statSync(TEST_ROOT).isDirectory()) {
    throw new Error(`Compiled unit-test directory does not exist: ${TEST_ROOT}`);
  }
  const tests = fs.readdirSync(TEST_ROOT)
    .filter((name) => name.endsWith(".test.js"))
    .sort()
    .map((name) => path.join(TEST_ROOT, name));
  if (tests.length === 0) {
    throw new Error(`No compiled *.test.js files found in ${TEST_ROOT}`);
  }
  return tests;
}

let artifact;
let tests;
try {
  artifact = nativeArtifact();
  tests = unitTests();
} catch (error) {
  console.error(`[test:unit] ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
}

const result = spawnSync(process.execPath, ["--test", ...tests], {
  cwd: ROOT,
  stdio: "inherit",
  env: {
    ...process.env,
    TYPE_BRIDGE_NODE_NATIVE_PATH: artifact,
  },
});

process.exit(result.status ?? 1);
