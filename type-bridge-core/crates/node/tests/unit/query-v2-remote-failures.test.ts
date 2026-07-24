import assert = require("node:assert/strict");
import crypto = require("node:crypto");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

import {
  QueryV2Authority,
  queryV2PrepareRemote,
  queryV2RemoteCapabilities,
} from "../../typescript/index.js";

interface FailureCase {
  readonly name: string;
  readonly reply: Readonly<Record<string, unknown>>;
  readonly expected: string;
  readonly replay_expected: string;
}

interface BoundFailureCase {
  readonly name: string;
  readonly diagnostic: Readonly<{
    category: string;
    code: string;
    format: string;
    message: string;
  }>;
  readonly fingerprint_canonicalization: string;
  readonly fingerprint_domain: string;
  readonly max_items: number;
  readonly replay_expected: string;
}

interface FailureCorpus {
  readonly format: string;
  readonly declared_b64: string;
  readonly plan_b64: string;
  readonly scope: string;
  readonly profile: string;
  readonly invocation: Readonly<Record<string, unknown>>;
  readonly capabilities: readonly string[];
  readonly bound_case: BoundFailureCase;
  readonly cases: readonly FailureCase[];
}

const corpusPath = path.resolve(
  process.cwd(),
  "../../../tests/fixtures/query-v2-remote-failures.json",
);
const corpus = JSON.parse(fs.readFileSync(corpusPath, "utf8")) as FailureCorpus;

function canonical(value: unknown): Uint8Array {
  // Every object in the checked-in corpus is written in lexicographic key
  // order. JSON.parse preserves that insertion order, so this reproduces the
  // contract codec's canonical compact bytes without a second JS codec.
  return Buffer.from(JSON.stringify(value));
}

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

function fingerprint(
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

function advertisement(): Uint8Array {
  return canonical({
    capabilities: corpus.capabilities,
    executor: {
      epoch: "node-binding-epoch-0001",
      identity: "node-binding-executor",
    },
    format: "typebridge.query-remote-capabilities/v1",
    reply_key: signingPublicKey.toString("hex"),
    reply_key_id: signingKeyId,
  });
}

function signedReply(payload: unknown): Uint8Array {
  const advertisementBytes = advertisement();
  const advertisementFingerprint = fingerprint(
    "typebridge.query.remote-capabilities",
    "typebridge.query-remote-capabilities/v1",
    advertisementBytes,
  );
  const key = signingPublicKey.toString("hex");
  const prefix = Buffer.from(
    `{"advertisement":"${advertisementFingerprint}",` +
      `"format":"typebridge.query-remote-signed-reply/v1",` +
      `"key":"${key}","key_id":"${signingKeyId}","payload":`,
  );
  const payloadBytes = canonical(payload);
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

function pendingReply(maxItems = 10n, maxBytes = 1n << 20n) {
  const authority = new QueryV2Authority(
    Buffer.from(corpus.declared_b64, "base64"),
    corpus.scope,
    corpus.profile,
  );
  return queryV2PrepareRemote(
    authority,
    Buffer.from(corpus.plan_b64, "base64"),
    JSON.stringify(corpus.invocation),
    advertisement(),
    {
      maxItems,
      maxBytes,
      maxCollectionMembers: 1_000n,
    },
  );
}

function requestFingerprint(
  request: Uint8Array,
  failure: BoundFailureCase,
): string {
  const digest = crypto.createHash("sha256");
  digest.update(Buffer.from("typebridge.fingerprint/v1\0"));
  for (const value of [
    failure.fingerprint_domain,
    failure.fingerprint_canonicalization,
  ]) {
    const encoded = Buffer.from(value);
    const length = Buffer.alloc(8);
    length.writeBigUInt64BE(BigInt(encoded.length));
    digest.update(length);
    digest.update(encoded);
  }
  digest.update(Buffer.from([0])); // no semantic profile
  const length = Buffer.alloc(8);
  length.writeBigUInt64BE(BigInt(request.length));
  digest.update(length);
  digest.update(request);
  return digest.digest("hex");
}

test("V2 string inputs fail at their semantic byte ceilings", () => {
  const declared = Buffer.from(corpus.declared_b64, "base64");
  assert.throws(
    () =>
      new QueryV2Authority(
        declared,
        "s".repeat(1024 * 1024 + 1),
        corpus.profile,
      ),
    /malformed_managed_scope_id: managed scope ID is empty or exceeds the canonical string limit/,
  );
  assert.throws(
    () =>
      new QueryV2Authority(
        declared,
        corpus.scope,
        "p".repeat(256),
      ),
    /invalid_fingerprint_identifier: fingerprint metadata identifier is malformed/,
  );

  const authority = new QueryV2Authority(
    declared,
    corpus.scope,
    corpus.profile,
  );
  assert.throws(
    () =>
      queryV2PrepareRemote(
        authority,
        Buffer.from(corpus.plan_b64, "base64"),
        " ".repeat(4 * 1024 * 1024 + 1),
        advertisement(),
        {
          maxItems: 10n,
          maxBytes: 1n << 20n,
          maxCollectionMembers: 1_000n,
        },
      ),
    /query_invocation_input_byte_limit: invocation input rows exceed the structural byte ceiling/,
  );
});

test("shared remote failure corpus rejects unbound diagnostics identically", async () => {
  assert.equal(corpus.format, "typebridge.query-remote-binding-failure-corpus/v1");
  assert.deepEqual(queryV2RemoteCapabilities(advertisement()), corpus.capabilities);

  for (const failure of corpus.cases) {
    const pending = pendingReply();
    const bytes = signedReply(failure.reply);
    await assert.rejects(
      pending.decodeReply(bytes),
      new RegExp(failure.expected),
      failure.name,
    );
    await assert.rejects(
      pending.decodeReply(bytes),
      new RegExp(failure.replay_expected),
      `${failure.name} consumes its one-shot reply handle`,
    );
  }
});

test("request-bound failure surfaces identically and consumes reply handle", async () => {
  const failure = corpus.bound_case;
  const pending = pendingReply(BigInt(failure.max_items));
  const request = Buffer.from(pending.requestBytes());
  const parsedRequest = JSON.parse(request.toString("utf8")) as {
    limits: { max_items: number };
    nonce: string;
  };
  assert.equal(parsedRequest.limits.max_items, 1);
  const bytes = signedReply({
    ...failure.diagnostic,
    nonce: parsedRequest.nonce,
    request: requestFingerprint(request, failure),
  });

  const decode = pending.decodeReply(bytes);
  assert.ok(decode instanceof Promise);
  await assert.rejects(
    decode,
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.equal(
        error.message,
        `${failure.diagnostic.code}: ${failure.diagnostic.message}`,
      );
      return true;
    },
  );
  await assert.rejects(
    pending.decodeReply(bytes),
    new RegExp(failure.replay_expected),
  );
});

test("success byte budget authenticates first and keeps failures decodable", async () => {
  const failure = corpus.bound_case;
  const forgedPending = pendingReply(10n, 0n);
  const forgedRequest = Buffer.from(forgedPending.requestBytes());
  const forgedNonce = (JSON.parse(forgedRequest.toString("utf8")) as {
    nonce: string;
  }).nonce;
  const signedSuccess = Buffer.from(
    signedReply({
      format: "typebridge.query-remote-response/v1",
      nonce: forgedNonce,
      outcome: { kind: "rows", rows: [] },
      plan: "0".repeat(64),
      request: requestFingerprint(forgedRequest, failure),
    }),
  );
  const forged = Buffer.from(signedSuccess);
  const marker = Buffer.from('"signature":"');
  const signature = forged.indexOf(marker) + marker.length;
  assert.ok(signature >= marker.length);
  forged[signature] = forged[signature] === 0x30 ? 0x31 : 0x30;
  await assert.rejects(
    forgedPending.decodeReply(forged),
    /query_remote_signature_invalid/,
  );

  const wrongRequestPending = pendingReply(10n, 0n);
  const wrongRequest = Buffer.from(wrongRequestPending.requestBytes());
  const wrongRequestNonce = (JSON.parse(wrongRequest.toString("utf8")) as {
    nonce: string;
  }).nonce;
  const planFingerprint = fingerprint(
    "typebridge.query.plan",
    "typebridge.query-plan-c14n/v1",
    Buffer.from(corpus.plan_b64, "base64"),
  );
  const signedWrongRequest = signedReply({
    format: "typebridge.query-remote-response/v1",
    nonce: wrongRequestNonce,
    outcome: { kind: "rows", rows: [] },
    plan: planFingerprint,
    request: "0".repeat(64),
  });
  await assert.rejects(
    wrongRequestPending.decodeReply(signedWrongRequest),
    /query_remote_request_mismatch/,
  );

  const validPending = pendingReply(10n, 0n);
  const validRequest = Buffer.from(validPending.requestBytes());
  const validNonce = (JSON.parse(validRequest.toString("utf8")) as {
    nonce: string;
  }).nonce;
  const signedFailure = signedReply({
    ...failure.diagnostic,
    nonce: validNonce,
    request: requestFingerprint(validRequest, failure),
  });
  assert.ok(signedFailure.length > 1); // maxBytes=0 must not truncate failure evidence.
  await assert.rejects(
    validPending.decodeReply(signedFailure),
    (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.equal(
        error.message,
        `${failure.diagnostic.code}: ${failure.diagnostic.message}`,
      );
      return true;
    },
  );
});

test("concurrent reply decodes admit exactly one one-shot claimant", async () => {
  const failure = corpus.bound_case;
  const pending = pendingReply(BigInt(failure.max_items));
  const request = Buffer.from(pending.requestBytes());
  const parsedRequest = JSON.parse(request.toString("utf8")) as { nonce: string };
  const bytes = signedReply({
    ...failure.diagnostic,
    nonce: parsedRequest.nonce,
    request: requestFingerprint(request, failure),
  });

  const results = await Promise.allSettled([
    pending.decodeReply(bytes),
    pending.decodeReply(bytes),
  ]);
  const messages = results.map((result) => {
    assert.equal(result.status, "rejected");
    const reason: unknown = result.status === "rejected" ? result.reason : undefined;
    assert.ok(reason instanceof Error);
    return reason.message;
  });
  assert.equal(
    messages.filter(
      (message) =>
        message === `${failure.diagnostic.code}: ${failure.diagnostic.message}`,
    ).length,
    1,
  );
  assert.equal(
    messages.filter((message) => message.includes(failure.replay_expected)).length,
    1,
  );
});

test("replay rejection does not inspect or snapshot response bytes", async () => {
  const failure = corpus.cases[0]!;
  const pending = pendingReply();
  await assert.rejects(
    pending.decodeReply(signedReply(failure.reply)),
    new RegExp(failure.expected),
  );

  const hostileReplay = new Proxy(new Uint8Array([0x7b, 0x7d]), {
    get(): never {
      throw new Error("replayed response was inspected");
    },
  });
  await assert.rejects(
    pending.decodeReply(hostileReplay),
    /query_remote_reply_replayed/,
  );
});

test("invalid first response types consume the claim before inspection", async () => {
  const pending = pendingReply();
  let inspected = false;
  const invalid = new Proxy({} as Uint8Array, {
    get(): never {
      inspected = true;
      throw new Error("invalid response was inspected");
    },
  });

  assert.throws(
    () => pending.decodeReply(invalid),
    /Expected a Buffer value/,
  );
  assert.equal(inspected, false);

  const hostileReplay = new Proxy(new Uint8Array([0x7b, 0x7d]), {
    get(): never {
      inspected = true;
      throw new Error("replayed response was inspected");
    },
  });
  await assert.rejects(
    pending.decodeReply(hostileReplay),
    /query_remote_reply_replayed/,
  );
  assert.equal(inspected, false);
});
