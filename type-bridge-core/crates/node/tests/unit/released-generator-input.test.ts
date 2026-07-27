import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

import { loadNative } from "../../typescript/index.js";

interface GeneratedFile {
  path: string;
  contents: string;
}

interface GeneratedPackage {
  files: GeneratedFile[];
}

interface DeclaredSnapshot {
  closed_world: boolean;
  unsupported_constructs: string[];
  entities: Array<{ label: string; parent: string | null; owns: unknown[] }>;
  plays: unknown[];
}

const native = loadNative();

function generatedPackage(source: string, target = "typescript"): GeneratedPackage {
  return JSON.parse(native.renderModelsJson(source, target, "{}")) as GeneratedPackage;
}

function declaredSnapshot(package_: GeneratedPackage): string {
  const file = package_.files.find(({ path }) => path === "declared-schema.json");
  assert.ok(file, "generated package carries declared-schema.json");
  return file.contents;
}

test("released empty and comment-only schemas share one canonical descriptor", () => {
  const sources = [
    "",
    " \t\r\n",
    "# released comment only\n",
    "// released comment only\r\n",
    "/* released comment only */",
  ];
  const canonical = declaredSnapshot(generatedPackage(""));

  for (const source of sources) {
    for (const target of ["python", "typescript", "rust"]) {
      const package_ = generatedPackage(source, target);
      assert.deepEqual(package_, generatedPackage("", target));
      assert.equal(declaredSnapshot(package_), canonical);
    }
  }
});

test("released function and struct-only schemas attach canonical empty descriptors", () => {
  const sources = [
    "define\nfun answer() -> integer:\n  return 1;\n",
    "define\nstruct payload, value note string;\n",
    [
      "define",
      "fun answer() -> integer:",
      "  return 1;",
      "struct payload, value note string;",
      "",
    ].join("\n"),
  ];
  const canonical = declaredSnapshot(generatedPackage(""));

  for (const source of sources) {
    for (const target of ["python", "typescript", "rust"]) {
      const package_ = generatedPackage(source, target);
      assert.equal(declaredSnapshot(package_), canonical);
      if (target === "python") {
        const paths = new Set(package_.files.map(({ path }) => path));
        assert.equal(paths.has("functions.py"), source.includes("fun answer"));
        assert.equal(paths.has("structs.py"), source.includes("struct payload"));
      }
    }
  }
});

test("released unresolved references attach exact open-world evidence", () => {
  const source = [
    "define",
    "entity child, sub missing-parent, owns missing-attribute @card(0..1), plays missing-relation:member;",
    "relation base, relates existing;",
    "relation specialized, sub base, relates replacement as absent;",
    "entity player, plays base:missing-role;",
    "ghost plays missing-relation:missing-role;",
    "",
  ].join("\n");
  const encoded = declaredSnapshot(generatedPackage(source));
  const snapshot = JSON.parse(encoded) as DeclaredSnapshot;

  assert.equal(snapshot.closed_world, false);
  assert.deepEqual(snapshot.unsupported_constructs, [
    "sub missing-parent",
    "owns missing-attribute @card(0..1)",
    "plays missing-relation:member",
    "relates replacement as absent",
    "plays base:missing-role",
    "plays missing-relation:missing-role",
  ]);
  assert.deepEqual(snapshot.plays, []);
  const child = snapshot.entities.find(({ label }) => label === "child");
  assert.ok(child);
  assert.equal(child.parent, null);
  assert.deepEqual(child.owns, []);
  assert.ok(snapshot.entities.some(({ label }) => label === "ghost"));

  for (const target of ["python", "typescript", "rust"]) {
    assert.equal(declaredSnapshot(generatedPackage(source, target)), encoded);
  }
});

test("released unresolved references produce a type-checkable package without phantom imports", async () => {
  const source = [
    "define",
    "entity child, sub missing-parent, owns missing-attribute @card(0..1), plays missing-relation:member;",
    "relation base, relates existing;",
    "relation specialized, sub base, relates replacement as absent;",
    "entity player, plays base:missing-role;",
    "ghost plays missing-relation:missing-role;",
    "",
  ].join("\n");
  const package_ = generatedPackage(source);
  const files = new Map(package_.files.map(({ path, contents }) => [path, contents]));
  const entities = files.get("entities.ts");
  const relations = files.get("relations.ts");
  assert.ok(entities);
  assert.ok(relations);
  assert.match(entities, /export class Child extends Entity\("child", \{\}\) \{\}/);
  assert.doesNotMatch(entities, /Missing/);
  assert.doesNotMatch(relations, /Missing|Absent/);
  assert.match(relations, /export class Base extends Relation\("base", \{\}\) \{\}/);
  assert.match(
    relations,
    /export class Specialized extends Relation\("specialized", \{\}, \{ parent: Base \}\) \{\}/,
  );
  assert.doesNotMatch(relations, /replacement/);

  const stage = await mkdtemp(join(tmpdir(), "type-bridge-released-generator-"));
  try {
    for (const { path, contents } of package_.files) {
      if (path.endsWith(".ts")) {
        await writeFile(join(stage, path), contents);
      }
    }
    const stub = join(stage, "node_modules/@type-bridge/node");
    await mkdir(stub, { recursive: true });
    await writeFile(
      join(stub, "package.json"),
      JSON.stringify({ name: "@type-bridge/node", type: "module", types: "index.d.ts" }),
    );
    await writeFile(
      join(stub, "index.d.ts"),
      [
        "export const attr: any;",
        "export const Entity: any;",
        "export const Relation: any;",
        "export const field: any;",
        "export const role: any;",
        "",
      ].join("\n"),
    );
    await writeFile(join(stage, "package.json"), JSON.stringify({ type: "module" }));
    await writeFile(
      join(stage, "tsconfig.json"),
      JSON.stringify({
        compilerOptions: {
          module: "NodeNext",
          moduleResolution: "NodeNext",
          noEmit: true,
          skipLibCheck: true,
          strict: true,
          target: "ES2022",
        },
        include: ["*.ts"],
      }),
    );

    const compiler = resolve(process.cwd(), "node_modules/typescript/bin/tsc");
    const result = spawnSync(process.execPath, [compiler, "--project", join(stage, "tsconfig.json")], {
      cwd: stage,
      encoding: "utf8",
    });
    assert.equal(
      result.status,
      0,
      `generated TypeScript package failed to type-check\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
    );
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
});
