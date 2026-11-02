import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { test } from "node:test";
import { createRequire } from "node:module";
import { QueryV2Authority, Database } from "./common/index.js";

const requirePackage = createRequire(import.meta.url);
const { ProxyConnection, QueryExecutionResourceLimits } = requirePackage(
  path.join(process.env["TYPE_BRIDGE_NODE_PACKAGE_ROOT"] ?? process.cwd(), "dist/runtime-projection.js"),
) as typeof import("../../typescript/runtime-projection.js");
const fixtureDir = process.env["TB_DEADLINE_FIXTURE_DIR"];

for (const transport of ["direct", "proxy"] as const) {
  test(`deadline none and typed timeout: ${transport}`, {
    skip: !fixtureDir && "requires the SDK deadline live fixture",
    timeout: 180_000,
  }, async () => {
    const live = JSON.parse(fs.readFileSync(path.join(fixtureDir!, "fixture.json"), "utf8"));
    const entry = live.entries[`node-${transport}`];
    const declared = Buffer.from(live.declared_hex, "hex");
    const plan = Buffer.from(live.purchase_hex, "hex");
    const authority = new QueryV2Authority(declared, entry.scope, live.profile);
    for (const [timeout, delay] of [[1_000n, 2_000], [0n, 61_000]] as const) {
      const db = transport === "direct"
        ? Database.connect(entry.address, entry.database, { httpPort: 43131 }) : undefined;
      const tx = db ? db.stagedTransaction(undefined, {
        timeoutMilliseconds: timeout, collectPhaseTimestamps: true,
      }) : await (await ProxyConnection.connect(authority, entry.endpoint, {
        answerLimits: new QueryExecutionResourceLimits({ timeoutMilliseconds: timeout, collectPhaseTimestamps: true }),
      })).transaction();
      try {
        const pending = tx.stagePlan(plan, declared);
        fs.writeFileSync(entry.arm, String(delay));
        const started = performance.now();
        if (timeout !== 0n) {
          await assert.rejects(tx.commit(), (error: any) => {
            assert.equal(error.code, "transaction_deadline_exceeded");
            const stamps = error.details.execution_timeline.value.stamps;
            assert.equal(stamps[0].phase, "fired");
            assert.equal(stamps.at(-1).phase, "failed");
            assert.equal(stamps.at(-1).failing_phase, transport === "direct" ? "fired" : "accepted");
            assert.equal(stamps.some((stamp: any) => stamp.phase === "db_write"), false);
            return true;
          });
          assert.throws(() => pending.get());
          await new Promise((resolve) => setTimeout(resolve, 2100));
        } else {
          await tx.commit();
          assert.ok(performance.now() - started > 60_000);
          assert.deepEqual(pending.get(), { kind: "matched", value: 1 });
        }
        assert.equal(fs.existsSync(entry.arm), false, "relay must actually delay the call");
      } finally {
        fs.rmSync(entry.arm, { force: true });
        tx.close();
        db?.close();
      }
    }
  });
}
