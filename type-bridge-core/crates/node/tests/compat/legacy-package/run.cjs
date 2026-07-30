#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const FIXTURE_ROOT = __dirname;
const PACKAGE_ROOT = path.resolve(FIXTURE_ROOT, "../../..");
const REPO_ROOT = path.resolve(PACKAGE_ROOT, "../../..");
const REPO_TMP = path.join(REPO_ROOT, "tmp");
const TSC_BIN = path.join(PACKAGE_ROOT, "node_modules", "typescript", "bin", "tsc");

function fail(message) {
  throw new Error(`[legacy-package-compat] ${message}`);
}

function run(command, args, options = {}) {
  const completed = spawnSync(command, args, {
    cwd: options.cwd,
    env: options.env,
    encoding: "utf8",
    stdio: options.capture ? "pipe" : "inherit",
  });
  if (completed.error) {
    throw completed.error;
  }
  if (completed.status !== 0) {
    const details = options.capture
      ? `\nstdout:\n${completed.stdout}\nstderr:\n${completed.stderr}`
      : "";
    fail(`${command} ${args.join(" ")} exited ${completed.status}${details}`);
  }
  return completed;
}

function copyFixture(consumerRoot, name) {
  fs.copyFileSync(path.join(FIXTURE_ROOT, name), path.join(consumerRoot, name));
}

function suppliedArtifact(argv) {
  if (argv.length === 0) {
    return null;
  }
  if (
    argv.length !== 2 ||
    !["--artifact", "--artifact-directory"].includes(argv[0])
  ) {
    fail(
      "usage: run.cjs [--artifact /path/to/prebuilt-package.tgz | " +
      "--artifact-directory /path/to/one-tarball-directory]",
    );
  }
  let artifact = path.resolve(process.cwd(), argv[1]);
  if (argv[0] === "--artifact-directory") {
    let entries;
    try {
      entries = fs
        .readdirSync(artifact, { withFileTypes: true })
        .filter((entry) => entry.isFile() && entry.name.endsWith(".tgz"));
    } catch (error) {
      fail(`could not read prebuilt artifact directory ${artifact}: ${error.message}`);
    }
    if (entries.length !== 1) {
      fail(
        `prebuilt artifact directory must contain exactly one .tgz file: ` +
        `${artifact} (found ${entries.length})`,
      );
    }
    artifact = path.join(artifact, entries[0].name);
  }
  let metadata;
  try {
    metadata = fs.statSync(artifact);
  } catch (error) {
    fail(`could not read prebuilt artifact ${artifact}: ${error.message}`);
  }
  if (!metadata.isFile() || !artifact.endsWith(".tgz")) {
    fail(`prebuilt artifact must be one .tgz file: ${artifact}`);
  }
  return artifact;
}

function listArtifactPaths(tarball) {
  const listing = run("tar", ["-tzf", tarball], { capture: true });
  const packedPaths = new Set();
  for (const rawEntry of listing.stdout.split(/\r?\n/u).filter(Boolean)) {
    const entry = rawEntry.endsWith("/") ? rawEntry.slice(0, -1) : rawEntry;
    if (entry === "package") {
      continue;
    }
    if (!entry.startsWith("package/")) {
      fail(`prebuilt artifact entry is outside package/: ${rawEntry}`);
    }
    const relative = entry.slice("package/".length);
    if (
      relative.length === 0 ||
      relative.includes("\\") ||
      path.posix.isAbsolute(relative) ||
      path.posix.normalize(relative) !== relative ||
      relative.split("/").includes("..")
    ) {
      fail(`prebuilt artifact has an unsafe path: ${rawEntry}`);
    }
    packedPaths.add(relative);
  }
  return packedPaths;
}

const prebuiltArtifact = suppliedArtifact(process.argv.slice(2));

if (prebuiltArtifact === null) {
  for (const required of [
    "dist/index.js",
    "dist/index.d.ts",
    "dist/native.js",
    "dist/query-v2-internals.js",
    "dist/query-v2-internals.d.ts",
    "dist/query-v2.js",
    "dist/query-v2.d.ts",
    "dist/typed/index.js",
    "dist/typed/index.d.ts",
  ]) {
    if (!fs.existsSync(path.join(PACKAGE_ROOT, required))) {
      fail(`missing ${required}; run the existing package build before this smoke`);
    }
  }
  const nativeArtifacts = fs
    .readdirSync(PACKAGE_ROOT)
    .filter((name) => name.endsWith(".node"));
  if (nativeArtifacts.length === 0) {
    fail("no package-root .node artifact exists; run the existing native build before this smoke");
  }
}
if (!fs.existsSync(TSC_BIN)) {
  fail("local TypeScript compiler is missing; prepare package dependencies outside this runner");
}

fs.mkdirSync(REPO_TMP, { recursive: true });
const tempRoot = fs.mkdtempSync(path.join(REPO_TMP, "node-legacy-package-"));
const keepTemp = process.env.TYPE_BRIDGE_KEEP_COMPAT_TMP === "1";

try {
  const packRoot = path.join(tempRoot, "pack");
  const unpackRoot = path.join(tempRoot, "unpack");
  const consumerRoot = path.join(tempRoot, "consumer");
  const installedRoot = path.join(
    consumerRoot,
    "node_modules",
    "@type-bridge",
    "node",
  );
  fs.mkdirSync(packRoot, { recursive: true });
  fs.mkdirSync(unpackRoot, { recursive: true });
  fs.mkdirSync(path.dirname(installedRoot), { recursive: true });
  fs.mkdirSync(consumerRoot, { recursive: true });

  let tarball = prebuiltArtifact;
  let packedFilename;
  let packedPaths;
  if (tarball === null) {
    const packed = run(
      "npm",
      ["pack", "--ignore-scripts", "--json", "--pack-destination", packRoot],
      { cwd: PACKAGE_ROOT, capture: true },
    );
    let packInfo;
    try {
      const report = JSON.parse(packed.stdout);
      packInfo = Array.isArray(report) ? report[0] : Object.values(report)[0];
    } catch (error) {
      fail(`could not parse npm pack JSON: ${error.message}\n${packed.stdout}`);
    }
    if (!packInfo || typeof packInfo.filename !== "string" || !Array.isArray(packInfo.files)) {
      fail("npm pack did not return one artifact with a file manifest");
    }
    tarball = path.join(packRoot, packInfo.filename);
    packedFilename = packInfo.filename;
    packedPaths = new Set(packInfo.files.map((entry) => entry.path));
  } else {
    packedFilename = path.basename(tarball);
    packedPaths = listArtifactPaths(tarball);
  }

  for (const required of [
    "dist/index.js",
    "dist/index.d.ts",
    "dist/native.js",
    "dist/query-v2-internals.js",
    "dist/query-v2-internals.d.ts",
    "dist/query-v2.js",
    "dist/query-v2.d.ts",
    "dist/typed/index.js",
    "dist/typed/index.d.ts",
  ]) {
    if (!packedPaths.has(required)) {
      fail(`packed artifact omitted ${required}`);
    }
  }
  if (![...packedPaths].some((entry) => entry.endsWith(".node"))) {
    fail("packed artifact omitted the native module");
  }
  for (const forbiddenPrefix of ["typescript/", "tests/", "src/"]) {
    if ([...packedPaths].some((entry) => entry.startsWith(forbiddenPrefix))) {
      fail(`packed artifact unexpectedly contains source-only ${forbiddenPrefix}`);
    }
  }

  run("tar", ["-xzf", tarball, "-C", unpackRoot]);
  const extractedRoot = path.join(unpackRoot, "package");
  if (!fs.existsSync(extractedRoot)) {
    fail(`tarball did not contain the expected package/ root: ${tarball}`);
  }
  fs.renameSync(extractedRoot, installedRoot);

  for (const name of [
    "package.json",
    "tsconfig.json",
    "consumer.ts",
    "typed-consumer.ts",
    "runtime-probe.cjs",
  ]) {
    copyFixture(consumerRoot, name);
  }

  const isolatedEnv = { ...process.env };
  delete isolatedEnv.NODE_PATH;
  delete isolatedEnv.TYPE_BRIDGE_NODE_NATIVE_PATH;
  isolatedEnv.TYPE_BRIDGE_EXPECTED_PACKAGE_ROOT = installedRoot;
  isolatedEnv.TYPE_BRIDGE_SOURCE_PACKAGE_ROOT = PACKAGE_ROOT;

  run(process.execPath, [TSC_BIN, "--project", "tsconfig.json", "--pretty", "false"], {
    cwd: consumerRoot,
    env: isolatedEnv,
  });
  run(process.execPath, [path.join(consumerRoot, "dist-consumer", "consumer.js")], {
    cwd: consumerRoot,
    env: isolatedEnv,
  });
  run(process.execPath, [path.join(consumerRoot, "dist-consumer", "typed-consumer.js")], {
    cwd: consumerRoot,
    env: isolatedEnv,
  });
  run(process.execPath, [path.join(consumerRoot, "runtime-probe.cjs")], {
    cwd: consumerRoot,
    env: isolatedEnv,
  });

  process.stdout.write(
    `[legacy-package-compat] packed consumer passed (${packedFilename}; ${os.platform()}/${os.arch()})\n`,
  );
} finally {
  if (keepTemp) {
    process.stderr.write(`[legacy-package-compat] preserved ${tempRoot}\n`);
  } else {
    fs.rmSync(tempRoot, { recursive: true, force: true });
  }
}
