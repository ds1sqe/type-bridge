import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { test } from "node:test";
import { QueryV2Authority, RustDatabase } from "./common/index.js";
import type { PlanAnswer, StagedTransaction } from "../../typescript/index.js";
import { createRequire } from "node:module";

// Load the built package like the shared helpers do, so classes are shared.
const { ProxyConnection } = createRequire(import.meta.url)(
  path.join(process.env["TYPE_BRIDGE_NODE_PACKAGE_ROOT"] ?? process.cwd(), "dist/runtime-projection.js"),
) as typeof import("../../typescript/runtime-projection.js");

// The live fixture is `sdk_staged_fixture_serves_until_stopped` in
// crates/server/tests/v3_staged_integration_tests.rs; it writes
// `fixture.json` into TB_STAGED_FIXTURE_DIR.
interface Fixture {
  readonly direct_database: string;
  readonly profile: string;
  readonly declared_hex: string;
  readonly plans_hex: Record<"purchase" | "count" | "exists", string>;
  readonly proxies: Record<"purchase" | "retry" | "refused", {
    readonly database: string;
    readonly endpoint: string;
    readonly scope: string;
  }>;
}

const fixtureDir = process.env["TB_STAGED_FIXTURE_DIR"];
const skip = !fixtureDir && "requires the staged proxy live fixture (TB_STAGED_FIXTURE_DIR)";

function fixture(): Fixture {
  return JSON.parse(
    fs.readFileSync(path.join(fixtureDir as string, "fixture.json"), "utf8"),
  ) as Fixture;
}

function bytes(hex: string): Uint8Array {
  return Buffer.from(hex, "hex");
}

async function proxyTransaction(
  live: Fixture,
  role: "purchase" | "retry" | "refused",
  headers: [string, string][] = [],
): Promise<StagedTransaction> {
  const proxy = live.proxies[role];
  const authority = new QueryV2Authority(bytes(live.declared_hex), proxy.scope, live.profile);
  const connection = await ProxyConnection.connect(authority, proxy.endpoint, { headers });
  return connection.transaction();
}

function stagePurchaseProgram(live: Fixture, tx: StagedTransaction): (() => PlanAnswer)[] {
  const declared = bytes(live.declared_hex);
  return (["purchase", "count", "exists"] as const).map((name) => {
    const pending = tx.stagePlan(bytes(live.plans_hex[name]), declared);
    return () => pending.get();
  });
}

async function failure(commit: Promise<void>): Promise<{ code: string; message: string }> {
  try {
    await commit;
  } catch (error) {
    return error as { code: string; message: string };
  }
  assert.fail("commit unexpectedly succeeded");
}

async function failureCode(commit: Promise<void>): Promise<string> {
  return (await failure(commit)).code;
}

test("a staged purchase over the proxy answers exactly as direct", { skip }, async () => {
  const live = fixture();
  const db = RustDatabase.connect(process.env["TYPEDB_ADDRESS"] ?? "localhost:1730", live.direct_database, {
    httpPort: Number(process.env["TYPEDB_HTTP_PORT"] ?? "8000"),
  });
  const direct = db.stagedTransaction();
  const directAnswers = stagePurchaseProgram(live, direct);
  await direct.commit();
  const remote = await proxyTransaction(live, "purchase");
  const remoteAnswers = stagePurchaseProgram(live, remote);
  assert.throws(() => remoteAnswers[0]?.(), /not committed/i);
  await remote.commit();
  assert.deepEqual(remoteAnswers.map((get) => get()), directAnswers.map((get) => get()));
  assert.deepEqual(remoteAnswers[0]?.(), { kind: "matched", value: 1 });
  // A direct transaction has no unknown outcome to retry.
  await assert.rejects(direct.commitRetry());
  db.close();
});

test("a lost reply stays unknown and commitRetry applies the purchase once", { skip }, async () => {
  const live = fixture();
  const tx = await proxyTransaction(live, "retry", [["x-tb-drop-reply", "1"]]);
  const [purchase] = stagePurchaseProgram(live, tx);
  assert.equal(await failureCode(tx.commit()), "commit_outcome_unknown");
  await tx.commitRetry();
  assert.deepEqual(purchase?.(), { kind: "matched", value: 1 });
  // The retry state is spent once the outcome is known.
  await assert.rejects(tx.commitRetry());
});

test("an unauthenticated staged write is refused with the typed failure", { skip }, async () => {
  const live = fixture();
  const tx = await proxyTransaction(live, "refused");
  const [purchase] = stagePurchaseProgram(live, tx);
  const refused = await failure(tx.commit());
  assert.equal(refused.code, "staged_write_not_authorized");
  assert.throws(() => purchase?.());
  await assert.rejects(tx.commitRetry());
});
