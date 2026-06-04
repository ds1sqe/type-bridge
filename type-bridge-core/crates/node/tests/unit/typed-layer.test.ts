import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Card,
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

class ParityParty extends Entity(TypeFlags({ name: "parity-party", abstract: true }), {
  id: field(ParityId, Key),
  name: field(ParityName).optional(),
}) {}

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
    });
    assert.deepEqual(resolveFlags([Flag(Card(1, 5))]), {
      kind: "flag",
      annotations: [{ Card: [1, 5] }],
      cardinality: [1, 5],
    });
    assert.deepEqual(resolveFlags([Flag(Card(0))]), {
      kind: "flag",
      annotations: [{ Card: [0, null] }],
      cardinality: [0, null],
    });
    assert.deepEqual(resolveFlags([Unique]), {
      kind: "flag",
      annotations: ["Unique"],
      cardinality: null,
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
      },
      {
        role_name: "organization",
        player_type_names: ["parity-company"],
        cardinality: [1, 1],
      },
      {
        role_name: "evidence",
        player_type_names: ["parity-person", "parity-email-message"],
        cardinality: [0, 5],
      },
    ]);
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
      relations: [ParityMembership.descriptor(), ParityTokenOrigin.descriptor()],
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

type DescriptorSnapshot = {
  version: number;
  entities: EntityDescriptor[];
  relations: RelationDescriptor[];
};

type Annotation = "Key" | "Unique" | { Card: [number, number | null] };
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
};
type RoleDescriptor = {
  role_name: string;
  player_type_names: string[];
  cardinality: [number, number | null] | null;
};

function attrDescriptor(
  fieldName: string,
  attrName: string,
  valueType: string,
  annotations: Annotation[],
  isOptional: boolean,
): OwnedAttributeDescriptor {
  return {
    field_name: fieldName,
    attr_name: attrName,
    value_type: valueType,
    annotations,
    is_optional: isOptional,
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
