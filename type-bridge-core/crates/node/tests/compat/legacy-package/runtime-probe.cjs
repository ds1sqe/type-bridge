"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");

const expectedPackageRoot = fs.realpathSync(process.env.TYPE_BRIDGE_EXPECTED_PACKAGE_ROOT);
const sourcePackageRoot = fs.realpathSync(process.env.TYPE_BRIDGE_SOURCE_PACKAGE_ROOT);
const resolvedEntry = fs.realpathSync(require.resolve("@type-bridge/node"));
const resolvedTypedEntry = fs.realpathSync(require.resolve("@type-bridge/node/typed"));
const resolvedRuntimeProjectionEntry = fs.realpathSync(
  require.resolve("@type-bridge/node/runtime-projection"),
);

function isWithin(candidate, root) {
  const relative = path.relative(root, candidate);
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative));
}

assert.ok(
  isWithin(resolvedEntry, expectedPackageRoot),
  `root import escaped the packed install: ${resolvedEntry}`,
);
assert.ok(
  !isWithin(resolvedEntry, sourcePackageRoot),
  `root import leaked to the source package: ${resolvedEntry}`,
);
assert.ok(
  isWithin(resolvedTypedEntry, expectedPackageRoot),
  `typed import escaped the packed install: ${resolvedTypedEntry}`,
);
assert.ok(
  !isWithin(resolvedTypedEntry, sourcePackageRoot),
  `typed import leaked to the source package: ${resolvedTypedEntry}`,
);
assert.ok(
  isWithin(resolvedRuntimeProjectionEntry, expectedPackageRoot),
  `runtime-projection import escaped the packed install: ${resolvedRuntimeProjectionEntry}`,
);
assert.ok(
  !isWithin(resolvedRuntimeProjectionEntry, sourcePackageRoot),
  `runtime-projection import leaked to the source package: ${resolvedRuntimeProjectionEntry}`,
);
assert.equal(
  fs.lstatSync(expectedPackageRoot).isSymbolicLink(),
  false,
  "packed package must be extracted, not linked to the source tree",
);
assert.equal(
  fs.existsSync(path.join(expectedPackageRoot, "typescript")),
  false,
  "consumer must not rely on unpublished TypeScript sources",
);
assert.equal(
  fs.existsSync(path.join(expectedPackageRoot, "tests")),
  false,
  "consumer must not rely on unpublished tests",
);

const typeBridge = require("@type-bridge/node");
const typedBridge = require("@type-bridge/node/typed");
const runtimeProjection = require("@type-bridge/node/runtime-projection");
assert.equal(typeof typeBridge.TypedQuery, "function", "root TypedQuery export must load");
assert.equal(typeof typeBridge.Entity, "function", "root model export must load");
assert.equal(typeof typeBridge.loadNative, "function", "root native loader must load");
assert.equal(
  typeBridge.QuerySession,
  undefined,
  "new typed facade symbols must not leak into the legacy root",
);
assert.equal(typeof typedBridge.QuerySession, "function", "typed QuerySession export must load");
assert.equal(typeof typedBridge.references, "function", "typed references export must load");
assert.equal(
  typeof runtimeProjection.installRuntimeProjection,
  "function",
  "runtime-projection installer export must load",
);
const installedManifest = JSON.parse(
  fs.readFileSync(path.join(expectedPackageRoot, "package.json"), "utf8"),
);
assert.deepEqual(
  Object.keys(installedManifest.exports),
  [".", "./typed", "./runtime-projection"],
  "packed artifact must preserve the legacy root and publish both additive subpaths",
);
assert.deepEqual(
  installedManifest.exports["."],
  {
    types: "./dist/index.d.ts",
    require: "./dist/index.js",
    default: "./dist/index.js",
  },
  "legacy root export targets must remain unchanged",
);
assert.deepEqual(
  installedManifest.exports["./typed"],
  {
    types: "./dist/typed/index.d.ts",
    require: "./dist/typed/index.js",
    default: "./dist/typed/index.js",
  },
  "typed export must resolve only to packed dist artifacts",
);
assert.deepEqual(
  installedManifest.exports["./runtime-projection"],
  {
    types: "./dist/runtime-projection.d.ts",
    require: "./dist/runtime-projection.js",
    default: "./dist/runtime-projection.js",
  },
  "runtime-projection export must resolve only to packed dist artifacts",
);

const native = typeBridge.loadNative();
assert.equal(typeof native, "object", "native loader must return the packed N-API module");
assert.equal(
  typeof native.NodeDescriptorRegistry,
  "function",
  "packed native module must expose NodeDescriptorRegistry",
);
const registry = new native.NodeDescriptorRegistry();
assert.equal(typeof registry.snapshotJson, "function", "native class must be constructible");
assert.equal(
  typeof native.NodeMatchSessionHandle,
  "function",
  "packed native module must expose opaque match handles",
);
assert.equal(
  typeof native.NodeValidatedMatchResultHandle,
  "function",
  "packed native module must expose the opaque validated-result symbol",
);
assert.equal(
  typeof native.NodeValidatedThingHandle,
  "function",
  "packed native module must expose the opaque validated-thing symbol",
);
assert.throws(
  () => new native.NodeValidatedMatchResultHandle(),
  /contains no `constructor`/,
  "validated results must remain nonconstructible",
);
assert.throws(
  () => new native.NodeValidatedThingHandle(),
  /contains no `constructor`/,
  "validated things must remain nonconstructible",
);

for (const loadedPath of Object.keys(require.cache)) {
  let realLoadedPath;
  try {
    realLoadedPath = fs.realpathSync(loadedPath);
  } catch {
    continue;
  }
  assert.ok(
    !isWithin(realLoadedPath, sourcePackageRoot),
    `runtime loaded source-tree module instead of packed artifact: ${realLoadedPath}`,
  );
}
