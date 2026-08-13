import assert from "node:assert/strict";
import crypto from "node:crypto";
import {
  closeSync,
  fstatSync,
  fsyncSync,
  lstatSync,
  openSync,
  readSync,
  writeSync,
} from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import { QueryV2Error } from "@type-bridge/node";
import { installRuntimeProjection } from "@type-bridge/node/runtime-projection";
import { loadNative } from "../../crates/node/dist/native.js";

import {
  Aliases,
  aggregate,
  Container,
  Employment,
  Event,
  Identifier,
  PLAYING_FACTS,
  PROJECTION_FINGERPRINT_JSON,
  Person,
  PlayerStats,
  QueryCancellation,
  QueryExecutionResourceLimits,
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
  integerInput,
  qualifyingScore,
} from "./generated_v2/dist/index.js";
import {
  Identifier as ForeignIdentifier,
  Person as ForeignPerson,
} from "./generated_foreign/dist/index.js";
import {
  Identifier as OrderedIdentifier,
  Membership as OrderedMembership,
  Person as OrderedPerson,
  PlainActivity as OrderedPlainActivity,
  Score as OrderedScore,
  Tag as OrderedTag,
  ValDatetime as OrderedValDatetime,
  ValDatetimeTz as OrderedValDatetimeTz,
  ValDouble as OrderedValDouble,
} from "./generated_ordered/dist/index.js";

function captureNativeDiagnostic(operation) {
  try {
    operation();
  } catch (error) {
    assert(error instanceof Error);
    return JSON.parse(error.message);
  }
  assert.fail("operation did not emit a native diagnostic");
}

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
  () =>
    Person.create({
      ...personValues,
      valConstrained: ValConstrained.create(19n),
    }),
  /range_violation/,
);
assert.throws(
  () =>
    Person.create({
      ...personValues,
      valConstrained: ValConstrained.create(81n),
    }),
  /range_violation/,
);
assert.throws(
  () =>
    Robot.create({
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
const hydratedIdentifier = Identifier[hydrateComplete](
  "identifier-iid",
  "provider-value",
);
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
  () =>
    Container.create({
      item: [eventReference, eventReference, eventReference],
    }),
  RangeError,
);
assert.throws(() => Employment.create({ employee: event }), TypeError);
assert.throws(
  () => Employment.create({ employee: person, member: person }),
  TypeError,
);

const orderedPerson = OrderedPerson.create({
  identifier: OrderedIdentifier.create("ordered-person"),
  score: OrderedScore.create(3n),
  tag: [OrderedTag.create("first"), OrderedTag.create("second")],
});
assert.deepEqual(
  orderedPerson.tag.map((tag) => tag.value),
  ["first", "second"],
);
const duplicateField = captureNativeDiagnostic(() =>
  OrderedPerson.create({
    identifier: OrderedIdentifier.create("ordered-duplicate"),
    score: OrderedScore.create(3n),
    tag: [OrderedTag.create("duplicate"), OrderedTag.create("duplicate")],
  }),
);
assert.equal(duplicateField.sdkCategory, "invalid_input");
assert.equal(duplicateField.code, "ordered_distinct_duplicate");
assert.deepEqual(duplicateField.details, {
  duplicate_index: { kind: "count", value: "1" },
  first_index: { kind: "count", value: "0" },
});
const constrainedField = captureNativeDiagnostic(() =>
  OrderedPerson.create({
    identifier: OrderedIdentifier.create("ordered-range"),
    score: OrderedScore.create(6n),
    tag: [],
  }),
);
assert.equal(constrainedField.sdkCategory, "invalid_input");
assert.equal(constrainedField.code, "range_constraint_violation");

// Hydration remains outside this constructor-only slice; use its existing hook
// only to provide canonical identity for role-constructor admission fixtures.
const orderedHydrate = Object.getOwnPropertySymbols(OrderedPerson).find(
  (symbol) => symbol.description === "typebridge.hydrate-complete",
);
assert(orderedHydrate);
const identifiedOrderedPerson = OrderedPerson[orderedHydrate]("0xa", {
  identifier: OrderedIdentifier.create("identified-ordered-person"),
  score: OrderedScore.create(3n),
  tag: [],
});
const duplicateRole = captureNativeDiagnostic(() =>
  OrderedMembership.create({
    member: [identifiedOrderedPerson, identifiedOrderedPerson],
  }),
);
assert.equal(duplicateRole.sdkCategory, "invalid_input");
assert.equal(duplicateRole.code, "ordered_distinct_duplicate");
assert.equal(
  OrderedPlainActivity.create({ participant: identifiedOrderedPerson })
    .participant,
  identifiedOrderedPerson,
);

assert.equal(OrderedScore.create(3n).value, 3n);
assert.throws(() => OrderedScore.create(1n << 63n), TypeError);
assert.throws(() => OrderedValDouble.create(Number.POSITIVE_INFINITY), TypeError);
const fractionalInstant = new Date("2026-07-29T01:02:03.120Z");
assert.equal(
  OrderedValDatetime.create(fractionalInstant).value,
  fractionalInstant,
);
assert.equal(
  OrderedValDatetimeTz.create(fractionalInstant).value,
  fractionalInstant,
);

const foreignPerson = ForeignPerson.create(personValues);
const foreignPackage = captureNativeDiagnostic(() =>
  OrderedMembership.create({ member: [foreignPerson] }),
);
assert.equal(foreignPackage.category, "integrity");
assert.equal(foreignPackage.sdkCategory, "integrity");
assert.equal(foreignPackage.code, "generated_token_package_mismatch");
assert.equal(
  foreignPackage.message,
  "The generated token belongs to a different installed schema package",
);
assert.deepEqual(
  foreignPackage.path.map((segment) => segment.kind),
  ["type", "role", "index"],
);
const nestedForeignHydration = captureNativeDiagnostic(() =>
  OrderedPerson[orderedHydrate]("0xb", {
    identifier: ForeignIdentifier.create("nested-foreign-key"),
    score: OrderedScore.create(3n),
    tag: [],
  }),
);
assert.equal(
  nestedForeignHydration.code,
  "generated_token_package_mismatch",
);
assert.deepEqual(
  nestedForeignHydration.path,
  [
    { kind: "type", value: { kind: "entity", label: "person" } },
    {
      kind: "field",
      value: {
        attribute: "identifier",
        owner: { kind: "entity", label: "person" },
      },
    },
    { kind: "index", value: 0 },
  ],
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
const membershipMemberId =
  '{"declaring_relation":"membership","label":"member"}';
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
assert(
  JSON.stringify(membershipPersonFact.metadata).includes("membership player"),
);
assert(
  JSON.stringify(membershipRobotFact.metadata).includes(
    "robot membership player",
  ),
);
assert.notDeepEqual(
  membershipPersonFact.metadata.id,
  membershipRobotFact.metadata.id,
);
const eventSubjectFacts = PLAYING_FACTS.filter(
  (fact) => fact.role === eventSubjectId,
);
assert.equal(eventSubjectFacts.length, 1);
const eventSubjectFact = eventSubjectFacts[0];
assert.equal(eventSubjectFact.player, personId);
assert.equal(eventSubjectFact.multiplicity.cardinality.max, "1");
assert(
  JSON.stringify(eventSubjectFact.metadata).includes("event subject player"),
);
assert.notDeepEqual(
  eventSubjectFact.metadata.id,
  membershipPersonFact.metadata.id,
);
const projection = JSON.parse(RUNTIME_PROJECTION_JSON);
assert.deepEqual(
  JSON.parse(SEMANTIC_SCHEMA_FINGERPRINT_JSON),
  projection.semantic_fingerprint,
);
assert.deepEqual(
  JSON.parse(PROJECTION_FINGERPRINT_JSON),
  projection.projection_fingerprint,
);
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
assert.throws(
  () =>
    installRuntimeProjection({
      projectionJson: RUNTIME_PROJECTION_JSON.replace(
        '"target_name":"Aliases"',
        '"target_name":"AliasesTampered"',
      ),
      semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
      projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
      bindings,
    }),
  /fingerprint|canonical/i,
);
assert.throws(
  () =>
    installRuntimeProjection({
      projectionJson: RUNTIME_PROJECTION_JSON,
      semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
      projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
      bindings: bindings.slice(1),
    }),
  /exactly|coverage/i,
);
assert.throws(
  () =>
    installRuntimeProjection({
      projectionJson: RUNTIME_PROJECTION_JSON,
      semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
      projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
      bindings: [
        { ...bindings[0], targetName: "WrongTarget" },
        ...bindings.slice(1),
      ],
    }),
  /registration|facet/i,
);
assert.throws(
  () => new QuerySession({}),
  /registered RustDatabase|RustTransactionContext/i,
);
assert.throws(
  () =>
    new RemoteQuerySession({}, async () => new Uint8Array(), {
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
  return Buffer.from(
    JSON.stringify({
      capabilities: remoteCapabilities,
      executor: {
        epoch: "node-generated-epoch-0001",
        identity: "node-generated-executor",
      },
      format: "typebridge.query-remote-capabilities/v1",
      reply_key: signingPublicKey.toString("hex"),
      reply_key_id: signingKeyId,
    }),
  );
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
  const signature = crypto
    .sign(null, digest, signingPrivateKey)
    .toString("hex");
  return Buffer.concat([
    prefix,
    payloadBytes,
    Buffer.from(`,"signature":"${signature}"}`),
  ]);
}

const generatedAdvertisement = remoteAdvertisement();
let generatedRemoteExchanges = 0;
let generatedFailureReply;
const generatedRemoteSession = new RemoteQuerySession(
  generatedAdvertisement,
  async (request) => {
    generatedRemoteExchanges += 1;
    const decoded = JSON.parse(Buffer.from(request).toString("utf8"));
    assert.deepEqual(
      Buffer.from(JSON.stringify(decoded)),
      Buffer.from(request),
    );
    generatedFailureReply = remoteSignedReply(
      {
        category: "integrity",
        code: "remote_application_failure",
        details: {
          attempt: { kind: "long", value: "-7" },
          expected: { kind: "text_list", value: ["person", "employee"] },
          retryable: { kind: "boolean", value: false },
          subject: { kind: "text", value: "person" },
        },
        format: "typebridge.query-remote-failure/v2",
        message: "the remote application rejected this query",
        nonce: decoded.nonce,
        path: [
          { kind: "field", value: "plan" },
          { kind: "index", value: 2 },
          { kind: "identifier", value: "person" },
        ],
        request: remoteFingerprint(
          "typebridge.query.remote-request",
          "typebridge.query-remote-request/v2",
          request,
        ),
      },
      generatedAdvertisement,
    );
    return generatedFailureReply;
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
const generatedRemoteMinimum = integerInput(generatedRemoteSession, Score.create(2n));
const generatedRemoteCall = qualifyingScore(
  generatedRemoteSession,
  generatedRemotePerson,
  generatedRemoteMinimum,
);
const generatedRemoteNestedCall = qualifyingScore(
  generatedRemoteSession,
  generatedRemotePerson,
  generatedRemoteCall,
);
assert.ok(generatedRemoteCall.gteCall(generatedRemoteNestedCall));
assert.throws(
  () => generatedRemoteSession.var(Person, "subtype"),
  /match mode must be "exact" or "subtypes"/i,
);
const native = loadNative();
const originalPrepareRemoteRows = native.queryV2PrepareRemoteModelRows;
let capturedFailurePending;
native.queryV2PrepareRemoteModelRows = (...arguments_) => {
  const pending = originalPrepareRemoteRows(...arguments_);
  capturedFailurePending = pending;
  return pending;
};
let remoteDiagnosticObservation;
try {
  await assert.rejects(
    generatedRemoteSession.query(generatedRemotePerson).one(),
    (error) => {
      assert(error instanceof QueryV2Error);
      assert.equal(error.category, "result_decode");
      assert.equal(error.sdkCategory, "integrity");
      assert.equal(error.queryCategory, "result_decode");
      assert.equal(error.code, "remote_application_failure");
      assert.equal(
        error.diagnosticMessage,
        "Typed query evidence does not match the validated request invocation",
      );
      assert.deepEqual(error.path, [
        { kind: "contract_field", value: "plan" },
        { kind: "index", value: 2 },
        { kind: "contract_identity", value: "person" },
      ]);
      assert.deepEqual(error.details, {
        attempt: { kind: "signed", value: "-7" },
        expected: {
          kind: "query_identity_list",
          value: ["person", "employee"],
        },
        retryable: { kind: "boolean", value: false },
        subject: { kind: "query_identity", value: "person" },
      });
      const serialized = JSON.stringify({
        message: error.diagnosticMessage,
        path: error.path,
        details: error.details,
      });
      remoteDiagnosticObservation = {
        category: error.sdkCategory,
        query_category: error.queryCategory,
        code: error.code,
        message: error.diagnosticMessage,
        path: error.path,
        details: error.details,
        redacted: [
          "the remote application rejected this query",
          "localhost",
          "password",
        ].every((secret) => !serialized.includes(secret)),
      };
      return true;
    },
  );
} finally {
  native.queryV2PrepareRemoteModelRows = originalPrepareRemoteRows;
}
assert.equal(generatedRemoteExchanges, 1);
assert(capturedFailurePending);
assert(generatedFailureReply);
await assert.rejects(
  capturedFailurePending.decodeReply(generatedFailureReply),
  (error) => {
    // The protected native pending is captured only to prove the shared
    // one-shot authority was consumed by the public generated execution.
    assert(error instanceof Error);
    assert.match(error.message, /query_remote_v2_reply_replayed/);
    return true;
  },
);
remoteDiagnosticObservation.claim_consumed = true;

const transportCancellation = new QueryCancellation();
let transportAbortObserved = false;
const cancelledRemoteSession = new RemoteQuerySession(
  generatedAdvertisement,
  (_request, signal) =>
    new Promise((_resolve, reject) => {
      signal?.addEventListener(
        "abort",
        () => {
          transportAbortObserved = true;
          reject(
            new Error(
              "transport abort must lose to canonical query cancellation",
            ),
          );
        },
        { once: true },
      );
    }),
  new QueryExecutionResourceLimits(),
  transportCancellation,
);
const cancelledRemotePerson = cancelledRemoteSession.exact(Person);
const cancelledRemoteExecution = cancelledRemoteSession
  .query(cancelledRemotePerson)
  .one();
await Promise.resolve();
transportCancellation.cancel();
await assert.rejects(cancelledRemoteExecution, (error) => {
  assert(error instanceof QueryV2Error);
  assert.equal(error.category, "cancelled");
  assert.equal(error.code, "provider_cancelled");
  return true;
});
assert.equal(transportAbortObserved, true);

const beforeExchangeCancellation = new QueryCancellation();
beforeExchangeCancellation.cancel();
let beforeExchangeCount = 0;
const beforeExchangeSession = new RemoteQuerySession(
  generatedAdvertisement,
  async () => {
    beforeExchangeCount += 1;
    throw new Error("pre-cancelled remote query reached caller transport");
  },
  new QueryExecutionResourceLimits(),
  beforeExchangeCancellation,
);
const beforeExchangePerson = beforeExchangeSession.exact(Person);
let beforeExchangeDiagnostic;
await assert.rejects(
  beforeExchangeSession.query(beforeExchangePerson).one(),
  (error) => {
    assert(error instanceof QueryV2Error);
    beforeExchangeDiagnostic = error;
    assert.equal(error.sdkCategory, "cancelled");
    assert.equal(error.code, "provider_cancelled");
    return true;
  },
);
assert.equal(beforeExchangeCount, 0);
beforeExchangeSession.close();

const duringDecodeCancellation = new QueryCancellation();
let duringDecodeExchangeCount = 0;
let serverExchangeCancelledAfterSend = false;
const duringDecodeSession = new RemoteQuerySession(
  generatedAdvertisement,
  async (request, signal) => {
    duringDecodeExchangeCount += 1;
    let responseConstructed = false;
    signal?.addEventListener(
      "abort",
      () => {
        serverExchangeCancelledAfterSend ||= !responseConstructed;
      },
      { once: true },
    );
    const decoded = JSON.parse(Buffer.from(request).toString("utf8"));
    const response = remoteSignedReply(
      {
        category: "integrity",
        code: "remote_application_failure",
        details: {},
        format: "typebridge.query-remote-failure/v2",
        message: "provider text must be redacted",
        nonce: decoded.nonce,
        path: [],
        request: remoteFingerprint(
          "typebridge.query.remote-request",
          "typebridge.query-remote-request/v2",
          request,
        ),
      },
      generatedAdvertisement,
    );
    responseConstructed = true;
    duringDecodeCancellation.cancel();
    return response;
  },
  new QueryExecutionResourceLimits(),
  duringDecodeCancellation,
);
const duringDecodePerson = duringDecodeSession.exact(Person);
let duringDecodeDiagnostic;
await assert.rejects(
  duringDecodeSession.query(duringDecodePerson).one(),
  (error) => {
    assert(error instanceof QueryV2Error);
    duringDecodeDiagnostic = error;
    assert.equal(error.sdkCategory, "cancelled");
    assert.equal(error.code, "provider_cancelled");
    return true;
  },
);
assert.equal(duringDecodeExchangeCount, 1);
duringDecodeSession.close();
const remoteCancellationObservation = {
  before_exchange: {
    category: beforeExchangeDiagnostic.sdkCategory,
    code: beforeExchangeDiagnostic.code,
    exchange_count: beforeExchangeCount,
    partial_result: false,
  },
  during_decode: {
    category: duringDecodeDiagnostic.sdkCategory,
    code: duringDecodeDiagnostic.code,
    exchange_count: duringDecodeExchangeCount,
    partial_result: false,
  },
  caller_transport_abort_supported: transportAbortObserved,
  server_exchange_cancelled_after_send: serverExchangeCancelledAfterSend,
};

function proofSourceIdentity(root, relative) {
  const path = resolve(root, relative);
  const metadata = lstatSync(path);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`workforce-v2 proof source is not a regular file: ${relative}`);
  }
  const descriptor = openSync(path, "r");
  try {
    const size = fstatSync(descriptor).size;
    const bytes = Buffer.alloc(size);
    let offset = 0;
    while (offset < size) {
      const count = readSync(descriptor, bytes, offset, size - offset, offset);
      if (count === 0) throw new Error(`workforce-v2 proof source was truncated: ${relative}`);
      offset += count;
    }
    return {
      path: relative,
      sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
    };
  } finally {
    closeSync(descriptor);
  }
}

function canonicalProofValue(value) {
  if (Array.isArray(value)) return value.map(canonicalProofValue);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, member]) => [key, canonicalProofValue(member)]),
    );
  }
  return value;
}

function emitWorkforceV2RemoteProofFragment() {
  const destination = process.env.TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT;
  if (destination === undefined) return;
  const runNonce = process.env.TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE;
  if (runNonce === undefined || !/^[0-9a-f]{64}$/.test(runNonce)) {
    throw new Error("workforce-v2 proof run nonce must be 64 lowercase hex characters");
  }
  if (!isAbsolute(destination)) {
    throw new Error("workforce-v2 proof fragment path must be absolute");
  }
  const parent = lstatSync(dirname(destination));
  if (!parent.isDirectory() || parent.isSymbolicLink()) {
    throw new Error("workforce-v2 proof fragment parent must be a regular directory");
  }
  const root = process.cwd();
  if (!lstatSync(resolve(root, "type-bridge-core")).isDirectory()) {
    throw new Error("workforce-v2 proof fragment emitter requires the repository root");
  }
  const contractPaths = {
    proof_schema:
      "tests/contracts/sdk_conformance/workforce-v2/proof-fragment-schema-v1.json",
    allowlist:
      "tests/contracts/sdk_conformance/workforce-v2/proof-fragment-allowlist-v1.json",
    journey: "tests/contracts/sdk_conformance/workforce-v2/journey-v2.json",
  };
  const producerPaths = [
    "type-bridge-core/crates/node/src/match_runtime.rs",
    "type-bridge-core/crates/node/src/query_v2_model_remote_runtime.rs",
    "type-bridge-core/crates/node/typescript/native.ts",
    "type-bridge-core/crates/node/typescript/runtime-projection.ts",
    "type-bridge-core/crates/schema-codegen/src/typescript/runtime.ts",
    "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/runtime_check.mjs",
  ];
  const fragment = {
    format: "typebridge.workforce-v2-proof-fragment/v1",
    binding: "node",
    semantic_profile: "typedb-3.12.1/v1",
    run_nonce: runNonce,
    contract: Object.fromEntries(
      Object.entries(contractPaths).map(([name, relative]) => [
        name,
        proofSourceIdentity(root, relative),
      ]),
    ),
    producer: {
      id: "node.generated_remote_acceptance",
      sources: producerPaths.map((relative) => proofSourceIdentity(root, relative)),
    },
    results: [
      {
        observation_ref: "cancellation_remote",
        proof_kind: "remote_runtime",
        test_id: "node.generated_remote_cancellation",
        outcome: "passed",
        observation: remoteCancellationObservation,
      },
      {
        observation_ref: "remote_structured_diagnostic",
        proof_kind: "diagnostic",
        test_id: "node.generated_remote_structured_diagnostic",
        outcome: "passed",
        observation: remoteDiagnosticObservation,
      },
    ],
  };
  const payload = Buffer.from(`${JSON.stringify(canonicalProofValue(fragment))}\n`);
  if (payload.length > 64 * 1024) {
    throw new Error("workforce-v2 proof fragment exceeds 64 KiB");
  }
  const descriptor = openSync(destination, "wx", 0o600);
  try {
    let offset = 0;
    while (offset < payload.length) {
      offset += writeSync(descriptor, payload, offset, payload.length - offset, offset);
    }
    fsyncSync(descriptor);
  } finally {
    closeSync(descriptor);
  }
}

emitWorkforceV2RemoteProofFragment();

const remoteLifecycle = generatedRemoteSession.query(generatedRemotePerson);
const remoteLifecycleClone = remoteLifecycle.clone();
const remoteLifecycleDerived = remoteLifecycle.where(
  generatedRemotePerson.field(Person.score).gte(Score.create(1n)),
);
const remoteLifecyclePage = generatedRemoteSession.query(generatedRemotePerson);
const remoteLifecycleCount = generatedRemoteSession.query(generatedRemotePerson);
const remoteLifecycleExists = generatedRemoteSession.query(generatedRemotePerson);
const remoteLifecycleReduce = generatedRemoteSession.query(generatedRemotePerson);
const remoteLifecycleScore = generatedRemotePerson.field(Person.score);
const remoteLifecycleBoolean = generatedRemotePerson.field(Person.valBool);
const remoteLifecycleGrouped = generatedRemoteSession
  .query(generatedRemotePerson)
  .groupBy(generatedRemotePerson, generatedRemotePerson);
const remoteLifecycleGroupedByField = generatedRemoteSession
  .query(generatedRemotePerson)
  .groupBy(generatedRemotePerson, remoteLifecycleBoolean);
const remoteLifecycleGroupedByFields = generatedRemoteSession
  .query(generatedRemotePerson)
  .groupBy(
    generatedRemotePerson,
    remoteLifecycleBoolean,
    remoteLifecycleScore,
  );
remoteLifecycle.close();
remoteLifecycle.close();
remoteLifecyclePage.close();
remoteLifecycleCount.close();
remoteLifecycleExists.close();
remoteLifecycleReduce.close();
remoteLifecycleGrouped.close();
remoteLifecycleGroupedByField.close();
remoteLifecycleGroupedByFields.close();
assert.equal(remoteLifecycle.isClosed, true);
assert.equal(remoteLifecycleClone.isClosed, false);
assert.equal(remoteLifecycleDerived.isClosed, false);
assert.throws(
  () => remoteLifecycle.where(generatedRemotePerson.iid("0x1")),
  /query_resource_closed/,
);
assert(remoteLifecycleClone.clone());
async function rejectsClosedRemoteTerminal(invoke) {
  const exchangesBefore = generatedRemoteExchanges;
  await assert.rejects(async () => invoke(), (error) => {
    assert(error instanceof QueryV2Error);
    assert.equal(error.code, "query_resource_closed");
    return true;
  });
  assert.equal(generatedRemoteExchanges, exchangesBefore);
}
await rejectsClosedRemoteTerminal(() => remoteLifecycle.one());
await rejectsClosedRemoteTerminal(() =>
  remoteLifecyclePage.pageBy(generatedRemotePerson, { limit: 1n }),
);
await rejectsClosedRemoteTerminal(() =>
  remoteLifecycleCount.countBy(generatedRemotePerson),
);
await rejectsClosedRemoteTerminal(() =>
  remoteLifecycleExists.existsBy(generatedRemotePerson),
);
await rejectsClosedRemoteTerminal(() =>
  remoteLifecycleReduce.aggregate(generatedRemotePerson, [aggregate.count()]),
);
await rejectsClosedRemoteTerminal(() =>
  remoteLifecycleGrouped.aggregate([aggregate.count()]),
);
await rejectsClosedRemoteTerminal(() =>
  remoteLifecycleGroupedByField.aggregate([aggregate.count()]),
);
await rejectsClosedRemoteTerminal(() =>
  remoteLifecycleGroupedByFields.aggregate([aggregate.count()]),
);
remoteLifecycleDerived.close();
assert.equal(remoteLifecycleClone.isClosed, false);
generatedRemoteSession.close();
generatedRemoteSession.close();
assert.equal(generatedRemoteSession.isClosed, true);
assert.equal(remoteLifecycleClone.isClosed, true);
await rejectsClosedRemoteTerminal(() => remoteLifecycleClone.one());
