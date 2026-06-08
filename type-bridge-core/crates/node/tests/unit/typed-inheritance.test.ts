import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { describe, test } from "node:test";

import {
  Entity,
  Key,
  TypeFlags,
  Unique,
  attr,
  field,
} from "../../typescript/index.js";

// ---------------------------------------------------------------------------
// Attribute classes shared with typed-layer.test.ts for parity-party and the
// scalar fields of parity-person (tags is Phase 2 — omitted here).
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

// ---------------------------------------------------------------------------
// Model declarations under test
// ---------------------------------------------------------------------------

// Abstract parent — no parent_type, is_abstract: true
class ParityParty extends Entity(TypeFlags({ name: "parity-party", abstract: true }), {
  id: field(ParityId, Key),
  name: field(ParityName).optional(),
}) {}

// Concrete child — parent_type: "parity-party", is_abstract: false.
// Phase 1 only: scalar fields, no multi-value `tags` (deferred to Phase 2).
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
  },
  { parent: ParityParty },
) {}

// ---------------------------------------------------------------------------
// Descriptor emission tests
// ---------------------------------------------------------------------------

describe("inheritance descriptor emission", () => {
  test("parity-party emits is_abstract:true and parent_type:null", () => {
    const d = ParityParty.descriptor();
    assert.equal(d.type_name, "parity-party");
    assert.equal(d.is_abstract, true);
    assert.equal(d.parent_type, null);
  });

  test("parity-person emits is_abstract:false and parent_type:'parity-party'", () => {
    const d = ParityPerson.descriptor();
    assert.equal(d.type_name, "parity-person");
    assert.equal(d.is_abstract, false);
    assert.equal(d.parent_type, "parity-party");
  });

  test("parity-person owned_attributes contains flattened parent fields", () => {
    const d = ParityPerson.descriptor();
    const fieldNames = d.owned_attributes.map((a) => a.field_name);
    // Parent attrs must be present in the child's list.
    assert.ok(fieldNames.includes("id"), "id (from parent) must be re-listed");
    assert.ok(fieldNames.includes("name"), "name (from parent) must be re-listed");
    assert.ok(fieldNames.includes("email"), "email (child-local) must be present");
  });

  test("parent field descriptors re-listed in child are content-identical to parent's", () => {
    const parentAttrs = ParityParty.descriptor().owned_attributes;
    const childAttrs = ParityPerson.descriptor().owned_attributes;

    for (const parentAttr of parentAttrs) {
      const childAttr = childAttrs.find((a) => a.field_name === parentAttr.field_name);
      assert.ok(childAttr != null, `inherited field "${parentAttr.field_name}" must appear in child`);
      assert.deepEqual(childAttr, parentAttr, `field "${parentAttr.field_name}" must be content-identical in child`);
    }
  });
});

// ---------------------------------------------------------------------------
// Parity gate: normalize + assert byte-identity against descriptors.json
// (parity-party and scalar fields of parity-person)
// ---------------------------------------------------------------------------

describe("descriptor parity against fixtures (Phase 1 inheritance subset)", () => {
  test("parity-party and scalar parity-person match fixture after normalization", () => {
    const fixture = JSON.parse(
      fs.readFileSync(
        path.resolve(process.cwd(), "../../../tests/integration/parity/fixtures/descriptors.json"),
        "utf8",
      ),
    ) as DescriptorSnapshot;

    // Actual: parity-party (full), parity-person minus `tags`.
    const personDescriptor = ParityPerson.descriptor();
    // Remove `tags` from our emission for the Phase 1 comparison (tags is Phase 2).
    const personDescriptorPhase1 = {
      ...personDescriptor,
      owned_attributes: personDescriptor.owned_attributes.filter((a) => a.field_name !== "tags"),
    };

    const actual = normalizeDescriptorSnapshot({
      version: 1,
      entities: [ParityParty.descriptor(), personDescriptorPhase1],
      relations: [],
    });

    // Expected: the same two entities from the fixture, also without `tags` on
    // parity-person (so both sides are in the same Phase 1 subset).
    const expected = normalizeDescriptorSnapshot({
      version: fixture.version,
      entities: fixture.entities
        .filter((d) => ["parity-party", "parity-person"].includes(d.type_name))
        .map((d) => {
          if (d.type_name !== "parity-person") return d;
          return {
            ...d,
            owned_attributes: d.owned_attributes.filter((a: OwnedAttributeDescriptor) => a.field_name !== "tags"),
          };
        }),
      relations: [],
    });

    assert.deepEqual(actual, expected);
  });
});

// ---------------------------------------------------------------------------
// Type-level checks (offline, no DB)
// ---------------------------------------------------------------------------

describe("inherited field type-level checks", () => {
  test("child instance can access inherited field with parent brand", () => {
    const person = new ParityPerson({
      // Inherited required field from parent:
      id: new ParityId("person-1"),
      // Child-local required field:
      email: new ParityEmail("p@example.com"),
    });

    // Inherited field read has the parent's brand type (ParityId).
    const id: ParityId = person.id;
    assert.equal(id.value, "person-1");

    // Inherited optional field is typed as ParityName | undefined.
    const name: ParityName | undefined = person.name;
    assert.equal(name, undefined);

    // Child-local field.
    const email: ParityEmail = person.email;
    assert.equal(email.value, "p@example.com");
  });

  test("constructing parity-person without inherited required field throws at runtime", () => {
    assert.throws(
      () => new ParityPerson({ email: new ParityEmail("p@example.com") } as never),
      /missing required field "id"/,
    );
  });
});

// ---------------------------------------------------------------------------
// Normalizer (TS port of tests/integration/parity/canonical.py)
// Reused verbatim from typed-layer.test.ts — do not introduce a second
// canonicalizer.
// ---------------------------------------------------------------------------

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
