#!/usr/bin/env node
"use strict";

const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "..");
const OUTPUT_ROOT = path.resolve(ROOT, "../../../tmp/node-projection-integration");
const TEST_FILE = path.join(
  OUTPUT_ROOT,
  "tests",
  "projection-integration",
  "generated-package-live.test.js",
);

const nativePath = process.env.TYPE_BRIDGE_NODE_NATIVE_PATH;
if (nativePath !== undefined && !fs.existsSync(nativePath)) {
  console.error(
    `[test:projection-integration] TYPE_BRIDGE_NODE_NATIVE_PATH does not exist: ${nativePath}`,
  );
  process.exit(1);
}

fs.rmSync(OUTPUT_ROOT, { recursive: true, force: true });
const compile = spawnSync("npm", ["run", "typecheck:projection-integration"], {
  cwd: ROOT,
  stdio: "inherit",
  shell: true,
});
if (compile.status !== 0) {
  process.exit(compile.status ?? 1);
}
fs.copyFileSync(
  path.join(ROOT, "tests", "projection-integration", "package.json"),
  path.join(OUTPUT_ROOT, "package.json"),
);
if (!fs.existsSync(TEST_FILE)) {
  console.error(`[test:projection-integration] Compiled test is missing: ${TEST_FILE}`);
  process.exit(1);
}

const result = spawnSync(
  process.execPath,
  ["--test", "--test-concurrency=1", TEST_FILE],
  {
    cwd: ROOT,
    stdio: "inherit",
    env: process.env,
  },
);
process.exit(result.status ?? 1);
