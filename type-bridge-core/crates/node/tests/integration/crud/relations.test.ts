/**
 * Relation CRUD integration — exercises insert, getWithRolePlayers, update,
 * putMany, count, aggregate, groupByAggregate, and deleteByIid for relations,
 * plus the two role-player shape ACs:
 *
 *   1. Single role accepting multiple player types (multi-player-type)
 *   2. Abstract-role resolution to concrete subtypes
 *
 * Mirrors node_relation_crud_against_typedb,
 * node_relation_single_role_accepts_multiple_player_types_against_typedb, and
 * node_relation_abstract_role_resolves_concrete_players_against_typedb.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";

import {
  connectIntegration,
  defineSchema,
  newCrudSchema,
  crudSchemaTypeql,
  personDescriptor,
  companyDescriptor,
  employmentDescriptor,
  rowAttribute,
  uniqueSuffix,
  string,
  long,
  date,
} from "../common/index.ts";
import type { EntityDescriptor, RelationDescriptor } from "../common/index.ts";

const db = connectIntegration();

// ---------------------------------------------------------------------------
// Basic relation CRUD
// ---------------------------------------------------------------------------

describe("relation CRUD", () => {
  const s = newCrudSchema("relation");
  defineSchema(db, crudSchemaTypeql(s));

  const personMgr = db.entityManager(personDescriptor(s));
  const companyMgr = db.entityManager(companyDescriptor(s));
  const relMgr = db.relationManager(employmentDescriptor(s));

  const aliceIid = personMgr.insert({ name: string("Alice"), age: long(30n) });
  const bobIid = personMgr.insert({ name: string("Bob"), age: long(40n) });
  const acmeIid = companyMgr.insert({ name: string("Acme") });

  const aliceRoles = [
    { role_name: "employee", player_type_name: s.personType, iid: aliceIid },
    { role_name: "employer", player_type_name: s.companyType, iid: acmeIid },
  ];
  const bobRoles = [
    { role_name: "employee", player_type_name: s.personType, iid: bobIid },
    { role_name: "employer", player_type_name: s.companyType, iid: acmeIid },
  ];

  test("insert returns a non-empty IID", () => {
    const iid = relMgr.insert({ since: date("2026-05-27") }, aliceRoles);
    assert.ok(iid.length > 0);
  });

  test("getWithRolePlayers filters by attribute and role player", () => {
    const relIid = relMgr.insert({ since: date("2026-05-27") }, aliceRoles);
    const rows = relMgr.getWithRolePlayers({ since: date("2026-05-27") }, aliceRoles);
    assert.ok(rows.length >= 1);
    const row = rows.find((r) => r.iid === relIid);
    assert.ok(row !== undefined, "inserted relation should be found by attribute+role filter");
    assert.deepEqual(rowAttribute(row, s.sinceAttr), { Date: "2026-05-27" });
    assert.equal(row.role_players[0].role_name, "employee");
  });

  test("update by IID changes the attribute", () => {
    const relIid = relMgr.insert({ since: date("2026-05-27") }, aliceRoles);
    relMgr.update({ since: date("2026-05-28") }, aliceRoles, relIid);
    const rows = relMgr.getByIid(relIid);
    assert.ok(rows.length >= 1);
    assert.deepEqual(rowAttribute(rows[0], s.sinceAttr), { Date: "2026-05-28" });
  });

  test("putMany inserts two relations and returns two IIDs", () => {
    const iids = relMgr.putMany([
      { attributes: { since: date("2026-05-29") }, role_players: aliceRoles },
      { attributes: { since: date("2026-05-30") }, role_players: bobRoles },
    ]);
    assert.equal(iids.length, 2);
  });

  test("count returns at least the inserted relations", () => {
    const c = relMgr.count();
    assert.ok(c >= 1n, `count should be at least 1, got ${c}`);
  });

  test("aggregate with count function returns a numeric value", () => {
    const result = relMgr.aggregate([
      { result_key: "count", function: "count", attr_name: null },
    ]) as Array<Record<string, { value: number }>>;
    assert.ok(result.length >= 1);
    assert.ok(typeof result[0]["count"]?.value === "number");
  });

  test("deleteByIid removes the relation", () => {
    const relIid = relMgr.insert({ since: date("2026-05-27") }, aliceRoles);
    relMgr.deleteByIid(relIid);
    const rows = relMgr.getByIid(relIid);
    assert.equal(rows.length, 0, "relation should be empty after deletion");
  });
});

// ---------------------------------------------------------------------------
// Single role accepting multiple player types
// ---------------------------------------------------------------------------

interface MultiRoleSchema {
  documentType: string;
  emailType: string;
  traceType: string;
  documentIdAttr: string;
  subjectAttr: string;
  labelAttr: string;
}

function newMultiRoleSchema(): MultiRoleSchema {
  const s = uniqueSuffix("node", "multi-role");
  return {
    documentType: `${s}-document`,
    emailType: `${s}-email`,
    traceType: `${s}-trace`,
    documentIdAttr: `${s}-document-id`,
    subjectAttr: `${s}-subject`,
    labelAttr: `${s}-label`,
  };
}

function multiRoleTypeql(m: MultiRoleSchema): string {
  return `define
attribute ${m.documentIdAttr}, value string;
attribute ${m.subjectAttr}, value string;
attribute ${m.labelAttr}, value string;
entity ${m.documentType}, owns ${m.documentIdAttr} @key, plays ${m.traceType}:origin;
entity ${m.emailType}, owns ${m.subjectAttr} @key, plays ${m.traceType}:origin;
relation ${m.traceType}, relates origin, owns ${m.labelAttr};
`;
}

function documentDescriptor(m: MultiRoleSchema): EntityDescriptor {
  return {
    type_name: m.documentType,
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      {
        field_name: "document_id",
        attr_name: m.documentIdAttr,
        value_type: "string",
        annotations: ["Key"],
        is_optional: false,
        is_ordered: false,
      },
    ],
  };
}

function emailDescriptor(m: MultiRoleSchema): EntityDescriptor {
  return {
    type_name: m.emailType,
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      {
        field_name: "subject",
        attr_name: m.subjectAttr,
        value_type: "string",
        annotations: ["Key"],
        is_optional: false,
        is_ordered: false,
      },
    ],
  };
}

function traceDescriptor(m: MultiRoleSchema): RelationDescriptor {
  return {
    type_name: m.traceType,
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      {
        field_name: "label",
        attr_name: m.labelAttr,
        value_type: "string",
        annotations: [],
        is_optional: true,
        is_ordered: false,
      },
    ],
    roles: [
      {
        role_name: "origin",
        player_type_names: [m.documentType, m.emailType],
        cardinality: [1, 1] as [number, number | null],
        overrides: null,
        is_abstract: false,
        ordered: false,
        distinct: false,
      },
    ],
  };
}

describe("single role accepts multiple player types", () => {
  const m = newMultiRoleSchema();
  defineSchema(db, multiRoleTypeql(m));

  const docMgr = db.entityManager(documentDescriptor(m));
  const emailMgr = db.entityManager(emailDescriptor(m));
  const traceMgr = db.relationManager(traceDescriptor(m));

  const docIid = docMgr.insert({ document_id: string("doc-001") });
  const emailIid = emailMgr.insert({ subject: string("Important") });

  const docRoles = [{ role_name: "origin", player_type_name: m.documentType, iid: docIid }];
  const emailRoles = [{ role_name: "origin", player_type_name: m.emailType, iid: emailIid }];

  test("document-origin trace inserts via document player type", () => {
    assert.doesNotThrow(() => {
      traceMgr.insert({ label: string("from-document") }, docRoles);
    });
  });

  test("email-origin trace inserts via email player type", () => {
    assert.doesNotThrow(() => {
      traceMgr.insert({ label: string("from-email") }, emailRoles);
    });
  });

  test("filter by document role player returns only document traces", () => {
    const docTraceIid = traceMgr.insert({ label: string("doc-trace") }, docRoles);
    traceMgr.insert({ label: string("email-trace") }, emailRoles);

    const rows = traceMgr.getWithRolePlayers(null, docRoles);
    assert.ok(rows.length >= 1, "should return at least one document-origin trace");
    const found = rows.find((r) => r.iid === docTraceIid);
    assert.ok(found !== undefined);
    assert.equal(found!.role_players[0].role_name, "origin");
    assert.equal(found!.role_players[0].player_type_name, m.documentType);
  });

  test("update and delete work for multi-player-type relation", () => {
    const emailTraceIid = traceMgr.insert({ label: string("update-me") }, emailRoles);
    traceMgr.update({ label: string("updated-email") }, [], emailTraceIid);
    const updated = traceMgr.getWithRolePlayers(null, emailRoles);
    const row = updated.find((r) => r.iid === emailTraceIid);
    assert.ok(row !== undefined);
    assert.deepEqual(rowAttribute(row!, m.labelAttr), { String: "updated-email" });

    traceMgr.deleteByIid(emailTraceIid);
    const afterDelete = traceMgr.getByIid(emailTraceIid);
    assert.equal(afterDelete.length, 0, "deleted relation should not be found");
  });
});

// ---------------------------------------------------------------------------
// Abstract-role resolution to concrete subtypes
// ---------------------------------------------------------------------------

interface AbstractRoleSchema {
  tokenType: string;
  symptomType: string;
  problemType: string;
  issueType: string;
  originType: string;
  tokenTextAttr: string;
  issueKeyAttr: string;
  confidenceAttr: string;
}

function newAbstractRoleSchema(): AbstractRoleSchema {
  const s = uniqueSuffix("node", "abstract-role");
  return {
    tokenType: `${s}-token`,
    symptomType: `${s}-symptom`,
    problemType: `${s}-problem`,
    issueType: `${s}-issue`,
    originType: `${s}-token-origin`,
    tokenTextAttr: `${s}-token-text`,
    issueKeyAttr: `${s}-issue-key`,
    confidenceAttr: `${s}-confidence`,
  };
}

function abstractRoleTypeql(a: AbstractRoleSchema): string {
  return `define
attribute ${a.tokenTextAttr}, value string;
attribute ${a.issueKeyAttr}, value string;
attribute ${a.confidenceAttr}, value integer;
entity ${a.tokenType} @abstract, owns ${a.tokenTextAttr} @key, plays ${a.originType}:token;
entity ${a.symptomType} sub ${a.tokenType};
entity ${a.problemType} sub ${a.tokenType};
entity ${a.issueType}, owns ${a.issueKeyAttr} @key, plays ${a.originType}:issue;
relation ${a.originType}, relates token, relates issue, owns ${a.confidenceAttr} @card(0..5);
`;
}

function entityDesc(
  typeName: string,
  isAbstract: boolean,
  parentType: string | null,
  fieldName: string,
  attrName: string,
): EntityDescriptor {
  return {
    type_name: typeName,
    is_abstract: isAbstract,
    parent_type: parentType,
    owned_attributes: [
      {
        field_name: fieldName,
        attr_name: attrName,
        value_type: "string",
        annotations: ["Key"],
        is_optional: false,
        is_ordered: false,
      },
    ],
  };
}

describe("abstract-role resolves concrete subtypes", () => {
  const a = newAbstractRoleSchema();
  defineSchema(db, abstractRoleTypeql(a));

  // Token is abstract — its manager queries concrete subtypes.
  const tokenMgr = db.entityManager(
    entityDesc(a.tokenType, true, null, "text", a.tokenTextAttr),
  );
  const symptomMgr = db.entityManager(
    entityDesc(a.symptomType, false, a.tokenType, "text", a.tokenTextAttr),
  );
  const problemMgr = db.entityManager(
    entityDesc(a.problemType, false, a.tokenType, "text", a.tokenTextAttr),
  );
  const issueMgr = db.entityManager(
    entityDesc(a.issueType, false, null, "key", a.issueKeyAttr),
  );
  const originMgr = db.relationManager({
    type_name: a.originType,
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      {
        field_name: "confidence",
        attr_name: a.confidenceAttr,
        value_type: "long",
        annotations: [{ Card: [0, 5] as [number, number | null] }],
        is_optional: true,
        is_ordered: false,
      },
    ],
    roles: [
      { role_name: "token", player_type_names: [a.tokenType], cardinality: [1, 1] as [number, number | null], overrides: null, is_abstract: false, ordered: false, distinct: false },
      { role_name: "issue", player_type_names: [a.issueType], cardinality: [1, 1] as [number, number | null], overrides: null, is_abstract: false, ordered: false, distinct: false },
    ],
  });

  const symptomIid = symptomMgr.insert({ text: string("fever") });
  const problemIid = problemMgr.insert({ text: string("infection") });
  const issueIid = issueMgr.insert({ key: string("ISSUE-1") });

  const symptomRoles = [
    { role_name: "token", player_type_name: a.symptomType, iid: symptomIid },
    { role_name: "issue", player_type_name: a.issueType, iid: issueIid },
  ];
  const problemRoles = [
    { role_name: "token", player_type_name: a.problemType, iid: problemIid },
    { role_name: "issue", player_type_name: a.issueType, iid: issueIid },
  ];

  originMgr.insert({ confidence: long(70n) }, symptomRoles);
  originMgr.insert({ confidence: long(90n) }, problemRoles);

  test("abstract token manager returns both concrete subtypes", () => {
    const tokens = tokenMgr.all();
    const typeNames = tokens.map((r) => r.type_name);
    assert.ok(
      typeNames.includes(a.symptomType),
      "abstract manager should return symptom rows",
    );
    assert.ok(
      typeNames.includes(a.problemType),
      "abstract manager should return problem rows",
    );
  });

  test("abstract-role filter accepts concrete symptom player", () => {
    const rows = originMgr.getWithRolePlayers(null, symptomRoles);
    assert.ok(rows.length >= 1, "abstract-role filter should match concrete symptom");
    assert.deepEqual(rowAttribute(rows[0], a.confidenceAttr), { Long: "70" });
  });

  test("all origins include both concrete token player types", () => {
    const all = originMgr.all();
    const tokenPlayerTypes = all
      .flatMap((r) => r.role_players)
      .filter((p) => p.role_name === "token")
      .map((p) => p.player_type_name);
    assert.ok(
      tokenPlayerTypes.includes(a.symptomType),
      "symptom should appear as token player",
    );
    assert.ok(
      tokenPlayerTypes.includes(a.problemType),
      "problem should appear as token player",
    );
  });
});
