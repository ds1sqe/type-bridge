"use strict";

const assert = require("node:assert/strict");
const { execSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const packageJson = require("../package.json");
const typeBridge = require("../");

const retainedFunctions = [
  "QueryV2Authority",
  "QueryV2Error",
  "RustDatabase",
  "RustTransactionContext",
  "TypedGroupByQuery",
  "TypedQuery",
  "TypedQueryError",
  "boolean",
  "date",
  "datetime",
  "datetimetz",
  "decimal",
  "double",
  "duration",
  "ensureDatabase",
  "long",
  "longFromNumberUnsafe",
  "queryV2ExecuteLocal",
  "queryV2PrepareRemote",
  "queryV2RemoteCapabilities",
  "string",
];
for (const name of retainedFunctions) {
  assert.equal(typeof typeBridge[name], "function", `${name} must remain public`);
}

assert.deepEqual(typeBridge.long(9223372036854775807n), {
  value_type: "long",
  value: "9223372036854775807",
});
assert.throws(() => typeBridge.long(1), /bigint/);

const removedAuthoringNames = [
  "Attribute",
  "AttributeFlags",
  "Card",
  "DescriptorRegistry",
  "Doc",
  "Entity",
  "Flag",
  "Key",
  "Marshalling",
  "Meta",
  "Relation",
  "RustDynamicEntityManager",
  "RustDynamicRelationManager",
  "TypeFlags",
  "TypeNameCase",
  "Unique",
  "attr",
  "buildRolePlayers",
  "entityManagerFor",
  "field",
  "formatTypeName",
  "generateDefineBlock",
  "generateModels",
  "generateModelsForTarget",
  "loadNative",
  "parseSchema",
  "relationManagerFor",
  "resolveFlags",
  "role",
];
for (const name of removedAuthoringNames) {
  assert.equal(
    Object.hasOwn(typeBridge, name),
    false,
    `${name} must be absent from the generated-only package root`,
  );
}

const nativeArtifact = process.env.TYPE_BRIDGE_NODE_NATIVE_PATH ?? fs
  .readdirSync(path.resolve(__dirname, ".."))
  .filter((name) => name.endsWith(".node"))
  .sort()[0];
assert.ok(nativeArtifact, "package smoke requires a built native artifact");
const native = require(path.isAbsolute(nativeArtifact)
  ? nativeArtifact
  : path.resolve(__dirname, "..", nativeArtifact));
for (const name of [
  "NodeDescriptorRegistry",
  "NodeDynamicEntityManager",
  "NodeDynamicRelationManager",
  "generateDefineBlockJson",
  "generatedDeclaredDescriptorsJson",
  "normalizeAggregatesJson",
  "normalizeAttributeValueJson",
  "normalizeEntityAttributesJson",
  "normalizeFiltersJson",
  "normalizeRelationAttributesJson",
  "normalizeRelationFiltersJson",
  "normalizeRelationWriteBatchJson",
  "normalizeRolePlayersJson",
  "parseSchemaJson",
  "renderModelsJson",
  "revalidateMatchDiagnostic",
  "validateMatchOrderTermCount",
]) {
  assert.equal(
    Object.hasOwn(native, name),
    false,
    `${name} must be absent from the native addon`,
  );
}
for (const name of [
  "NodeRuntimeProjection",
  "connectRustDatabase",
  "ensureRustDatabase",
  "queryV2Authority",
]) {
  assert.equal(typeof native[name], "function", `${name} must remain native`);
}
assert.throws(
  () => new native.NodeMatchSessionHandle(),
  /contains no `constructor`|not a constructor/i,
  "match sessions must come from a verified runtime projection",
);
assert.equal(
  Object.hasOwn(native.NodeRustDatabase.prototype, "entityManagerJson"),
  false,
);
assert.equal(
  Object.hasOwn(native.NodeRustDatabase.prototype, "relationManagerJson"),
  false,
);
assert.equal(
  Object.hasOwn(native.NodeRustTransactionContext.prototype, "entityManagerJson"),
  false,
);
assert.equal(
  Object.hasOwn(native.NodeRustTransactionContext.prototype, "relationManagerJson"),
  false,
);

assert.equal(
  Object.hasOwn(typeBridge.RustDatabase.prototype, "entityManager"),
  false,
  "RustDatabase must not construct descriptor-driven entity managers",
);
assert.equal(
  Object.hasOwn(typeBridge.RustDatabase.prototype, "relationManager"),
  false,
  "RustDatabase must not construct descriptor-driven relation managers",
);
assert.equal(
  Object.hasOwn(typeBridge.RustTransactionContext.prototype, "entityManager"),
  false,
  "RustTransactionContext must not construct descriptor-driven entity managers",
);
assert.equal(
  Object.hasOwn(typeBridge.RustTransactionContext.prototype, "relationManager"),
  false,
  "RustTransactionContext must not construct descriptor-driven relation managers",
);

assert.equal(packageJson.main, "dist/public.js");
assert.equal(packageJson.types, "dist/public.d.ts");
assert.equal(packageJson.exports["."].default, "./dist/public.js");
assert.equal(packageJson.exports["."].types, "./dist/public.d.ts");
for (const forbiddenSubpath of [
  "./attribute",
  "./flags",
  "./generator",
  "./index",
  "./manager",
  "./model",
  "./native",
  "./parser",
  "./typed",
]) {
  assert.equal(
    Object.hasOwn(packageJson.exports, forbiddenSubpath),
    false,
    `${forbiddenSubpath} must not be a package export`,
  );
}

const queryV2 = require("@type-bridge/node/query-v2");
assert.deepEqual(Object.keys(queryV2).sort(), [
  "AuthoredQueryInvocation",
  "AuthoredQueryPlan",
  "QueryPlanBuilder",
  "QueryV2Authority",
]);
assert.equal(queryV2.QueryV2Authority, typeBridge.QueryV2Authority);

const packed = JSON.parse(execSync("npm pack --dry-run --json", { encoding: "utf8" }));
const packInfo = Array.isArray(packed) ? packed[0] : Object.values(packed)[0];
assert.ok(packInfo && Array.isArray(packInfo.files), "npm pack must return a file manifest");
const packedFiles = packInfo.files.map((file) => file.path);
for (const required of [
  "dist/public.js",
  "dist/public.d.ts",
  "dist/query-v2.js",
  "dist/query-v2.d.ts",
  "dist/runtime-projection.js",
  "dist/runtime-projection.d.ts",
  "THIRD_PARTY_NOTICES.md",
]) {
  assert.ok(packedFiles.includes(required), `tarball must include ${required}`);
}
assert.ok(packedFiles.some((file) => file.endsWith(".node")), "tarball must include native code");
assert.ok(
  !packedFiles.some((file) => file.startsWith("dist/typescript/")),
  "tarball must not include stale duplicate TypeScript outputs",
);
for (const removedModule of [
  "attribute",
  "codec",
  "flags",
  "generator",
  "iid",
  "manager",
  "model",
  "parser",
  "typed",
]) {
  assert.ok(
    !packedFiles.some((file) =>
      file === `dist/${removedModule}.js` ||
      file === `dist/${removedModule}.d.ts` ||
      file.startsWith(`dist/${removedModule}/`)
    ),
    `tarball must not contain removed authoring module ${removedModule}`,
  );
}
