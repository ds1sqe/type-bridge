import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Card,
  DescriptorRegistry,
  Entity,
  Flag,
  Key,
  Relation,
  TypeFlags,
  TypeNameCase,
  Unique,
  attr,
  field,
  formatTypeName,
  resolveFlags,
  role,
  type SchemaInfo,
} from "../../typescript/index.js";

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
// Multi-value attribute for parity-person.tags (added for Phase 3 full-corpus coverage).
class ParityTag extends attr.String("parity-tag") {}

class ParityParty extends Entity(TypeFlags({ name: "parity-party", abstract: true }), {
  id: field(ParityId, Key),
  name: field(ParityName).optional(),
}) {}

// Full parity-person: inherits from ParityParty and adds the multi-value `tags`
// field. The Phase 1/2 files declared this progressively; here we declare it in
// full so the complete corpus parity test can reference a single model class.
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

class ParityContribution extends Relation("parity-contribution", {
  contributor: role("parity-person", { abstract: true }),
  work: role(ParityEmailMessage),
}) {}

class ParityAuthoring extends Relation(
  "parity-authoring",
  {
    author: role("parity-person", { overrides: "contributor" }),
  },
  { parent: ParityContribution },
) {}

// ---------------------------------------------------------------------------
// Synthetic inherited-relation pair.
//
// The fixture's parity-contribution/parity-authoring pair covers role
// specialization against descriptors.json; this inline pair covers the
// plain-inheritance case (no `as` override, inherited role flattened into the
// child) with hand-written expected descriptors, independent of the fixture.
//
// synthetic-base-rel  — abstract parent relation (no parent_type).
// synthetic-child-rel — concrete child relation that inherits from base.
// ---------------------------------------------------------------------------

class SyntheticBaseRel extends Relation(
  TypeFlags({ name: "synthetic-base-rel", abstract: true }),
  {
    anchor: role("parity-company", { cardinality: Card(1, 1) }),
    note: field(ParityNote),
  },
) {}

class SyntheticChildRel extends Relation(
  "synthetic-child-rel",
  {
    extra: role("parity-person", { cardinality: Card(0, 5) }),
    kind: field(ParityKind),
  },
  { parent: SyntheticBaseRel },
) {}

class RelatesOnlyRel extends Relation("typed-relates-only-rel", {
  definition: role(),
  actor: role(ParityCompany),
}) {}

class PlaysCardEmployment extends Relation("typed-plays-card-employment", {
  employee: role(ParityPerson),
  employer: role(ParityCompany, { cardinality: Card(1, 1), playsCardinality: Card(0, 1) }),
}) {}

class PlaysCardMulti extends Relation("typed-plays-card-multi", {
  participant: role(ParityPerson, ParityCompany, { playsCardinality: Card(0, 5) }),
}) {}

class PlaysCardContract extends Relation("typed-plays-card-contract", {
  party: role(ParityPerson),
}) {}

class PlaysCardDispute extends Relation("typed-plays-card-dispute", {
  subject: role(PlaysCardContract, { playsCardinality: Card(0, 1) }),
}) {}

describe("typed attribute and flag layer", () => {
  test("all attribute factories expose the expected wire value type", () => {
    assert.equal(ParityName.valueType, "string");
    assert.equal(ParityAge.valueType, "long");
    assert.equal(ParityScore.valueType, "double");
    assert.equal(ParityActive.valueType, "boolean");
    assert.equal(ParityBirthDate.valueType, "date");
    assert.equal(ParityLoginAt.valueType, "datetime");
    assert.equal(ParitySeenAt.valueType, "datetime-tz");
    assert.equal(ParityBalance.valueType, "decimal");
    assert.equal(ParitySessionLength.valueType, "duration");
    assert.equal(new ParityAge(30n).value, 30n);
  });

  test("formatTypeName matches the Python case formatter", () => {
    assert.equal(formatTypeName("PersonName", TypeNameCase.LOWERCASE), "personname");
    assert.equal(formatTypeName("PersonName", TypeNameCase.CLASS_NAME), "PersonName");
    assert.equal(formatTypeName("PersonName", TypeNameCase.SNAKE_CASE), "person_name");
    assert.equal(formatTypeName("HTTPResponse", TypeNameCase.SNAKE_CASE), "http_response");
  });

  test("Flag and Card resolve to descriptor annotations and cardinality", () => {
    assert.deepEqual(resolveFlags([Flag(Key)]), {
      kind: "flag",
      annotations: ["Key"],
      cardinality: [1, 1],
      isOrdered: false,
      isDistinct: false,
    });
    assert.deepEqual(resolveFlags([Flag(Card(1, 5))]), {
      kind: "flag",
      annotations: [{ Card: [1, 5] }],
      cardinality: [1, 5],
      isOrdered: false,
      isDistinct: false,
    });
    assert.deepEqual(resolveFlags([Flag(Card(0))]), {
      kind: "flag",
      annotations: [{ Card: [0, null] }],
      cardinality: [0, null],
      isOrdered: false,
      isDistinct: false,
    });
    assert.deepEqual(resolveFlags([Unique]), {
      kind: "flag",
      annotations: ["Unique"],
      cardinality: null,
      isOrdered: false,
      isDistinct: false,
    });
  });

  test("attribute type metadata reaches schemaInfo without changing descriptor JSON", () => {
    class TsRootCode extends attr.String("ts-root-code", { abstract: true }) {}
    class TsCode extends attr.String("ts-code", {
      parent: TsRootCode,
      regex: "^[A-Z]+$",
    }) {}
    class TsState extends attr.String("ts-state", {
      values: ["open", "closed"],
      independent: true,
    }) {}
    class TsScore extends attr.Integer("ts-score", { range: ["1", "5"] }) {}
    class TsAnnotated extends Entity("ts-annotated", {
      code: field(TsCode),
      state: field(TsState),
      score: field(TsScore),
    }) {}

    const descriptor = TsAnnotated.descriptor();
    assert.deepEqual(descriptor.owned_attributes, [
      {
        field_name: "code",
        attr_name: "ts-code",
        value_type: "string",
        annotations: [],
        is_optional: false,
        is_ordered: false,
      },
      {
        field_name: "state",
        attr_name: "ts-state",
        value_type: "string",
        annotations: [],
        is_optional: false,
        is_ordered: false,
      },
      {
        field_name: "score",
        attr_name: "ts-score",
        value_type: "long",
        annotations: [],
        is_optional: false,
        is_ordered: false,
      },
    ]);

    const registry = new DescriptorRegistry(fakeNativeRegistry());
    registry.registerEntity(descriptor);
    const info = registry.schemaInfo();

    assert.deepEqual(info.attributes["ts-root-code"], {
      attr_name: "ts-root-code",
      value_type: "string",
      is_abstract: true,
    });
    assert.deepEqual(info.attributes["ts-code"], {
      attr_name: "ts-code",
      value_type: "string",
      parent_type: "ts-root-code",
      regex: "^[A-Z]+$",
    });
    assert.deepEqual(info.attributes["ts-state"], {
      attr_name: "ts-state",
      value_type: "string",
      is_independent: true,
      allowed_values: ["open", "closed"],
    });
    assert.deepEqual(info.attributes["ts-score"], {
      attr_name: "ts-score",
      value_type: "long",
      range: ["1", "5"],
    });
  });
});

describe("typed Entity and Relation factories", () => {
  test("instances preserve branded field values and optional fields", () => {
    const company = new ParityCompany({
      id: new ParityId("company-1"),
      name: new ParityName("Acme"),
    });
    assert.equal(company.id.value, "company-1");
    assert.equal(company.name.value, "Acme");

    const party = new ParityParty({ id: new ParityId("party-1") });
    assert.equal(party.name, undefined);
  });

  test("constructing without a required field throws at runtime", () => {
    assert.throws(
      () => new ParityCompany({ id: new ParityId("company-1") } as never),
      /missing required field "name"/,
    );
  });

  test("subclasses can add methods that read typed fields", () => {
    class Person extends Entity("parity-person-flat", {
      name: field(ParityName),
      age: field(ParityAge).optional(),
    }) {
      label(): string {
        return `${this.name.value}:${this.age?.value ?? "unknown"}`;
      }
    }

    const person = new Person({ name: new ParityName("Alice"), age: new ParityAge(30n) });
    assert.equal(person.label(), "Alice:30");
  });

  test("relations emit existing role descriptor shape with multi-player roles", () => {
    assert.deepEqual(ParityMembership.descriptor().roles, [
      {
        role_name: "member",
        player_type_names: ["parity-person"],
        cardinality: [1, 1],
        plays_cardinality: null,
        overrides: null,
        is_abstract: false,
        ordered: false,
        distinct: false,
      },
      {
        role_name: "organization",
        player_type_names: ["parity-company"],
        cardinality: [1, 1],
        plays_cardinality: null,
        overrides: null,
        is_abstract: false,
        ordered: false,
        distinct: false,
      },
      {
        role_name: "evidence",
        player_type_names: ["parity-person", "parity-email-message"],
        cardinality: [0, 5],
        plays_cardinality: null,
        overrides: null,
        is_abstract: false,
        ordered: false,
        distinct: false,
      },
    ]);
  });

  test("relations can emit a relates-only role without player types", () => {
    assert.deepEqual(RelatesOnlyRel.descriptor().roles, [
      {
        role_name: "definition",
        player_type_names: [],
        cardinality: null,
        plays_cardinality: null,
        overrides: null,
        is_abstract: false,
        ordered: false,
        distinct: false,
      },
      {
        role_name: "actor",
        player_type_names: ["parity-company"],
        cardinality: null,
        plays_cardinality: null,
        overrides: null,
        is_abstract: false,
        ordered: false,
        distinct: false,
      },
    ]);
  });

  test("playsCardinality is emitted into role descriptor and rejects relates-only roles", () => {
    const spec = role(ParityCompany, { playsCardinality: Card(0, 1) });
    assert.deepEqual(spec.playsCardinality, [0, 1]);

    const descriptor = PlaysCardEmployment.descriptor() as RelationDescriptor;
    assert.deepEqual(descriptor.roles, [
      {
        role_name: "employee",
        player_type_names: ["parity-person"],
        cardinality: null,
        plays_cardinality: null,
        overrides: null,
        is_abstract: false,
        ordered: false,
        distinct: false,
      },
      {
        role_name: "employer",
        player_type_names: ["parity-company"],
        cardinality: [1, 1],
        plays_cardinality: [0, 1],
        overrides: null,
        is_abstract: false,
        ordered: false,
        distinct: false,
      },
    ]);
    assert.deepEqual(JSON.parse(JSON.stringify(descriptor)), descriptor);

    assert.throws(
      () => role({ playsCardinality: Card(0, 1) } as never),
      /playsCardinality requires at least one role player/,
    );
  });

  test("plays_cardinality is emitted on role descriptors for Rust from_descriptors to consume", () => {
    // The plays_cardinalities overlay is built by Rust SchemaInfo::from_descriptors using the
    // plays_cardinality field on each role descriptor. Verify the authoring datum is present
    // on the descriptor so the Rust layer can build the overlay.
    const employerRole = PlaysCardEmployment.descriptor().roles.find(
      (r) => r.role_name === "employer",
    );
    assert.deepEqual(employerRole?.plays_cardinality, [0, 1]);

    const employeeRole = PlaysCardEmployment.descriptor().roles.find(
      (r) => r.role_name === "employee",
    );
    assert.deepEqual(employeeRole?.plays_cardinality, null);

    const relatesOnlyRole = RelatesOnlyRel.descriptor().roles.find(
      (r) => r.role_name === "definition",
    );
    assert.deepEqual(relatesOnlyRole?.plays_cardinality, null);

    // Verify the descriptor round-trips through JSON (plays_cardinality must be enumerable).
    const descriptor = PlaysCardEmployment.descriptor();
    assert.deepEqual(JSON.parse(JSON.stringify(descriptor)), descriptor);
  });
});

describe("descriptor emission parity", () => {
  test("07 corpus descriptors match the shared Python fixture after normalization", () => {
    const fixture = JSON.parse(
      fs.readFileSync(
        path.resolve(process.cwd(), "../../../tests/integration/parity/fixtures/descriptors.json"),
        "utf8",
      ),
    ) as DescriptorSnapshot;

    const actual = normalizeDescriptorSnapshot({
      version: 1,
      entities: [
        ParityParty.descriptor(),
        ParityCompany.descriptor(),
        ParityEmailMessage.descriptor(),
      ],
      relations: [
        ParityMembership.descriptor(),
        ParityTokenOrigin.descriptor(),
        ParityContribution.descriptor(),
        ParityAuthoring.descriptor(),
      ],
    });
    const expected = normalizeDescriptorSnapshot({
      version: fixture.version,
      entities: fixture.entities.filter((descriptor) =>
        ["parity-party", "parity-company", "parity-email-message"].includes(descriptor.type_name),
      ),
      relations: fixture.relations,
    });

    assert.deepEqual(actual, expected);
  });

  test("flat all-value-types descriptor covers key, unique, optional, and explicit Card", () => {
    class ParityAllValues extends Entity("parity-all-values", {
      id: field(ParityId, Key),
      email: field(ParityEmail, Unique),
      name: field(ParityName).optional(),
      age: field(ParityAge),
      score: field(ParityScore),
      active: field(ParityActive),
      birth_date: field(ParityBirthDate),
      login_at: field(ParityLoginAt),
      seen_at: field(ParitySeenAt),
      balance: field(ParityBalance),
      session_length: field(ParitySessionLength),
      carded: field(ParityName, Card(1, 5)),
    }) {}

    assert.deepEqual(ParityAllValues.descriptor(), {
      type_name: "parity-all-values",
      is_abstract: false,
      parent_type: null,
      owned_attributes: [
        attrDescriptor("id", "parity-id", "string", ["Key"], false),
        attrDescriptor("email", "parity-email", "string", ["Unique"], false),
        attrDescriptor("name", "parity-name", "string", [], true),
        attrDescriptor("age", "parity-age", "long", [], false),
        attrDescriptor("score", "parity-score", "double", [], false),
        attrDescriptor("active", "parity-active", "boolean", [], false),
        attrDescriptor("birth_date", "parity-birth-date", "date", [], false),
        attrDescriptor("login_at", "parity-login-at", "datetime", [], false),
        attrDescriptor("seen_at", "parity-seen-at", "datetime-tz", [], false),
        attrDescriptor("balance", "parity-balance", "decimal", [], false),
        attrDescriptor("session_length", "parity-session-length", "duration", [], false),
        attrDescriptor("carded", "parity-name", "string", [{ Card: [1, 5] }], false),
      ],
    });
  });
});

// ---------------------------------------------------------------------------
// Phase 3: Full-corpus descriptor parity gate (all entities + relations)
//
// This is the offline descriptor-equivalence gate the typed surface owes before
// Plan 11 wires it into the live parity job. Every type in descriptors.json must
// emit a normalized descriptor byte-identical to the fixture. parity-person —
// previously deferred in the 07-corpus test above — is included in full here
// (parent_type, flattened owned_attributes including the multi-value `tags` Card).
// ---------------------------------------------------------------------------

describe("Phase 3 full-corpus parity gate (all types in descriptors.json)", () => {
  test("all entities and relations in descriptors.json match the fixture after normalization", () => {
    const fixture = JSON.parse(
      fs.readFileSync(
        path.resolve(process.cwd(), "../../../tests/integration/parity/fixtures/descriptors.json"),
        "utf8",
      ),
    ) as DescriptorSnapshot;

    const actual = normalizeDescriptorSnapshot({
      version: 1,
      entities: [
        ParityParty.descriptor(),
        ParityPerson.descriptor(),
        ParityCompany.descriptor(),
        ParityEmailMessage.descriptor(),
      ],
      relations: [
        ParityMembership.descriptor(),
        ParityTokenOrigin.descriptor(),
        ParityContribution.descriptor(),
        ParityAuthoring.descriptor(),
      ],
    });

    const expected = normalizeDescriptorSnapshot({
      version: fixture.version,
      entities: fixture.entities,
      relations: fixture.relations,
    });

    assert.deepEqual(actual, expected);
  });

  test("parity-person parent_type and full flattened owned_attributes pass (parity-person deferred gap closed)", () => {
    const fixture = JSON.parse(
      fs.readFileSync(
        path.resolve(process.cwd(), "../../../tests/integration/parity/fixtures/descriptors.json"),
        "utf8",
      ),
    ) as DescriptorSnapshot;

    const fixturePersonRaw = fixture.entities.find((d) => d.type_name === "parity-person");
    assert.ok(fixturePersonRaw != null, "parity-person must exist in fixture");

    const actual = normalizeDescriptorSnapshot({ version: 1, entities: [ParityPerson.descriptor()], relations: [] });
    const expected = normalizeDescriptorSnapshot({ version: fixture.version, entities: [fixturePersonRaw], relations: [] });

    assert.deepEqual(actual, expected);
  });
});

// ---------------------------------------------------------------------------
// Phase 3: Relation inheritance coverage (synthetic pair, no live TypeDB).
//
// No relation in descriptors.json carries a parent_type, so coverage for
// relation inheritance uses the inline synthetic-base-rel / synthetic-child-rel
// pair declared at the top of this file. Expected descriptors are hand-written
// to match what the typed factory must emit under the same flattening rules as
// entity inheritance.
// ---------------------------------------------------------------------------

describe("Phase 3 relation inheritance (synthetic parent + child relation)", () => {
  test("synthetic-base-rel emits is_abstract:true, parent_type:null, its own roles and attrs", () => {
    const d = SyntheticBaseRel.descriptor() as RelationDescriptor;
    assert.equal(d.type_name, "synthetic-base-rel");
    assert.equal(d.is_abstract, true);
    assert.equal(d.parent_type, null);

    // Owned attributes: just `note` (the only FieldSpec in the base schema).
    assert.deepEqual(d.owned_attributes, [
      attrDescriptor("note", "parity-note", "string", [], false),
    ]);

    // Roles: just `anchor` from the base schema.
    assert.deepEqual(d.roles, [
      { role_name: "anchor", player_type_names: ["parity-company"], cardinality: [1, 1], plays_cardinality: null, overrides: null, is_abstract: false, ordered: false, distinct: false },
    ]);
  });

  test("synthetic-child-rel emits parent_type and flattened inherited roles + attrs", () => {
    const d = SyntheticChildRel.descriptor() as RelationDescriptor;
    assert.equal(d.type_name, "synthetic-child-rel");
    assert.equal(d.is_abstract, false);
    assert.equal(d.parent_type, "synthetic-base-rel");

    // owned_attributes: parent `note` re-listed, then child-local `kind`.
    // (Order is normalizer-insensitive; both must be present with correct content.)
    const fieldNames = d.owned_attributes.map((a) => a.field_name);
    assert.ok(fieldNames.includes("note"), "inherited `note` must be re-listed");
    assert.ok(fieldNames.includes("kind"), "child-local `kind` must be present");

    const noteAttr = d.owned_attributes.find((a) => a.field_name === "note");
    assert.deepEqual(noteAttr, attrDescriptor("note", "parity-note", "string", [], false));

    // roles: inherited `anchor` re-listed, then child-local `extra`.
    const roleNames = d.roles.map((r) => r.role_name);
    assert.ok(roleNames.includes("anchor"), "inherited `anchor` role must be re-listed");
    assert.ok(roleNames.includes("extra"), "child-local `extra` role must be present");

    const anchorRole = d.roles.find((r) => r.role_name === "anchor");
    assert.deepEqual(anchorRole, {
      role_name: "anchor",
      player_type_names: ["parity-company"],
      cardinality: [1, 1],
      plays_cardinality: null,
      overrides: null,
      is_abstract: false,
      ordered: false,
      distinct: false,
    });

    const extraRole = d.roles.find((r) => r.role_name === "extra");
    assert.deepEqual(extraRole, {
      role_name: "extra",
      player_type_names: ["parity-person"],
      cardinality: [0, 5],
      plays_cardinality: null,
      overrides: null,
      is_abstract: false,
      ordered: false,
      distinct: false,
    });
  });

  test("synthetic-child-rel normalized descriptor matches a hand-written expected descriptor", () => {
    // Hand-written expected: the full descriptor the factory must produce for a
    // child relation with parent_type, flattened roles, and flattened attrs.
    const expected = normalizeDescriptorSnapshot({
      version: 1,
      entities: [],
      relations: [
        {
          type_name: "synthetic-child-rel",
          is_abstract: false,
          parent_type: "synthetic-base-rel",
          owned_attributes: [
            // parent attrs re-listed
            attrDescriptor("note", "parity-note", "string", [], false),
            // child-local attr
            attrDescriptor("kind", "parity-kind", "string", [], false),
          ],
          roles: [
            // parent role re-listed
            { role_name: "anchor", player_type_names: ["parity-company"], cardinality: [1, 1] as [number, number | null], plays_cardinality: null, overrides: null, is_abstract: false, ordered: false, distinct: false },
            // child-local role
            { role_name: "extra", player_type_names: ["parity-person"], cardinality: [0, 5] as [number, number | null], plays_cardinality: null, overrides: null, is_abstract: false, ordered: false, distinct: false },
          ],
        },
      ],
    });

    const actual = normalizeDescriptorSnapshot({
      version: 1,
      entities: [],
      relations: [SyntheticChildRel.descriptor() as RelationDescriptor],
    });

    assert.deepEqual(actual, expected);
  });
});

type DescriptorSnapshot = {
  version: number;
  entities: EntityDescriptor[];
  relations: RelationDescriptor[];
};

type Annotation = "Key" | "Unique" | "Distinct" | { Card: [number, number | null] };
type EntityDescriptor = {
  type_name: string;
  is_abstract: boolean;
  parent_type: string | null;
  owned_attributes: OwnedAttributeDescriptor[];
};
type RelationDescriptor = EntityDescriptor & { roles: RoleDescriptor[] };
type OwnedAttributeDescriptor = {
  field_name: string;
  attr_name: string;
  value_type: string;
  annotations: Annotation[];
  is_optional: boolean;
  is_ordered: boolean;
};
type RoleDescriptor = {
  role_name: string;
  player_type_names: string[];
  cardinality: [number, number | null] | null;
  plays_cardinality: [number, number | null] | null;
  overrides: string | null;
  is_abstract: boolean;
  ordered: boolean;
  distinct: boolean;
};
type SchemaValueType = SchemaInfo["attributes"][string]["value_type"];

function attrDescriptor(
  fieldName: string,
  attrName: string,
  valueType: string,
  annotations: Annotation[],
  isOptional: boolean,
  isOrdered = false,
): OwnedAttributeDescriptor {
  return {
    field_name: fieldName,
    attr_name: attrName,
    value_type: valueType,
    annotations,
    is_optional: isOptional,
    is_ordered: isOrdered,
  };
}

// Byte-faithful TS port of tests/integration/parity/canonical.py
// (normalize_descriptor_snapshot). That module is the source of truth for the
// cross-language parity contract; keep this in sync if it changes.
function normalizeDescriptorSnapshot(snapshot: DescriptorSnapshot): DescriptorSnapshot {
  return {
    version: snapshot.version,
    entities: snapshot.entities.map(normalizeTypeDescriptor).sort(byTypeName),
    relations: snapshot.relations.map(normalizeRelationDescriptor).sort(byTypeName),
  };
}

function normalizeTypeDescriptor(descriptor: EntityDescriptor): EntityDescriptor {
  return {
    ...descriptor,
    owned_attributes: descriptor.owned_attributes
      .map(normalizeAttributeDescriptor)
      .sort((left, right) => left.field_name.localeCompare(right.field_name)),
  };
}

function fakeNativeRegistry() {
  const entities = new Map<string, EntityDescriptor>();
  const relations = new Map<string, RelationDescriptor>();

  return {
    registerEntityJson(descriptorJson: string): string {
      const descriptor = JSON.parse(descriptorJson) as EntityDescriptor;
      entities.set(descriptor.type_name, descriptor);
      return JSON.stringify(descriptor);
    },
    registerRelationJson(descriptorJson: string): string {
      const descriptor = JSON.parse(descriptorJson) as RelationDescriptor;
      relations.set(descriptor.type_name, descriptor);
      return JSON.stringify(descriptor);
    },
    entityJson(typeName: string): string {
      return JSON.stringify(entities.get(typeName));
    },
    relationJson(typeName: string): string {
      return JSON.stringify(relations.get(typeName));
    },
    snapshotJson(): string {
      return JSON.stringify([
        ...[...entities.values()].map((descriptor) => ({ kind: "entity", descriptor })),
        ...[...relations.values()].map((descriptor) => ({ kind: "relation", descriptor })),
      ]);
    },
    schemaInfoJson(): string {
      const info: SchemaInfo = { entities: {}, relations: {}, attributes: {} };
      for (const descriptor of entities.values()) {
        info.entities[descriptor.type_name] = {
          type_name: descriptor.type_name,
          is_abstract: descriptor.is_abstract,
          parent_type: descriptor.parent_type,
          owned_attributes: schemaOwnedAttributes(descriptor.owned_attributes),
        };
        rememberFallbackAttributes(info, descriptor.owned_attributes);
      }
      for (const descriptor of relations.values()) {
        info.relations[descriptor.type_name] = {
          type_name: descriptor.type_name,
          is_abstract: descriptor.is_abstract,
          parent_type: descriptor.parent_type,
          owned_attributes: schemaOwnedAttributes(descriptor.owned_attributes),
          roles: descriptor.roles,
        };
        rememberFallbackAttributes(info, descriptor.owned_attributes);
      }
      return JSON.stringify(info);
    },
  };
}

function schemaOwnedAttributes(
  attributes: EntityDescriptor["owned_attributes"],
): SchemaInfo["entities"][string]["owned_attributes"] {
  return attributes.map((attribute) => ({
    attr_name: attribute.attr_name,
    value_type: attribute.value_type as SchemaValueType,
    annotations: attribute.annotations,
    is_ordered: attribute.is_ordered,
  }));
}

function rememberFallbackAttributes(
  info: SchemaInfo,
  attributes: EntityDescriptor["owned_attributes"],
): void {
  for (const attribute of attributes) {
    info.attributes[attribute.attr_name] ??= {
      attr_name: attribute.attr_name,
      value_type: attribute.value_type as SchemaValueType,
    };
  }
}

function normalizeRelationDescriptor(descriptor: RelationDescriptor): RelationDescriptor {
  return {
    ...normalizeTypeDescriptor(descriptor),
    roles: descriptor.roles
      .map((roleDescriptor) => ({
        ...roleDescriptor,
        player_type_names: [...roleDescriptor.player_type_names].sort(),
      }))
      .sort((left, right) => left.role_name.localeCompare(right.role_name)),
  };
}

function normalizeAttributeDescriptor(attribute: OwnedAttributeDescriptor): OwnedAttributeDescriptor {
  return {
    ...attribute,
    annotations: normalizeAnnotations(attribute.annotations, attribute.is_optional),
  };
}

function normalizeAnnotations(annotations: Annotation[], isOptional: boolean): Annotation[] {
  return annotations
    .filter((annotation) => {
      if (isOptional && isCardAnnotation(annotation, 0, 1)) {
        return false;
      }
      return !isCardAnnotation(annotation, 1, 1);
    })
    .sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right)));
}

function isCardAnnotation(
  annotation: Annotation,
  min: number,
  max: number | null,
): annotation is { Card: [number, number | null] } {
  return (
    typeof annotation === "object" &&
    annotation !== null &&
    annotation.Card[0] === min &&
    annotation.Card[1] === max
  );
}

function byTypeName<T extends { type_name: string }>(left: T, right: T): number {
  return left.type_name.localeCompare(right.type_name);
}
