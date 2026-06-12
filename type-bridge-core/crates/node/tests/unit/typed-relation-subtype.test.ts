import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  Entity,
  Key,
  Relation,
  TypeFlags,
  attr,
  field,
  role,
} from "../../typescript/index.js";

// ---------------------------------------------------------------------------
// Shared attribute + entity classes
// ---------------------------------------------------------------------------

class ContribName extends attr.String("contrib-name") {}
class ContribId extends attr.String("contrib-id") {}

class ContribPerson extends Entity("contrib-person", {
  id: field(ContribId, Key),
}) {}

class ContribWork extends Entity("contrib-work", {
  name: field(ContribName, Key),
}) {}

class ContribAuthor extends Entity("contrib-author", {
  name: field(ContribName, Key),
}) {}

// ---------------------------------------------------------------------------
// Relation hierarchy under test: contribution / authoring
// ---------------------------------------------------------------------------

// Root relation: two plain roles.
class Contribution extends Relation("contribution", {
  contributor: role(ContribPerson),
  work: role(ContribWork),
}) {}

// Subtype: 'author' specializes 'contributor'; 'work' is plain-inherited.
class Authoring extends Relation(
  "authoring",
  {
    author: role(ContribAuthor, { overrides: "contributor" }),
  },
  { parent: Contribution },
) {}

// ---------------------------------------------------------------------------
// Effective-set role descriptor tests
// ---------------------------------------------------------------------------

describe("relation subtype effective role set", () => {
  test("Contribution descriptor lists both own roles in declaration order", () => {
    const d = Contribution.descriptor();
    assert.equal(d.type_name, "contribution");
    assert.equal(d.parent_type, null);
    const roleNames = d.roles.map((r) => r.role_name);
    assert.deepEqual(roleNames, ["contributor", "work"]);
  });

  test("Authoring descriptor emits effective set: [work, author]", () => {
    const d = Authoring.descriptor();
    assert.equal(d.type_name, "authoring");
    assert.equal(d.parent_type, "contribution");

    const roleNames = d.roles.map((r) => r.role_name);
    // 'contributor' is overridden by 'author' → excluded.
    // 'work' is plain-inherited → first.
    // 'author' is the child specializing role → second.
    assert.deepEqual(roleNames, ["work", "author"], `unexpected role order: ${JSON.stringify(roleNames)}`);
  });

  test("Authoring 'work' role carries the correct player type", () => {
    const d = Authoring.descriptor();
    const workRole = d.roles.find((r) => r.role_name === "work");
    assert.ok(workRole != null, "work role must be present");
    assert.deepEqual(workRole.player_type_names, ["contrib-work"]);
    assert.equal(workRole.cardinality, null);
  });

  test("Authoring 'author' role carries the correct player type", () => {
    const d = Authoring.descriptor();
    const authorRole = d.roles.find((r) => r.role_name === "author");
    assert.ok(authorRole != null, "author role must be present");
    assert.deepEqual(authorRole.player_type_names, ["contrib-author"]);
    assert.equal(authorRole.cardinality, null);
  });

  test("Authoring descriptor does not include the overridden 'contributor' role", () => {
    const d = Authoring.descriptor();
    const contributorRole = d.roles.find((r) => r.role_name === "contributor");
    assert.equal(contributorRole, undefined, "overridden 'contributor' must be absent");
  });

  test("Contribution descriptor is not mutated by Authoring declaration", () => {
    // Access both; order matters to confirm the parent is unchanged.
    const parent = Contribution.descriptor();
    const child = Authoring.descriptor();
    void child;

    const parentRoleNames = parent.roles.map((r) => r.role_name);
    assert.deepEqual(parentRoleNames, ["contributor", "work"]);
  });
});

// ---------------------------------------------------------------------------
// Plain-inherited-only subtype (no overrides)
// ---------------------------------------------------------------------------

class ExtendedContrib extends Relation(
  "extended-contrib",
  {},
  { parent: Contribution },
) {}

describe("plain-inheritance only (no specialization)", () => {
  test("child with no own roles re-lists all parent roles", () => {
    const d = ExtendedContrib.descriptor();
    const roleNames = d.roles.map((r) => r.role_name);
    assert.deepEqual(roleNames, ["contributor", "work"]);
  });
});

// ---------------------------------------------------------------------------
// Entity subtype owned_attributes regression pin
// ---------------------------------------------------------------------------

class ExtendedPerson extends Entity(
  "extended-person",
  { name: field(ContribName) },
  { parent: ContribPerson },
) {}

describe("entity subtype owned_attributes regression", () => {
  test("entity child descriptor still flattens owned_attributes", () => {
    const d = ExtendedPerson.descriptor();
    const fieldNames = d.owned_attributes.map((a) => a.field_name);
    assert.ok(fieldNames.includes("id"), "inherited 'id' must be re-listed");
    assert.ok(fieldNames.includes("name"), "own 'name' must be present");
    // No 'roles' key on entity descriptors.
    assert.ok(!("roles" in d), "entity descriptor must not have a 'roles' key");
  });
});

// ---------------------------------------------------------------------------
// Abstract parent relation with a subtype that adds a specializing role
// ---------------------------------------------------------------------------

class BaseInteraction extends Relation(
  TypeFlags({ name: "base-interaction", abstract: true }),
  {
    participant: role(ContribPerson),
  },
) {}

class FocusedInteraction extends Relation(
  "focused-interaction",
  {
    lead: role(ContribAuthor, { overrides: "participant" }),
    topic: role(ContribWork),
  },
  { parent: BaseInteraction },
) {}

describe("abstract parent with multiple child roles", () => {
  test("FocusedInteraction effective set excludes 'participant', includes own roles", () => {
    const d = FocusedInteraction.descriptor();
    const roleNames = d.roles.map((r) => r.role_name);
    // 'participant' is overridden → excluded; 'lead' and 'topic' are own.
    // No plain-inherited roles remain.
    assert.ok(!roleNames.includes("participant"), "'participant' must be excluded");
    assert.ok(roleNames.includes("lead"), "'lead' must be present");
    assert.ok(roleNames.includes("topic"), "'topic' must be present");
  });
});
