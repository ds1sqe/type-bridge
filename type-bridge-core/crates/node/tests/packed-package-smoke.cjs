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

function verifyInstalledPackage(installedRoot) {
  const stage = path.dirname(path.dirname(path.dirname(installedRoot)));
  const requirePackage = createRequire(path.join(stage, "consumer.cjs"));
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
    "Iid",
    "PlanAuthority",
    "Database",
    "TransactionContext",
    "ensureDatabase",
    "long",
  ]) {
    assert.equal(typeof typeBridge[name], "function", `${name} must remain public`);
  }
  for (const name of [
    "AggregateSpec",
    "BooleanExpr",
    "ComparisonExpr",
    "NotExpr",
    "QueryExpr",
    "SortExpr",
    "TypedGroupByQuery",
    "TypedQuery",
    "TypedQueryError",
    "agg",
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
  assert.throws(
    () => requirePackage("@type-bridge/node/query-v2"),
    (error) => error?.code === "ERR_PACKAGE_PATH_NOT_EXPORTED",
  );
  const runtimeProjection = requirePackage("@type-bridge/node/runtime-projection");
  assert.deepEqual(Object.keys(runtimeProjection).sort(), [
    "InstalledRuntimeProjection",
    "ProxyConnection",
    "QueryCancellation",
    "QueryExecutionResourceLimits",
    "QueryLimits",
    "installGeneratedSchemaAuthority",
    "installRuntimeProjection",
    "projectedBatchDeleted",
    "projectedManagerNativeCall",
    "writeResult",
  ]);

  assert.equal(packageJson.main, "dist/public.js");
  assert.equal(packageJson.types, "dist/public.d.ts");
  assert.deepEqual(packageJson.exports["./runtime-projection"], {
    types: "./dist/runtime-projection.d.ts",
    require: "./dist/runtime-projection.js",
    default: "./dist/runtime-projection.js",
  });
  assert.equal(Object.hasOwn(packageJson.exports, "./typed"), false);

  const native = require(path.join(installedRoot, "dist", "native.js")).loadNative();
  assert.equal(typeof native.NodeRuntimeProjection, "function");
  assert.equal(typeof native.planAuthority, "function");
  for (const name of [
    "NodeDescriptorRegistry",
    "NodeDynamicEntityManager",
    "NodeDynamicRelationManager",
    "parseSchemaJson",
    "renderModelsJson",
  ]) {
    assert.equal(Object.hasOwn(native, name), false, `${name} must be absent natively`);
  }
}

function verifyPackedArtifact(artifactDirectory) {
  const artifacts = fs
    .readdirSync(artifactDirectory)
    .filter((name) => name.endsWith(".tgz"))
    .sort();
  assert.deepEqual(artifacts.length, 1, "exactly one packed artifact is required");
  const artifact = path.resolve(artifactDirectory, artifacts[0]);
  const npmExecPath = process.env.npm_execpath;
  assert.ok(npmExecPath, "npm_execpath is required; invoke this smoke through npm run");

  const stage = fs.mkdtempSync(path.join(os.tmpdir(), "type-bridge-node-packed-"));
  try {
    const install = spawnSync(
      process.execPath,
      [
        npmExecPath,
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
      { encoding: "utf8", shell: false },
    );
    assert.ifError(install.error);
    assert.equal(install.status, 0, install.stderr || install.stdout);

    fs.writeFileSync(path.join(stage, "consumer.ts"), `
import { Database, type TimeoutMilliseconds } from "@type-bridge/node";
import { QueryExecutionResourceLimits } from "@type-bridge/node/runtime-projection";
const signal: AbortSignal = new AbortController().signal;
const timeout: TimeoutMilliseconds = 30000;
const limits = new QueryExecutionResourceLimits({ timeoutMilliseconds: timeout });
declare const database: Database;
database.databaseExistsControlled({ timeoutMilliseconds: 0n });
void signal;
void limits;
`);
    fs.writeFileSync(path.join(stage, "tsconfig.json"), JSON.stringify({
      compilerOptions: {
        target: "ES2022", lib: ["ES2022"], types: [], module: "NodeNext",
        moduleResolution: "NodeNext", strict: true, skipLibCheck: false, noEmit: true,
      },
      include: ["consumer.ts"],
    }));
    const typecheck = spawnSync(process.execPath, [
      path.resolve(__dirname, "../node_modules/typescript/bin/tsc"),
      "--project", path.join(stage, "tsconfig.json"),
    ], { encoding: "utf8", shell: false });
    assert.ifError(typecheck.error);
    assert.equal(typecheck.status, 0, typecheck.stderr || typecheck.stdout);

    // Native modules remain locked for the lifetime of their Node process on
    // Windows. Load and inspect the installed package in a child so the DLL is
    // unloaded before this parent removes the isolated installation.
    const installedRoot = path.join(stage, "node_modules", "@type-bridge", "node");
    const verify = spawnSync(
      process.execPath,
      [__filename, "--installed-root", installedRoot],
      { encoding: "utf8", shell: false },
    );
    assert.ifError(verify.error);
    assert.equal(verify.status, 0, verify.stderr || verify.stdout);
  } finally {
    fs.rmSync(stage, {
      recursive: true,
      force: true,
      maxRetries: 5,
      retryDelay: 100,
    });
  }
}

const installedRoot = argument("--installed-root");
if (installedRoot === undefined) {
  const artifactDirectory = argument("--artifact-directory");
  assert.ok(artifactDirectory, "--artifact-directory is required");
  verifyPackedArtifact(artifactDirectory);
} else {
  verifyInstalledPackage(path.resolve(installedRoot));
}
