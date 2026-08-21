import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const CORE = resolve(HERE, "../../../..");
const ROOT = resolve(CORE, "..");
const STAGE = resolve(CORE, "target/schema-codegen-typescript-acceptance");
const GENERATED = resolve(STAGE, "generated_v2");
const ORDERED = resolve(STAGE, "generated_ordered");
const FOREIGN = resolve(STAGE, "generated_foreign");
const PHASE2 = resolve(STAGE, "generated_phase2");
const PHASE2_FOREIGN = resolve(STAGE, "generated_phase2_foreign");
const NODE_PACKAGE = resolve(CORE, "crates/node");
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
      `${program} ${args.join(" ")} returned ${completed.status}\nstdout:\n${completed.stdout}\nstderr:\n${completed.stderr}`,
    );
  }
}

for (const fixture of [
  "positive.ts",
  "negative.ts",
  "ordered_batch_positive.ts",
  "ordered_batch_negative.ts",
  "runtime_check.mjs",
  "phase2_parity_check.mjs",
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
  "positive.ts",
  "ordered_batch_positive.ts",
  "runtime_check.mjs",
]) {
  const source = readFileSync(resolve(HERE, fixture), "utf8");
  for (const forbidden of ["QueryV2Authority", "declared-schema.json", "readFileSync"]) {
    if (source.includes(forbidden)) {
      throw new Error(`${fixture} bypasses generated embedded authority with ${forbidden}`);
    }
  }
}
for (const forbidden of ["QueryV2Authority", "declared-schema.json", "readFileSync"]) {
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
const phase2Schema = resolve(
  ROOT,
  "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml",
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
  phase2Schema,
  PHASE2,
]);
const phase2Source = readFileSync(phase2Schema, "utf8");
const phase2ForeignSource = phase2Source.replace(
  "member: { card: { min: 0, max: 2 }, doc: membership player }",
  "member: { card: { min: 0, max: 3 }, doc: membership player }",
);
if (phase2ForeignSource === phase2Source) {
  throw new Error(
    "Phase-2 foreign package variant did not modify one playing fact",
  );
}
const phase2ForeignSchema = resolve(STAGE, "phase2-foreign-schema.yaml");
writeFileSync(phase2ForeignSchema, phase2ForeignSource);
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
  phase2ForeignSchema,
  PHASE2_FOREIGN,
]);
mkdirSync(resolve(STAGE, "node_modules/@type-bridge"), { recursive: true });
symlinkSync(NODE_PACKAGE, resolve(STAGE, "node_modules/@type-bridge/node"), "dir");
command("tsc", ["--project", resolve(GENERATED, "tsconfig.json")]);
command("tsc", ["--project", resolve(ORDERED, "tsconfig.json")]);
command("tsc", ["--project", resolve(FOREIGN, "tsconfig.json")]);
command("tsc", ["--project", resolve(PHASE2, "tsconfig.json")]);
command("tsc", ["--project", resolve(PHASE2_FOREIGN, "tsconfig.json")]);

const unorderedRuntimeDts = readFileSync(resolve(GENERATED, "dist/runtime.d.ts"), "utf8");
const unorderedModelsDts = readFileSync(resolve(GENERATED, "dist/models.d.ts"), "utf8");
const orderedRuntimeDts = readFileSync(resolve(ORDERED, "dist/runtime.d.ts"), "utf8");
const orderedModelsDts = readFileSync(resolve(ORDERED, "dist/models.d.ts"), "utf8");
for (const successorName of [
  "ProjectedBatchUpdate",
  "OrderedProjectedModelManager",
  "OrderedModelToken",
]) {
  if (unorderedRuntimeDts.includes(successorName) || unorderedModelsDts.includes(successorName)) {
    throw new Error(`unordered declarations expose ordered successor ${successorName}`);
  }
  if (!orderedRuntimeDts.includes(successorName)) {
    throw new Error(`ordered runtime declaration omits ${successorName}`);
  }
}
for (const method of ["updateMany", "deleteMany"]) {
  if (unorderedRuntimeDts.includes(`${method}(`)) {
    throw new Error(`unordered declarations expose ordered successor ${method}`);
  }
  if (!orderedRuntimeDts.includes(`${method}(`)) {
    throw new Error(`ordered runtime declaration omits ${method}`);
  }
}
if (!orderedModelsDts.includes("OrderedModelToken as ModelToken")) {
  throw new Error("ordered model declarations lose the successor token alias");
}

for (const fixture of [
  "positive.ts",
  "negative.ts",
  "ordered_batch_positive.ts",
  "ordered_batch_negative.ts",
  "runtime_check.mjs",
  "authority_rejection_check.mjs",
  "phase2_parity_check.mjs",
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
      "positive.ts",
      "negative.ts",
      "ordered_batch_positive.ts",
      "ordered_batch_negative.ts",
      "documented_examples.ts",
      "generated_v2/src/**/*.ts",
      "generated_ordered/src/**/*.ts",
      "generated_foreign/src/**/*.ts",
    ],
  }, null, 2)}\n`,
);
command("tsc", ["--project", resolve(STAGE, "tsconfig.json")]);
command("node", [resolve(STAGE, "authority_rejection_check.mjs")]);
command("node", [resolve(STAGE, "runtime_check.mjs")]);
const externalPhase2Report = process.env.TYPE_BRIDGE_PHASE2_PARITY_REPORT;
const phase2Report = externalPhase2Report ?? resolve(STAGE, "phase2-node-report.json");
if (externalPhase2Report !== undefined && (!isAbsolute(phase2Report) || existsSync(phase2Report))) {
  throw new Error("external Phase-2 parity report must be absent and absolute");
}
command(
  "node",
  [resolve(STAGE, "phase2_parity_check.mjs")],
  ROOT,
  {
    ...process.env,
    TYPE_BRIDGE_PHASE2_PARITY_REPORT: phase2Report,
    TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT: ROOT,
  },
);
command("uv", [
  "run",
  "python",
  "-c",
  [
    "import importlib.util, pathlib, sys",
    "path = pathlib.Path(sys.argv[1])",
    "spec = importlib.util.spec_from_file_location('phase2_comparator', path)",
    "module = importlib.util.module_from_spec(spec)",
    "sys.modules[spec.name] = module",
    "spec.loader.exec_module(module)",
    "module._load_report(pathlib.Path(sys.argv[2]), module.load_contract())",
  ].join("; "),
  resolve(ROOT, "scripts/ci/compare_phase2_projection_parity.py"),
  phase2Report,
]);
console.log("schema-codegen TypeScript acceptance passed");
