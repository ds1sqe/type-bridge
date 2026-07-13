import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Card,
  Entity,
  Key,
  TypeFlags,
  Unique,
  attr,
  field,
} from "../../typescript/index.js";

// ---------------------------------------------------------------------------
// Attribute classes (same parity corpus as typed-inheritance.test.ts, plus Tag)
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
// Multi-value attribute for the `tags` field.
class ParityTag extends attr.String("parity-tag") {}

// ---------------------------------------------------------------------------
// Model declarations
// ---------------------------------------------------------------------------

// Abstract parent — matches parity-party in descriptors.json.
class ParityParty extends Entity(TypeFlags({ name: "parity-party", abstract: true }), {
  id: field(ParityId, Key),
  name: field(ParityName).optional(),
}) {}

// Concrete child — inherits from ParityParty and adds the multi-value `tags`
// field alongside the scalar fields. This is the FULL parity-person, including
// the list attribute that was deferred from Phase 1.
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
    // Multi-value list attribute: Card(0, 5) means 0–5 values, is_optional: true.
    tags: field(ParityTag).list(Card(0, 5)),
  },
  { parent: ParityParty },
) {}

// ---------------------------------------------------------------------------
// Descriptor emission tests for the list field
// ---------------------------------------------------------------------------

describe("list field descriptor emission", () => {
  test("field(Tag).list(Card(0,5)) emits Card annotation and is_optional:true", () => {
    const d = ParityPerson.descriptor();
    const tagsAttr = d.owned_attributes.find((a) => a.field_name === "tags");
    assert.ok(tagsAttr != null, "tags must be present in owned_attributes");
    assert.deepEqual(tagsAttr.annotations, [{ Card: [0, 5] }]);
    assert.equal(tagsAttr.is_optional, true);
    assert.equal(tagsAttr.attr_name, "parity-tag");
    assert.equal(tagsAttr.value_type, "string");
  });

  test("unbounded list Card(0) emits [0, null]", () => {
    // Declare a minimal entity with an unbounded list field.
    class TagAttr extends attr.String("parity-tag") {}
    class Minimal extends Entity("minimal-test", {
      tags: field(TagAttr).list(Card(0)),
    }) {}

    const d = Minimal.descriptor();
    const tagsAttr = d.owned_attributes.find((a) => a.field_name === "tags");
    assert.ok(tagsAttr != null, "tags must be present");
    assert.deepEqual(tagsAttr.annotations, [{ Card: [0, null] }]);
    assert.equal(tagsAttr.is_optional, true);
  });

  test("list with card_min > 0 is NOT optional", () => {
    class TagAttr extends attr.String("parity-tag") {}
    class Minimal extends Entity("minimal-req", {
      tags: field(TagAttr).list(Card(1, 5)),
    }) {}

    const d = Minimal.descriptor();
    const tagsAttr = d.owned_attributes.find((a) => a.field_name === "tags");
    assert.ok(tagsAttr != null);
    assert.deepEqual(tagsAttr.annotations, [{ Card: [1, 5] }]);
    assert.equal(tagsAttr.is_optional, false);
  });

  test("list retains Unique before its explicit Card annotation", () => {
    class AliasAttr extends attr.String("unique-list-alias") {}
    class UniqueList extends Entity("unique-list", {
      aliases: field(AliasAttr, Unique).list(Card(0, 3)),
    }) {}

    const aliases = UniqueList.descriptor().owned_attributes[0];
    assert.ok(aliases != null);
    assert.deepEqual(aliases.annotations, ["Unique", { Card: [0, 3] }]);
    assert.equal(aliases.is_optional, true);
  });

  test("ordered().distinct() emits a list descriptor without inventing Card", () => {
    class OrderedAttr extends attr.String("ordered-list-value") {}
    class OrderedList extends Entity("ordered-list", {
      values: field(OrderedAttr).optional().ordered().distinct(),
    }) {}

    const values = OrderedList.descriptor().owned_attributes[0];
    assert.ok(values != null);
    assert.deepEqual(values.annotations, ["Distinct"]);
    assert.equal(values.is_optional, true);
    assert.equal(values.is_ordered, true);
    assert.throws(
      () => field(OrderedAttr).list(Card(0, 3)).distinct(),
      /requires an ordered list field/,
    );
  });

  test("single-value field emits NO Card annotation (regression guard)", () => {
    const d = ParityPerson.descriptor();
    const emailAttr = d.owned_attributes.find((a) => a.field_name === "email");
    assert.ok(emailAttr != null, "email must be present");
    // A scalar field with Unique should have ["Unique"] but NO Card entry.
    const hasCard = emailAttr.annotations.some(
      (a) => typeof a === "object" && "Card" in a,
    );
    assert.equal(hasCard, false, "scalar field must not emit a Card annotation");
  });

  test("scalar optional field emits NO Card annotation", () => {
    const d = ParityPerson.descriptor();
    const nameAttr = d.owned_attributes.find((a) => a.field_name === "name");
    assert.ok(nameAttr != null, "name (inherited optional scalar) must be present");
    const hasCard = nameAttr.annotations.some(
      (a) => typeof a === "object" && "Card" in a,
    );
    assert.equal(hasCard, false, "optional scalar field must not emit a Card annotation");
  });
});

// ---------------------------------------------------------------------------
// Runtime construction tests for list fields
// ---------------------------------------------------------------------------

describe("list field runtime construction", () => {
  test("list field accepts an array of the correct brand", () => {
    const person = new ParityPerson({
      id: new ParityId("p1"),
      email: new ParityEmail("p@example.com"),
      tags: [new ParityTag("ts"), new ParityTag("js")],
    });
    assert.ok(Array.isArray(person.tags), "tags must be an array");
    assert.equal((person.tags as ParityTag[]).length, 2);
    assert.equal((person.tags as ParityTag[])[0].value, "ts");
  });

  test("list field can be omitted when card_min == 0 (optional)", () => {
    const person = new ParityPerson({
      id: new ParityId("p2"),
      email: new ParityEmail("p2@example.com"),
    });
    // tags is optional (Card(0,5)), so omitting it should yield undefined.
    assert.equal(person.tags, undefined);
  });

  test("list field accepts an empty array", () => {
    const person = new ParityPerson({
      id: new ParityId("p3"),
      email: new ParityEmail("p3@example.com"),
      tags: [],
    });
    assert.ok(Array.isArray(person.tags));
    assert.equal((person.tags as ParityTag[]).length, 0);
  });
});

// ---------------------------------------------------------------------------
// Full parity-person descriptor byte-identity gate (Phase 2 integration smoke)
//
// This is the first time the COMPLETE inherited + multi-value parity-person
// descriptor reaches byte-identity with the fixture from the typed surface.
// ---------------------------------------------------------------------------

describe("full parity-person descriptor byte-identity (Phase 2)", () => {
  test("parity-person with tags matches fixture after normalization", () => {
    const fixture = JSON.parse(
      fs.readFileSync(
        path.resolve(process.cwd(), "../../../tests/integration/parity/fixtures/descriptors.json"),
        "utf8",
      ),
    ) as DescriptorSnapshot;

    const actual = normalizeDescriptorSnapshot({
      version: 1,
      entities: [ParityPerson.descriptor()],
      relations: [],
    });

    const expected = normalizeDescriptorSnapshot({
      version: fixture.version,
      entities: fixture.entities.filter((d) => d.type_name === "parity-person"),
      relations: [],
    });

    assert.deepEqual(actual, expected);
  });
});

// ---------------------------------------------------------------------------
// Normalizer (TS port of tests/integration/parity/canonical.py)
// This is the same normalizer inlined in typed-layer.test.ts and
// typed-inheritance.test.ts. Do NOT introduce a different canonicalizer.
// ---------------------------------------------------------------------------

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
  overrides?: string | null;
  is_abstract?: boolean;
  ordered?: boolean;
  distinct?: boolean;
};

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
