import assert from "node:assert/strict";
import crypto from "node:crypto";
import { QueryV2Error } from "@type-bridge/node";
import { installRuntimeProjection } from "@type-bridge/node/runtime-projection";

import {
  Aliases,
  Container,
  Employment,
  Event,
  Identifier,
  PLAYING_FACTS,
  PROJECTION_FINGERPRINT_JSON,
  Person,
  PlayerStats,
  QuerySession,
  RUNTIME_PROJECTION_JSON,
  RemoteQuerySession,
  Robot,
  RobotId,
  SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  Party,
  Score,
  ValBool,
  ValConstrained,
  ValDate,
  ValDatetime,
  ValDatetimeTz,
  ValDecimal,
  ValDouble,
  ValDuration,
} from "./generated_v2/dist/index.js";

const identifier = Identifier.create("person-1");
assert.equal(identifier.value, "person-1");
assert.equal(identifier.iid, null);
const score = Score.create(3n);
assert.equal(score.value, 3n);
assert.equal(score.iid, null);
const personValues = {
  identifier,
  score,
  valBool: ValBool.create(true),
  valConstrained: ValConstrained.create(20n),
  valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
  valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
  valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
  valDecimal: ValDecimal.create("3.5"),
  valDouble: ValDouble.create(3.5),
  valDuration: ValDuration.create("PT3S"),
};
const person = Person.create({
  ...personValues,
  aliases: [Aliases.create("first"), Aliases.create("second")],
});
assert.throws(
  () => Person.create({ ...personValues, valConstrained: ValConstrained.create(19n) }),
  /range_violation/,
);
assert.throws(
  () => Person.create({ ...personValues, valConstrained: ValConstrained.create(81n) }),
  /range_violation/,
);
assert.throws(
  () => Robot.create({
    robotId: RobotId.create(1n),
    valConstrained: ValConstrained.create(51n),
  }),
  /range_violation/,
);
assert.equal(
  Person.create({ ...personValues, valConstrained: ValConstrained.create(51n) })
    .valConstrained.value,
  51n,
);
assert.equal(person.__typebridgeForm, "complete");
assert.equal(person.iid, null);
assert.equal(person.nickname, null);
assert(Object.isFrozen(person));
assert.equal(Person.identifier.kind, "field");
assert.equal(Person.identifier.key, true);
assert.equal(Person.identifier.unique, true);
assert.equal(Person.identifier.multiplicity.cardinality.min, "1");
assert.equal(Person.identifier.multiplicity.cardinality.max, "1");

const event = Event.create({ subject: person });
assert.equal(event.subject, person);
const reference = Person.reference("person-iid", { identifier });
assert.equal(reference.__typebridgeForm, "reference");
assert.equal(reference.iid, "person-iid");
assert.throws(() => Identifier.create({}), TypeError);
assert.throws(() => Score.create(3), TypeError);
assert.throws(() => Score.create(1n << 63n), TypeError);

const hydrateComplete = Object.getOwnPropertySymbols(Identifier).find(
  (symbol) => symbol.description === "typebridge.hydrate-complete",
);
assert(hydrateComplete);
const hydratedIdentifier = Identifier[hydrateComplete]("identifier-iid", "provider-value");
assert.equal(hydratedIdentifier.iid, "identifier-iid");
assert.equal(hydratedIdentifier.value, "provider-value");
assert(Object.isFrozen(hydratedIdentifier));
const hydratedPerson = Person[hydrateComplete]("person-iid", {
  ...personValues,
  aliases: [],
  nickname: null,
});
assert.equal(hydratedPerson.iid, "person-iid");
assert.equal(hydratedPerson.nickname, null);
assert.deepEqual(hydratedPerson.aliases, []);
Employment.create({ employee: person });

const eventReference = Event.reference("event-iid", {});
Container.create({ item: [eventReference] });
assert.throws(
  () => Container.create({ item: [eventReference, eventReference, eventReference] }),
  RangeError,
);
assert.throws(() => Employment.create({ employee: event }), TypeError);
assert.throws(
  () => Employment.create({ employee: person, member: person }),
  TypeError,
);

assert.equal(Employment.employee.kind, "role");
assert.notEqual(Employment.employee.specializes, null);
assert(JSON.stringify(Employment.metadata).includes("role_upcasts"));

const stats = PlayerStats({ wins: 3n });
assert.equal(stats.nickname, null);
assert(Object.isFrozen(stats));

assert.equal(PLAYING_FACTS.length, 12);
assert(PLAYING_FACTS.every((fact) => fact.kind === "plays"));
const personId = '{"kind":"entity","label":"person"}';
const robotId = '{"kind":"entity","label":"robot"}';
const membershipMemberId = '{"declaring_relation":"membership","label":"member"}';
const eventSubjectId = '{"declaring_relation":"event","label":"subject"}';
const membershipFacts = PLAYING_FACTS.filter(
  (fact) => fact.role === membershipMemberId,
);
assert.equal(membershipFacts.length, 2);
const membershipPersonFact = membershipFacts.find(
  (fact) => fact.player === personId,
);
const membershipRobotFact = membershipFacts.find(
  (fact) => fact.player === robotId,
);
assert(membershipPersonFact);
assert(membershipRobotFact);
assert.equal(membershipPersonFact.multiplicity.cardinality.max, "2");
assert.equal(membershipRobotFact.multiplicity.cardinality.max, "2");
assert(JSON.stringify(membershipPersonFact.metadata).includes("membership player"));
assert(JSON.stringify(membershipRobotFact.metadata).includes("robot membership player"));
assert.notDeepEqual(membershipPersonFact.metadata.id, membershipRobotFact.metadata.id);
const eventSubjectFacts = PLAYING_FACTS.filter(
  (fact) => fact.role === eventSubjectId,
);
assert.equal(eventSubjectFacts.length, 1);
const eventSubjectFact = eventSubjectFacts[0];
assert.equal(eventSubjectFact.player, personId);
assert.equal(eventSubjectFact.multiplicity.cardinality.max, "1");
assert(JSON.stringify(eventSubjectFact.metadata).includes("event subject player"));
assert.notDeepEqual(eventSubjectFact.metadata.id, membershipPersonFact.metadata.id);
const projection = JSON.parse(RUNTIME_PROJECTION_JSON);
assert.deepEqual(JSON.parse(SEMANTIC_SCHEMA_FINGERPRINT_JSON), projection.semantic_fingerprint);
assert.deepEqual(JSON.parse(PROJECTION_FINGERPRINT_JSON), projection.projection_fingerprint);
const bindings = projection.models.map((model) => ({
  typeKey: JSON.stringify(model.id),
  targetName: model.target_name,
  create: model.create.enabled,
  reference: model.reference_read.target_name !== null,
}));
const installed = installRuntimeProjection({
  projectionJson: RUNTIME_PROJECTION_JSON,
  semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
  bindings,
});
assert(installed.matchSession().exact("person"));
assert(installed.matchSession().subtypes("party"));
assert.throws(() => installed.matchSession().exact("unprojected-model"));
assert.throws(() => installRuntimeProjection({
  projectionJson: RUNTIME_PROJECTION_JSON.replace('"target_name":"Aliases"', '"target_name":"AliasesTampered"'),
  semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
  bindings,
}), /fingerprint|canonical/i);
assert.throws(() => installRuntimeProjection({
  projectionJson: RUNTIME_PROJECTION_JSON,
  semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
  bindings: bindings.slice(1),
}), /exactly|coverage/i);
assert.throws(() => installRuntimeProjection({
  projectionJson: RUNTIME_PROJECTION_JSON,
  semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
  bindings: [{ ...bindings[0], targetName: "WrongTarget" }, ...bindings.slice(1)],
}), /registration|facet/i);
assert.throws(() => new QuerySession({}), /registered RustDatabase|RustTransactionContext/i);
assert.throws(
  () => new RemoteQuerySession({}, async () => new Uint8Array(), {
    maxItems: 1n,
    maxBytes: 1n,
    maxCollectionMembers: 1n,
    maxGraphNodes: 1n,
    maxAttributeValues: 1n,
    maxRolePlayers: 1n,
  }),
  /Uint8Array|advertisement/i,
);

const remoteCapabilities = [
  "query.execution.batch-identity-rebind",
  "query.execution.same-snapshot-hydration",
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
const signingSeed = Buffer.alloc(32, 0x42);
const signingPrivateKey = crypto.createPrivateKey({
  key: Buffer.concat([
    Buffer.from("302e020100300506032b657004220420", "hex"),
    signingSeed,
  ]),
  format: "der",
  type: "pkcs8",
});
const signingPublicKey = crypto
  .createPublicKey(signingPrivateKey)
  .export({ format: "der", type: "spki" })
  .subarray(-32);
const signingKeyId = crypto
  .createHash("sha256")
  .update(Buffer.from("typebridge.query.remote-reply-key-id/v1\0"))
  .update(signingPublicKey)
  .digest("hex");

function remoteFingerprint(domain, canonicalization, payload) {
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
  return Buffer.from(JSON.stringify({
    capabilities: remoteCapabilities,
    executor: {
      epoch: "node-generated-epoch-0001",
      identity: "node-generated-executor",
    },
    format: "typebridge.query-remote-capabilities/v1",
    reply_key: signingPublicKey.toString("hex"),
    reply_key_id: signingKeyId,
  }));
}

function remoteSignedReply(payload, advertisement) {
  const advertisementFingerprint = remoteFingerprint(
    "typebridge.query.remote-capabilities",
    "typebridge.query-remote-capabilities/v1",
    advertisement,
  );
  const key = signingPublicKey.toString("hex");
  const prefix = Buffer.from(
    `{"advertisement":"${advertisementFingerprint}",` +
      `"format":"typebridge.query-remote-signed-reply/v1",` +
      `"key":"${key}","key_id":"${signingKeyId}","payload":`,
  );
  const payloadBytes = Buffer.from(JSON.stringify(payload));
  const digest = crypto
    .createHash("sha256")
    .update(Buffer.from("typebridge.query.remote-reply-signature/v1\0"))
    .update(prefix)
    .update(payloadBytes)
    .update(Buffer.from("}"))
    .digest();
  const signature = crypto.sign(null, digest, signingPrivateKey).toString("hex");
  return Buffer.concat([
    prefix,
    payloadBytes,
    Buffer.from(`,"signature":"${signature}"}`),
  ]);
}

const generatedAdvertisement = remoteAdvertisement();
let generatedRemoteExchanges = 0;
const generatedRemoteSession = new RemoteQuerySession(
  generatedAdvertisement,
  async (request) => {
    generatedRemoteExchanges += 1;
    const decoded = JSON.parse(Buffer.from(request).toString("utf8"));
    assert.deepEqual(Buffer.from(JSON.stringify(decoded)), Buffer.from(request));
    return remoteSignedReply({
      category: "invalid_contract",
      code: "remote_application_failure",
      details: {
        attempt: { kind: "long", value: "7" },
        expected: { kind: "text_list", value: ["person", "employee"] },
        retryable: { kind: "boolean", value: false },
        subject: { kind: "text", value: "person" },
      },
      format: "typebridge.query-remote-failure/v2",
      message: "the remote application rejected this query",
      nonce: decoded.nonce,
      path: [
        { kind: "field", value: "plan" },
        { kind: "index", value: 0 },
        { kind: "identifier", value: "person" },
      ],
      request: remoteFingerprint(
        "typebridge.query.remote-request",
        "typebridge.query-remote-request/v2",
        request,
      ),
    }, generatedAdvertisement);
  },
  {
    maxItems: 11n,
    maxBytes: 1n << 20n,
    maxCollectionMembers: 12n,
    maxGraphNodes: 13n,
    maxAttributeValues: 14n,
    maxRolePlayers: 15n,
  },
);
const generatedRemotePerson = generatedRemoteSession.exact(Person);
assert.throws(
  () => generatedRemoteSession.var(Person, "subtype"),
  /match mode must be "exact" or "subtypes"/i,
);
await assert.rejects(
  generatedRemoteSession.query(generatedRemotePerson).one(),
  (error) => {
    assert(error instanceof QueryV2Error);
    assert.equal(error.category, "invalid_contract");
    assert.equal(error.code, "remote_application_failure");
    assert.equal(error.diagnosticMessage, "the remote application rejected this query");
    assert.deepEqual(error.path, [
      { kind: "field", value: "plan" },
      { kind: "index", value: 0 },
      { kind: "identifier", value: "person" },
    ]);
    assert.deepEqual(error.details, {
      attempt: { kind: "long", value: "7" },
      expected: { kind: "text_list", value: ["person", "employee"] },
      retryable: { kind: "boolean", value: false },
      subject: { kind: "text", value: "person" },
    });
    return true;
  },
);
assert.equal(generatedRemoteExchanges, 1);
