import assert = require("node:assert/strict");
import crypto = require("node:crypto");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

import {
  Entity,
  Key,
  QueryV2Authority,
  QueryV2Error,
  attr,
  field,
} from "../../typescript/index.js";
import { queryV2AuthorityHandle } from "../../typescript/query-v2-internals.js";
import {
  RemoteQuerySession,
  TypedMatchError,
  references,
  type QueryOrder,
} from "../../typescript/typed/index.js";
import {
  createRemoteQuery,
  executeRemoteExchange,
  type RemoteQueryExchange,
  type RemoteQueryRuntime,
} from "../../typescript/typed/remote-query.js";
import {
  preparePageTerminal,
  prepareRowsTerminal,
} from "../../typescript/typed/query.js";
import { diagnosticQuerySession } from "../../typescript/typed/session.js";

class RemoteRuntimeName extends attr.String("remote-runtime-name") {}
class RemoteRuntimePerson extends Entity("remote-runtime-person", {
  name: field(RemoteRuntimeName, Key),
}) {}

class RemoteSmokeName extends attr.String("smoke-name") {}
class RemoteSmokePerson extends Entity("smoke-person", {
  name: field(RemoteSmokeName, Key),
}) {}
class RemoteSmokeEmployee extends Entity(
  "smoke-employee",
  {},
  { parent: RemoteSmokePerson },
) {}
const remoteSmokeRefs = references(RemoteSmokePerson);

type NativePending = Parameters<typeof executeRemoteExchange>[0];
type NativeResult = Awaited<ReturnType<NativePending["decodeReply"]>>;

class FakePending {
  requestCalls = 0;
  decodeCalls = 0;
  readonly responses: Uint8Array[] = [];

  constructor(
    readonly request: Uint8Array,
    readonly result: NativeResult,
  ) {}

  requestBytes(): Uint8Array {
    this.requestCalls += 1;
    return this.request;
  }

  async decodeReply(response: Uint8Array): Promise<NativeResult> {
    this.decodeCalls += 1;
    this.responses.push(response);
    return this.result;
  }
}

function nativePending(pending: FakePending): NativePending {
  return pending as unknown as NativePending;
}

test("remote exchange snapshots request and performs exactly one callback and decode", async () => {
  const source = new Uint8Array([1, 2, 3]);
  const response = new Uint8Array([4, 5, 6]);
  const result = {} as NativeResult;
  const pending = new FakePending(source, result);
  let calls = 0;
  const exchange: RemoteQueryExchange = async (request) => {
    calls += 1;
    source[0] = 9;
    assert.deepEqual(request, new Uint8Array([1, 2, 3]));
    return response;
  };

  assert.strictEqual(
    await executeRemoteExchange(nativePending(pending), exchange),
    result,
  );
  assert.equal(calls, 1);
  assert.equal(pending.requestCalls, 1);
  assert.equal(pending.decodeCalls, 1);
  assert.deepEqual(pending.responses, [response]);
});

test("remote callback failure is surfaced unchanged without retry or decode", async () => {
  const expected = new Error("transport failed");
  const pending = new FakePending(
    new Uint8Array([1]),
    {} as NativeResult,
  );
  let calls = 0;
  const exchange: RemoteQueryExchange = async () => {
    calls += 1;
    throw expected;
  };

  await assert.rejects(
    executeRemoteExchange(nativePending(pending), exchange),
    (error: unknown) => error === expected,
  );
  assert.equal(calls, 1);
  assert.equal(pending.decodeCalls, 0);
});

test("remote callback must return Uint8Array before native decode", async () => {
  const pending = new FakePending(
    new Uint8Array([1]),
    {} as NativeResult,
  );
  let calls = 0;
  const exchange = (async () => {
    calls += 1;
    return "not bytes";
  }) as unknown as RemoteQueryExchange;

  await assert.rejects(
    executeRemoteExchange(nativePending(pending), exchange),
    /must resolve to a Uint8Array/,
  );
  assert.equal(calls, 1);
  assert.equal(pending.decodeCalls, 0);
});

interface ErrorSnapshot {
  readonly constructor: string;
  readonly name: string;
  readonly message: string;
  readonly category: unknown;
  readonly code: unknown;
  readonly path: unknown;
  readonly details: unknown;
}

function errorSnapshot(error: unknown): ErrorSnapshot {
  assert.ok(error instanceof Error);
  return {
    constructor: error.constructor.name,
    name: error.name,
    message: error.message,
    category: Reflect.get(error, "category"),
    code: Reflect.get(error, "code"),
    path: Reflect.get(error, "path"),
    details: Reflect.get(error, "details"),
  };
}

function captureSync(operation: () => unknown): ErrorSnapshot {
  try {
    operation();
  } catch (error) {
    return errorSnapshot(error);
  }
  throw new Error("operation unexpectedly succeeded");
}

async function captureAsync(
  operation: () => Promise<unknown>,
): Promise<ErrorSnapshot> {
  try {
    await operation();
  } catch (error) {
    return errorSnapshot(error);
  }
  throw new Error("operation unexpectedly succeeded");
}

test("direct and remote terminals preserve exact pre-I/O failures", async () => {
  const session = diagnosticQuerySession();
  const person = session.var(RemoteRuntimePerson);
  const direct = session.query(person);
  let exchanges = 0;
  const runtime = Object.freeze({
    context: null,
    exchange: async () => {
      exchanges += 1;
      return new Uint8Array();
    },
  }) as unknown as RemoteQueryRuntime;
  const remote = createRemoteQuery(direct, runtime);

  const cases: readonly [
    () => unknown,
    () => Promise<unknown>,
  ][] = [
    [
      () => (direct.rows as (options: unknown) => unknown)({ limit: 0 }),
      () => (remote.rows as (options: unknown) => Promise<unknown>)({ limit: 0 }),
    ],
    [
      () =>
        (direct.rows as (options: unknown) => unknown)({
          limit: 1,
          orderBy: [{} as QueryOrder],
        }),
      () =>
        (remote.rows as (options: unknown) => Promise<unknown>)({
          limit: 1,
          orderBy: [{} as QueryOrder],
        }),
    ],
    [
      () =>
        (direct.pageBy as (root: unknown, options: unknown) => unknown)(
          {},
          { limit: 1 },
        ),
      () =>
        (remote.pageBy as (
          root: unknown,
          options: unknown,
        ) => Promise<unknown>)({}, { limit: 1 }),
    ],
    [
      () =>
        (direct.pageBy as (root: unknown, options: unknown) => unknown)(
          person,
          { limit: 1, includeTotal: 1 },
        ),
      () =>
        (remote.pageBy as (
          root: unknown,
          options: unknown,
        ) => Promise<unknown>)(person, { limit: 1, includeTotal: 1 }),
    ],
    [
      () => (direct.countBy as (root: unknown) => unknown)({}),
      () => (remote.countBy as (root: unknown) => Promise<unknown>)({}),
    ],
    [
      () => (direct.existsBy as (root: unknown) => unknown)({}),
      () => (remote.existsBy as (root: unknown) => Promise<unknown>)({}),
    ],
  ];

  for (const [invokeDirect, invokeRemote] of cases) {
    assert.deepEqual(
      await captureAsync(invokeRemote),
      captureSync(invokeDirect),
    );
  }
  assert.equal(exchanges, 0);
});

test("public direct and remote order arrays reject before mapping excess entries", async () => {
  const session = diagnosticQuerySession();
  const person = session.var(RemoteSmokePerson);
  const direct = session.query(person);
  let exchanges = 0;
  const remote = createRemoteQuery(
    direct,
    Object.freeze({
      context: null,
      exchange: async () => {
        exchanges += 1;
        return new Uint8Array();
      },
    }) as unknown as RemoteQueryRuntime,
  );
  const valid = person.field(remoteSmokeRefs.fields.name).asc();
  let inspectedEntries = 0;
  const oversized = new Proxy(
    Array.from({ length: 65 }, () => valid),
    {
      get(target, property, receiver): unknown {
        if (
          typeof property === "string" &&
          /^(?:0|[1-9][0-9]*)$/u.test(property)
        ) {
          inspectedEntries += 1;
        }
        return Reflect.get(target, property, receiver);
      },
    },
  );
  const options = { limit: 1, orderBy: oversized } as const;

  const directFailure = captureSync(() => direct.rows(options));
  const remoteFailure = await captureAsync(() => remote.rows(options));

  assert.deepEqual(remoteFailure, directFailure);
  assert.equal(directFailure.constructor, TypedMatchError.name);
  assert.equal(directFailure.category, "resource_limit");
  assert.equal(directFailure.code, "structural_limit_exceeded");
  assert.deepEqual(directFailure.path, [{ kind: "operation" }]);
  assert.deepEqual(directFailure.details, {
    actual: { kind: "unsigned", value: 65 },
    limit: { kind: "text", value: "order_terms" },
    maximum: { kind: "unsigned", value: 64 },
  });
  assert.equal(inspectedEntries, 0);
  assert.equal(exchanges, 0);
});

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
] as const;

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

function modelAdvertisement(): Uint8Array {
  return Buffer.from(JSON.stringify({
    capabilities: remoteCapabilities,
    executor: {
      epoch: "node-model-epoch-0001",
      identity: "node-model-executor",
    },
    format: "typebridge.query-remote-capabilities/v1",
    reply_key: signingPublicKey.toString("hex"),
    reply_key_id: signingKeyId,
  }));
}

function modelFingerprint(
  domain: string,
  canonicalization: string,
  payload: Uint8Array,
): string {
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

function modelSignedReply(payload: unknown): Uint8Array {
  const advertisementFingerprint = modelFingerprint(
    "typebridge.query.remote-capabilities",
    "typebridge.query-remote-capabilities/v1",
    modelAdvertisement(),
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

function modelAuthority(): QueryV2Authority {
  const fixture = fs.readFileSync(path.resolve(
    process.cwd(),
    "../../../tests/fixtures/query-v2-model-remote-declared.json",
  ));
  const declared = fixture.at(-1) === 0x0a
    ? fixture.subarray(0, fixture.length - 1)
    : fixture;
  return new QueryV2Authority(
    declared,
    "node-model-remote",
    "typedb-3.12.1/v1",
  );
}

interface ModelRemoteRequest {
  readonly format: string;
  readonly limits: Readonly<Record<string, unknown>>;
  readonly nonce: string;
  readonly plan: Readonly<{
    compatibility: Readonly<{
      model_query: Readonly<Record<string, unknown>>;
    }>;
    format: string;
  }>;
  readonly result: string;
}

function decodeModelRequest(request: Uint8Array): ModelRemoteRequest {
  const decoded = JSON.parse(
    Buffer.from(request).toString("utf8"),
  ) as ModelRemoteRequest;
  assert.deepEqual(Buffer.from(JSON.stringify(decoded)), Buffer.from(request));
  return decoded;
}

function modelRequestFingerprint(request: Uint8Array): string {
  return modelFingerprint(
    "typebridge.query.remote-request",
    "typebridge.query-remote-request/v2",
    request,
  );
}

function modelPlanFingerprint(request: ModelRemoteRequest): string {
  assert.equal(request.plan.format, "typebridge.query-plan/v2");
  return modelFingerprint(
    "typebridge.query.plan",
    "typebridge.query-plan-c14n/v2",
    Buffer.from(JSON.stringify(request.plan)),
  );
}

test("public remote model terminals emit one V2 request with exact operation contracts", async () => {
  const requests: ModelRemoteRequest[] = [];
  const session = new RemoteQuerySession(
    modelAuthority(),
    modelAdvertisement(),
    async (request) => {
      requests.push(decodeModelRequest(request));
      return Buffer.from("{}");
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
  const person = session.var(RemoteSmokePerson);
  const query = session.query(person);

  const operations: readonly [
    () => Promise<unknown>,
    string,
    string,
  ][] = [
    [() => query.one(), "hydrated_rows", "exactly_one"],
    [
      () => query.rows({ limit: 2, offset: 1 }),
      "hydrated_rows",
      "bounded_many",
    ],
    [
      () => query.pageBy(person, { limit: 2, includeTotal: true }),
      "hydrated_page",
      "page",
    ],
    [() => query.countBy(person), "distinct_count", "distinct_count"],
    [() => query.existsBy(person), "distinct_exists", "distinct_exists"],
  ];

  for (const [invoke, expectedResult, expectedModelKind] of operations) {
    const before = requests.length;
    await assert.rejects(invoke(), /query_remote_reply_malformed/);
    assert.equal(requests.length, before + 1);
    const request = requests.at(-1)!;
    assert.equal(request.format, "typebridge.query-remote-request/v2");
    assert.equal(request.result, expectedResult);
    const model = request.plan.compatibility.model_query;
    if (expectedModelKind === "exactly_one" || expectedModelKind === "bounded_many") {
      assert.equal(model["kind"], "rows");
      assert.equal(model["cardinality"], expectedModelKind);
    } else {
      assert.equal(model["kind"], expectedModelKind);
    }
    assert.deepEqual(request.limits, {
      deadline_ms: null,
      max_attribute_values: 14,
      max_bytes: 1 << 20,
      max_collection_members: 12,
      max_graph_nodes: 13,
      max_items: 11,
      max_role_players: 15,
    });
  }
});

test("public remote facade authenticates and hydrates the registered concrete subtype", async () => {
  let exchanges = 0;
  const session = new RemoteQuerySession(
    modelAuthority(),
    modelAdvertisement(),
    async (requestBytes) => {
      exchanges += 1;
      const request = decodeModelRequest(requestBytes);
      assert.equal(request.result, "hydrated_rows");
      return modelSignedReply({
        format: "typebridge.query-remote-response/v2",
        nonce: request.nonce,
        outcome: {
          graph: {
            nodes: [{
              attributes: [{
                attribute: "smoke-name",
                values: [{ kind: "string", value: "Alice" }],
              }],
              concrete: { kind: "entity", label: "smoke-employee" },
              id: 0,
              iid: "0x01",
              kind: "entity",
              roles: [],
            }],
          },
          kind: "hydrated_rows",
          rows: [{
            slots: [{
              kind: "singular",
              value: {
                declared: { kind: "entity", label: "smoke-person" },
                node: 0,
              },
            }],
          }],
        },
        plan: modelPlanFingerprint(request),
        request: modelRequestFingerprint(requestBytes),
      });
    },
    {
      maxItems: 1n,
      maxBytes: 1n << 20n,
      maxCollectionMembers: 1n,
      maxGraphNodes: 1n,
      maxAttributeValues: 1n,
      maxRolePlayers: 1n,
    },
  );
  session.registerModels(RemoteSmokeEmployee);
  const person = session.var(RemoteSmokePerson, "subtypes");

  const hydrated = await session.query(person).one();
  assert.equal(exchanges, 1);
  assert.ok(hydrated instanceof RemoteSmokeEmployee);
  assert.equal(hydrated._iid, "0x01");
  assert.equal(hydrated.name.value, "Alice");
  assert.ok(Object.isFrozen(hydrated));
  assert.ok(Object.isFrozen(hydrated.name));
});

test("public remote facade preserves every authenticated structured failure field", async () => {
  let exchanges = 0;
  const session = new RemoteQuerySession(
    modelAuthority(),
    modelAdvertisement(),
    async (requestBytes) => {
      exchanges += 1;
      const request = decodeModelRequest(requestBytes);
      return modelSignedReply({
        category: "invalid_contract",
        code: "remote_application_failure",
        details: {
          attempt: { kind: "long", value: "7" },
          retryable: { kind: "boolean", value: false },
          subject: { kind: "text", value: "smoke-person" },
        },
        format: "typebridge.query-remote-failure/v2",
        message: "the remote application rejected this query",
        nonce: request.nonce,
        path: [
          { kind: "field", value: "plan" },
          { kind: "index", value: 0 },
          { kind: "identifier", value: "smoke-person" },
        ],
        request: modelRequestFingerprint(requestBytes),
      });
    },
    {
      maxItems: 1n,
      maxBytes: 1n << 20n,
      maxCollectionMembers: 1n,
      maxGraphNodes: 1n,
      maxAttributeValues: 1n,
      maxRolePlayers: 1n,
    },
  );
  const person = session.var(RemoteSmokePerson);

  await assert.rejects(
    session.query(person).one(),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.category, "invalid_contract");
      assert.equal(error.code, "remote_application_failure");
      assert.equal(
        error.diagnosticMessage,
        "the remote application rejected this query",
      );
      assert.deepEqual(error.path, [
        { kind: "field", value: "plan" },
        { kind: "index", value: 0 },
        { kind: "identifier", value: "smoke-person" },
      ]);
      assert.deepEqual(error.details, {
        attempt: { kind: "long", value: "7" },
        retryable: { kind: "boolean", value: false },
        subject: { kind: "text", value: "smoke-person" },
      });
      return true;
    },
  );
  assert.equal(exchanges, 1);
});

interface RawModelRemotePending {
  requestBytes(): Uint8Array;
  decodeReply(response: unknown): Promise<object>;
}

interface RawModelRemoteNative {
  queryV2RemoteModelContext(
    authority: object,
    advertisement: unknown,
    maxItems: unknown,
    maxBytes: unknown,
    maxCollectionMembers: unknown,
    maxGraphNodes: unknown,
    maxAttributeValues: unknown,
    maxRolePlayers: unknown,
    deadlineMs?: unknown,
  ): object;
  queryV2PrepareRemoteModelRows(
    query: object,
    context: object,
    orders: object[],
    offset: unknown,
    limit: unknown,
    cardinality: string,
  ): RawModelRemotePending;
  queryV2PrepareRemoteModelPage(
    query: object,
    context: object,
    root: object,
    orders: object[],
    offset: unknown,
    limit: unknown,
    includeTotal: unknown,
  ): RawModelRemotePending;
}

function rawModelRemoteFixture(): Readonly<{
  native: RawModelRemoteNative;
  context: object;
  query: object;
  root: object;
  order: object;
}> {
  const nativePath = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  assert.ok(nativePath, "native test path");
  // Security probes deliberately bypass the package facade and exercise the
  // raw addon trust boundary.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const native = require(nativePath) as RawModelRemoteNative;
  const authority = queryV2AuthorityHandle(modelAuthority());
  assert.ok(authority);
  const context = native.queryV2RemoteModelContext(
    authority,
    modelAdvertisement(),
    1n,
    1n << 20n,
    1n,
    1n,
    1n,
    1n,
    null,
  );
  const session = diagnosticQuerySession();
  const person = session.var(RemoteSmokePerson);
  const direct = session.query(person);
  const terminal = preparePageTerminal(direct, person, {
    limit: 1,
    orderBy: [person.field(remoteSmokeRefs.fields.name).asc()],
  });
  const [order] = terminal.orders;
  assert.ok(order);
  return Object.freeze({
    native,
    context,
    query: terminal.state.handle,
    root: terminal.nativeRoot,
    order,
  });
}

function rawPending(
  fixture: ReturnType<typeof rawModelRemoteFixture>,
): RawModelRemotePending {
  return fixture.native.queryV2PrepareRemoteModelRows(
    fixture.query,
    fixture.context,
    [],
    0n,
    1n,
    "exactly_one",
  );
}

function sharedModelBytes(values: readonly number[]): Buffer {
  const storage = new SharedArrayBuffer(values.length);
  const view = new Uint8Array(storage);
  view.set(values);
  return Buffer.from(storage);
}

test("raw model remote context rejects hostile limits and shared advertisements", () => {
  const nativePath = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  assert.ok(nativePath, "native test path");
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const native = require(nativePath) as RawModelRemoteNative;
  const authority = queryV2AuthorityHandle(modelAuthority());
  assert.ok(authority);
  const invoke = (
    maxItems: unknown,
    advertisement: unknown = modelAdvertisement(),
    deadline: unknown = null,
  ): object =>
    native.queryV2RemoteModelContext(
      authority,
      advertisement,
      maxItems,
      1n << 20n,
      1n,
      1n,
      1n,
      1n,
      deadline,
    );

  for (const hostile of [
    true,
    1,
    1.5,
    "1",
    -1n,
    1n << 128n,
    null,
  ]) {
    assert.throws(
      () => invoke(hostile),
      /query_remote_limit_invalid/,
    );
  }
  for (const hostileDeadline of [true, 1, -1n, 1n << 128n]) {
    assert.throws(
      () => invoke(1n, modelAdvertisement(), hostileDeadline),
      /query_remote_limit_invalid/,
    );
  }
  assert.throws(
    () => invoke(1n, sharedModelBytes([0x7b, 0x7d])),
    /query_v2_shared_buffer_unsupported/,
  );
});

test("raw model remote order arrays are bounded before handle extraction", () => {
  const fixture = rawModelRemoteFixture();
  const oversized = Array.from({ length: 65 }, () => fixture.order);
  const preparations = [
    () =>
      fixture.native.queryV2PrepareRemoteModelRows(
        fixture.query,
        fixture.context,
        oversized,
        0n,
        1n,
        "bounded_many",
      ),
    () =>
      fixture.native.queryV2PrepareRemoteModelPage(
        fixture.query,
        fixture.context,
        fixture.root,
        oversized,
        0n,
        1n,
        false,
      ),
  ];
  for (const prepare of preparations) {
    assert.throws(prepare, (error: unknown) => {
      if (!(error instanceof Error)) return false;
      const payload = JSON.parse(error.message) as {
        category: string;
        code: string;
        path: unknown;
        details: unknown;
      };
      assert.equal(payload.category, "resource_limit");
      assert.equal(payload.code, "structural_limit_exceeded");
      assert.deepEqual(payload.path, [{ kind: "operation" }]);
      assert.deepEqual(payload.details, {
        actual: { kind: "unsigned", value: 65 },
        limit: { kind: "text", value: "order_terms" },
        maximum: { kind: "unsigned", value: 64 },
      });
      return true;
    });
  }
});

test("raw model pending consumes invalid first replies and rejects replay before inspection", async () => {
  const fixture = rawModelRemoteFixture();

  const wrongType = rawPending(fixture);
  assert.throws(
    () => wrongType.decodeReply("not bytes"),
    /Expected a Buffer value/,
  );
  await assert.rejects(
    wrongType.decodeReply(sharedModelBytes([0x01])),
    /query_remote_v2_reply_replayed/,
  );

  const sharedFirst = rawPending(fixture);
  assert.throws(
    () => sharedFirst.decodeReply(sharedModelBytes([0x02])),
    /query_v2_shared_buffer_unsupported/,
  );
  let inspected = 0;
  const hostileReplay = new Proxy(Object.create(null) as object, {
    get(): never {
      inspected += 1;
      throw new Error("replayed response was inspected");
    },
    getPrototypeOf(): never {
      inspected += 1;
      throw new Error("replayed response was classified");
    },
  });
  await assert.rejects(
    sharedFirst.decodeReply(hostileReplay),
    /query_remote_v2_reply_replayed/,
  );
  assert.equal(inspected, 0);
});
