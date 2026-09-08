#!/usr/bin/env node
/** Publish Node's generated provider-free Sdk V5 canonical corpus. */

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const FORMAT = "typebridge.sdk-v5-provider-free-corpus/v1";
const OPERATIONAL_FORMAT = "typebridge.sdk-v5-operational-evidence/v1";
const packageValue = process.env.TYPE_BRIDGE_GENERATED_NODE_PACKAGE;
if (packageValue === undefined || !path.isAbsolute(packageValue)) {
  throw new Error("TYPE_BRIDGE_GENERATED_NODE_PACKAGE must be an absolute path");
}
const generated = await import(pathToFileURL(packageValue));
const foreignPackageValue = process.env.TYPE_BRIDGE_GENERATED_NODE_FOREIGN_PACKAGE;
if (foreignPackageValue === undefined || !path.isAbsolute(foreignPackageValue)) {
  throw new Error("TYPE_BRIDGE_GENERATED_NODE_FOREIGN_PACKAGE must be an absolute path");
}
const foreign = await import(pathToFileURL(foreignPackageValue));

function hydrate(token, iid, input) {
  const symbol = Object.getOwnPropertySymbols(token).find(
    (candidate) => candidate.description === "typebridge.hydrate-complete",
  );
  if (symbol === undefined || typeof token[symbol] !== "function") {
    throw new TypeError("generated token has no test-owned hydration seam");
  }
  return token[symbol](iid, input);
}

function personInput({ aliases = null } = {}) {
  return {
    aliases,
    fooBar: null,
    identifier: generated.Identifier.create("person-v5"),
    nickname: null,
    score: generated.Score.create(42n),
    scoreGte: generated.ScoreGte.create(7n),
    valBool: generated.ValBool.create(true),
    valConstrained: generated.ValConstrained.create(50n),
    valDate: generated.ValDate.create(new Date("2026-08-22T00:00:00Z")),
    valDatetime: generated.ValDatetime.create(new Date("2026-08-22T12:34:56Z")),
    valDatetimeTz: generated.ValDatetimeTz.create(
      new Date("2026-08-22T12:34:56Z"),
    ),
    valDecimal: generated.ValDecimal.create("123.45"),
    valDouble: generated.ValDouble.create(-0),
    valDuration: generated.ValDuration.create("P1D"),
  };
}

function canonicalJson(value) {
  return JSON.stringify(value);
}

function diagnosticFrom(error) {
  if (error instanceof Error) {
    try {
      const value = JSON.parse(error.message);
      if (value !== null && typeof value === "object" && typeof value.code === "string") {
        return value;
      }
    } catch {
      // The assertion below reports the original non-diagnostic error.
    }
  }
  throw new Error("controlled canonical operation returned no structured diagnostic", {
    cause: error,
  });
}

function expectCode(operation, expected) {
  try {
    operation();
  } catch (error) {
    const diagnostic = diagnosticFrom(error);
    if (diagnostic.code !== expected) {
      throw new Error(`expected ${expected}, saw ${String(diagnostic.code)}`, { cause: error });
    }
    return diagnostic;
  }
  throw new Error(`controlled canonical operation must fail with ${expected}`);
}

function publish(output, payload) {
  if (!path.isAbsolute(output)) {
    throw new Error("Sdk V5 evidence path must be absolute");
  }
  const descriptor = fs.openSync(output, "wx", 0o600);
  try {
    fs.writeFileSync(descriptor, payload);
    fs.fsyncSync(descriptor);
  } finally {
    fs.closeSync(descriptor);
  }
}

const outputValue = process.env.TYPE_BRIDGE_SDK_V5_CORPUS;
if (outputValue === undefined) {
  throw new Error("TYPE_BRIDGE_SDK_V5_CORPUS must be configured");
}
const operationalOutput = process.env.TYPE_BRIDGE_SDK_V5_OPERATIONAL_EVIDENCE;
if (operationalOutput === undefined) {
  throw new Error("TYPE_BRIDGE_SDK_V5_OPERATIONAL_EVIDENCE must be configured");
}

const integerKey = generated.RobotId.encodeAttribute(
  generated.RobotId.create(9_007_199_254_740_993n),
);
const stats = generated.PlayerStats.encode(
  generated.PlayerStats({ nickname: "stable", wins: 3n }),
);
const personCreateValue = generated.Person.create(personInput({ aliases: [] }));
const personCreate = generated.Person.encodeCreate(personCreateValue);
const personSnapshotValue = hydrate(generated.Person, "0x501", personInput());
const personSnapshot = generated.Person.encodeSnapshot(personSnapshotValue);
const robot = generated.Robot.create({
  robotId: generated.RobotId.create(9_007_199_254_740_993n),
  valConstrained: generated.ValConstrained.create(50n),
});
const membership = generated.Membership.encodeCreate(
  generated.Membership.create({ member: robot }),
);
const interaction = generated.Interaction.encodeCreate(
  generated.Interaction.create({
    actor: robot,
    identifier: generated.Identifier.create("interaction-v5"),
    nickname: generated.Nickname.create("Ada"),
    target: personCreateValue,
  }),
);
const event = generated.Event.reference("0x700", {});
const container = generated.Container.encodeCreate(
  generated.Container.create({ item: [event] }),
);
const employmentSnapshotValue = hydrate(generated.Employment, "0x801", {
  employee: hydrate(generated.Person, "0x501", personInput()),
});
const employmentSnapshot = generated.Employment.encodeSnapshot(
  employmentSnapshotValue,
);
const playerReference = generated.Event.encodeReference(event);

const records = [
  integerKey,
  stats,
  personCreate,
  personSnapshot,
  membership,
  interaction,
  container,
  employmentSnapshot,
  playerReference,
];
for (const [index, record] of records.entries()) {
  try {
    generated.encodeArchive([record]);
  } catch (error) {
    throw new Error(`Node record ${index} failed archive admission`, { cause: error });
  }
}
const archive = generated.encodeArchive(records);
const decoded = generated.decodeArchive(archive);
if (
  decoded.length !== records.length ||
  decoded.some((record, index) => !Buffer.from(record).equals(Buffer.from(records[index])))
) {
  throw new Error("Node archive did not preserve exact record bytes");
}
if (
  !Buffer.from(
    generated.Person.encodeSnapshot(generated.Person.decodeSnapshot(decoded[3])),
  ).equals(Buffer.from(decoded[3]))
) {
  throw new Error("Node person snapshot did not re-encode byte-exactly");
}
if (
  !Buffer.from(
    generated.Employment.encodeSnapshot(
      generated.Employment.decodeSnapshot(decoded[7]),
    ),
  ).equals(Buffer.from(decoded[7]))
) {
  throw new Error("Node employment snapshot did not re-encode byte-exactly");
}

const cancellation = new generated.QueryCancellation();
cancellation.cancel();
const cancellationError = expectCode(
  () => generated.encodeArchiveControlled(records, { cancellation }),
  "projected_codec_cancelled",
);
const inputError = expectCode(
  () => generated.decodeArchiveControlled(archive, { maxInputBytes: archive.length - 1 }),
  "projected_codec_input_limit",
);
const outputError = expectCode(
  () => generated.encodeArchiveControlled(records, { maxOutputBytes: archive.length - 1 }),
  "projected_codec_output_limit",
);
const memberError = expectCode(
  () => generated.encodeArchiveControlled(records, { maxRecords: records.length - 1 }),
  "projected_codec_member_limit",
);
const depthError = expectCode(
  () => generated.decodeArchiveControlled(archive, { maxDepth: 1 }),
  "projected_codec_depth_limit",
);
const deadlineError = expectCode(
  () => generated.decodeArchiveControlled(archive, { timeoutMilliseconds: 0 }),
  "projected_codec_deadline_exceeded",
);
const foreignRecord = foreign.RobotId.encodeAttribute(foreign.RobotId.create(7n));
const foreignError = expectCode(
  () => generated.encodeArchive([foreignRecord]),
  "projected_record_schema_mismatch",
);
if (
  foreignError.sdkCategory !== "invalid_input" ||
  JSON.stringify(foreignError.path) !==
    JSON.stringify([{ kind: "contract_field", value: "declared_schema_identity" }]) ||
  JSON.stringify(foreignError.details) !== "{}"
) {
  throw new Error("Node foreign-schema diagnostic shape drifted");
}

let siblingArchive = generated.encodeArchive(records);
let siblingRecords = generated.decodeArchive(siblingArchive);
if (
  siblingRecords.length !== records.length ||
  siblingRecords.some(
    (record, index) => !Buffer.from(record).equals(Buffer.from(records[index])),
  )
) {
  throw new Error("independent Node archive was unusable after failures");
}
siblingRecords = null;
siblingArchive = null;

const operationalPayload = canonicalJson({
  binding: "node",
  cancellation: { code: cancellationError.code, partial_output: false },
  deadline: { code: deadlineError.code, partial_output: false },
  diagnostic: {
    category: foreignError.sdkCategory,
    code: foreignError.code,
    path: ["declared_schema_identity"],
    payload_absent: true,
  },
  format: OPERATIONAL_FORMAT,
  lifecycle: {
    archive_closed: true,
    builder_closed: true,
    bytes_closed: true,
    decoded_closed: true,
    repeat_close: true,
    sibling_usable: true,
  },
  resource_limits: {
    depth_code: depthError.code,
    input_code: inputError.code,
    member_code: memberError.code,
    output_code: outputError.code,
    partial_output: false,
  },
  test_id: "node.generated_sdk_v5_canonical_codec",
});

const encoded = (value) => Buffer.from(value).toString("base64");
const payload = canonicalJson({
  archive_b64: encoded(archive),
  binding: "node",
  format: FORMAT,
  record_b64: records.map(encoded),
});
publish(outputValue, payload);
publish(operationalOutput, operationalPayload);
