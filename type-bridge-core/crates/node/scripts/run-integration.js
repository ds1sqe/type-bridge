#!/usr/bin/env node
/**
 * Integration test runner for the Node package.
 *
 * Protocol:
 *   1. If TYPE_BRIDGE_NODE_NATIVE_PATH is set, trust the caller supplied a
 *      freshly built artifact and skip the build step.
 *   2. Otherwise, run `npm run build:native` to produce a fresh artifact.
 *      If that fails, hard-fail — do not fall through to the stale tmp/ binary.
 *   3. Run `node --test` with Node's built-in type-stripping over all
 *      .test.ts files under tests/integration/.
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

// Step 3: Run node --test over all .test.ts files.
// Collect files explicitly so the runner receives individual paths — Node
// --test does not perform shell glob expansion on all platforms.
// Collect matching files synchronously via fs.readdirSync recursion.
function* walkTs(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      yield* walkTs(full);
    } else if (entry.name.endsWith(".test.ts")) {
      yield full;
    }
  }
}

const testFiles = Array.from(walkTs(path.join(ROOT, "tests", "integration")));
if (testFiles.length === 0) {
  console.error("[test:integration] No .test.ts files found under tests/integration/");
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
