import assert = require("node:assert/strict");
import crypto = require("node:crypto");
import fs = require("node:fs");
import net = require("node:net");
import path = require("node:path");
import test = require("node:test");

import { PlanAuthority } from "../../typescript/index.js";
import { loadNative } from "../../typescript/native.js";
import {
  InstalledRuntimeProjection,
  ProxyConnection,
  QueryExecutionResourceLimits,
} from "../../typescript/runtime-projection.js";

interface Corpus {
  readonly declared_b64: string;
  readonly scope: string;
  readonly profile: string;
  readonly capabilities: readonly string[];
}

const corpus = JSON.parse(fs.readFileSync(
  path.resolve(process.cwd(), "../../../tests/fixtures/plan-authority.json"),
  "utf8",
)) as Corpus;

function authority(): PlanAuthority {
  return new PlanAuthority(
    Buffer.from(corpus.declared_b64, "base64"),
    corpus.scope,
    corpus.profile,
  );
}

/** A valid advertisement, keys sorted the way the contract codec writes them. */
function advertisement(capabilities: readonly string[] = corpus.capabilities): Uint8Array {
  const privateKey = crypto.createPrivateKey({
    key: Buffer.concat([
      Buffer.from("302e020100300506032b657004220420", "hex"),
      Buffer.alloc(32, 0x24),
    ]),
    format: "der",
    type: "pkcs8",
  });
  const publicKey = crypto
    .createPublicKey(privateKey)
    .export({ format: "der", type: "spki" })
    .subarray(-32);
  const keyId = crypto
    .createHash("sha256")
    .update(Buffer.from("typebridge.query.remote-reply-key-id/v1\0"))
    .update(publicKey)
    .digest("hex");
  return Buffer.from(JSON.stringify({
    capabilities,
    executor: { epoch: "node-proxy-epoch-0001", identity: "node-proxy-executor" },
    format: "typebridge.query-remote-capabilities/v1",
    reply_key: publicKey.toString("hex"),
    reply_key_id: keyId,
  }));
}

/** An address nothing listens on. */
async function closedEndpoint(): Promise<string> {
  const server = net.createServer();
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as net.AddressInfo;
  await new Promise<void>((resolve) => server.close(() => resolve()));
  return `http://127.0.0.1:${port}`;
}

async function code(connecting: Promise<unknown>): Promise<string> {
  try {
    await connecting;
  } catch (error) {
    return (error as { code: string }).code;
  }
  assert.fail("proxy connection unexpectedly opened");
}

test("malformed proxy endpoints and headers are refused by name", async () => {
  assert.equal(
    await code(ProxyConnection.connect(authority(), "https://proxy:8443")),
    "proxy_endpoint_invalid",
  );
  assert.equal(
    await code(ProxyConnection.connect(authority(), "http://proxy:8080", {
      tls: "native_roots",
    })),
    "proxy_endpoint_invalid",
  );
  assert.equal(
    await code(ProxyConnection.connect(authority(), "http://proxy:8080", {
      headers: [["Content-Length", "1"]],
    })),
    "proxy_header_invalid",
  );
});

test("an unreachable proxy fails as a redacted provider operation", async () => {
  const endpoint = await closedEndpoint();
  let failure: unknown;
  try {
    await ProxyConnection.connect(authority(), endpoint, {
      headers: [["Authorization", "Bearer secret-token"]],
      connectionLimits: new QueryExecutionResourceLimits({ timeoutMilliseconds: 5_000n }),
    });
  } catch (error) {
    failure = error;
  }
  assert.equal((failure as { code: string }).code, "provider_operation_failed");
  const serialized = `${String(failure)} ${JSON.stringify(failure)}`;
  assert.ok(!serialized.includes("secret-token"));
  assert.ok(!serialized.includes(new URL(endpoint).port));
});

test("a pinned advertisement connects without reaching the endpoint", async () => {
  const connection = await ProxyConnection.connect(authority(), await closedEndpoint(), {
    advertisement: advertisement(),
    answerLimits: new QueryExecutionResourceLimits({ items: 8n }),
  });
  assert.ok(connection instanceof ProxyConnection);
  assert.ok(Object.isFrozen(connection));
  assert.equal(connection.answerLimits.items, 8n);
});

test("a proxy connection without answer limits runs at the proxy defaults", async () => {
  const connection = await ProxyConnection.connect(authority(), await closedEndpoint(), {
    advertisement: advertisement(),
  });
  const native = loadNative();
  const limits = connection.answerLimits;
  assert.deepEqual(
    [
      limits.timeoutMilliseconds,
      limits.items,
      limits.bytes,
      limits.graphNodes,
      limits.attributeValues,
      limits.collectionMembers,
      limits.rolePlayers,
      limits.statements,
    ],
    [
      native.DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS,
      native.DEFAULT_PROXY_QUERY_ITEMS,
      native.DEFAULT_PROXY_QUERY_BYTES,
      native.DEFAULT_PROXY_QUERY_GRAPH_NODES,
      native.DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES,
      native.DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS,
      native.DEFAULT_PROXY_QUERY_ROLE_PLAYERS,
      undefined,
    ],
  );
  assert.notEqual(limits.items, new QueryExecutionResourceLimits().items);
});

test("proxy connect rejects JavaScript type confusion before native work", async () => {
  await assert.rejects(
    ProxyConnection.connect({} as PlanAuthority, "http://proxy:8080"),
    /PlanAuthority/,
  );
  await assert.rejects(
    ProxyConnection.connect(authority(), "http://proxy:8080", {
      headers: [["Authorization"]] as unknown as [string, string][],
    }),
    /string pairs/,
  );
});

test("a staged proxy transaction fetches the staged advertisement at open", async () => {
  const endpoint = await closedEndpoint();
  const connection = await ProxyConnection.connect(authority(), endpoint, {
    advertisement: advertisement([]),
    answerLimits: new QueryExecutionResourceLimits({ timeoutMilliseconds: 5_000n }),
  });
  // The empty bootstrap advertisement does not cover `/v3/capabilities`, so an
  // unreachable server fails the open as a redacted provider operation.
  const failure = await connection.transaction("write").then(
    () => assert.fail("staged transaction unexpectedly opened"),
    (error: unknown) => error,
  );
  assert.equal((failure as { code: string }).code, "provider_operation_failed");
  assert.ok(!`${String(failure)}`.includes(new URL(endpoint).port));
});

test("a staged proxy transaction refuses an unknown kind before any exchange", async () => {
  const connection = await ProxyConnection.connect(authority(), await closedEndpoint(), {
    advertisement: advertisement(),
  });
  await assert.rejects(
    connection.transaction("bogus" as "read"),
    /unknown transaction kind 'bogus'/,
  );
});

test("admission window defaults and round-trips independently of execution", async () => {
  const endpoint = await closedEndpoint();
  const defaults = await ProxyConnection.connect(authority(), endpoint, { advertisement: advertisement() });
  assert.equal(defaults.admissionWindowMilliseconds, 60_000n);
  const configured = await ProxyConnection.connect(authority(), endpoint, {
    advertisement: advertisement(), admissionWindowMilliseconds: 1234n,
  });
  assert.equal(configured.admissionWindowMilliseconds, 1234n);
  await assert.rejects(ProxyConnection.connect(authority(), endpoint, { admissionWindowMilliseconds: 0n }));
});

test("query policy intersects independent staged server ceilings", async () => {
  const { QueryLimits } = await import("../../typescript/runtime-projection.js");
  const raw = JSON.parse(Buffer.from(advertisement()).toString("utf8"));
  const { reply_key, reply_key_id, ...prefix } = raw;
  prefix.capabilities = [...new Set([...prefix.capabilities, "query.plan.v3", "query.remote.envelope-v3"])].sort();
  const pinned = Buffer.from(JSON.stringify({ ...prefix,
    query_limits: { bindings: 768, boolean_terms: 300, selected_slots: 256 },
    reply_key, reply_key_id,
  }));
  const connection = await ProxyConnection.connect(authority(), await closedEndpoint(), {
    advertisement: pinned,
    queryLimits: new QueryLimits({ bindings: 1024n, boolean_terms: 512n, input_rows: 13n }),
  });
  assert.equal(connection.queryLimits.get("bindings"), 768n);
  assert.equal(connection.queryLimits.get("boolean_terms"), 300n);
  assert.equal(connection.queryLimits.get("input_rows"), 13n);
  assert.equal(connection.queryLimits.get("selected_slots"), 16n);
});

test("invalid staged query policy is refused before proxy I/O", async () => {
  const { QueryLimits } = await import("../../typescript/runtime-projection.js");
  await assert.rejects(ProxyConnection.connect(authority(), await closedEndpoint(), {
    queryLimits: new QueryLimits({ bindings: 8n }),
  }), /selected_slots/);
});

test("invalid staged query policy is refused before direct I/O", async () => {
  const { QueryLimits } = await import("../../typescript/runtime-projection.js");
  const { Database } = await import("../../typescript/index.js");
  assert.throws(() => Database.connect("127.0.0.1:1", "app", {
    queryLimits: new QueryLimits({ bindings: 8n }),
  }), /selected_slots/);
});


test("generated managers select the registered proxy without exposing its native handle", async () => {
  const connection = await ProxyConnection.connect(authority(), await closedEndpoint(), {
    advertisement: advertisement(),
  });
  const manager = Object.freeze({});
  let calls = 0;
  const projection = new InstalledRuntimeProjection({
    managerForProxy: (typeKey: string, native: unknown) => {
      assert.equal(typeKey, "entity:person");
      assert.ok(native);
      calls++;
      return manager;
    },
  } as never);
  assert.equal(projection.manager("entity:person", connection), manager);
  assert.equal(calls, 1);
  assert.equal(projection.direct(connection), connection);
  assert.doesNotThrow(() => projection.assertConnection(connection));
  const forged = Object.create(ProxyConnection.prototype) as ProxyConnection;
  assert.throws(() => projection.manager("entity:person", forged), TypeError);
  assert.throws(() => projection.direct(forged), TypeError);
  assert.throws(() => projection.assertConnection(forged), TypeError);
  assert.equal(calls, 1);
});
