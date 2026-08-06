#!/usr/bin/env node
/**
 * Integration test runner for the Node package.
 *
 * Protocol:
 *   1. If TYPE_BRIDGE_NODE_NATIVE_PATH is set, trust the caller supplied a
 *      freshly built artifact and skip the build step.
 *   2. Otherwise, run `npm run build:native` to produce a fresh artifact.
 *      If that fails, hard-fail — do not fall through to the stale tmp/ binary.
 *   3. Compile integration sources to JavaScript with the documented Node 18+
 *      TypeScript toolchain.
 *   4. Run `node --test` over the compiled JavaScript files.
 *
 * Environment variables (all optional with defaults):
 *   TYPE_BRIDGE_NODE_NATIVE_PATH  — path to a pre-built .node artifact
 *   TYPEDB_ADDRESS                — default localhost:1730
 *   TYPE_BRIDGE_NODE_INTG_DATABASE — default type_bridge_test
 *   TYPEDB_USERNAME               — default admin
 *   TYPEDB_PASSWORD               — default password
 */

"use strict";

const { spawnSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "..");
const OUTPUT_ROOT = path.resolve(ROOT, "../../../tmp/node-integration");

// Step 1: Resolve the NAPI artifact.
const nativePath = process.env.TYPE_BRIDGE_NODE_NATIVE_PATH;
if (nativePath) {
  if (!fs.existsSync(nativePath)) {
    console.error(
      `[test:integration] TYPE_BRIDGE_NODE_NATIVE_PATH is set to '${nativePath}' but the file does not exist.`,
    );
    process.exit(1);
  }
  console.error(`[test:integration] Using pre-built artifact: ${nativePath}`);
} else {
  // Step 2: Build the native artifact.
  console.error("[test:integration] TYPE_BRIDGE_NODE_NATIVE_PATH not set — running build:native");
  const build = spawnSync("npm", ["run", "build:native"], {
    cwd: ROOT,
    stdio: "inherit",
    shell: true,
  });
  if (build.status !== 0) {
    console.error(
      "[test:integration] build:native failed — cannot proceed without a native artifact.",
    );
    process.exit(1);
  }
}

// Step 3: compile before execution. Node 18/20 cannot parse TypeScript-only
// syntax, and the supported runtime matrix must not depend on type stripping.
fs.rmSync(OUTPUT_ROOT, { recursive: true, force: true });
const compile = spawnSync("npm", ["run", "typecheck:integration"], {
  cwd: ROOT,
  stdio: "inherit",
  shell: true,
});
if (compile.status !== 0) {
  process.exit(compile.status ?? 1);
}
fs.copyFileSync(
  path.join(ROOT, "tests", "integration", "package.json"),
  path.join(OUTPUT_ROOT, "package.json"),
);

// Step 4: collect compiled tests explicitly; node --test does not expand globs
// consistently across supported platforms.
function* walkJs(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walkJs(full);
    } else if (entry.name.endsWith(".test.js")) {
      yield full;
    }
  }
}

const testFiles = Array.from(
  walkJs(path.join(OUTPUT_ROOT, "tests", "integration")),
);
if (testFiles.length === 0) {
  console.error("[test:integration] No compiled .test.js files found");
  process.exit(1);
}

console.error(`[test:integration] Running ${testFiles.length} test file(s)`);

const result = spawnSync(
  process.execPath,
  ["--test", ...testFiles],
  {
    cwd: ROOT,
    stdio: "inherit",
    env: {
      ...process.env,
      // Apply env defaults only when not already set.
      TYPEDB_ADDRESS: process.env.TYPEDB_ADDRESS ?? "localhost:1730",
      TYPE_BRIDGE_NODE_INTG_DATABASE:
        process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test",
      TYPEDB_USERNAME: process.env.TYPEDB_USERNAME ?? "admin",
      TYPEDB_PASSWORD: process.env.TYPEDB_PASSWORD ?? "password",
    },
  },
);

process.exit(result.status ?? 1);
