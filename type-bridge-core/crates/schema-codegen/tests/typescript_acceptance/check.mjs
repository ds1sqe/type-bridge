import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const CORE = resolve(HERE, "../../../..");
const ROOT = resolve(CORE, "..");
const STAGE = resolve(CORE, "target/schema-codegen-typescript-acceptance");
const GENERATED = resolve(STAGE, "generated_package");
const BLOCKERS = resolve(STAGE, "generated_blockers");
const ORDERED = resolve(STAGE, "generated_ordered");
const PLAN_IDIOMS = resolve(STAGE, "generated_plan_idioms");
const PLAN_NAMES = resolve(STAGE, "generated_plan_names");
const FOREIGN = resolve(STAGE, "generated_foreign");
const PROJECTED = resolve(STAGE, "generated_projected");
const PROJECTED_FOREIGN = resolve(STAGE, "generated_projected_foreign");
const NODE_PACKAGE = resolve(CORE, "crates/node");
const TSC = resolve(NODE_PACKAGE, "node_modules/.bin/tsc");
if (!existsSync(TSC)) {
  throw new Error(`required tsc is unavailable: ${TSC}; run npm ci in ${NODE_PACKAGE}`);
}
const DOCUMENTED_EXAMPLES = resolve(
  ROOT,
  "tests/contracts/typed_query/typescript/documented_examples.ts",
);

function command(program, args, cwd = ROOT, environment = process.env) {
  const completed = spawnSync(program, args, {
    cwd,
    encoding: "utf8",
    env: environment,
    stdio: "pipe",
  });
  if (completed.status !== 0) {
    throw new Error(
      `${program} ${args.join(" ")} returned ${completed.status}: ${completed.error?.message ?? completed.signal ?? "process failure"}\nstdout:\n${completed.stdout}\nstderr:\n${completed.stderr}`,
    );
  }
}

for (const fixture of [
  "blockers_positive.ts",
  "positive.ts",
  "negative.ts",
  "ordered_batch_positive.ts",
  "ordered_batch_negative.ts",
  "blockers_check.mjs",
  "runtime_check.mjs",
  "projected_parity_check.mjs",
]) {
  const source = readFileSync(resolve(HERE, fixture), "utf8");
  for (const forbidden of ["as unknown as", "@ts-ignore"]) {
    if (source.includes(forbidden)) {
      throw new Error(`${fixture} contains forbidden escape ${forbidden}`);
    }
  }
}
for (const forbidden of ["as unknown as", "@ts-ignore"]) {
  if (readFileSync(DOCUMENTED_EXAMPLES, "utf8").includes(forbidden)) {
    throw new Error(`documented_examples.ts contains forbidden escape ${forbidden}`);
  }
}
for (const fixture of [
  "blockers_positive.ts",
  "positive.ts",
  "ordered_batch_positive.ts",
  "blockers_check.mjs",
  "runtime_check.mjs",
]) {
  const source = readFileSync(resolve(HERE, fixture), "utf8");
  for (const forbidden of ["PlanAuthority", "declared-schema.json", "readFileSync"]) {
    if (source.includes(forbidden)) {
      throw new Error(`${fixture} bypasses generated embedded authority with ${forbidden}`);
    }
  }
}
for (const forbidden of ["PlanAuthority", "declared-schema.json", "readFileSync"]) {
  if (readFileSync(DOCUMENTED_EXAMPLES, "utf8").includes(forbidden)) {
    throw new Error(`documented_examples.ts bypasses generated embedded authority with ${forbidden}`);
  }
}
for (const fixture of ["negative.ts", "ordered_batch_negative.ts"]) {
  if (!readFileSync(resolve(HERE, fixture), "utf8").includes("@ts-expect-error")) {
    throw new Error(`${fixture} has no @ts-expect-error assertions`);
  }
}

rmSync(STAGE, { recursive: true, force: true });
mkdirSync(STAGE, { recursive: true });
command("npm", ["run", "build"], NODE_PACKAGE);
command("cargo", [
  "run",
  "--quiet",
  "--manifest-path",
  resolve(CORE, "Cargo.toml"),
  "--package",
  "type-bridge-schema-codegen",
  "--example",
  "emit_typescript_acceptance",
  "--",
  resolve(HERE, "../acceptance/schema.yaml"),
  GENERATED,
]);
command("cargo", [
  "run",
  "--quiet",
  "--manifest-path",
  resolve(CORE, "Cargo.toml"),
  "--package",
  "type-bridge-schema-codegen",
  "--example",
  "emit_typescript_acceptance",
  "--",
  resolve(HERE, "schema-ordered.yaml"),
  ORDERED,
]);
command("cargo", ["run", "--quiet", "--manifest-path", resolve(CORE, "Cargo.toml"), "--package", "type-bridge-schema-codegen", "--example", "emit_typescript_acceptance", "--", resolve(HERE, "../acceptance/schema-plan-idioms.yaml"), PLAN_IDIOMS]);
command("cargo", ["run", "--quiet", "--manifest-path", resolve(CORE, "Cargo.toml"), "--package", "type-bridge-schema-codegen", "--example", "emit_typescript_acceptance", "--", resolve(HERE, "schema-plan-names.yaml"), PLAN_NAMES], ROOT, {
  ...process.env,
  TYPE_BRIDGE_ACCEPTANCE_TYPE_NAMES: JSON.stringify([[{ kind: "attribute", label: "set" }, "Setting"]]),
});
const schemaPath = resolve(HERE, "../acceptance/schema.yaml");
const schemaSource = readFileSync(schemaPath, "utf8");
const foreignSource = schemaSource.replace(
  "member: { card: { min: 0, max: 2 }, doc: membership player }",
  "member: { card: { min: 0, max: 3 }, doc: membership player }",
);
if (foreignSource === schemaSource) {
  throw new Error("foreign authority fixture did not modify the schema");
}
const foreignSchema = resolve(STAGE, "schema-foreign.yaml");
writeFileSync(foreignSchema, foreignSource);
command("cargo", [
  "run",
  "--quiet",
  "--manifest-path",
  resolve(CORE, "Cargo.toml"),
  "--package",
  "type-bridge-schema-codegen",
  "--example",
  "emit_typescript_acceptance",
  "--",
  foreignSchema,
  FOREIGN,
]);
const projectedSchema = resolve(
  ROOT,
  "tests/contracts/sdk_conformance/models/schema.yaml",
);
command("cargo", [
  "run",
  "--quiet",
  "--manifest-path",
  resolve(CORE, "Cargo.toml"),
  "--package",
  "type-bridge-schema-codegen",
  "--example",
  "emit_typescript_acceptance",
  "--",
  projectedSchema,
  PROJECTED,
]);
const projectedSource = readFileSync(projectedSchema, "utf8");
const projectedForeignSource = projectedSource.replace(
  "member: { card: { min: 0, max: 2 }, doc: membership player }",
  "member: { card: { min: 0, max: 3 }, doc: membership player }",
);
if (projectedForeignSource === projectedSource) {
  throw new Error(
    "Projected foreign package variant did not modify one playing fact",
  );
}
const projectedForeignSchema = resolve(STAGE, "projected-foreign-schema.yaml");
writeFileSync(projectedForeignSchema, projectedForeignSource);
command("cargo", [
  "run",
  "--quiet",
  "--manifest-path",
  resolve(CORE, "Cargo.toml"),
  "--package",
  "type-bridge-schema-codegen",
  "--example",
  "emit_typescript_acceptance",
  "--",
  projectedForeignSchema,
  PROJECTED_FOREIGN,
]);
command("cargo", [
  "run", "--quiet", "--manifest-path", resolve(CORE, "Cargo.toml"),
  "--package", "type-bridge-schema-codegen", "--example", "emit_typescript_acceptance",
  "--", resolve(HERE, "schema-blockers.yaml"), BLOCKERS,
], ROOT, {
  ...process.env,
  TYPE_BRIDGE_ACCEPTANCE_TYPE_NAMES: JSON.stringify([
    [{ kind: "attribute", label: "powertrain_ref" }, "PowertrainReferenceValue"],
  ]),
});
mkdirSync(resolve(STAGE, "node_modules/@type-bridge"), { recursive: true });
symlinkSync(NODE_PACKAGE, resolve(STAGE, "node_modules/@type-bridge/node"), "dir");
command(TSC, ["--project", resolve(BLOCKERS, "tsconfig.json")]);
command(TSC, ["--project", resolve(GENERATED, "tsconfig.json")]);
command(TSC, ["--project", resolve(ORDERED, "tsconfig.json")]);
command(TSC, ["--project", resolve(PLAN_IDIOMS, "tsconfig.json")]);
command(TSC, ["--project", resolve(PLAN_NAMES, "tsconfig.json")]);
command(TSC, ["--project", resolve(FOREIGN, "tsconfig.json")]);
command(TSC, ["--project", resolve(PROJECTED, "tsconfig.json")]);
command(TSC, ["--project", resolve(PROJECTED_FOREIGN, "tsconfig.json")]);

const unorderedRuntimeDts = readFileSync(resolve(GENERATED, "dist/runtime.d.ts"), "utf8");
const unorderedModelsDts = readFileSync(resolve(GENERATED, "dist/models.d.ts"), "utf8");
const orderedRuntimeDts = readFileSync(resolve(ORDERED, "dist/runtime.d.ts"), "utf8");
const orderedModelsDts = readFileSync(resolve(ORDERED, "dist/models.d.ts"), "utf8");
for (const [kind, runtimeDts, modelsDts] of [
  ["unordered", unorderedRuntimeDts, unorderedModelsDts],
  ["ordered", orderedRuntimeDts, orderedModelsDts],
]) {
  for (const nativeName of ["ProjectedBatchUpdate", "OrderedProjectedModelManager", "OrderedModelToken"]) {
    if (!runtimeDts.includes(nativeName)) {
      throw new Error(`${kind} runtime declaration omits ${nativeName}`);
    }
  }
  for (const method of ["updateMany", "deleteMany"]) {
    if (!runtimeDts.includes(`${method}(`)) {
      throw new Error(`${kind} runtime declaration omits ${method}`);
    }
  }
  if (!modelsDts.includes("OrderedModelToken as ModelToken")) {
    throw new Error(`${kind} model declarations lose the native token alias`);
  }
}

for (const fixture of [
  "blockers_positive.ts",
  "positive.ts",
  "negative.ts",
  "ordered_batch_positive.ts",
  "ordered_batch_negative.ts",
  "blockers_check.mjs",
  "runtime_check.mjs",
  "authority_rejection_check.mjs",
  "plan_idioms_positive.ts",
  "plan_idioms_negative.ts",
  "plan_idioms_check.mjs",
  "projected_parity_check.mjs",
]) {
  copyFileSync(resolve(HERE, fixture), resolve(STAGE, fixture));
}
copyFileSync(DOCUMENTED_EXAMPLES, resolve(STAGE, "documented_examples.ts"));
writeFileSync(resolve(STAGE, "package.json"), "{\"type\":\"module\"}\n");
writeFileSync(
  resolve(STAGE, "tsconfig.json"),
  `${JSON.stringify({
    compilerOptions: {
      target: "ES2022",
      module: "NodeNext",
      moduleResolution: "NodeNext",
      strict: true,
      exactOptionalPropertyTypes: true,
      noUncheckedIndexedAccess: true,
      verbatimModuleSyntax: true,
      noEmit: true,
      skipLibCheck: false,
    },
    include: [
      "blockers_positive.ts",
      "positive.ts",
      "negative.ts",
      "ordered_batch_positive.ts",
      "ordered_batch_negative.ts",
      "documented_examples.ts",
      "plan_idioms_positive.ts",
      "plan_idioms_negative.ts",
      "generated_plan_idioms/src/**/*.ts",
      "generated_plan_names/src/**/*.ts",
      "generated_package/src/**/*.ts",
      "generated_ordered/src/**/*.ts",
      "generated_foreign/src/**/*.ts",
    ],
  }, null, 2)}\n`,
);
command(TSC, ["--project", resolve(STAGE, "tsconfig.json")]);
command("node", [resolve(STAGE, "authority_rejection_check.mjs")]);
command("node", [resolve(STAGE, "blockers_check.mjs")]);
command("node", [resolve(STAGE, "runtime_check.mjs")]);
command("node", [resolve(STAGE, "plan_idioms_check.mjs")]);
const externalProjectedReport = process.env.TYPE_BRIDGE_PROJECTED_PARITY_REPORT;
const projectedReport = externalProjectedReport ?? resolve(STAGE, "projected-node-report.json");
if (externalProjectedReport !== undefined && (!isAbsolute(projectedReport) || existsSync(projectedReport))) {
  throw new Error("external Projected parity report must be absent and absolute");
}
command(
  "node",
  [resolve(STAGE, "projected_parity_check.mjs")],
  ROOT,
  {
    ...process.env,
    TYPE_BRIDGE_PROJECTED_PARITY_REPORT: projectedReport,
    TYPE_BRIDGE_PROJECTED_REPOSITORY_ROOT: ROOT,
  },
);
command(process.env.PYTHON ?? "python3", [
  "-c",
  [
    "import importlib.util, pathlib, sys",
    "path = pathlib.Path(sys.argv[1])",
    "spec = importlib.util.spec_from_file_location('projected_comparator', path)",
    "module = importlib.util.module_from_spec(spec)",
    "sys.modules[spec.name] = module",
    "spec.loader.exec_module(module)",
    "module._load_report(pathlib.Path(sys.argv[2]), module.load_contract())",
  ].join("; "),
  resolve(ROOT, "scripts/ci/compare_projected_parity.py"),
  projectedReport,
]);
command(process.env.PYTHON ?? "python3", [resolve(HERE, "codec_check.py")]);
console.log("schema-codegen TypeScript acceptance passed");
