"use strict";

/** Execute the exact packed artifact as an isolated generated-only consumer. */

const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const { createRequire } = require("node:module");
const os = require("node:os");
const path = require("node:path");

function argument(name) {
  const index = process.argv.indexOf(name);
  return index === -1 ? undefined : process.argv[index + 1];
}

const artifactDirectory = argument("--artifact-directory");
assert.ok(artifactDirectory, "--artifact-directory is required");
const artifacts = fs
  .readdirSync(artifactDirectory)
  .filter((name) => name.endsWith(".tgz"))
  .sort();
assert.deepEqual(artifacts.length, 1, "exactly one packed artifact is required");
const artifact = path.resolve(artifactDirectory, artifacts[0]);

const stage = fs.mkdtempSync(path.join(os.tmpdir(), "type-bridge-node-packed-"));
try {
  const install = spawnSync(
    "npm",
    [
      "install",
      "--ignore-scripts",
      "--no-audit",
      "--no-fund",
      "--no-package-lock",
      "--no-save",
      "--prefix",
      stage,
      artifact,
    ],
    { encoding: "utf8" },
  );
  assert.equal(install.status, 0, install.stderr || install.stdout);

  const requirePackage = createRequire(path.join(stage, "consumer.cjs"));
  const installedRoot = path.join(stage, "node_modules", "@type-bridge", "node");
  const resolvedRoot = fs.realpathSync(requirePackage.resolve("@type-bridge/node"));
  assert.ok(
    resolvedRoot.startsWith(`${fs.realpathSync(installedRoot)}${path.sep}`),
    `package root escaped the isolated install: ${resolvedRoot}`,
  );

  const packageJson = JSON.parse(
    fs.readFileSync(path.join(installedRoot, "package.json"), "utf8"),
  );
  const typeBridge = requirePackage("@type-bridge/node");
  for (const name of [
    "QueryV2Authority",
    "RustDatabase",
    "RustTransactionContext",
    "TypedQuery",
    "ensureDatabase",
    "long",
  ]) {
    assert.equal(typeof typeBridge[name], "function", `${name} must remain public`);
  }
  for (const name of [
    "DescriptorRegistry",
    "Entity",
    "Relation",
    "RustDynamicEntityManager",
    "RustDynamicRelationManager",
    "attr",
    "field",
    "generateModels",
    "parseSchema",
    "role",
  ]) {
    assert.equal(Object.hasOwn(typeBridge, name), false, `${name} must be absent`);
  }
  assert.deepEqual(typeBridge.long(9223372036854775807n), {
    value_type: "long",
    value: "9223372036854775807",
  });

  assert.throws(
    () => requirePackage("@type-bridge/node/typed"),
    (error) => error?.code === "ERR_PACKAGE_PATH_NOT_EXPORTED",
  );
  const queryV2 = requirePackage("@type-bridge/node/query-v2");
  assert.equal(queryV2.QueryV2Authority, typeBridge.QueryV2Authority);

  assert.equal(packageJson.main, "dist/public.js");
  assert.equal(packageJson.types, "dist/public.d.ts");
  assert.equal(Object.hasOwn(packageJson.exports, "./typed"), false);

  const native = require(path.join(installedRoot, "dist", "native.js")).loadNative();
  assert.equal(typeof native.NodeRuntimeProjection, "function");
  assert.equal(typeof native.queryV2Authority, "function");
  for (const name of [
    "NodeDescriptorRegistry",
    "NodeDynamicEntityManager",
    "NodeDynamicRelationManager",
    "parseSchemaJson",
    "renderModelsJson",
  ]) {
    assert.equal(Object.hasOwn(native, name), false, `${name} must be absent natively`);
  }
} finally {
  fs.rmSync(stage, { recursive: true, force: true });
}
