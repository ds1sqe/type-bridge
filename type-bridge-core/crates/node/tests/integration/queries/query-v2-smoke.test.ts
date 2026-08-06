/**
 * Cross-binding smoke: one publicly authored V2 plan, local and remote.
 *
 * The declared-schema bytes remain a canonical authority fixture. The plan
 * and invocation are authored at runtime through the public query-v2
 * subpath. Local execution runs through the native module against a fresh
 * isolated database; remote execution travels the versioned envelope over
 * HTTP (or verified HTTPS in the TLS lane) to the `v2_smoke_server` example
 * serving the same database, and both paths must return byte-identical typed
 * outcome JSON.
 */

import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import net from "node:net";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  TYPEDB_ADDRESS,
  TYPEDB_HTTP_PORT,
  TYPEDB_PASSWORD,
  TYPEDB_USERNAME,
} from "../common/index.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const packageRoot = process.cwd();
const _require = createRequire(import.meta.url);
const pkg = _require(packageRoot) as typeof import("../../../typescript/public.js");
const queryV2 = _require(
  path.join(packageRoot, "dist/query-v2.js"),
) as typeof import("../../../typescript/query-v2.js");

const DECLARED_B64 =
  "eyJkZWNsYXJlZF9pZGVudGl0eSI6eyJhbGdvcml0aG0iOiJzaGEyNTYiLCJjYW5vbmljYWxpemF0aW9u" +
  "IjoidHlwZWJyaWRnZS5zY2hlbWEtY2Fub25pY2FsLWpzb24vdjEiLCJkaWdlc3QiOiJiZGFiNzEzOGE1" +
  "NzIzOGVlMjNkZmNlYjY5ZTdmMDk4OTNjZmE3YjUzNmQ5ZTcwMzU2ZDFhOTg2YTEzMjQ5OWZlIiwiZG9t" +
  "YWluIjoidHlwZWJyaWRnZS5zY2hlbWEuZGVjbGFyZWQtaWRlbnRpdHkifSwiZmFjdHMiOlt7ImtpbmQi" +
  "OiJ0eXBlIiwidmFsdWUiOnsiaWQiOnsia2luZCI6ImVudGl0eSIsImxhYmVsIjoic21va2UtcGVyc29u" +
  "In19fSx7ImtpbmQiOiJ0eXBlIiwidmFsdWUiOnsiaWQiOnsia2luZCI6ImF0dHJpYnV0ZSIsImxhYmVs" +
  "Ijoic21va2UtbmFtZSJ9fX0seyJraW5kIjoidmFsdWUiLCJ2YWx1ZSI6eyJpZCI6InNtb2tlLW5hbWUi" +
  "LCJ2YWx1ZV90eXBlIjoic3RyaW5nIn19LHsia2luZCI6Im93bnMiLCJ2YWx1ZSI6eyJpZCI6eyJhdHRy" +
  "aWJ1dGUiOiJzbW9rZS1uYW1lIiwib3duZXIiOnsia2luZCI6ImVudGl0eSIsImxhYmVsIjoic21va2Ut" +
  "cGVyc29uIn19fX1dLCJmb3JtYXRfdmVyc2lvbiI6MSwicmVxdWlyZWRfY2FwYWJpbGl0aWVzIjpbXX0=";
const SCOPE = "binding-smoke";
const PROFILE = "typedb-3.12.1/v1";
const CROSS_BINDING_PLAN_FINGERPRINT =
  "d605b3bc6e8a9c59a03d2a79d7ec497dd637109b2cbf70ebaa5ac4b951f53502";
const CROSS_BINDING_INVOCATION_SHA256 =
  "ca5bc9a7657a21c5cf330e99a678a4c0fc25d803828f456c8b520625f54143b7";

function requiredEnvironment(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`TLS binding smoke requires ${name}`);
  }
  return value;
}

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const probe = net.createServer();
    probe.listen(0, "127.0.0.1", () => {
      const address = probe.address();
      if (address === null || typeof address === "string") {
        reject(new Error("no probe address"));
        return;
      }
      probe.close(() => resolve(address.port));
    });
  });
}

async function waitForPort(
  port: number,
  server: ChildProcess,
  timeoutMs: number,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (server.exitCode !== null) {
      throw new Error(`smoke server exited early with code ${server.exitCode}`);
    }
    const reachable = await new Promise<boolean>((resolve) => {
      const socket = net.connect({ host: "127.0.0.1", port, timeout: 1000 });
      socket.once("connect", () => {
        socket.destroy();
        resolve(true);
      });
      socket.once("error", () => resolve(false));
      socket.once("timeout", () => {
        socket.destroy();
        resolve(false);
      });
    });
    if (reachable) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error("smoke server never became reachable");
}

function authorPlan(authority: InstanceType<typeof queryV2.QueryV2Authority>) {
  const builder = new queryV2.QueryPlanBuilder(authority);
  const person = builder.binding("person");
  const name = builder.binding("name");
  const wantedName = builder.input("wanted_name", "string", false);
  builder.match([
    builder.isa(person, "entity", "smoke-person", true),
    builder.has(person, name, "smoke-name"),
    builder.value(
      "equal",
      builder.bindingOperand(name),
      builder.inputOperand(wantedName),
    ),
  ]);
  builder.select([person, name]);
  builder.require([name]);
  builder.distinct();
  builder.sort([builder.order(name, "ascending")]);
  const plan = builder.finalizeRows([person, name]);
  const invocation = plan.rows([["ada"], ["bob"]]);
  return {
    invocation: Buffer.from(invocation.canonicalBytes).toString("utf8"),
    invocationDigest: createHash("sha256")
      .update(invocation.canonicalBytes)
      .digest("hex"),
    plan: Buffer.from(plan.canonicalBytes),
    planFingerprint: plan.fingerprint,
  };
}

// The prepared exchange asserts the 3.12.1 semantic profile and the native
// given transport; legacy servers reject both before I/O, so the smoke is
// meaningful only against a 3.12+ lane.
const smokeServerVersion = process.env.TYPEDB_VERSION ?? "3.12.1";
const [smokeMajor = 0, smokeMinor = 0] = smokeServerVersion.split(".").map(Number);
const smokeServerIsV2Conformant = smokeMajor > 3 || (smokeMajor === 3 && smokeMinor >= 12);

const smokeSkip = smokeServerIsV2Conformant ? false : "requires a TypeDB 3.12+ server";

test("prepared plan executes locally and remotely", { skip: smokeSkip }, async () => {
  const tlsEnvironment = [
    process.env.TYPEDB_TLS_ADDRESS,
    process.env.TYPEDB_TLS_HTTP_PORT,
    process.env.TYPEDB_TLS_ROOT_CA,
  ];
  const tlsEnabled = tlsEnvironment.some((value) => value !== undefined);
  const address = tlsEnabled
    ? requiredEnvironment("TYPEDB_TLS_ADDRESS")
    : TYPEDB_ADDRESS;
  const httpPort = tlsEnabled
    ? Number(requiredEnvironment("TYPEDB_TLS_HTTP_PORT"))
    : TYPEDB_HTTP_PORT;
  const tlsRootCa = tlsEnabled
    ? requiredEnvironment("TYPEDB_TLS_ROOT_CA")
    : undefined;
  const connectionOptions = {
    username: TYPEDB_USERNAME,
    password: TYPEDB_PASSWORD,
    httpPort,
    ...(tlsEnabled ? { tlsEnabled: true, tlsRootCa } : {}),
  };
  const serverTlsEnvironment = tlsEnabled
    ? {
        SMOKE_TYPEDB_TLS: "true",
        SMOKE_TYPEDB_TLS_ROOT_CA: requiredEnvironment("TYPEDB_TLS_ROOT_CA"),
        SMOKE_TLS_CERT: requiredEnvironment("SMOKE_TLS_CERT"),
        SMOKE_TLS_KEY: requiredEnvironment("SMOKE_TLS_KEY"),
      }
    : {};
  if (tlsEnabled) {
    // Node reads this at process startup; test.sh supplies it for the lane so
    // built-in fetch performs ordinary certificate and hostname verification.
    requiredEnvironment("NODE_EXTRA_CA_CERTS");
  }
  const remoteScheme = tlsEnabled ? "https" : "http";
  const database = `tb_v2_node_smoke_${process.pid}_${Date.now()}`;
  pkg.ensureDatabase(
    address,
    database,
    connectionOptions,
  );
  const db = pkg.RustDatabase.connect(
    address,
    database,
    connectionOptions,
  );
  try {
    const schemaTx = db.transaction("schema");
    schemaTx.query(
      "define attribute smoke-name, value string; " +
        "entity smoke-person, owns smoke-name;",
    );
    schemaTx.commit();
    const writeTx = db.transaction("write");
    writeTx.query(
      'insert $a isa smoke-person, has smoke-name "ada"; ' +
        '$b isa smoke-person, has smoke-name "bob";',
    );
    writeTx.commit();

    const declared = Buffer.from(DECLARED_B64, "base64");
    const authority = new queryV2.QueryV2Authority(declared, SCOPE, PROFILE);
    const {
      plan,
      invocation,
      invocationDigest,
      planFingerprint,
    } = authorPlan(authority);
    assert.equal(planFingerprint, CROSS_BINDING_PLAN_FINGERPRINT);
    assert.equal(invocationDigest, CROSS_BINDING_INVOCATION_SHA256);
    const localAuthority = pkg.QueryV2Authority.queryOnly(
      db,
      declared,
      SCOPE,
      PROFILE,
    );
    for (const invalidDeadline of [1n << 200n, -(1n << 200n)]) {
      assert.throws(
        () => pkg.queryV2ExecuteLocal(
          db,
          localAuthority,
          plan,
          invocation,
          invalidDeadline,
        ),
        /query_remote_limit_invalid/,
      );
    }
    assert.throws(
      () => pkg.queryV2ExecuteLocal(
        db,
        localAuthority,
        plan,
        invocation,
        86_400_001n,
      ),
      /query_remote_deadline_limit/,
    );
    const local = await pkg.queryV2ExecuteLocal(
      db,
      localAuthority,
      plan,
      invocation,
      30_000n,
    );
    assert.ok(local.includes('"ada"'), local);
    assert.ok(local.includes('"bob"'), local);

    const port = await freePort();
    const server = spawn(
      "cargo",
      [
        "run",
        "--quiet",
        "-p",
        "type-bridge-server",
        "--features",
        "v2-query",
        "--example",
        "v2_smoke_server",
      ],
      {
        cwd: path.resolve(packageRoot, "../.."),
        env: {
          ...process.env,
          SMOKE_TYPEDB_ADDRESS: address,
          SMOKE_TYPEDB_USERNAME: TYPEDB_USERNAME,
          SMOKE_TYPEDB_PASSWORD: TYPEDB_PASSWORD,
          SMOKE_TYPEDB_HTTP_PORT: String(httpPort),
          SMOKE_DATABASE: database,
          SMOKE_DECLARED_B64: DECLARED_B64,
          SMOKE_SCOPE: SCOPE,
          SMOKE_PROFILE: PROFILE,
          SMOKE_PORT: String(port),
          ...serverTlsEnvironment,
        },
        stdio: "ignore",
      },
    );
    try {
      await waitForPort(port, server, 300_000);
      const advertisementResponse = await fetch(
        `${remoteScheme}://127.0.0.1:${port}/v2/capabilities`,
      );
      const advertisement = Buffer.from(
        await advertisementResponse.arrayBuffer(),
      );
      const capabilities = pkg.queryV2RemoteCapabilities(advertisement);
      assert.ok(capabilities.includes("query.plan"), String(capabilities));
      const limits = {
        maxItems: 100n,
        maxBytes: BigInt(1 << 20),
        maxCollectionMembers: 1_000n,
        deadlineMs: 30_000n,
      } as const;

      async function postRemote(request: Uint8Array): Promise<Buffer> {
        const response = await fetch(
          `${remoteScheme}://127.0.0.1:${port}/v2/query`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: new Uint8Array(request),
          },
        );
        return Buffer.from(await response.arrayBuffer());
      }

      const limited = pkg.queryV2PrepareRemote(
        authority,
        plan,
        invocation,
        advertisement,
        { ...limits, maxItems: 1n },
      );
      const limitedRequest = Buffer.from(limited.requestBytes());
      const limitedBody = await postRemote(limitedRequest);
      const limitedReply = JSON.parse(limitedBody.toString("utf8")) as {
        payload: {
          code: string;
          nonce: string;
          request: string;
        };
      };
      assert.equal(limitedReply.payload.code, "processed_item_limit");
      assert.equal(
        limitedReply.payload.nonce,
        (JSON.parse(limitedRequest.toString("utf8")) as { nonce: string }).nonce,
      );
      assert.match(limitedReply.payload.request, /^[0-9a-f]{64}$/u);
      await assert.rejects(
        limited.decodeReply(limitedBody),
        (error: unknown) => {
          assert.ok(error instanceof Error);
          assert.equal(
            error.message,
            "processed_item_limit: provider answer exceeded the processed-item ceiling",
          );
          return true;
        },
      );
      await assert.rejects(
        limited.decodeReply(limitedBody),
        /query_remote_reply_replayed/,
      );

      // A failure drains and closes its request transaction; a fresh
      // nonce/request must still execute successfully on the same server.
      const pending = pkg.queryV2PrepareRemote(
        authority,
        plan,
        invocation,
        advertisement,
        limits,
      );
      const request = pending.requestBytes();
      const secondPending = pkg.queryV2PrepareRemote(
        authority,
        plan,
        invocation,
        advertisement,
        limits,
      );
      const firstNonce = (JSON.parse(Buffer.from(request).toString("utf8")) as {
        nonce: string;
      }).nonce;
      const secondNonce = (JSON.parse(
        Buffer.from(secondPending.requestBytes()).toString("utf8"),
      ) as { nonce: string }).nonce;
      assert.match(firstNonce, /^[0-9a-f]{32}$/u);
      assert.notEqual(firstNonce, secondNonce);
      for (const invalid of [1n << 200n, -(1n << 200n)]) {
        assert.throws(
          () => pkg.queryV2PrepareRemote(
            authority,
            plan,
            invocation,
            advertisement,
            { ...limits, maxCollectionMembers: invalid },
          ),
          /query_remote_limit_invalid/,
        );
      }
      assert.throws(
        () => pkg.queryV2PrepareRemote(
          authority,
          plan,
          invocation,
          advertisement,
          { ...limits, deadlineMs: 86_400_001n },
        ),
        /query_remote_deadline_limit/,
      );
      const body = await postRemote(request);
      const remote = await pending.decodeReply(body);
      assert.equal(remote, local);
      await assert.rejects(
        pending.decodeReply(body),
        /query_remote_reply_replayed/,
      );
    } finally {
      // Wait for the server process to die before deleting the shared
      // database: a lingering connection makes the delete fail as in-use.
      const exited = new Promise((resolve) => server.once("exit", resolve));
      server.kill("SIGKILL");
      await exited;
    }
  } finally {
    db.deleteDatabase();
  }
});
