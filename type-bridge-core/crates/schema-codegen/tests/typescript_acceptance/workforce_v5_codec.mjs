#!/usr/bin/env node
/** Publish Node's generated provider-free Workforce V5 canonical corpus. */

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const FORMAT = "typebridge.workforce-v5-provider-free-corpus/v1";
const packageValue = process.env.TYPE_BRIDGE_GENERATED_NODE_PACKAGE;
if (packageValue === undefined || !path.isAbsolute(packageValue)) {
  throw new Error("TYPE_BRIDGE_GENERATED_NODE_PACKAGE must be an absolute path");
}
const generated = await import(pathToFileURL(packageValue));

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

const outputValue = process.env.TYPE_BRIDGE_WORKFORCE_V5_CORPUS;
if (outputValue === undefined) {
  throw new Error("TYPE_BRIDGE_WORKFORCE_V5_CORPUS must be configured");
}
if (!outputValue.startsWith("/")) {
  throw new Error("Workforce V5 corpus path must be absolute");
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

const encoded = (value) => Buffer.from(value).toString("base64");
const payload = canonicalJson({
  archive_b64: encoded(archive),
  binding: "node",
  format: FORMAT,
  record_b64: records.map(encoded),
});
const descriptor = fs.openSync(outputValue, "wx", 0o600);
try {
  fs.writeFileSync(descriptor, payload);
  fs.fsyncSync(descriptor);
} finally {
  fs.closeSync(descriptor);
}
