import { copyFileSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const CORE = resolve(HERE, "../../../..");
const ROOT = resolve(CORE, "..");
const STAGE = resolve(CORE, "target/schema-codegen-typescript-acceptance");
const GENERATED = resolve(STAGE, "generated_v2");
const NODE_PACKAGE = resolve(CORE, "crates/node");

function command(program, args, cwd = ROOT) {
  const completed = spawnSync(program, args, {
    cwd,
    encoding: "utf8",
    stdio: "pipe",
  });
  if (completed.status !== 0) {
    throw new Error(
      `${program} ${args.join(" ")} returned ${completed.status}\nstdout:\n${completed.stdout}\nstderr:\n${completed.stderr}`,
    );
  }
}

for (const fixture of ["positive.ts", "negative.ts", "runtime_check.mjs"]) {
  const source = readFileSync(resolve(HERE, fixture), "utf8");
  for (const forbidden of ["as unknown as", "@ts-ignore"]) {
    if (source.includes(forbidden)) {
      throw new Error(`${fixture} contains forbidden escape ${forbidden}`);
    }
  }
}
if (!readFileSync(resolve(HERE, "negative.ts"), "utf8").includes("@ts-expect-error")) {
  throw new Error("negative fixture has no @ts-expect-error assertions");
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
mkdirSync(resolve(STAGE, "node_modules/@type-bridge"), { recursive: true });
symlinkSync(NODE_PACKAGE, resolve(STAGE, "node_modules/@type-bridge/node"), "dir");
command("tsc", ["--project", resolve(GENERATED, "tsconfig.json")]);

for (const fixture of ["positive.ts", "negative.ts", "runtime_check.mjs"]) {
  copyFileSync(resolve(HERE, fixture), resolve(STAGE, fixture));
}
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
    include: ["positive.ts", "negative.ts", "generated_v2/src/**/*.ts"],
  }, null, 2)}\n`,
);
command("tsc", ["--project", resolve(STAGE, "tsconfig.json")]);
command("node", [resolve(STAGE, "runtime_check.mjs")]);
console.log("schema-codegen TypeScript acceptance passed");
