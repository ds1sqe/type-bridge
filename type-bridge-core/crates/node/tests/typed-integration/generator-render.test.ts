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

const social = fs.readFileSync(
  path.join(process.cwd(), "..", "..", "..", "tests", "integration", "generator", "fixtures", "social_media.tql"),
  "utf8",
);
const socialOut = fs.mkdtempSync(path.join(os.tmpdir(), "tsgen-social-"));
generateModels(social, socialOut, { native });
const socialAttributes = fs.readFileSync(path.join(socialOut, "attributes.ts"), "utf8");

const playsCardinalitySchema = `
define
entity ts-card-person,
  plays ts-card-employment:employee @card(1..1),
  plays ts-card-review:reviewer @card(0..5);
entity ts-card-company,
  plays ts-card-employment:employer @card(0..1);
relation ts-card-employment,
  relates employee,
  relates employer @card(1..1);
relation ts-card-review,
  relates reviewer @card(2..2);
`;
const playsCardinalityOut = fs.mkdtempSync(path.join(os.tmpdir(), "tsgen-plays-card-"));
generateModels(playsCardinalitySchema, playsCardinalityOut, { native });
const playsCardinalityRelations = fs.readFileSync(
  path.join(playsCardinalityOut, "relations.ts"),
  "utf8",
);

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
    // @unique keeps the default card(0..1): the field stays optional.
    assert.match(entities, /parity_email: field\(ParityEmail, Unique\)\.optional\(\)/);
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

  test("attribute type metadata is emitted for constraints and subtyping", () => {
    assert.match(socialAttributes, /export class Id extends attr\.String\("id", \{ abstract: true \}\) \{\}/);
    assert.match(socialAttributes, /export class PostId extends attr\.String\("post-id", \{ parent: Id \}\) \{\}/);
    assert.match(
      socialAttributes,
      /export class Emoji extends attr\.String\("emoji", \{ values: \["like","love","funny","surprise","sad","angry"\] \}\) \{\}/,
    );
    assert.match(socialAttributes, /export class PostImage extends attr\.String\("post-image", \{ regex: /);
    assert.match(socialAttributes, /export class Payload extends attr\.String\("payload", \{ abstract: true \}\) \{\}/);
    assert.match(
      socialAttributes,
      /export class TextPayload extends attr\.String\("text-payload", \{ parent: Payload, abstract: true \}\) \{\}/,
    );
  });

  test("plays-side cardinality renders as playsCardinality on relation roles", () => {
    assert.match(
      playsCardinalityRelations,
      /employee: role\(TsCardPerson, \{ playsCardinality: Card\(1, 1\) \}\)/,
    );
    assert.match(
      playsCardinalityRelations,
      /employer: role\(TsCardCompany, \{ playsCardinality: Card\(0, 1\) \}\)/,
    );
    assert.match(
      playsCardinalityRelations,
      /reviewer: role\(TsCardPerson, \{ cardinality: Card\(2, 2\), playsCardinality: Card\(0, 5\) \}\)/,
    );
    assert.match(playsCardinalityRelations, /import \{ Card, Relation, field, role \}/);
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
    // The overrides marker is emitted on the specializing role.
    assert.match(
      relations,
      /export class Authoring extends Relation\("authoring", \{\s*author: role\(Contributor, \{ overrides: "contributor" \}\),\s*\}, \{ parent: Contribution \}\) \{\}/,
    );
    // The base relation still declares its own roles in full.
    assert.match(relations, /export class Contribution extends Relation\("contribution", \{/);
    assert.match(relations, /contributor: role\(Contributor\)/);
    assert.match(relations, /work: role\(Book\)/);
  });
});
