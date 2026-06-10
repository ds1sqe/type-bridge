/**
 * Unit tests for toDict() / fromDict() on typed model instances.
 *
 * These are OFFLINE tests (no DB, no NAPI). They exercise:
 *   1. Round-trip identity: fromDict(toDict(x)) equals x field-by-field.
 *   2. Parity against write-data.json / expected-canonical.json: toDict()
 *      output matches the canonical plain-dict shape Python to_dict() produces.
 *   3. Optional-field omission: absent optional fields are omitted from toDict().
 *   4. Runtime error guards: unknown key and missing required field.
 *   5. Relation toDict: attribute fields only (roles excluded).
 *
 * Descriptor byte-identity is the Plan 10 gate (typed-layer.test.ts); this
 * file covers the complementary VALUE round-trip gate (Plan 11 Phase 1).
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Card,
  Entity,
  Key,
  Relation,
  TypeFlags,
  TypedCodecError,
  Unique,
  attr,
  field,
  role,
} from "../../typescript/index.js";

// ---------------------------------------------------------------------------
// Fixture loading
// ---------------------------------------------------------------------------

interface WriteDataFixture {
  fixture_id: string;
  version: number;
  entities: Array<{
    stable_id: string;
    type: string;
    attributes: Record<string, WriteDataAttr | WriteDataAttr[]>;
  }>;
  relations: Array<{
    stable_id: string;
    type: string;
    attributes: Record<string, WriteDataAttr | WriteDataAttr[]>;
    roles: Record<string, Array<{ stable_id: string; type: string }>>;
  }>;
}

interface WriteDataAttr {
  type: string;
  value: string | number | boolean;
}

const FIXTURES_DIR = path.resolve(
  process.cwd(),
  "../../../tests/integration/parity/fixtures",
);

function loadWriteData(): WriteDataFixture {
  return JSON.parse(
    fs.readFileSync(path.join(FIXTURES_DIR, "write-data.json"), "utf8"),
  ) as WriteDataFixture;
}

// ---------------------------------------------------------------------------
// Attribute classes — full parity corpus
// ---------------------------------------------------------------------------

class ParityId extends attr.String("parity-id") {}
class ParityName extends attr.String("parity-name") {}
class ParityEmail extends attr.String("parity-email") {}
class ParityAge extends attr.Integer("parity-age") {}
class ParityScore extends attr.Double("parity-score") {}
class ParityActive extends attr.Boolean("parity-active") {}
class ParityBirthDate extends attr.Date("parity-birth-date") {}
class ParityLoginAt extends attr.DateTime("parity-login-at") {}
class ParitySeenAt extends attr.DateTimeTZ("parity-seen-at") {}
class ParityBalance extends attr.Decimal("parity-balance") {}
class ParitySessionLength extends attr.Duration("parity-session-length") {}
class ParityNote extends attr.String("parity-note") {}
class ParitySince extends attr.Date("parity-since") {}
class ParityConfidence extends attr.Integer("parity-confidence") {}
class ParityKind extends attr.String("parity-kind") {}
class ParityTag extends attr.String("parity-tag") {}

// ---------------------------------------------------------------------------
// Model declarations — full parity corpus (mirrors typed-layer.test.ts)
// ---------------------------------------------------------------------------

class ParityParty extends Entity(TypeFlags({ name: "parity-party", abstract: true }), {
  id: field(ParityId, Key),
  name: field(ParityName).optional(),
}) {}

class ParityPerson extends Entity(
  "parity-person",
  {
    email: field(ParityEmail, Unique),
    age: field(ParityAge).optional(),
    score: field(ParityScore).optional(),
    active: field(ParityActive).optional(),
    birth_date: field(ParityBirthDate).optional(),
    login_at: field(ParityLoginAt).optional(),
    seen_at: field(ParitySeenAt).optional(),
    balance: field(ParityBalance).optional(),
    session_length: field(ParitySessionLength).optional(),
    tags: field(ParityTag).list(Card(0, 5)),
  },
  { parent: ParityParty },
) {}

class ParityCompany extends Entity("parity-company", {
  id: field(ParityId, Key),
  name: field(ParityName),
}) {}

class ParityEmailMessage extends Entity("parity-email-message", {
  id: field(ParityId, Key),
  note: field(ParityNote),
}) {}

class ParityMembership extends Relation("parity-membership", {
  member: role("parity-person", { cardinality: Card(1, 1) }),
  organization: role(ParityCompany, { cardinality: Card(1, 1) }),
  evidence: role("parity-person", ParityEmailMessage, { cardinality: Card(0, 5) }),
  since: field(ParitySince),
  confidence: field(ParityConfidence).optional(),
}) {}

class ParityTokenOrigin extends Relation("parity-token-origin", {
  token: role(ParityParty, "parity-person", { cardinality: Card(1, 1) }),
  issue: role(ParityCompany, { cardinality: Card(1, 1) }),
  kind: field(ParityKind),
}) {}

// ---------------------------------------------------------------------------
// Helpers for building typed instances from write-data.json attribute maps
// ---------------------------------------------------------------------------

/**
 * Extract the plain primitive from a write-data.json attribute entry.
 * `long` values are stored as decimal strings ("37") and must be converted to
 * bigint to match the TS `attr.Integer` value type.
 */
function attrPlain(attr: WriteDataAttr): string | number | boolean | bigint {
  if (attr.type === "long") {
    return BigInt(attr.value as string);
  }
  return attr.value;
}

function buildAlice(): InstanceType<typeof ParityPerson> {
  const writeData = loadWriteData();
  const row = writeData.entities.find((e) => e.stable_id === "person:alice")!;
  const a = row.attributes;

  const tagsRaw = a["tags"] as WriteDataAttr[];
  return new ParityPerson({
    id: new ParityId(attrPlain(a["id"] as WriteDataAttr) as string),
    name: new ParityName(attrPlain(a["name"] as WriteDataAttr) as string),
    email: new ParityEmail(attrPlain(a["email"] as WriteDataAttr) as string),
    age: new ParityAge(attrPlain(a["age"] as WriteDataAttr) as bigint),
    score: new ParityScore(attrPlain(a["score"] as WriteDataAttr) as number),
    active: new ParityActive(attrPlain(a["active"] as WriteDataAttr) as boolean),
    birth_date: new ParityBirthDate(attrPlain(a["birth_date"] as WriteDataAttr) as string),
    login_at: new ParityLoginAt(attrPlain(a["login_at"] as WriteDataAttr) as string),
    seen_at: new ParitySeenAt(attrPlain(a["seen_at"] as WriteDataAttr) as string),
    balance: new ParityBalance(attrPlain(a["balance"] as WriteDataAttr) as string),
    session_length: new ParitySessionLength(
      attrPlain(a["session_length"] as WriteDataAttr) as string,
    ),
    tags: tagsRaw.map((t) => new ParityTag(t.value as string)),
  });
}

function buildBob(): InstanceType<typeof ParityPerson> {
  const writeData = loadWriteData();
  const row = writeData.entities.find((e) => e.stable_id === "person:bob")!;
  const a = row.attributes;
  return new ParityPerson({
    id: new ParityId(attrPlain(a["id"] as WriteDataAttr) as string),
    email: new ParityEmail(attrPlain(a["email"] as WriteDataAttr) as string),
    age: new ParityAge(attrPlain(a["age"] as WriteDataAttr) as bigint),
    active: new ParityActive(attrPlain(a["active"] as WriteDataAttr) as boolean),
  });
}

function buildAcme(): InstanceType<typeof ParityCompany> {
  const writeData = loadWriteData();
  const row = writeData.entities.find((e) => e.stable_id === "company:acme")!;
  const a = row.attributes;
  return new ParityCompany({
    id: new ParityId(attrPlain(a["id"] as WriteDataAttr) as string),
    name: new ParityName(attrPlain(a["name"] as WriteDataAttr) as string),
  });
}

function buildEmailMessage(): InstanceType<typeof ParityEmailMessage> {
  const writeData = loadWriteData();
  const row = writeData.entities.find((e) => e.stable_id === "email:evidence-1")!;
  const a = row.attributes;
  return new ParityEmailMessage({
    id: new ParityId(attrPlain(a["id"] as WriteDataAttr) as string),
    note: new ParityNote(attrPlain(a["note"] as WriteDataAttr) as string),
  });
}

function buildMembership(): InstanceType<typeof ParityMembership> {
  const writeData = loadWriteData();
  const row = writeData.relations.find((r) => r.stable_id === "membership:alice-acme")!;
  const a = row.attributes;
  // Role player instances (we need something for the role fields).
  const alice = buildAlice();
  const acme = buildAcme();
  const bob = buildBob();
  const emailMsg = buildEmailMessage();
  return new ParityMembership({
    member: alice,
    organization: acme,
    evidence: [bob, emailMsg],
    since: new ParitySince(attrPlain(a["since"] as WriteDataAttr) as string),
    confidence: new ParityConfidence(attrPlain(a["confidence"] as WriteDataAttr) as bigint),
  });
}

// ---------------------------------------------------------------------------
// Tests: scalar toDict() parity against write-data.json
// ---------------------------------------------------------------------------

describe("toDict() scalar parity against write-data.json", () => {
  test("parity-company toDict matches write-data scalar shape", () => {
    const acme = buildAcme();
    const dict = acme.toDict();

    assert.equal(dict["id"], "company:acme");
    assert.equal(dict["name"], "Acme Research");
    // No extra keys (only 2 fields in parity-company schema).
    assert.deepEqual(Object.keys(dict).sort(), ["id", "name"]);
  });

  test("parity-email-message toDict matches write-data scalar shape", () => {
    const msg = buildEmailMessage();
    const dict = msg.toDict();

    assert.equal(dict["id"], "email:evidence-1");
    assert.equal(dict["note"], "welcome thread");
    assert.deepEqual(Object.keys(dict).sort(), ["id", "note"]);
  });

  test("parity-person (alice, all fields) toDict matches write-data shape", () => {
    const alice = buildAlice();
    const dict = alice.toDict();

    // string fields
    assert.equal(dict["id"], "person:alice");
    assert.equal(dict["name"], "Alice");
    assert.equal(dict["email"], "alice@example.test");

    // long → bigint
    assert.equal(typeof dict["age"], "bigint");
    assert.equal(dict["age"], 37n);

    // double
    assert.equal(dict["score"], 98.25);

    // boolean
    assert.equal(dict["active"], true);

    // date / datetime / datetime-tz / decimal / duration — strings
    assert.equal(dict["birth_date"], "1989-01-02");
    assert.equal(dict["login_at"], "2026-06-01T10:30:00");
    assert.equal(dict["seen_at"], "2026-06-01T10:30:00+00:00");
    assert.equal(dict["balance"], "1234.56");
    assert.equal(dict["session_length"], "PT2H30M");

    // multi-value list: plain string[]
    assert.deepEqual(dict["tags"], ["admin", "writer"]);
  });
});

// ---------------------------------------------------------------------------
// Tests: optional-field omission
// ---------------------------------------------------------------------------

describe("toDict() optional-field omission", () => {
  test("absent optional scalar fields are omitted from toDict output", () => {
    // Bob has only id, email, age, active — name and all other optional fields absent.
    const bob = buildBob();
    const dict = bob.toDict();

    const keys = Object.keys(dict).sort();
    assert.deepEqual(keys, ["active", "age", "email", "id"]);

    // Absent fields not present even as undefined keys.
    assert.equal("name" in dict, false);
    assert.equal("score" in dict, false);
    assert.equal("birth_date" in dict, false);
    assert.equal("tags" in dict, false);
  });

  test("absent optional list field (tags) is omitted, not empty array", () => {
    // Construct a person without tags to verify list optional omission.
    const p = new ParityPerson({
      id: new ParityId("test:no-tags"),
      email: new ParityEmail("notags@example.test"),
    });
    const dict = p.toDict();
    assert.equal("tags" in dict, false);
  });

  test("present list field (tags) serializes to plain string[]", () => {
    const alice = buildAlice();
    const dict = alice.toDict();
    assert.ok(Array.isArray(dict["tags"]));
    const tags = dict["tags"] as string[];
    assert.deepEqual(tags, ["admin", "writer"]);
    // Each element is a plain string, not an Attribute instance.
    for (const tag of tags) {
      assert.equal(typeof tag, "string");
    }
  });
});

// ---------------------------------------------------------------------------
// Tests: round-trip identity fromDict(toDict(x))
// ---------------------------------------------------------------------------

describe("fromDict(toDict(x)) round-trip identity", () => {
  test("scalar round-trip (parity-company)", () => {
    const acme = buildAcme();
    const roundTripped = ParityCompany.fromDict(acme.toDict());

    assert.ok(roundTripped.id instanceof ParityId);
    assert.equal(roundTripped.id.value, "company:acme");
    assert.ok(roundTripped.name instanceof ParityName);
    assert.equal(roundTripped.name.value, "Acme Research");
  });

  test("scalar round-trip (parity-person, alice, all fields)", () => {
    const alice = buildAlice();
    const roundTripped = ParityPerson.fromDict(alice.toDict());

    // Inherited fields preserved with parent brand.
    assert.ok(roundTripped.id instanceof ParityId);
    assert.equal(roundTripped.id.value, "person:alice");
    assert.ok(roundTripped.name instanceof ParityName);
    assert.equal(roundTripped.name.value, "Alice");

    // Child fields.
    assert.ok(roundTripped.email instanceof ParityEmail);
    assert.equal(roundTripped.email.value, "alice@example.test");

    // long/bigint identity.
    assert.ok(roundTripped.age instanceof ParityAge);
    assert.equal(roundTripped.age.value, 37n);

    assert.ok(roundTripped.score instanceof ParityScore);
    assert.equal(roundTripped.score.value, 98.25);

    assert.ok(roundTripped.active instanceof ParityActive);
    assert.equal(roundTripped.active.value, true);

    assert.ok(roundTripped.birth_date instanceof ParityBirthDate);
    assert.equal(roundTripped.birth_date.value, "1989-01-02");

    assert.ok(roundTripped.balance instanceof ParityBalance);
    assert.equal(roundTripped.balance.value, "1234.56");
  });

  test("multi-value tags round-trip: Attr[] → string[] → Attr[]", () => {
    const alice = buildAlice();
    const roundTripped = ParityPerson.fromDict(alice.toDict());

    assert.ok(Array.isArray(roundTripped.tags));
    const tags = roundTripped.tags!;
    assert.equal(tags.length, 2);
    assert.ok(tags[0] instanceof ParityTag);
    assert.equal(tags[0].value, "admin");
    assert.ok(tags[1] instanceof ParityTag);
    assert.equal(tags[1].value, "writer");
  });

  test("inherited fields (parity-person inherits from parity-party) preserved", () => {
    // Alice has id and name from the parent (ParityParty).
    const alice = buildAlice();
    const d = alice.toDict();

    // id is a required inherited field, name is an optional inherited field.
    assert.equal(d["id"], "person:alice");
    assert.equal(d["name"], "Alice");

    const roundTripped = ParityPerson.fromDict(d);
    assert.ok(roundTripped.id instanceof ParityId);
    assert.ok(roundTripped.name instanceof ParityName);
  });

  test("optional inherited field omitted from round-trip when absent", () => {
    // Bob has no name field (inherited optional).
    const bob = buildBob();
    const d = bob.toDict();
    assert.equal("name" in d, false);

    const roundTripped = ParityPerson.fromDict(d);
    assert.equal(roundTripped.name, undefined);
  });
});

// ---------------------------------------------------------------------------
// Tests: relation toDict (attribute fields only, roles excluded)
// ---------------------------------------------------------------------------

describe("toDict() on relation: attribute fields only, roles excluded", () => {
  test("parity-membership toDict includes only attribute fields", () => {
    const membership = buildMembership();
    const dict = membership.toDict();

    // Attribute fields present: since (required), confidence (optional, present).
    assert.ok("since" in dict, "since must be present");
    assert.equal(dict["since"], "2024-05-01");

    assert.ok("confidence" in dict, "confidence must be present");
    assert.equal(dict["confidence"], 9n);

    // Role fields (member, organization, evidence) must NOT appear in toDict output.
    assert.equal("member" in dict, false);
    assert.equal("organization" in dict, false);
    assert.equal("evidence" in dict, false);

    // Only the attribute fields.
    assert.deepEqual(Object.keys(dict).sort(), ["confidence", "since"]);
  });

  test("parity-membership toDict → fromDict round-trip on attributes", () => {
    const membership = buildMembership();
    const dict = membership.toDict();

    // fromDict re-brands attribute values only (no role players needed).
    const roundTripped = ParityMembership.fromDict(dict);

    assert.ok(roundTripped.since instanceof ParitySince);
    assert.equal(roundTripped.since.value, "2024-05-01");

    assert.ok(roundTripped.confidence instanceof ParityConfidence);
    assert.equal(roundTripped.confidence.value, 9n);
  });
});

// ---------------------------------------------------------------------------
// Tests: runtime error guards
// ---------------------------------------------------------------------------

describe("fromDict() runtime error guards", () => {
  test("throws TypedCodecError on unknown key", () => {
    const acme = buildAcme();
    const dict = acme.toDict() as Record<string, unknown>;
    dict["unknown_field"] = "oops";

    assert.throws(
      () => ParityCompany.fromDict(dict as never),
      (err: unknown) => {
        assert.ok(err instanceof TypedCodecError);
        assert.ok((err as TypedCodecError).message.includes("unknown_field"));
        return true;
      },
    );
  });

  test("throws TypeError on missing required field", () => {
    // Pass a dict that omits the required `id` field.
    assert.throws(
      () => ParityCompany.fromDict({ name: "Acme" } as never),
      (err: unknown) => {
        assert.ok(err instanceof TypeError);
        assert.ok((err as TypeError).message.includes("id"));
        return true;
      },
    );
  });

  test("fromDict on parity-person with missing required email throws TypeError", () => {
    // email is required for parity-person; id is inherited required.
    assert.throws(
      () =>
        ParityPerson.fromDict({
          id: "person:test",
          // email deliberately omitted
        } as never),
      (err: unknown) => {
        assert.ok(err instanceof TypeError);
        return true;
      },
    );
  });
});

// ---------------------------------------------------------------------------
// Tests: long/bigint and decimal/datetime encoding correctness
// ---------------------------------------------------------------------------

describe("canonical value encoding", () => {
  test("long round-trips as bigint, not number", () => {
    const alice = buildAlice();
    const dict = alice.toDict();

    assert.equal(typeof dict["age"], "bigint");
    assert.equal(dict["age"], 37n);
  });

  test("decimal round-trips as string (no 'dec' suffix)", () => {
    const alice = buildAlice();
    const dict = alice.toDict();

    assert.equal(typeof dict["balance"], "string");
    assert.equal(dict["balance"], "1234.56");
    assert.ok(!(dict["balance"] as string).endsWith("dec"));
  });

  test("datetime round-trips as string without trailing zero-nanoseconds", () => {
    const alice = buildAlice();
    const dict = alice.toDict();
    assert.equal(dict["login_at"], "2026-06-01T10:30:00");
  });

  test("datetime-tz round-trips with +00:00 suffix (not 'Z')", () => {
    const alice = buildAlice();
    const dict = alice.toDict();
    assert.equal(dict["seen_at"], "2026-06-01T10:30:00+00:00");
  });

  test("duration round-trips as ISO 8601 duration string", () => {
    const alice = buildAlice();
    const dict = alice.toDict();
    assert.equal(dict["session_length"], "PT2H30M");
  });
});

// ---------------------------------------------------------------------------
// Tests: integration smoke — each parity entity type from write-data.json
// rounds through toDict() and produces the expected canonical attribute shape.
// ---------------------------------------------------------------------------

describe("integration smoke: toDict parity against write-data.json corpus", () => {
  /**
   * The canonical attribute dict from write-data.json for a given entity.
   * Converts `long` string values to bigint to match the TS encoding, and
   * multi-value attributes to plain arrays. This mirrors what Python's to_dict()
   * produces (plain primitives keyed by field name).
   */
  function writeDataToExpected(
    attrs: Record<string, WriteDataAttr | WriteDataAttr[]>,
  ): Record<string, unknown> {
    const result: Record<string, unknown> = {};
    for (const [fieldName, attrOrList] of Object.entries(attrs)) {
      if (Array.isArray(attrOrList)) {
        result[fieldName] = attrOrList.map((a) => attrPlain(a));
      } else {
        result[fieldName] = attrPlain(attrOrList);
      }
    }
    return result;
  }

  test("parity-person alice: toDict() matches write-data.json attribute shape", () => {
    const writeData = loadWriteData();
    const row = writeData.entities.find((e) => e.stable_id === "person:alice")!;
    const expected = writeDataToExpected(row.attributes);

    const alice = buildAlice();
    // Widen the precise InstanceDict to a plain record for the dynamic-key
    // comparison loop below. The precise mapped-type guarantee is verified
    // separately in typed-serialization.typecheck.ts; this loop legitimately
    // uses a string-indexable view to compare against the canonical oracle.
    const actual = alice.toDict() as Record<string, unknown>;

    for (const [key, expectedValue] of Object.entries(expected)) {
      if (Array.isArray(expectedValue)) {
        assert.deepEqual(actual[key], expectedValue, `field ${key} should match`);
      } else {
        assert.equal(actual[key], expectedValue, `field ${key} should match`);
      }
    }
    // toDict() should not include extra keys beyond the write-data attributes.
    assert.deepEqual(Object.keys(actual).sort(), Object.keys(expected).sort());
  });

  test("parity-person bob: toDict() matches write-data.json attribute shape", () => {
    const writeData = loadWriteData();
    const row = writeData.entities.find((e) => e.stable_id === "person:bob")!;
    const expected = writeDataToExpected(row.attributes);

    const bob = buildBob();
    const actual = bob.toDict() as Record<string, unknown>;

    for (const [key, expectedValue] of Object.entries(expected)) {
      assert.equal(actual[key], expectedValue, `field ${key} should match`);
    }
    assert.deepEqual(Object.keys(actual).sort(), Object.keys(expected).sort());
  });

  test("parity-company acme: toDict() matches write-data.json attribute shape", () => {
    const writeData = loadWriteData();
    const row = writeData.entities.find((e) => e.stable_id === "company:acme")!;
    const expected = writeDataToExpected(row.attributes);

    const acme = buildAcme();
    const actual = acme.toDict() as Record<string, unknown>;

    assert.deepEqual(Object.keys(actual).sort(), Object.keys(expected).sort());
    for (const [key, expectedValue] of Object.entries(expected)) {
      assert.equal(actual[key], expectedValue);
    }
  });

  test("parity-email-message: toDict() matches write-data.json attribute shape", () => {
    const writeData = loadWriteData();
    const row = writeData.entities.find((e) => e.stable_id === "email:evidence-1")!;
    const expected = writeDataToExpected(row.attributes);

    const msg = buildEmailMessage();
    const actual = msg.toDict() as Record<string, unknown>;

    assert.deepEqual(Object.keys(actual).sort(), Object.keys(expected).sort());
    for (const [key, expectedValue] of Object.entries(expected)) {
      assert.equal(actual[key], expectedValue);
    }
  });

  test("parity-membership toDict() attribute shape matches write-data.json", () => {
    const writeData = loadWriteData();
    const row = writeData.relations.find((r) => r.stable_id === "membership:alice-acme")!;
    const expected = writeDataToExpected(row.attributes);
    // Roles are NOT in toDict output; only attribute fields.

    const membership = buildMembership();
    const actual = membership.toDict() as Record<string, unknown>;

    assert.deepEqual(Object.keys(actual).sort(), Object.keys(expected).sort());
    for (const [key, expectedValue] of Object.entries(expected)) {
      assert.equal(actual[key], expectedValue, `membership field ${key} should match`);
    }
  });
});
