/**
 * Typed cross-language parity reader (Plan 11 Phase 2).
 *
 * Sibling to `tests/integration/parity/node_reader.cjs`. Where `node_reader.cjs`
 * reads fixture rows through the DYNAMIC manager surface, this reader builds /
 * reads them through the TYPED `Entity()` / `Relation()` surface and serializes
 * each instance through the typed `toDict()` API. Both readers feed the SAME
 * `expected-canonical.json` value oracle via the single canonicalizer in
 * `cross_language.py`.
 *
 * This proves the typed surface produces the same canonical VALUE shape the
 * dynamic surface and Python `to_dict()` produce. It COMPLEMENTS — does not
 * duplicate — Plan 10's descriptor byte-identity gate: that gate asserts the
 * typed factory emits byte-identical descriptor SHAPE; this asserts the typed
 * serialization emits byte-identical VALUE shape.
 *
 * Modes (argv):
 *   --offline : build each fixture instance from `write-data.json` (no DB) and
 *               emit its `toDict()`. Entities and relation attribute fields.
 *   (default) : connect to TypeDB, read each entity type through the typed
 *               manager `.all()`, and emit each instance's `toDict()`. Entities
 *               only — typed relation `toDict()` excludes roles by contract, so
 *               the role-bearing relation oracle stays the dynamic reader's job.
 *
 * Env (set by `cross_language.py`):
 *   TYPE_BRIDGE_NODE_PACKAGE_DIR  — the `crates/node` dir (root index.js / NAPI).
 *   TYPE_BRIDGE_PARITY_WRITE_DATA — `write-data.json` path (offline mode).
 *   TYPEDB_ADDRESS / TYPE_BRIDGE_PARITY_DATABASE / TYPEDB_USERNAME / TYPEDB_PASSWORD.
 *   TYPE_BRIDGE_NODE_NATIVE_PATH  — prebuilt `.node` (honored by root index.js).
 *
 * The runtime split mirrors the typed integration tests: the typed factory comes
 * from the compiled `typescript/**`; the NAPI runtime (`RustDatabase`) comes from
 * the root `index.js` loaded via `createRequire`, with `db` injected into the
 * typed manager.
 */

import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";

import {
  Card,
  Entity,
  Key,
  Relation,
  TypeFlags,
  Unique,
  attr,
  field,
  role,
} from "../../typescript/index.js";

// ---------------------------------------------------------------------------
// Attribute classes — full parity corpus (mirrors typed-serialization.test.ts)
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
// Model declarations — full parity corpus (mirrors typed-serialization.test.ts)
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

const ENTITY_READERS = [
  { typeName: "parity-person", model: ParityPerson },
  { typeName: "parity-company", model: ParityCompany },
  { typeName: "parity-email-message", model: ParityEmailMessage },
] as const;

// ---------------------------------------------------------------------------
// Fixture types + plain-value accessors
// ---------------------------------------------------------------------------

interface WriteAttr {
  type: string;
  value: string | number | boolean;
}

interface EntityRow {
  stable_id: string;
  type: string;
  attributes: Record<string, WriteAttr | WriteAttr[]>;
}

interface RelationRow {
  stable_id: string;
  type: string;
  attributes: Record<string, WriteAttr | WriteAttr[]>;
  roles: Record<string, Array<{ stable_id: string; type: string }>>;
}

interface WriteDataFixture {
  entities: EntityRow[];
  relations: RelationRow[];
}

function asAttr(value: WriteAttr | WriteAttr[]): WriteAttr {
  return value as WriteAttr;
}

function str(value: WriteAttr | WriteAttr[]): string {
  return asAttr(value).value as string;
}

function num(value: WriteAttr | WriteAttr[]): number {
  return asAttr(value).value as number;
}

function bool(value: WriteAttr | WriteAttr[]): boolean {
  return asAttr(value).value as boolean;
}

/** `long` fixture values are decimal strings ("37"); the TS brand uses bigint. */
function big(value: WriteAttr | WriteAttr[]): bigint {
  return BigInt(asAttr(value).value as string);
}

function loadWriteData(): WriteDataFixture {
  const writeDataPath = process.env.TYPE_BRIDGE_PARITY_WRITE_DATA;
  if (!writeDataPath) {
    throw new Error("TYPE_BRIDGE_PARITY_WRITE_DATA is not set");
  }
  return JSON.parse(fs.readFileSync(writeDataPath, "utf8")) as WriteDataFixture;
}

// ---------------------------------------------------------------------------
// Offline instance builders (mirror the Python writer's _build_* helpers)
// ---------------------------------------------------------------------------

function buildPerson(row: EntityRow): InstanceType<typeof ParityPerson> {
  const a = row.attributes;
  const input: Record<string, unknown> = {
    id: new ParityId(str(a["id"])),
    email: new ParityEmail(str(a["email"])),
  };
  if (a["name"]) input["name"] = new ParityName(str(a["name"]));
  if (a["age"]) input["age"] = new ParityAge(big(a["age"]));
  if (a["score"]) input["score"] = new ParityScore(num(a["score"]));
  if (a["active"]) input["active"] = new ParityActive(bool(a["active"]));
  if (a["birth_date"]) input["birth_date"] = new ParityBirthDate(str(a["birth_date"]));
  if (a["login_at"]) input["login_at"] = new ParityLoginAt(str(a["login_at"]));
  if (a["seen_at"]) input["seen_at"] = new ParitySeenAt(str(a["seen_at"]));
  if (a["balance"]) input["balance"] = new ParityBalance(str(a["balance"]));
  if (a["session_length"]) {
    input["session_length"] = new ParitySessionLength(str(a["session_length"]));
  }
  if (a["tags"]) {
    input["tags"] = (a["tags"] as WriteAttr[]).map((t) => new ParityTag(t.value as string));
  }
  return new ParityPerson(input as never);
}

function buildCompany(row: EntityRow): InstanceType<typeof ParityCompany> {
  const a = row.attributes;
  return new ParityCompany({
    id: new ParityId(str(a["id"])),
    name: new ParityName(str(a["name"])),
  });
}

function buildEmailMessage(row: EntityRow): InstanceType<typeof ParityEmailMessage> {
  const a = row.attributes;
  return new ParityEmailMessage({
    id: new ParityId(str(a["id"])),
    note: new ParityNote(str(a["note"])),
  });
}

function buildEntity(row: EntityRow): InstanceType<(typeof ENTITY_READERS)[number]["model"]> {
  switch (row.type) {
    case "parity-person":
      return buildPerson(row);
    case "parity-company":
      return buildCompany(row);
    case "parity-email-message":
      return buildEmailMessage(row);
    default:
      throw new Error(`unknown entity fixture type: ${row.type}`);
  }
}

function buildRelation(
  row: RelationRow,
  entitiesById: Map<string, EntityRow>,
): InstanceType<typeof ParityMembership> | InstanceType<typeof ParityTokenOrigin> {
  const a = row.attributes;
  const player = (stableId: string): EntityRow => {
    const found = entitiesById.get(stableId);
    if (!found) throw new Error(`unknown role player: ${stableId}`);
    return found;
  };
  if (row.type === "parity-membership") {
    const input: Record<string, unknown> = {
      member: buildEntity(player(row.roles["member"][0].stable_id)),
      organization: buildEntity(player(row.roles["organization"][0].stable_id)),
      evidence: row.roles["evidence"].map((p) => buildEntity(player(p.stable_id))),
      since: new ParitySince(str(a["since"])),
    };
    if (a["confidence"]) input["confidence"] = new ParityConfidence(big(a["confidence"]));
    return new ParityMembership(input as never);
  }
  if (row.type === "parity-token-origin") {
    return new ParityTokenOrigin({
      token: buildEntity(player(row.roles["token"][0].stable_id)),
      issue: buildEntity(player(row.roles["issue"][0].stable_id)),
      kind: new ParityKind(str(a["kind"])),
    } as never);
  }
  throw new Error(`unknown relation fixture type: ${row.type}`);
}

// ---------------------------------------------------------------------------
// Read modes
// ---------------------------------------------------------------------------

interface ReaderSection {
  type_name: string;
  rows: Record<string, unknown>[];
}

function readOffline(): { version: number; mode: string; entities: ReaderSection[]; relations: ReaderSection[] } {
  const writeData = loadWriteData();
  const entitiesById = new Map(writeData.entities.map((row) => [row.stable_id, row]));

  const entities: ReaderSection[] = ENTITY_READERS.map(({ typeName }) => ({
    type_name: typeName,
    rows: writeData.entities
      .filter((row) => row.type === typeName)
      .map((row) => buildEntity(row).toDict()),
  }));

  const relationTypes = ["parity-membership", "parity-token-origin"];
  const relations: ReaderSection[] = relationTypes.map((typeName) => ({
    type_name: typeName,
    rows: writeData.relations
      .filter((row) => row.type === typeName)
      .map((row) => buildRelation(row, entitiesById).toDict()),
  }));

  return { version: 1, mode: "offline", entities, relations };
}

function readLive(): { version: number; mode: string; entities: ReaderSection[]; relations: ReaderSection[] } {
  const requirePackage = createRequire(path.join(process.cwd(), "parity-typed-reader.cjs"));
  const packageDir = process.env.TYPE_BRIDGE_NODE_PACKAGE_DIR;
  if (!packageDir) {
    throw new Error("TYPE_BRIDGE_NODE_PACKAGE_DIR is not set");
  }
  type RuntimePackage = typeof import("../../typescript/index.js");
  const typeBridge = requirePackage(packageDir) as RuntimePackage;

  const address = process.env.TYPEDB_ADDRESS ?? "localhost:1730";
  const database =
    process.env.TYPE_BRIDGE_PARITY_DATABASE ??
    process.env.TYPE_BRIDGE_NODE_INTG_DATABASE ??
    "type_bridge_test";
  const username = process.env.TYPEDB_USERNAME ?? "admin";
  const password = process.env.TYPEDB_PASSWORD ?? "password";
  const httpPort = Number(process.env.TYPEDB_HTTP_PORT ?? "8000");
  const db = typeBridge.RustDatabase.connect(address, database, { username, password, httpPort });

  const entities: ReaderSection[] = ENTITY_READERS.map(({ typeName, model }) => ({
    type_name: typeName,
    rows: model.manager(db).all().map((instance) => instance.toDict()),
  }));

  // Relations are read by the dynamic reader: typed relation toDict() excludes
  // role players by contract, so it cannot reproduce the role-bearing relation
  // oracle. Entity value parity is the typed reader's complementary coverage.
  return { version: 1, mode: "live", entities, relations: [] };
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

function main(): void {
  const offline = process.argv.includes("--offline");
  const payload = offline ? readOffline() : readLive();
  // bigint (long / integer fields) is not JSON-serializable; emit as a decimal
  // string. The Python canonicalizer applies str() for `long`-typed fields, so
  // the value round-trips identically whether it arrives as a JS bigint or a
  // string.
  process.stdout.write(
    JSON.stringify(payload, (_key, value) =>
      typeof value === "bigint" ? value.toString() : value,
    ),
  );
}

main();
