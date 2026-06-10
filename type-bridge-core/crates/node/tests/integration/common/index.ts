/**
 * Shared harness for the Node package integration suite.
 *
 * All tests import from here.  The harness:
 *   - loads the NAPI artifact via TYPE_BRIDGE_NODE_NATIVE_PATH (never the
 *     stale tmp/ binary — that path must be set explicitly or build:native
 *     must have placed one next to dist/index.js);
 *   - ensures the target database exists (hard-fails on missing TypeDB);
 *   - defines the test schema inside an isolated uniquely-suffixed namespace
 *     so concurrent test suites sharing one database do not collide;
 *   - provides descriptor factories mirroring integration_support.rs so the
 *     TS tests stay in sync with the Rust reference.
 *
 * Runtime note: the package's main entry is CommonJS (dist/index.js).  We load it
 * via createRequire so that the test files stay ESM (node:test requires it)
 * while remaining compatible with the package's CJS surface.
 */

import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import path from "node:path";

// Re-use the public TypeScript types from the package source.
// At runtime these imports are stripped by Node 25's type-stripping.
import type {
  EntityDescriptor,
  RelationDescriptor,
  OwnedAttributeDescriptor,
  AttributeValue,
  DynamicEntityRow,
  DynamicRelationRow,
} from "../../../typescript/index.ts";

// Resolve package root relative to this file.
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const packageRoot = path.resolve(__dirname, "../../..");

// Load the CJS package at runtime via require so we stay compatible with
// the package's CommonJS surface without relying on static ESM named-export
// analysis (which is brittle for complex CJS objects).
const _require = createRequire(import.meta.url);
const pkg = _require(packageRoot) as typeof import("../../../typescript/index.ts");

// Re-export attribute builders and classes as named exports so test files
// have a single harness import.
export const {
  RustDatabase,
  RustDynamicEntityManager,
  RustDynamicRelationManager,
  RustTransactionContext,
  ensureDatabase,
  string,
  long,
  longFromNumberUnsafe,
  double,
  boolean,
  date,
  datetime,
  datetimetz,
  decimal,
  duration,
} = pkg;

export type {
  EntityDescriptor,
  RelationDescriptor,
  OwnedAttributeDescriptor,
  AttributeValue,
  DynamicEntityRow,
  DynamicRelationRow,
};

// ---------------------------------------------------------------------------
// Environment / connection parameters
// ---------------------------------------------------------------------------

export const TYPEDB_ADDRESS =
  process.env.TYPEDB_ADDRESS ?? "localhost:1730";
export const INTG_DATABASE =
  process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ?? "type_bridge_test";
export const TYPEDB_USERNAME = process.env.TYPEDB_USERNAME ?? "admin";
export const TYPEDB_PASSWORD = process.env.TYPEDB_PASSWORD ?? "password";

// ---------------------------------------------------------------------------
// Schema suffix generator — mirrors integration_support.rs unique_schema_suffix
// ---------------------------------------------------------------------------

let nextSchemaId = 1;

/** Return a unique kebab-case suffix for schema type names. */
export function uniqueSuffix(prefix: string, scope: string): string {
  return `${prefix}-${scope}-${process.pid}-${nextSchemaId++}`;
}

// ---------------------------------------------------------------------------
// Database setup — called once per suite
// ---------------------------------------------------------------------------

/**
 * Ensure the integration database exists, then connect and return a handle.
 *
 * Throws (hard-fails) if TypeDB is unreachable — this is intentional.  The
 * integration suite must not silently skip when the server is down.
 */
export function connectIntegration() {
  ensureDatabase(TYPEDB_ADDRESS, INTG_DATABASE, {
    username: TYPEDB_USERNAME,
    password: TYPEDB_PASSWORD,
  });
  return RustDatabase.connect(TYPEDB_ADDRESS, INTG_DATABASE, {
    username: TYPEDB_USERNAME,
    password: TYPEDB_PASSWORD,
  });
}

// ---------------------------------------------------------------------------
// Schema definition helpers
// ---------------------------------------------------------------------------

/**
 * Define schema inside a schema transaction and commit.
 */
export function defineSchema(
  db: ReturnType<typeof connectIntegration>,
  typeql: string,
): void {
  const tx = db.transaction("schema");
  try {
    tx.query(typeql);
    tx.commit();
  } catch (err) {
    tx.close();
    throw err;
  }
}

// ---------------------------------------------------------------------------
// NodeCrudSchema — mirrors integration_support.rs NodeCrudSchema
// ---------------------------------------------------------------------------

export interface CrudSchema {
  personType: string;
  companyType: string;
  employmentType: string;
  nameAttr: string;
  companyNameAttr: string;
  ageAttr: string;
  scoreAttr: string;
  activeAttr: string;
  birthdayAttr: string;
  loginAtAttr: string;
  seenAtAttr: string;
  balanceAttr: string;
  sessionLengthAttr: string;
  sinceAttr: string;
}

/** Build a uniquely-suffixed CrudSchema for the given test scope. */
export function newCrudSchema(scope: string): CrudSchema {
  const s = uniqueSuffix("node", scope);
  return {
    personType: `${s}-person`,
    companyType: `${s}-company`,
    employmentType: `${s}-employment`,
    nameAttr: `${s}-name`,
    companyNameAttr: `${s}-company-name`,
    ageAttr: `${s}-age`,
    scoreAttr: `${s}-score`,
    activeAttr: `${s}-active`,
    birthdayAttr: `${s}-birthday`,
    loginAtAttr: `${s}-login-at`,
    seenAtAttr: `${s}-seen-at`,
    balanceAttr: `${s}-balance`,
    sessionLengthAttr: `${s}-session-length`,
    sinceAttr: `${s}-since`,
  };
}

/** TypeQL define block for a CrudSchema — mirrors define_schema_source(). */
export function crudSchemaTypeql(s: CrudSchema): string {
  return `define
attribute ${s.nameAttr}, value string;
attribute ${s.companyNameAttr}, value string;
attribute ${s.ageAttr}, value integer;
attribute ${s.scoreAttr}, value double;
attribute ${s.activeAttr}, value boolean;
attribute ${s.birthdayAttr}, value date;
attribute ${s.loginAtAttr}, value datetime;
attribute ${s.seenAtAttr}, value datetime-tz;
attribute ${s.balanceAttr}, value decimal;
attribute ${s.sessionLengthAttr}, value duration;
attribute ${s.sinceAttr}, value date;
entity ${s.personType}, owns ${s.nameAttr} @key, owns ${s.ageAttr} @card(0..5), owns ${s.scoreAttr} @card(0..5), owns ${s.activeAttr} @card(0..5), owns ${s.birthdayAttr} @card(0..5), owns ${s.loginAtAttr} @card(0..5), owns ${s.seenAtAttr} @card(0..5), owns ${s.balanceAttr} @card(0..5), owns ${s.sessionLengthAttr} @card(0..5), plays ${s.employmentType}:employee;
entity ${s.companyType}, owns ${s.companyNameAttr} @key, plays ${s.employmentType}:employer;
relation ${s.employmentType}, relates employee, relates employer, owns ${s.sinceAttr} @card(0..5);
`;
}

// ---------------------------------------------------------------------------
// Descriptor factories — mirrors integration_support.rs JSON helpers
// ---------------------------------------------------------------------------

function keyAttr(
  fieldName: string,
  attrName: string,
  valueType: OwnedAttributeDescriptor["value_type"],
): OwnedAttributeDescriptor {
  return {
    field_name: fieldName,
    attr_name: attrName,
    value_type: valueType,
    annotations: ["Key"],
    is_optional: false,
  };
}

function cardAttr(
  fieldName: string,
  attrName: string,
  valueType: OwnedAttributeDescriptor["value_type"],
): OwnedAttributeDescriptor {
  return {
    field_name: fieldName,
    attr_name: attrName,
    value_type: valueType,
    annotations: [{ Card: [0, 5] as [number, number] }],
    is_optional: true,
  };
}

export function personDescriptor(s: CrudSchema): EntityDescriptor {
  return {
    type_name: s.personType,
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      keyAttr("name", s.nameAttr, "string"),
      cardAttr("age", s.ageAttr, "long"),
      cardAttr("score", s.scoreAttr, "double"),
      cardAttr("active", s.activeAttr, "boolean"),
      cardAttr("birthday", s.birthdayAttr, "date"),
      cardAttr("login_at", s.loginAtAttr, "datetime"),
      cardAttr("seen_at", s.seenAtAttr, "datetime-tz"),
      cardAttr("balance", s.balanceAttr, "decimal"),
      cardAttr("session_length", s.sessionLengthAttr, "duration"),
    ],
  };
}

export function companyDescriptor(s: CrudSchema): EntityDescriptor {
  return {
    type_name: s.companyType,
    is_abstract: false,
    parent_type: null,
    owned_attributes: [keyAttr("name", s.companyNameAttr, "string")],
  };
}

export function employmentDescriptor(s: CrudSchema): RelationDescriptor {
  return {
    type_name: s.employmentType,
    is_abstract: false,
    parent_type: null,
    owned_attributes: [cardAttr("since", s.sinceAttr, "date")],
    roles: [
      { role_name: "employee", player_type_names: [s.personType], cardinality: [1, 1] as [number, number | null] },
      { role_name: "employer", player_type_names: [s.companyType], cardinality: [1, 1] as [number, number | null] },
    ],
  };
}

// ---------------------------------------------------------------------------
// Row-inspection helpers — mirrors row_attribute / row_attributes in Rust
// ---------------------------------------------------------------------------

/** Return the first attribute value for the given attribute name. */
export function rowAttribute(
  row: DynamicEntityRow,
  attrName: string,
): unknown | undefined {
  const pair = row.attributes.find(([name]) => name === attrName);
  return pair?.[1];
}

/** Return all attribute values for the given attribute name (multi-value). */
export function rowAttributes(
  row: DynamicEntityRow,
  attrName: string,
): unknown[] {
  return row.attributes
    .filter(([name]) => name === attrName)
    .map(([, value]) => value);
}
