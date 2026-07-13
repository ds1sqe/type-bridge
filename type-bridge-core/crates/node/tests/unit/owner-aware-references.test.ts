import assert = require("node:assert/strict");
import test = require("node:test");

import { Entity, Relation, attr, field, role } from "../../typescript/index.js";
import {
  QuerySession,
  TypedMatchError,
  TypedReferenceError,
  references,
} from "../../typescript/typed/index.js";
import { diagnosticQuerySession } from "../../typescript/typed/session.js";
import { corpusError } from "./semantic-corpus.js";

class RuntimeName extends attr.String("owner-runtime-name") {}
class RuntimePerson extends Entity("owner-runtime-person", {
  name: field(RuntimeName),
}) {}
class RuntimeCompany extends Entity("owner-runtime-company", {
  name: field(RuntimeName),
}) {}
class RuntimeEmployment extends Relation("owner-runtime-employment", {
  employee: role(RuntimePerson),
  employer: role(RuntimeCompany),
}) {}

const sharedFieldSpec = field(RuntimeName);
class RuntimeSharedPerson extends Entity("owner-runtime-shared-person", {
  name: sharedFieldSpec,
}) {}
class RuntimeSharedCompany extends Entity("owner-runtime-shared-company", {
  name: sharedFieldSpec,
}) {}

const sharedRoleSpec = role(RuntimePerson);
class RuntimeSharedEmployment extends Relation(
  "owner-runtime-shared-employment",
  {
    participant: sharedRoleSpec,
  },
) {}
class RuntimeSharedCollaboration extends Relation(
  "owner-runtime-shared-collaboration",
  {
    participant: sharedRoleSpec,
  },
) {}

class RuntimeParty extends Entity("owner-runtime-party", {
  name: field(RuntimeName),
}) {}
class RuntimeEmployee extends Entity(
  "owner-runtime-employee",
  {},
  { parent: RuntimeParty },
) {}
class RuntimeAssociation extends Relation("owner-runtime-association", {
  participant: role(RuntimePerson),
}) {}
class RuntimeSpecialAssociation extends Relation(
  "owner-runtime-special-association",
  {},
  { parent: RuntimeAssociation },
) {}

const personRefs = references(RuntimePerson);
const companyRefs = references(RuntimeCompany);
const employmentRefs = references(RuntimeEmployment);
const sharedPersonRefs = references(RuntimeSharedPerson);
const sharedCompanyRefs = references(RuntimeSharedCompany);
const sharedEmploymentRefs = references(RuntimeSharedEmployment);
const sharedCollaborationRefs = references(RuntimeSharedCollaboration);
const partyRefs = references(RuntimeParty);
const associationRefs = references(RuntimeAssociation);

test("owner-aware references stay opaque and same-model vars stay distinct", () => {
  const session = diagnosticQuerySession();
  const first = session.var(RuntimePerson);
  const second = session.var(RuntimePerson);
  const company = session.var(RuntimeCompany);
  const employment = session.var(RuntimeEmployment);

  assert.notStrictEqual(first, second);
  assert.ok(
    first
      .field(personRefs.fields.name)
      .eq(second.field(personRefs.fields.name)),
  );
  assert.ok(
    first
      .field(personRefs.fields.name)
      .eq(company.field(companyRefs.fields.name)),
  );
  assert.ok(
    employment
      .role(employmentRefs.roles.employee)
      .connects(first)
      .and(employment.role(employmentRefs.roles.employer).connects(company)),
  );
  assert.ok(
    first
      .collect()
      .distinct()
      .orderBy(first.field(personRefs.fields.name).asc()),
  );
  assert.ok(Object.isFrozen(personRefs));
  assert.ok(Object.isFrozen(personRefs.fields));
  assert.deepEqual(Object.keys(first), []);
});

test("incompatible role players fail with the corpus diagnostic", () => {
  const session = diagnosticQuerySession();
  const company = session.var(RuntimeCompany);
  const employment = session.var(RuntimeEmployment);
  const incompatible = employment
    .role(employmentRefs.roles.employee)
    .connects(company as never);

  assert.throws(
    () => session.query(employment, company).where(incompatible).one(),
    (error: unknown) => {
      if (!(error instanceof TypedMatchError)) return false;
      assert.deepEqual(
        [error.category, error.code],
        corpusError("references.incompatible-role-player"),
      );
      return true;
    },
  );
});

test("cross-session and forged reference use fails before any query terminal", () => {
  const firstSession = diagnosticQuerySession();
  const secondSession = diagnosticQuerySession();
  const first = firstSession.var(RuntimePerson);
  const second = secondSession.var(RuntimePerson);

  assert.throws(
    () =>
      first
        .field(personRefs.fields.name)
        .eq(second.field(personRefs.fields.name)),
    (error: unknown) => {
      if (!(error instanceof TypedMatchError)) return false;
      assert.equal(error.category, "invalid_plan");
      assert.equal(error.code, "cross_session_handle");
      return true;
    },
  );

  assert.throws(
    () => first.field(companyRefs.fields.name as never),
    (error: unknown) => {
      if (!(error instanceof TypedReferenceError)) return false;
      assert.match(error.message, /does not belong/);
      return true;
    },
  );

  assert.throws(
    () => first.field(Object.freeze({}) as never),
    (error: unknown) => {
      if (!(error instanceof TypedReferenceError)) return false;
      assert.match(error.message, /references\(model\)/);
      return true;
    },
  );
});

test("native owner identity closes shared-spec field and role aliases", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeSharedPerson);
  session.var(RuntimeSharedCompany);
  const employment = session.var(RuntimeSharedEmployment);
  session.var(RuntimeSharedCollaboration);

  assert.throws(
    () => person.field(sharedCompanyRefs.fields.name as never),
    (error: unknown) => {
      if (!(error instanceof TypedMatchError)) return false;
      assert.equal(error.code, corpusError("references.cross-owner-field")[1]);
      return true;
    },
  );
  assert.throws(
    () => employment.role(sharedCollaborationRefs.roles.participant as never),
    (error: unknown) => {
      if (!(error instanceof TypedMatchError)) return false;
      assert.equal(error.code, corpusError("references.cross-owner-role")[1]);
      return true;
    },
  );

  assert.ok(person.field(sharedPersonRefs.fields.name));
  assert.ok(employment.role(sharedEmploymentRefs.roles.participant));
});

test("reference owner labels are immutable provenance snapshots", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeSharedPerson);
  const employment = session.var(RuntimeSharedEmployment);
  const companyFlags = RuntimeSharedCompany.flags as unknown as {
    name: string | null;
  };
  const collaborationFlags = RuntimeSharedCollaboration.flags as unknown as {
    name: string | null;
  };
  const companyTypeName = companyFlags.name;
  const collaborationTypeName = collaborationFlags.name;

  try {
    companyFlags.name = RuntimeSharedPerson.typeName;
    collaborationFlags.name = RuntimeSharedEmployment.typeName;

    assert.throws(
      () => person.field(sharedCompanyRefs.fields.name as never),
      (error: unknown) =>
        error instanceof TypedReferenceError &&
        /owner type name changed/.test(error.message),
    );
    assert.throws(
      () => employment.role(sharedCollaborationRefs.roles.participant as never),
      (error: unknown) =>
        error instanceof TypedReferenceError &&
        /owner type name changed/.test(error.message),
    );
  } finally {
    companyFlags.name = companyTypeName;
    collaborationFlags.name = collaborationTypeName;
  }
});

test("fresh references cannot alias owners through mutable type labels", () => {
  const session = diagnosticQuerySession();
  const person = session.var(RuntimeSharedPerson);
  const employment = session.var(RuntimeSharedEmployment);
  const personFlags = RuntimeSharedPerson.flags as unknown as {
    name: string | null;
  };
  const companyFlags = RuntimeSharedCompany.flags as unknown as {
    name: string | null;
  };
  const employmentFlags = RuntimeSharedEmployment.flags as unknown as {
    name: string | null;
  };
  const collaborationFlags = RuntimeSharedCollaboration.flags as unknown as {
    name: string | null;
  };
  const personTypeName = personFlags.name;
  const companyTypeName = companyFlags.name;
  const employmentTypeName = employmentFlags.name;
  const collaborationTypeName = collaborationFlags.name;

  try {
    companyFlags.name = personTypeName;
    collaborationFlags.name = employmentTypeName;
    const aliasedCompanyRefs = references(RuntimeSharedCompany);
    const aliasedCollaborationRefs = references(RuntimeSharedCollaboration);

    assert.throws(
      () => person.field(aliasedCompanyRefs.fields.name as never),
      (error: unknown) =>
        error instanceof TypedReferenceError &&
        /reference owner does not match the bound model/.test(error.message),
    );
    assert.throws(
      () =>
        employment.role(aliasedCollaborationRefs.roles.participant as never),
      (error: unknown) =>
        error instanceof TypedReferenceError &&
        /reference owner does not match the bound model/.test(error.message),
    );

    personFlags.name = "owner-runtime-shared-person-drifted";
    employmentFlags.name = "owner-runtime-shared-employment-drifted";

    assert.throws(
      () => person.field(aliasedCompanyRefs.fields.name as never),
      (error: unknown) =>
        error instanceof TypedReferenceError &&
        /bound variable model type name changed/.test(error.message),
    );
    assert.throws(
      () =>
        employment.role(aliasedCollaborationRefs.roles.participant as never),
      (error: unknown) =>
        error instanceof TypedReferenceError &&
        /bound variable model type name changed/.test(error.message),
    );
  } finally {
    personFlags.name = personTypeName;
    companyFlags.name = companyTypeName;
    employmentFlags.name = employmentTypeName;
    collaborationFlags.name = collaborationTypeName;
  }
});

test("parent-owned references bind plain-inherited fields and roles", () => {
  const session = diagnosticQuerySession();
  const employee = session.var(RuntimeEmployee);
  const special = session.var(RuntimeSpecialAssociation);
  const person = session.var(RuntimePerson);

  assert.ok(employee.field(partyRefs.fields.name).eq(new RuntimeName("Pat")));
  assert.ok(special.role(associationRefs.roles.participant).connects(person));
});
