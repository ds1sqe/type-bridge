import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import { QueryV2Error, RustDatabase } from "./common/index.js";
import type { RustTransactionContext } from "../../typescript/index.js";

const fixture = JSON.parse(fs.readFileSync("../../../tests/contracts/typed-failure-scenarios.json", "utf8")) as Record<"schema" | "seed" | "purchase" | "cnt5" | "cnt9" | "paused" | "stock_read", string>;

for (const scenario of ["stc2", "cnt5", "cnt9", "paused"] as const) {
  test(`typed provider failure through Node SDK: ${scenario}`, {
    skip: !process.env["TYPE_BRIDGE_FAILURE_CONTAINER"] && "requires the dedicated owned pause-container smoke",
  }, () => {
    const address = process.env["TYPEDB_ADDRESS"];
    const container = process.env["TYPE_BRIDGE_FAILURE_CONTAINER"];
    assert.ok(address && container, "requires the isolated failure-smoke environment");
    assert.ok(container.startsWith("tb-w11b-"), "only the owned server may be paused");
    const db = RustDatabase.connect(address, `tb_w11b_node_${randomUUID().replaceAll("-", "")}`, {
      httpPort: Number(process.env["TYPEDB_HTTP_PORT"]),
    });
    const contexts: RustTransactionContext[] = [];
    db.createDatabase();
    try {
      const schema = db.transaction("schema"); contexts.push(schema);
      schema.query(fixture.schema);
      schema.commit();
      const seed = db.transaction("write"); contexts.push(seed);
      seed.query(fixture.seed); seed.commit();
      const tx = db.transaction("write"); contexts.push(tx);
      let operation: () => void;
      if (scenario === "stc2") {
        const other = db.transaction("write"); contexts.push(other);
        tx.query(fixture.purchase);
        other.query(fixture.purchase);
        tx.commit(); operation = () => other.commit();
        const read = db.transaction("read"); contexts.push(read);
        assert.deepEqual(read.query(fixture.stock_read), [{ stock: 0 }]);
      } else if (scenario === "cnt5") {
        tx.query(fixture.cnt5);
        operation = () => tx.commit();
      } else if (scenario === "cnt9") {
        operation = () => { tx.query(fixture.cnt9); };
      } else {
        tx.query(fixture.paused);
        execFileSync("docker", ["pause", container]);
        operation = () => { try { tx.commit(); } finally { execFileSync("docker", ["unpause", container]); } };
      }
      assert.throws(operation, (error: unknown) => {
        assert.ok(error instanceof QueryV2Error);
        assert.ok(error.failure);
        assert.equal(error.failure.retry_safe, scenario !== "paused");
        assert.equal(error.failure.phase, scenario === "cnt9" ? "query" : "commit");
        assert.equal(error.failure.reason, { stc2: "conflict", cnt5: "cardinality", cnt9: "unique", paused: null }[scenario]);
        assert.ok(Object.isFrozen(error.failure));
        // JSON.stringify skips the non-enumerable message and stack.
        assert.ok(!error.message.includes("SECRET_SENTINEL"));
        assert.ok(!(error.stack ?? "").includes("SECRET_SENTINEL"));
        assert.ok(!JSON.stringify(error).includes("SECRET_SENTINEL"));
        console.log(scenario, error.code, error.failure);
        return true;
      });
    } finally {
      for (const context of contexts) { try { context.close(); } catch { /* Already terminal. */ } }
      db.deleteDatabase(); db.close();
    }
  });
}
