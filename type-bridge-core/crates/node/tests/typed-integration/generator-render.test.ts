import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { describe, test } from "node:test";

import { generateModels } from "../../typescript/index.js";

type RuntimePackage = typeof import("../../typescript/index.js");

// Render-decision gate for the generator. Asserts the EMITTED source reflects
// the Python generator's naming + flag decisions on the real corpus.
// Cross-language descriptor parity (generated TS descriptor == generated Python
// descriptor) is verified by the separate cross-language test that compiles the output.
const requirePackage = createRequire(path.join(process.cwd(), "generator-render.cjs"));
const native = (requirePackage(process.cwd()) as RuntimePackage).loadNative();

const schemaText = fs.readFileSync(
  path.join(process.cwd(), "..", "..", "..", "tests", "integration", "parity", "fixtures", "schema.tql"),
  "utf8",
);

const out = fs.mkdtempSync(path.join(os.tmpdir(), "tsgen-render-"));
generateModels(schemaText, out, { native });
const attributes = fs.readFileSync(path.join(out, "attributes.ts"), "utf8");
const entities = fs.readFileSync(path.join(out, "entities.ts"), "utf8");

describe("generator render decisions", () => {
  test("attributes map value_type to the correct attr.* kind, class name = toClassName", () => {
    assert.match(attributes, /export class ParityId extends attr\.String\("parity-id"\) \{\}/);
    assert.match(attributes, /export class ParityAge extends attr\.Integer\("parity-age"\) \{\}/);
    assert.match(attributes, /export class ParityScore extends attr\.Double\("parity-score"\) \{\}/);
    assert.match(attributes, /export class ParityActive extends attr\.Boolean\("parity-active"\) \{\}/);
    assert.match(attributes, /export class ParitySeenAt extends attr\.DateTimeTZ\("parity-seen-at"\) \{\}/);
    assert.match(attributes, /export class ParityBalance extends attr\.Decimal\("parity-balance"\) \{\}/);
    assert.match(attributes, /export class ParitySessionLength extends attr\.Duration\("parity-session-length"\) \{\}/);
  });

  test("entity field keys use toFieldName (kebab->snake, prefix kept), not human names", () => {
    // The mechanical generator name, matching the Python generator — NOT the
    // hand-authored corpus's `id`/`tags`.
    assert.match(entities, /parity_id: field\(ParityId, Key\)/);
    assert.match(entities, /parity_birth_date: field\(ParityBirthDate\)\.optional\(\)/);
    assert.ok(!entities.includes("tags:"), "must not pluralize parity-tag to `tags`");
    assert.match(entities, /parity_tag: field\(ParityTag\)\.list\(Card\(0, 5\)\)/);
  });

  test("flags: Key, Unique, optional, and multi-value list are emitted correctly", () => {
    assert.match(entities, /parity_email: field\(ParityEmail, Unique\)/);
    assert.match(entities, /parity_name: field\(ParityName\)\.optional\(\)/);
    assert.match(entities, /parity_tag: field\(ParityTag\)\.list\(Card\(0, 5\)\)/);
  });

  test("abstract entity uses TypeFlags; inherited entity carries { parent }", () => {
    assert.match(entities, /extends Entity\(TypeFlags\(\{ name: "parity-party", abstract: true \}\)/);
    assert.match(entities, /export class ParityPerson extends Entity\("parity-person",[\s\S]*\}, \{ parent: ParityParty \}\) \{\}/);
  });

  test("emitted imports target the package entrypoint, not a relative surface path", () => {
    assert.match(attributes, /from "@type-bridge\/node"/);
    assert.match(entities, /from "@type-bridge\/node"/);
    assert.match(entities, /from "\.\/attributes\.js"/);
    assert.ok(!entities.includes("typescript/"), "no hardcoded in-tree surface path");
    assert.ok(!entities.includes("dist/"), "no hardcoded dist path");
  });

  test("implicitKeyAttributes promotes an attribute to Key without downgrading a schema key", () => {
    // Generate into a fresh temp dir with parity-name declared as an implicit key.
    const outImplicit = fs.mkdtempSync(path.join(os.tmpdir(), "tsgen-implicit-"));
    generateModels(schemaText, outImplicit, { native, implicitKeyAttributes: ["parity-name"] });
    const entitiesImplicit = fs.readFileSync(path.join(outImplicit, "entities.ts"), "utf8");

    // parity-name was optional() without the option; with it, it must be Key.
    assert.match(entitiesImplicit, /parity_name: field\(ParityName, Key\)/);

    // parity-id is already @key in the schema — implicit key must not change it.
    assert.match(entitiesImplicit, /parity_id: field\(ParityId, Key\)/);
  });
});

describe("generator relation inheritance (source)", () => {
  // The parity corpus has no relation inheritance; the bookstore fixture has
  // `relation authoring sub contribution, relates author as contributor`. The
  // generator must emit only the relation's OWN (overriding) role plus a
  // `{ parent }` reference — never the inherited parent roles. (The runtime
  // descriptor flattening for parented relations is a separate concern; this
  // locks the generated source, which the cross-language test relies on.)
  const bookstore = fs.readFileSync(
    path.join(process.cwd(), "..", "..", "..", "tests", "integration", "generator", "fixtures", "bookstore.tql"),
    "utf8",
  );
  const bookstoreOut = fs.mkdtempSync(path.join(os.tmpdir(), "tsgen-bookstore-"));
  generateModels(bookstore, bookstoreOut, { native });
  const relations = fs.readFileSync(path.join(bookstoreOut, "relations.ts"), "utf8");

  test("a child relation emits only its own role and a { parent } reference", () => {
    // `authoring sub contribution, relates author as contributor`: only `author`,
    // with { parent: Contribution } — NOT the inherited contributor/work roles.
    assert.match(
      relations,
      /export class Authoring extends Relation\("authoring", \{\s*author: role\(Contributor\),\s*\}, \{ parent: Contribution \}\) \{\}/,
    );
    // The base relation still declares its own roles in full.
    assert.match(relations, /export class Contribution extends Relation\("contribution", \{/);
    assert.match(relations, /contributor: role\(Contributor\)/);
    assert.match(relations, /work: role\(Book\)/);
  });
});
