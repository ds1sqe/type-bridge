#!/usr/bin/env node
/** Produce Node's provider-free Phase-2 projected parity report. */

import crypto from "node:crypto";
import {
  closeSync,
  constants,
  fstatSync,
  fsyncSync,
  lstatSync,
  openSync,
  readSync,
  writeSync,
} from "node:fs";
import { basename, dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const REPORT_FORMAT = "typebridge.phase2-projected-parity-report/v1";
export const SEMANTIC_PROFILE = "typedb-3.12.1/v1";
export const OUTPUT_ENV = "TYPE_BRIDGE_PHASE2_PARITY_REPORT";
export const REPOSITORY_ROOT_ENV = "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT";
export const MAX_REPORT_BYTES = 256 * 1024;
export const MAX_AUTHORITY_BYTES = 1024 * 1024;
export const SCHEMA_RELATIVE =
  "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml";
export const JOURNEY_RELATIVE =
  "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json";

export class ProducerError extends Error {
  constructor(code, message) {
    super(`${code}: ${message}`);
    this.name = "ProducerError";
    this.code = code;
  }
}

function requireCondition(condition, message) {
  if (!condition) throw new Error(message);
}

function canonicalValue(value, ancestors = new Set()) {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new ProducerError(
        "invalid_report_value",
        "report contains a non-finite number",
      );
    }
    return value;
  }
  if (typeof value !== "object") {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a non-JSON value",
    );
  }
  if (ancestors.has(value)) {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a recursive value",
    );
  }
  const nextAncestors = new Set(ancestors);
  nextAncestors.add(value);
  if (Array.isArray(value)) {
    return value.map((member) => canonicalValue(member, nextAncestors));
  }
  if (Object.getPrototypeOf(value) !== Object.prototype) {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a non-plain object",
    );
  }
  return Object.fromEntries(
    Object.entries(value)
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
      .map(([key, member]) => [key, canonicalValue(member, nextAncestors)]),
  );
}

function canonicalString(value) {
  let encoded;
  try {
    encoded = JSON.stringify(canonicalValue(value));
  } catch (error) {
    if (error instanceof ProducerError) throw error;
    throw new ProducerError(
      "invalid_report_value",
      "report cannot be canonicalized",
    );
  }
  if (encoded === undefined) {
    throw new ProducerError(
      "invalid_report_value",
      "report cannot be canonicalized",
    );
  }
  return encoded;
}

export function canonicalBytes(value) {
  return Buffer.from(`${canonicalString(value)}\n`);
}

function boundedRegularBytes(path, label) {
  let metadata;
  try {
    metadata = lstatSync(path);
  } catch (error) {
    throw new ProducerError(
      "invalid_authority",
      `${label} cannot be inspected`,
    );
  }
  if (metadata.isSymbolicLink() || !metadata.isFile()) {
    throw new ProducerError(
      "invalid_authority",
      `${label} must be a regular file`,
    );
  }
  if (metadata.size > MAX_AUTHORITY_BYTES) {
    throw new ProducerError("authority_size_limit", `${label} is too large`);
  }
  let descriptor;
  try {
    descriptor = openSync(path, constants.O_RDONLY);
    const opened = fstatSync(descriptor);
    if (!opened.isFile() || opened.size > MAX_AUTHORITY_BYTES) {
      throw new ProducerError(
        opened.size > MAX_AUTHORITY_BYTES
          ? "authority_size_limit"
          : "invalid_authority",
        opened.size > MAX_AUTHORITY_BYTES
          ? `${label} is too large`
          : `${label} must be a regular file`,
      );
    }
    const bytes = Buffer.alloc(opened.size);
    let offset = 0;
    while (offset < bytes.length) {
      const count = readSync(
        descriptor,
        bytes,
        offset,
        bytes.length - offset,
        offset,
      );
      if (count === 0) {
        throw new ProducerError(
          "invalid_authority",
          `${label} was truncated while reading`,
        );
      }
      offset += count;
    }
    return bytes;
  } catch (error) {
    if (error instanceof ProducerError) throw error;
    throw new ProducerError("invalid_authority", `${label} cannot be read`);
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

export function sourceIdentity(root, relative, label) {
  const bytes = boundedRegularBytes(resolve(root, relative), label);
  return {
    path: relative,
    sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
  };
}

export function outputPath(environment = process.env) {
  const value = environment[OUTPUT_ENV];
  if (value === undefined || value.length === 0) {
    throw new ProducerError(
      "missing_output_path",
      `${OUTPUT_ENV} must be set`,
    );
  }
  if (!isAbsolute(value)) {
    throw new ProducerError(
      "invalid_output_path",
      "report path must be absolute",
    );
  }
  if (["", ".", ".."].includes(basename(value))) {
    throw new ProducerError(
      "invalid_output_path",
      "report path must name a file",
    );
  }
  return value;
}

export function publishReport(path, report) {
  const payload = canonicalBytes(report);
  if (payload.length > MAX_REPORT_BYTES) {
    throw new ProducerError(
      "report_size_limit",
      "canonical report is too large",
    );
  }
  let parent;
  try {
    parent = lstatSync(dirname(path));
  } catch (error) {
    throw new ProducerError(
      "invalid_output_parent",
      "report parent cannot be inspected",
    );
  }
  if (parent.isSymbolicLink() || !parent.isDirectory()) {
    throw new ProducerError(
      "invalid_output_parent",
      "report parent must be a real directory",
    );
  }
  let descriptor;
  try {
    descriptor = openSync(
      path,
      constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL,
      0o600,
    );
    let offset = 0;
    while (offset < payload.length) {
      const count = writeSync(
        descriptor,
        payload,
        offset,
        payload.length - offset,
        offset,
      );
      if (count === 0) {
        throw new ProducerError(
          "output_write_failed",
          "report could not be written completely",
        );
      }
      offset += count;
    }
    fsyncSync(descriptor);
  } catch (error) {
    if (error?.code === "EEXIST") {
      throw new ProducerError(
        "output_exists",
        "report destination already exists",
      );
    }
    if (error instanceof ProducerError) throw error;
    throw new ProducerError(
      "output_write_failed",
      "report could not be written",
    );
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

function repositoryRoot(environment = process.env) {
  const value = environment[REPOSITORY_ROOT_ENV];
  if (value === undefined || value.length === 0) {
    throw new ProducerError(
      "missing_repository_root",
      `${REPOSITORY_ROOT_ENV} must be set`,
    );
  }
  if (!isAbsolute(value)) {
    throw new ProducerError(
      "invalid_repository_root",
      "repository root must be absolute",
    );
  }
  let metadata;
  try {
    metadata = lstatSync(value);
  } catch (error) {
    throw new ProducerError(
      "invalid_repository_root",
      "repository root cannot be inspected",
    );
  }
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    throw new ProducerError(
      "invalid_repository_root",
      "repository root must be a real directory",
    );
  }
  return value;
}

const REMOTE_CAPABILITIES = [
  "query.execution.batch-identity-rebind",
  "query.execution.same-snapshot-hydration",
  "query.input.given-rows",
  "query.operation.distinct-count",
  "query.operation.distinct-exists",
  "query.operation.exactly-one",
  "query.operation.page",
  "query.order.stable-collection",
  "query.order.stable-root",
  "query.order.stable-selected",
  "query.output.collect",
  "query.output.collect-distinct",
  "query.output.hydrated",
  "query.output.named",
  "query.output.rows",
  "query.pattern.has",
  "query.pattern.iid",
  "query.pattern.isa",
  "query.pattern.isa-subtypes",
  "query.plan",
  "query.plan.v2",
  "query.remote.envelope-v2",
  "query.remote.structured-diagnostic",
  "query.stage.distinct",
  "query.stage.limit",
  "query.stage.offset",
  "query.stage.require",
  "query.stage.select",
  "query.stage.sort",
];
const SIGNING_SEED = Buffer.alloc(32, 0x42);
const SIGNING_PRIVATE_KEY = crypto.createPrivateKey({
  key: Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"),
    SIGNING_SEED,
  ]),
  format: "der",
  type: "pkcs8",
});
const SIGNING_PUBLIC_KEY = crypto
  .createPublicKey(SIGNING_PRIVATE_KEY)
  .export({ format: "der", type: "spki" })
  .subarray(-32);
const SIGNING_KEY_ID = crypto
  .createHash("sha256")
  .update(Buffer.from("typebridge.query.remote-reply-key-id/v1\0"))
  .update(SIGNING_PUBLIC_KEY)
  .digest("hex");

function fingerprint(domain, canonicalization, payload) {
  const digest = crypto.createHash("sha256");
  digest.update(Buffer.from("typebridge.fingerprint/v1\0"));
  for (const value of [domain, canonicalization]) {
    const encoded = Buffer.from(value);
    const length = Buffer.alloc(8);
    length.writeBigUInt64BE(BigInt(encoded.length));
    digest.update(length);
    digest.update(encoded);
  }
  digest.update(Buffer.from([0]));
  const length = Buffer.alloc(8);
  length.writeBigUInt64BE(BigInt(payload.length));
  digest.update(length);
  digest.update(payload);
  return digest.digest("hex");
}

function remoteAdvertisement() {
  return Buffer.from(
    canonicalString({
      capabilities: REMOTE_CAPABILITIES,
      executor: {
        epoch: "node-phase2-epoch-0001",
        identity: "node-phase2-executor",
      },
      format: "typebridge.query-remote-capabilities/v1",
      reply_key: SIGNING_PUBLIC_KEY.toString("hex"),
      reply_key_id: SIGNING_KEY_ID,
    }),
  );
}

function remoteSignedReply(payload, advertisement) {
  const advertisementFingerprint = fingerprint(
    "typebridge.query.remote-capabilities",
    "typebridge.query-remote-capabilities/v1",
    advertisement,
  );
  const prefix = Buffer.from(
    `{"advertisement":"${advertisementFingerprint}",` +
      `"format":"typebridge.query-remote-signed-reply/v1",` +
      `"key":"${SIGNING_PUBLIC_KEY.toString("hex")}",` +
      `"key_id":"${SIGNING_KEY_ID}","payload":`,
  );
  const payloadBytes = Buffer.from(canonicalString(payload));
  const digest = crypto
    .createHash("sha256")
    .update(Buffer.from("typebridge.query.remote-reply-signature/v1\0"))
    .update(prefix)
    .update(payloadBytes)
    .update(Buffer.from("}"))
    .digest();
  const signature = crypto
    .sign(null, digest, SIGNING_PRIVATE_KEY)
    .toString("hex");
  return Buffer.concat([
    prefix,
    payloadBytes,
    Buffer.from(`,"signature":"${signature}"}`),
  ]);
}

function typeId(kind, label) {
  return { kind, label };
}

function wireValue(kind, value) {
  return { kind, value };
}

const ADA_SCALARS = Object.freeze({
  boolean: false,
  date: new Date("2026-08-12T00:00:00.000Z"),
  datetime: new Date("2026-08-12T09:30:00.000Z"),
  datetime_tz: new Date("2026-08-12T09:30:00.000Z"),
  decimal: "38.5",
  double: 38.0,
  duration: "PT38S",
  long: 38n,
  string: "Ada",
});

function canonicalDateTime(value, timezone) {
  const withoutZeroFraction = value.toISOString().replace(".000Z", "Z");
  return timezone ? withoutZeroFraction : withoutZeroFraction.slice(0, -1);
}

function wireScalar(kind, value) {
  if (kind === "double" && typeof value === "number") {
    const encoded = Buffer.alloc(8);
    encoded.writeDoubleBE(value);
    return { bits: encoded.toString("hex"), kind };
  }
  if (kind === "long" && typeof value === "bigint") {
    return wireValue(kind, value.toString());
  }
  if (kind === "boolean" && typeof value === "boolean") {
    return wireValue(kind, value);
  }
  if (kind === "date" && value instanceof Date) {
    return wireValue(kind, value.toISOString().slice(0, 10));
  }
  if (kind === "datetime" && value instanceof Date) {
    return wireValue(kind, canonicalDateTime(value, false));
  }
  if (kind === "datetime_tz" && value instanceof Date) {
    return wireValue(kind, {
      effective_offset_seconds: 0,
      local: canonicalDateTime(value, false),
      zone: { kind: "utc" },
    });
  }
  if (
    (kind === "decimal" || kind === "duration" || kind === "string") &&
    typeof value === "string"
  ) {
    return wireValue(kind, value);
  }
  throw new Error(`unexpected ${kind} scalar`);
}

function reportScalar(kind, value) {
  if (kind === "datetime_tz") {
    requireCondition(value instanceof Date, "datetime-tz value is not a Date");
    return wireValue(kind, canonicalDateTime(value, true));
  }
  return wireScalar(kind, value);
}

function personNode(
  nodeId,
  iid,
  { identifier, nickname, aliases, score, fooBar = null, scoreGte = null },
) {
  const scalars = { ...ADA_SCALARS, long: score, string: nickname };
  return {
    attributes: [
      {
        attribute: "aliases",
        values: aliases.map((alias) => wireScalar("string", alias)),
      },
      {
        attribute: "foo__bar",
        values: fooBar === null ? [] : [wireScalar("long", fooBar)],
      },
      {
        attribute: "identifier",
        values: [wireScalar("string", identifier)],
      },
      {
        attribute: "nickname",
        values: [wireScalar("string", nickname)],
      },
      { attribute: "score", values: [wireScalar("long", score)] },
      {
        attribute: "score__gte",
        values: scoreGte === null ? [] : [wireScalar("long", scoreGte)],
      },
      {
        attribute: "val_bool",
        values: [wireScalar("boolean", scalars.boolean)],
      },
      {
        attribute: "val_constrained",
        values: [wireScalar("long", score)],
      },
      {
        attribute: "val_date",
        values: [wireScalar("date", scalars.date)],
      },
      {
        attribute: "val_datetime",
        values: [wireScalar("datetime", scalars.datetime)],
      },
      {
        attribute: "val_datetime_tz",
        values: [wireScalar("datetime_tz", scalars.datetime_tz)],
      },
      {
        attribute: "val_decimal",
        values: [wireScalar("decimal", scalars.decimal)],
      },
      {
        attribute: "val_double",
        values: [wireScalar("double", scalars.double)],
      },
      {
        attribute: "val_duration",
        values: [wireScalar("duration", scalars.duration)],
      },
    ],
    concrete: typeId("entity", "person"),
    id: nodeId,
    iid,
    kind: "entity",
    roles: [],
  };
}

function robotNode(nodeId, iid, key) {
  return {
    attributes: [
      { attribute: "nickname", values: [] },
      { attribute: "robot_id", values: [wireScalar("long", key)] },
      {
        attribute: "val_constrained",
        values: [wireScalar("long", 38n)],
      },
    ],
    concrete: typeId("entity", "robot"),
    id: nodeId,
    iid,
    kind: "entity",
    roles: [],
  };
}

function reference(kind, label, node) {
  return { declared: typeId(kind, label), node };
}

function role(declaringRelation, label, players) {
  return {
    players,
    role: { declaring_relation: declaringRelation, label },
  };
}

function relationNode(
  nodeId,
  iid,
  label,
  { attributes = [], roles = [] } = {},
) {
  return {
    attributes,
    concrete: typeId("relation", label),
    id: nodeId,
    iid,
    kind: "relation",
    roles,
  };
}

function adaNode(nodeId = 0, iid = "0x01") {
  return personNode(nodeId, iid, {
    identifier: "data-ada",
    nickname: "Ada",
    aliases: ["analyst", "mathematician"],
    score: 38n,
    fooBar: 1n,
    scoreGte: 2n,
  });
}

function danaNode(nodeId = 1, iid = "0x02") {
  return personNode(nodeId, iid, {
    identifier: "data-dana",
    nickname: "Dana",
    aliases: [],
    score: 41n,
  });
}

function hydratedRowsReply(request, advertisement, graph, declared, node) {
  const requestBytes = Buffer.from(request);
  const decoded = JSON.parse(requestBytes.toString("utf8"));
  requireCondition(
    decoded !== null && typeof decoded.plan === "object",
    "generated remote request is malformed",
  );
  const plan = Buffer.from(canonicalString(decoded.plan));
  return remoteSignedReply(
    {
      format: "typebridge.query-remote-response/v2",
      nonce: decoded.nonce,
      outcome: {
        graph,
        kind: "hydrated_rows",
        rows: [
          {
            slots: [
              {
                kind: "singular",
                value: { declared, node },
              },
            ],
          },
        ],
      },
      plan: fingerprint(
        "typebridge.query.plan",
        "typebridge.query-plan-c14n/v2",
        plan,
      ),
      request: fingerprint(
        "typebridge.query.remote-request",
        "typebridge.query-remote-request/v2",
        requestBytes,
      ),
    },
    advertisement,
  );
}

async function hydrateExact(
  package_,
  model,
  graph,
  rootNode,
  { corruptSignature = false } = {},
) {
  const advertisement = remoteAdvertisement();
  let exchanges = 0;
  const declared = JSON.parse(model.typeKey);
  const session = new package_.RemoteQuerySession(
    advertisement,
    async (request) => {
      exchanges += 1;
      const reply = hydratedRowsReply(
        request,
        advertisement,
        graph,
        declared,
        rootNode,
      );
      if (corruptSignature) {
        const signatureOffset = reply.lastIndexOf(Buffer.from('"signature":"'));
        requireCondition(
          signatureOffset >= 0,
          "signed remote reply has no signature field",
        );
        const mutated = Buffer.from(reply);
        const byte = signatureOffset + Buffer.byteLength('"signature":"');
        mutated[byte] = mutated[byte] === 0x30 ? 0x31 : 0x30;
        return mutated;
      }
      return reply;
    },
    {
      maxItems: 8n,
      maxBytes: 1n << 20n,
      maxCollectionMembers: 16n,
      maxGraphNodes: 16n,
      maxAttributeValues: 128n,
      maxRolePlayers: 64n,
    },
  );
  try {
    const result = await session.query(session.exact(model)).one();
    requireCondition(
      exchanges === 1,
      "generated hydration did not use exactly one caller transport",
    );
    return result;
  } finally {
    session.close();
  }
}

async function proveAuthenticatedHydration(package_) {
  try {
    await hydrateExact(
      package_,
      package_.Person,
      { nodes: [adaNode()] },
      0,
      { corruptSignature: true },
    );
  } catch (error) {
    const text = error instanceof Error ? error.message : String(error);
    requireCondition(
      /signature|signed|remote_reply/i.test(text),
      "corrupted hydration reply did not fail at authentication",
    );
    return;
  }
  throw new Error("corrupted hydration signature was accepted");
}

function makePerson(
  package_,
  {
    identifier = "data-ada",
    nickname = "Ada",
    aliases = ["analyst", "mathematician"],
    score = 38n,
    fooBar = 1n,
    scoreGte = 2n,
    ...extra
  } = {},
) {
  return package_.Person.create({
    aliases: aliases.map((alias) => package_.Aliases.create(alias)),
    fooBar: fooBar === null ? null : package_.FooBar.create(fooBar),
    identifier: package_.Identifier.create(identifier),
    nickname: package_.Nickname.create(nickname),
    score: package_.Score.create(score),
    scoreGte:
      scoreGte === null ? null : package_.ScoreGte.create(scoreGte),
    valBool: package_.ValBool.create(ADA_SCALARS.boolean),
    valConstrained: package_.ValConstrained.create(score),
    valDate: package_.ValDate.create(ADA_SCALARS.date),
    valDatetime: package_.ValDatetime.create(ADA_SCALARS.datetime),
    valDatetimeTz: package_.ValDatetimeTz.create(ADA_SCALARS.datetime_tz),
    valDecimal: package_.ValDecimal.create(ADA_SCALARS.decimal),
    valDouble: package_.ValDouble.create(ADA_SCALARS.double),
    valDuration: package_.ValDuration.create(ADA_SCALARS.duration),
    ...extra,
  });
}

function modelLabel(value) {
  const identity = JSON.parse(value.__typebridgeModel);
  requireCondition(
    typeof identity.label === "string",
    "generated value has no canonical model label",
  );
  return identity.label;
}

function attributeValues(person) {
  const fields = {
    boolean: "valBool",
    date: "valDate",
    datetime: "valDatetime",
    datetime_tz: "valDatetimeTz",
    decimal: "valDecimal",
    double: "valDouble",
    duration: "valDuration",
    long: "score",
    string: "nickname",
  };
  return Object.fromEntries(
    Object.entries(fields).map(([domain, field]) => [
      domain,
      reportScalar(domain, person[field].value),
    ]),
  );
}

function hasAnnotation(token, kind) {
  return token.metadata.annotations.some(
    (annotation) => annotation.id.kind.kind === kind,
  );
}

function tokenObservation(token, bindingName) {
  const owner = JSON.parse(token.owner);
  const attribute = JSON.parse(token.attribute);
  requireCondition(
    typeof owner.kind === "string" &&
      typeof owner.label === "string" &&
      typeof attribute === "string",
    "generated field token identity is malformed",
  );
  return {
    binding_name: bindingName,
    canonical_attribute: `attribute:${attribute}`,
    canonical_owner: `${owner.kind}:${owner.label}`,
    owns_fact: `${owner.label}:${attribute}`,
  };
}

function diagnosticFrom(error) {
  if (
    error !== null &&
    typeof error === "object" &&
    typeof error.sdkCategory === "string" &&
    typeof error.code === "string"
  ) {
    return {
      category: error.sdkCategory,
      code: error.code,
      details: error.details ?? {},
      path: error.path ?? [],
    };
  }
  if (error instanceof Error) {
    try {
      const value = JSON.parse(error.message);
      if (
        value !== null &&
        typeof value === "object" &&
        typeof value.sdkCategory === "string" &&
        typeof value.code === "string"
      ) {
        return {
          category: value.sdkCategory,
          code: value.code,
          details: value.details ?? {},
          path: value.path ?? [],
        };
      }
    } catch {
      return null;
    }
  }
  return null;
}

function expectRejection(
  action,
  { names = [], category = null, code = null } = {},
) {
  try {
    action();
  } catch (error) {
    if (names.length > 0 && !names.includes(error?.name)) {
      throw new Error(
        `expected ${names.join(", ")}, received ${error?.name ?? typeof error}`,
      );
    }
    const diagnostic = diagnosticFrom(error);
    if (category !== null && diagnostic?.category !== category) {
      throw new Error(
        `expected rejection category ${category}, received ${diagnostic?.category}`,
      );
    }
    if (code !== null && diagnostic?.code !== code) {
      throw new Error(
        `expected rejection code ${code}, received ${diagnostic?.code}`,
      );
    }
    return { diagnostic, error };
  }
  throw new Error("provider-free projection probe unexpectedly succeeded");
}

function constructedObservations(package_) {
  const ada = makePerson(package_);
  const dana = makePerson(package_, {
    identifier: "data-dana",
    nickname: "Dana",
    aliases: [],
    score: 41n,
    fooBar: null,
    scoreGte: null,
  });
  const negativeRobot = package_.Robot.create({
    robotId: package_.RobotId.create(-7n),
    valConstrained: package_.ValConstrained.create(38n),
  });
  const positiveRobot = package_.Robot.create({
    robotId: package_.RobotId.create(7n),
    valConstrained: package_.ValConstrained.create(38n),
  });
  const plain = package_.PlainActivity.create({ participant: ada });
  const link = package_.NetworkLink.create({
    identifier: package_.Identifier.create("network-link-ordered"),
    destination: dana,
    origin: ada,
    participant: [ada, dana],
  });
  const interactionPerson = package_.Interaction.create({
    identifier: package_.Identifier.create("interaction-person"),
    actor: ada,
    target: ada,
  });
  const interactionRobot = package_.Interaction.create({
    identifier: package_.Identifier.create("interaction-robot"),
    actor: positiveRobot,
    target: ada,
  });
  const interactionAbsent = package_.Interaction.create({
    identifier: package_.Identifier.create("interaction-absent"),
    target: ada,
  });
  const event = package_.Event.reference("0x20", {});
  const container = package_.Container.create({ item: [event] });
  return {
    ada,
    container,
    dana,
    interactionAbsent,
    interactionPerson,
    interactionRobot,
    link,
    negativeRobot,
    plain,
    positiveRobot,
  };
}

async function hydratedObservations(package_) {
  const ada = await hydrateExact(package_, package_.Person, { nodes: [adaNode()] }, 0);
  const negativeRobot = await hydrateExact(
    package_,
    package_.Robot,
    { nodes: [robotNode(0, "0x01", -7n)] },
    0,
  );
  const positiveRobot = await hydrateExact(
    package_,
    package_.Robot,
    { nodes: [robotNode(0, "0x01", 7n)] },
    0,
  );
  const plain = await hydrateExact(
    package_,
    package_.PlainActivity,
    {
      nodes: [
        adaNode(),
        relationNode(1, "0x10", "plain-activity", {
          roles: [
            role("plain-activity", "participant", [
              reference("entity", "person", 0),
            ]),
          ],
        }),
      ],
    },
    1,
  );
  const link = await hydrateExact(
    package_,
    package_.NetworkLink,
    {
      nodes: [
        adaNode(),
        danaNode(),
        relationNode(2, "0x10", "network-link", {
          attributes: [
            {
              attribute: "identifier",
              values: [wireScalar("string", "network-link-ordered")],
            },
            { attribute: "nickname", values: [] },
          ],
          roles: [
            role("network-link", "destination", [
              reference("entity", "person", 1),
            ]),
            role("network-link", "origin", [
              reference("entity", "person", 0),
            ]),
            role("network-link", "participant", [
              reference("entity", "person", 0),
              reference("entity", "person", 1),
            ]),
          ],
        }),
      ],
    },
    2,
  );

  async function interaction(identifier, actor) {
    const nodes = [adaNode()];
    let actorReferences = [];
    if (actor !== null) {
      const actorNode = structuredClone(actor);
      const actorType = actorNode.concrete;
      let actorIndex = 0;
      if (actorType.label !== "person") {
        actorIndex = 1;
        actorNode.id = actorIndex;
        actorNode.iid = "0x02";
        nodes.push(actorNode);
      }
      actorReferences = [
        reference(actorType.kind, actorType.label, actorIndex),
      ];
    }
    const relationId = nodes.length;
    nodes.push(
      relationNode(relationId, "0x10", "interaction", {
        attributes: [
          {
            attribute: "identifier",
            values: [wireScalar("string", identifier)],
          },
          { attribute: "nickname", values: [] },
        ],
        roles: [
          role("interaction", "actor", actorReferences),
          role("interaction", "target", [reference("entity", "person", 0)]),
        ],
      }),
    );
    return hydrateExact(
      package_,
      package_.Interaction,
      { nodes },
      relationId,
    );
  }

  const interactionPerson = await interaction("interaction-person", adaNode());
  const interactionRobot = await interaction(
    "interaction-robot",
    robotNode(0, "0x00", 7n),
  );
  const interactionAbsent = await interaction("interaction-absent", null);
  const container = await hydrateExact(
    package_,
    package_.Container,
    {
      nodes: [
        relationNode(0, "0x01", "event"),
        relationNode(1, "0x10", "container", {
          roles: [
            role("container", "item", [reference("relation", "event", 0)]),
          ],
        }),
      ],
    },
    1,
  );
  return {
    ada,
    container,
    interactionAbsent,
    interactionPerson,
    interactionRobot,
    link,
    negativeRobot,
    plain,
    positiveRobot,
  };
}

function constraintObservation(package_, constructed) {
  const providerCalls = 0;
  const families = new Set();
  const rejected = (family, action, expectation = {}) => {
    const before = providerCalls;
    const rejection = expectRejection(action, expectation);
    requireCondition(
      providerCalls === before,
      `${family} rejection crossed the provider boundary`,
    );
    families.add(family);
    return rejection;
  };

  rejected("abstract_constructibility", () => package_.Actor.create({}), {
    names: ["TypeError"],
  });
  rejected("allowed_values", () => package_.Nickname.create("Grace"), {
    category: "invalid_input",
    code: "values_constraint_violation",
  });
  requireCondition(
    !package_.Person.metadata.create.fields.some(
      (field) => field.token.attribute === "party_name",
    ),
    "person exposed a non-constructible ownership",
  );
  rejected(
    "field_constructibility",
    () => makePerson(package_, { partyName: package_.PartyName.create("Ada") }),
    { names: ["TypeError"] },
  );

  requireCondition(
    package_.Person.nickname.metadata.declaring_id.owner.kind === "entity" &&
      package_.Person.nickname.metadata.declaring_id.owner.label === "actor",
    "person did not retain inherited nickname ownership",
  );
  rejected(
    "inherited_owns",
    () => makePerson(package_, { nickname: package_.Score.create(38n) }),
  );

  const playingIdentity = canonicalString({
    player: typeId("entity", "person"),
    role: { declaring_relation: "base-activity", label: "participant" },
  });
  requireCondition(
    package_.PLAYING_FACTS.some(
      (fact) => canonicalString({
        player: JSON.parse(fact.player),
        role: JSON.parse(fact.role),
      }) === playingIdentity,
    ),
    "person did not retain its inherited playing fact",
  );
  rejected(
    "inherited_plays",
    () =>
      package_.PlainActivity.create({ participant: constructed.positiveRobot }),
    { names: ["TypeError"] },
  );

  const inheritedRole = JSON.parse(package_.PlainActivity.participant.role);
  requireCondition(
    canonicalString(inheritedRole) ===
      canonicalString({
        declaring_relation: "base-activity",
        label: "participant",
      }),
    "plain activity lost its inherited role identity",
  );
  rejected("inherited_relates", () => package_.PlainActivity.create({}), {
    names: ["RangeError"],
  });
  rejected(
    "invalid_player_type",
    () => package_.Event.create({ subject: constructed.positiveRobot }),
    { names: ["TypeError"] },
  );

  requireCondition(
    package_.Person.identifier.key === true,
    "person identifier lost its key fact",
  );
  const personWithoutKey = {
    aliases: constructed.ada.aliases,
    fooBar: constructed.ada.fooBar,
    nickname: constructed.ada.nickname,
    score: constructed.ada.score,
    scoreGte: constructed.ada.scoreGte,
    valBool: constructed.ada.valBool,
    valConstrained: constructed.ada.valConstrained,
    valDate: constructed.ada.valDate,
    valDatetime: constructed.ada.valDatetime,
    valDatetimeTz: constructed.ada.valDatetimeTz,
    valDecimal: constructed.ada.valDecimal,
    valDouble: constructed.ada.valDouble,
    valDuration: constructed.ada.valDuration,
  };
  rejected("key", () => package_.Person.create(personWithoutKey), {
    names: ["RangeError"],
  });

  rejected(
    "maximum_cardinality",
    () => makePerson(package_, { aliases: ["one", "two", "three", "four"] }),
    { names: ["RangeError"] },
  );
  const playerDuplicate = rejected(
    "ordered_distinct_player",
    () =>
      package_.NetworkLink.create({
        identifier: package_.Identifier.create("network-link-duplicate"),
        destination: constructed.dana,
        origin: constructed.ada,
        participant: [constructed.ada, constructed.ada],
      }),
    {
      category: "invalid_input",
      code: "ordered_distinct_duplicate",
    },
  );
  const scalarDuplicate = rejected(
    "ordered_distinct_scalar",
    () => makePerson(package_, { aliases: ["analyst", "analyst"] }),
    {
      category: "invalid_input",
      code: "ordered_distinct_duplicate",
    },
  );

  const personWithoutScore = { ...personWithoutKey, identifier: constructed.ada.identifier };
  delete personWithoutScore.score;
  rejected(
    "ownership_cardinality",
    () => package_.Person.create(personWithoutScore),
    { names: ["RangeError"] },
  );
  const rangeError = rejected(
    "range",
    () => package_.ValConstrained.create(81n),
    { category: "invalid_input", code: "range_constraint_violation" },
  );
  rejected("regex", () => package_.Nickname.create("ada"), {
    category: "invalid_input",
    code: "regex_constraint_violation",
  });
  rejected("required_cardinality", () => package_.Counter.create({}), {
    names: ["RangeError"],
  });
  rejected(
    "role_cardinality",
    () =>
      package_.Container.create({
        item: [
          package_.Event.reference("0x01", {}),
          package_.Event.reference("0x02", {}),
          package_.Event.reference("0x03", {}),
        ],
      }),
    { names: ["RangeError"] },
  );
  requireCondition(
    !package_.Employment.metadata.create.roles.some(
      (entry) => entry.role.label === "member",
    ),
    "employment exposed its specialized-away role",
  );
  rejected(
    "role_constructibility",
    () =>
      package_.Employment.create({
        employee: constructed.ada,
        member: constructed.ada,
      }),
    { names: ["TypeError"] },
  );
  rejected("scalar_domain", () => package_.Score.create(1n << 63n));

  const expectedFamilies = [
    "abstract_constructibility",
    "allowed_values",
    "field_constructibility",
    "inherited_owns",
    "inherited_plays",
    "inherited_relates",
    "invalid_player_type",
    "key",
    "maximum_cardinality",
    "ordered_distinct_player",
    "ordered_distinct_scalar",
    "ownership_cardinality",
    "range",
    "regex",
    "required_cardinality",
    "role_cardinality",
    "role_constructibility",
    "scalar_domain",
  ];
  requireCondition(
    canonicalString([...families].sort()) === canonicalString(expectedFamilies),
    "projected rejection-family ledger is incomplete",
  );

  const scalarDomains = [
    package_.ValBool,
    package_.ValDate,
    package_.ValDatetime,
    package_.ValDatetimeTz,
    package_.ValDecimal,
    package_.ValDouble,
    package_.ValDuration,
    package_.Score,
    package_.Nickname,
  ]
    .map((model) => model.valueType)
    .sort();
  requireCondition(
    new Set(scalarDomains).size === scalarDomains.length,
    "generated package did not expose all nine scalar domains",
  );
  requireCondition(
    package_.Person.aliases.unique === true,
    "generated projection did not retain provider-owned uniqueness",
  );

  const diagnostic = rangeError.diagnostic;
  requireCondition(
    diagnostic !== null &&
      diagnostic.path.length === 1 &&
      typeof diagnostic.path[0].value === "object" &&
      typeof diagnostic.details.actual === "object" &&
      typeof diagnostic.details.maximum === "object",
    "representative range diagnostic is malformed",
  );
  const rangeType = diagnostic.path[0].value;
  return {
    observation: {
      provider_enforced_families: [
        {
          family: "unique",
          local_preflight: "not_applicable",
          projection_fact_retained: true,
          provider_enforced: true,
        },
      ],
      rejection_families: expectedFamilies.map((family) => ({
        family,
        rejected: true,
        rejected_before_provider_io: true,
      })),
      representative_diagnostic: {
        category: diagnostic.category,
        code: diagnostic.code,
        details: {
          actual: {
            kind: diagnostic.details.actual.kind,
            value: String(diagnostic.details.actual.value),
          },
          maximum: {
            kind: diagnostic.details.maximum.kind,
            value: String(diagnostic.details.maximum.value),
          },
        },
        path: [
          {
            kind: "type",
            value: `${rangeType.kind}:${rangeType.label}`,
          },
        ],
        provider_calls: providerCalls,
      },
      scalar_domains: scalarDomains,
    },
    playerDuplicate: playerDuplicate.diagnostic,
    scalarDuplicate: scalarDuplicate.diagnostic,
  };
}

function duplicateIndices(diagnostic) {
  requireCondition(
    diagnostic !== null &&
      diagnostic.details.first_index?.kind === "count" &&
      diagnostic.details.duplicate_index?.kind === "count",
    "ordered duplicate diagnostic lost its indices",
  );
  const first = Number(diagnostic.details.first_index.value);
  const duplicate = Number(diagnostic.details.duplicate_index.value);
  requireCondition(
    Number.isSafeInteger(first) && Number.isSafeInteger(duplicate),
    "ordered duplicate diagnostic indices are not exact integers",
  );
  return [first, duplicate];
}

function hydrationSymbol(model) {
  const symbol = Object.getOwnPropertySymbols(model).find(
    (candidate) => candidate.description === "typebridge.hydrate-complete",
  );
  requireCondition(symbol !== undefined, "generated model has no hydration hook");
  return symbol;
}

async function foreignHydrationRejection(package_, foreign) {
  const foreignPerson = await hydrateExact(
    foreign,
    foreign.Person,
    { nodes: [adaNode()] },
    0,
  );
  let publicResultPublished = false;
  const hydrate = hydrationSymbol(package_.PlainActivity);
  const rejection = expectRejection(
    () => {
      const result = package_.PlainActivity[hydrate]("0xf0", {
        participant: foreignPerson,
      });
      publicResultPublished = result !== undefined;
    },
    {
      category: "integrity",
      code: "generated_token_package_mismatch",
    },
  );
  return { diagnostic: rejection.diagnostic, publicResultPublished };
}

async function buildReport(root, package_, foreign) {
  await proveAuthenticatedHydration(package_);
  const constructed = constructedObservations(package_);
  const hydrated = await hydratedObservations(package_);
  const constraints = constraintObservation(package_, constructed);
  const localConstruction =
    constructed.ada.__typebridgeModel === package_.Person.typeKey &&
    constructed.ada.__typebridgeForm === "complete" &&
    Object.isFrozen(constructed.ada);
  const localHydration =
    hydrated.ada.__typebridgeModel === package_.Person.typeKey &&
    hydrated.ada.__typebridgeForm === "complete" &&
    hydrated.ada.iid === "0x01" &&
    Object.isFrozen(hydrated.ada);
  requireCondition(
    localConstruction && localHydration,
    "local package construction or hydration changed its exact facade",
  );

  const authoredScalars = Object.fromEntries(
    Object.entries(ADA_SCALARS).map(([domain, value]) => [
      domain,
      reportScalar(domain, value),
    ]),
  );
  const constructedScalars = attributeValues(constructed.ada);
  const hydratedScalars = attributeValues(hydrated.ada);
  requireCondition(
    canonicalString(authoredScalars) === canonicalString(constructedScalars) &&
      canonicalString(constructedScalars) === canonicalString(hydratedScalars),
    "canonical scalar construction and hydration diverged",
  );

  const fooToken = package_.Person.fooBar;
  const scoreGteToken = package_.Person.scoreGte;
  const generatedTokens = [
    tokenObservation(fooToken, "foo__bar"),
    tokenObservation(scoreGteToken, "score__gte"),
  ];
  const tokenIdentitiesDistinct =
    fooToken !== scoreGteToken && fooToken.attribute !== scoreGteToken.attribute;
  const packageBranded =
    fooToken !== foreign.Person.fooBar &&
    scoreGteToken !== foreign.Person.scoreGte &&
    package_.Person !== foreign.Person;
  requireCondition(
    tokenIdentitiesDistinct && packageBranded,
    "generated field token identities are not exact and package-owned",
  );

  const inheritedRole = JSON.parse(package_.PlainActivity.participant.role);
  const inheritedConstructed =
    modelLabel(constructed.plain) === "plain-activity" &&
    modelLabel(constructed.plain.participant) === "person";
  const inheritedHydrated =
    modelLabel(hydrated.plain) === "plain-activity" &&
    modelLabel(hydrated.plain.participant) === "person";
  const roleIdentityPreserved =
    canonicalString(inheritedRole) ===
    canonicalString({
      declaring_relation: "base-activity",
      label: "participant",
    });
  requireCondition(
    inheritedConstructed && inheritedHydrated && roleIdentityPreserved,
    "inherited abstract relation role did not round-trip exactly",
  );

  const negativeConstructed = constructed.negativeRobot.robotId.value;
  const positiveConstructed = constructed.positiveRobot.robotId.value;
  const negativeHydrated = hydrated.negativeRobot.robotId.value;
  const positiveHydrated = hydrated.positiveRobot.robotId.value;
  requireCondition(
    negativeConstructed === -7n &&
      negativeHydrated === negativeConstructed &&
      positiveConstructed === 7n &&
      positiveHydrated === positiveConstructed,
    "signed integer key hydration was not exact",
  );

  const polymorphicPlayers = [
    constructed.interactionPerson.actor,
    constructed.interactionRobot.actor,
    hydrated.interactionPerson.actor,
    hydrated.interactionRobot.actor,
  ]
    .map(modelLabel)
    .filter((value, index, values) => values.indexOf(value) === index)
    .sort();
  requireCondition(
    canonicalString(polymorphicPlayers) === canonicalString(["person", "robot"]),
    "optional polymorphic role did not preserve both player models",
  );
  requireCondition(
    constructed.interactionAbsent.actor === null &&
      hydrated.interactionAbsent.actor === null,
    "absent optional role materialized as a present player",
  );
  requireCondition(
    hydrated.interactionRobot.actor.robotId.value === 7n,
    "polymorphic robot player lost its signed key",
  );

  const relationPlayerConstructed = constructed.container.item[0];
  const relationPlayerHydrated = hydrated.container.item[0];
  const relationPlayerPreserved =
    modelLabel(relationPlayerConstructed) === "event" &&
    modelLabel(relationPlayerHydrated) === "event";
  requireCondition(
    relationPlayerPreserved,
    "relation-as-player materialization changed its model identity",
  );

  const constructedAliases = constructed.ada.aliases.map((value) => value.value);
  const hydratedAliases = hydrated.ada.aliases.map((value) => value.value);
  const constructedParticipants = constructed.link.participant.map(
    (player) => player.identifier.value,
  );
  const hydratedParticipants = hydrated.link.participant.map(
    (player) => player.identifier.value,
  );
  const authoredAliases = ["analyst", "mathematician"];
  const authoredParticipants = ["data-ada", "data-dana"];
  requireCondition(
    canonicalString(constructedAliases) === canonicalString(authoredAliases) &&
      canonicalString(hydratedAliases) === canonicalString(authoredAliases) &&
      canonicalString(constructedParticipants) ===
        canonicalString(authoredParticipants) &&
      canonicalString(hydratedParticipants) ===
        canonicalString(authoredParticipants),
    "ordered projected collections did not preserve caller order",
  );

  const aliasesMode = package_.Person.aliases.multiplicity.collection_mode;
  const participantMode =
    package_.NetworkLink.participant.multiplicity.collection_mode;
  requireCondition(
    aliasesMode === "ordered_list" && participantMode === "ordered_list",
    "ordered collection mode was not retained",
  );
  requireCondition(
    hasAnnotation(package_.Person.aliases, "distinct") &&
      hasAnnotation(package_.NetworkLink.participant, "distinct"),
    "ordered-distinct projection facts were not retained",
  );
  const unorderedDefault =
    package_.Container.item.multiplicity.collection_mode === undefined &&
    package_.Container.item.multiplicity.container === "sequence";
  requireCondition(
    unorderedDefault,
    "legacy unordered collection compatibility changed",
  );
  const [scalarFirst, scalarSecond] = duplicateIndices(
    constraints.scalarDuplicate,
  );
  const [playerFirst, playerSecond] = duplicateIndices(
    constraints.playerDuplicate,
  );

  const foreignPerson = makePerson(foreign);
  const constructionRejection = expectRejection(
    () => package_.PlainActivity.create({ participant: foreignPerson }),
    {
      category: "integrity",
      code: "generated_token_package_mismatch",
    },
  );
  const hydrationRejection = await foreignHydrationRejection(package_, foreign);
  requireCondition(
    !hydrationRejection.publicResultPublished,
    "foreign hydration published a generated model before rejection",
  );
  const diagnosticText =
    `${constructionRejection.error}\n` +
    canonicalString(hydrationRejection.diagnostic);
  const providerTextExposed = diagnosticText.includes("phase2-provider-secret");
  requireCondition(
    !providerTextExposed,
    "foreign-package diagnostic exposed provider text",
  );

  const observations = {
    canonical_scalar_values: {
      authored: authoredScalars,
      constructed: constructedScalars,
      hydrated: hydratedScalars,
    },
    field_name_identity: {
      generated_tokens: generatedTokens,
      package_branded: packageBranded,
      token_identities_distinct: tokenIdentitiesDistinct,
    },
    inherited_relation_role: {
      constructed: inheritedConstructed,
      hydrated: inheritedHydrated,
      inherited_relation: inheritedRole.declaring_relation,
      inherited_role: inheritedRole.label,
      model: modelLabel(constructed.plain),
      player_model: modelLabel(constructed.plain.participant),
      role_identity_preserved: roleIdentityPreserved,
    },
    integer_key_polymorphic_role: {
      absent: {
        relation_ref: hydrated.interactionAbsent.identifier.value,
        role_present: hydrated.interactionAbsent.actor !== null,
        round_trip_exact:
          constructed.interactionAbsent.identifier.value ===
          hydrated.interactionAbsent.identifier.value,
      },
      integer_keys: [
        {
          model: modelLabel(hydrated.negativeRobot),
          round_trip_exact: negativeConstructed === negativeHydrated,
          sign: "negative",
          value: negativeHydrated.toString(),
        },
        {
          model: modelLabel(hydrated.positiveRobot),
          round_trip_exact: positiveConstructed === positiveHydrated,
          sign: "positive",
          value: positiveHydrated.toString(),
        },
      ],
      optional_role: JSON.parse(package_.Interaction.actor.role).label,
      polymorphic_players_observed: polymorphicPlayers,
      present: [
        {
          player: {
            key: hydrated.interactionPerson.actor.identifier.value,
            model: modelLabel(hydrated.interactionPerson.actor),
          },
          relation_ref: hydrated.interactionPerson.identifier.value,
        },
        {
          player: {
            key: hydrated.interactionRobot.actor.robotId.value.toString(),
            model: modelLabel(hydrated.interactionRobot.actor),
          },
          relation_ref: hydrated.interactionRobot.identifier.value,
        },
      ],
      relation: modelLabel(hydrated.interactionPerson),
      relation_as_player: {
        owner_model: modelLabel(hydrated.container),
        player_model: modelLabel(relationPlayerHydrated),
        preserved: relationPlayerPreserved,
        role: JSON.parse(package_.Container.item.role).label,
      },
    },
    ordered_distinct_collections: {
      owns: {
        authored: authoredAliases,
        constructed: constructedAliases,
        distinct: hasAnnotation(package_.Person.aliases, "distinct"),
        field: JSON.parse(package_.Person.aliases.attribute),
        hydrated: hydratedAliases,
        mode: aliasesMode,
      },
      player_duplicate: {
        canonical_player: {
          key: constructed.ada.identifier.value,
          model: modelLabel(constructed.ada),
        },
        category: constraints.playerDuplicate.category,
        code: constraints.playerDuplicate.code,
        duplicate_index: playerSecond,
        first_index: playerFirst,
        rejected_before_provider_io: true,
      },
      relates: {
        authored: authoredParticipants,
        constructed: constructedParticipants,
        distinct: hasAnnotation(package_.NetworkLink.participant, "distinct"),
        hydrated: hydratedParticipants,
        mode: participantMode,
        role: JSON.parse(package_.NetworkLink.participant.role).label,
      },
      scalar_duplicate: {
        canonical_value: constructed.ada.aliases[0].value,
        category: constraints.scalarDuplicate.category,
        code: constraints.scalarDuplicate.code,
        duplicate_index: scalarSecond,
        first_index: scalarFirst,
        rejected_before_provider_io: true,
      },
      unordered_compatibility_default: unorderedDefault,
    },
    projected_constraint_validation: constraints.observation,
    token_package_fencing: {
      accepted_local: {
        construction: localConstruction,
        hydration: localHydration,
      },
      foreign_rejections: {
        construction: {
          category: constructionRejection.diagnostic.category,
          code: constructionRejection.diagnostic.code,
          rejected_before_provider_io: true,
        },
        hydration: {
          category: hydrationRejection.diagnostic.category,
          code: hydrationRejection.diagnostic.code,
          public_result_published: hydrationRejection.publicResultPublished,
        },
      },
      provider_text_exposed: providerTextExposed,
    },
  };
  return {
    authority: {
      journey: sourceIdentity(root, JOURNEY_RELATIVE, "Phase-2 journey"),
      schema: sourceIdentity(root, SCHEMA_RELATIVE, "Phase-2 schema"),
    },
    binding: "node",
    format: REPORT_FORMAT,
    observations,
    semantic_profile: SEMANTIC_PROFILE,
  };
}

export async function main() {
  try {
    const [package_, foreign] = await Promise.all([
      import("./generated_phase2/dist/index.js"),
      import("./generated_phase2_foreign/dist/index.js"),
    ]);
    const report = await buildReport(repositoryRoot(), package_, foreign);
    publishReport(outputPath(), report);
    return 0;
  } catch (error) {
    process.stderr.write(
      `Node Phase-2 parity producer rejected: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    return 1;
  }
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  process.exitCode = await main();
}
