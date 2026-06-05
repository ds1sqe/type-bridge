import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { describe, test } from "node:test";

import { parseSchema } from "../../typescript/index.js";

type RuntimePackage = typeof import("../../typescript/index.js");

// Load the real native module the way every runtime test does: require the
// package root (its loadNative() resolves the built .node) and inject the
// result. This exercises the Rust parser -> NAPI -> TS marshalling boundary
// end to end. DB-free: parseSchema is a pure offline parse.
const requirePackage = createRequire(path.join(process.cwd(), "parser-smoke.cjs"));
const typeBridge = requirePackage(process.cwd()) as RuntimePackage;
const native = typeBridge.loadNative();

const schemaPath = path.join(
  process.cwd(),
  "..",
  "..",
  "..",
  "tests",
  "integration",
  "parity",
  "fixtures",
  "schema.tql",
);
const schemaText = fs.readFileSync(schemaPath, "utf8");

describe("parseSchema (Rust parser -> NAPI -> TS)", () => {
  const schema = parseSchema(schemaText, native);

  test("returns the corpus entities, relations, and attributes by name", () => {
    assert.deepEqual(Object.keys(schema.entities).sort(), [
      "parity-company",
      "parity-email-message",
      "parity-party",
      "parity-person",
    ]);
    assert.deepEqual(Object.keys(schema.relations).sort(), [
      "parity-membership",
      "parity-token-origin",
    ]);
    assert.ok(schema.attributes["parity-id"]);
    assert.ok(schema.attributes["parity-name"]);
    assert.equal(schema.attributes["parity-active"]?.value_type, "boolean");
  });

  test("inheritance is resolved by the Rust core", () => {
    const person = schema.entities["parity-person"];
    assert.ok(person);
    assert.equal(person.parent, "parity-party");
    // The key attribute declared on the abstract parent appears in the child's
    // resolved owned set — proof the Rust core resolved inheritance, not TS.
    const idOwned = person.owns.find((owned) => owned.name === "parity-id");
    assert.ok(idOwned, "parity-person should inherit parity-id from parity-party");
    assert.equal(idOwned.is_key, true);
  });

  test("multi-value cardinality and relation roles survive marshalling", () => {
    const person = schema.entities["parity-person"];
    const tag = person?.owns.find((owned) => owned.name === "parity-tag");
    assert.deepEqual(tag?.cardinality, { min: 0, max: 5 });

    const membership = schema.relations["parity-membership"];
    assert.ok(membership);
    assert.deepEqual(
      membership.roles.map((role) => role.name).sort(),
      ["evidence", "member", "organization"],
    );
  });
});
