import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import * as fs from "node:fs";
import { randomUUID } from "node:crypto";
import { test } from "node:test";
import { Database, INTG_DATABASE, RawQueryError } from "./common/index.js";
import type { RawTransaction } from "../../typescript/raw-query.js";

const fixture = JSON.parse(fs.readFileSync("../../../tests/contracts/typed-failure-scenarios.json", "utf8")) as Record<"schema" | "seed" | "purchase" | "cnt5" | "cnt9" | "paused" | "stock_read", string>;

for (const scenario of ["stc2", "cnt5", "cnt9", "paused"] as const) {
  test(`raw provider failure through Node SDK: ${scenario}`, {
    skip: !process.env["TYPE_BRIDGE_FAILURE_CONTAINER"] && "requires the dedicated owned pause-container smoke",
  }, async () => {
    const address = process.env["TYPEDB_ADDRESS"];
    const container = process.env["TYPE_BRIDGE_FAILURE_CONTAINER"];
    assert.ok(address && container, "requires the isolated failure-smoke environment");
    assert.ok(container.startsWith("tb-failure-smoke-"), "only the owned server may be paused");
    await rawFailure(scenario, address, `tb_failure_node_${randomUUID().replaceAll("-", "")}`, container);
  });
}

for (const scenario of ["stc2", "cnt5", "cnt9"] as const) {
  test(`raw provider failure without pause: ${scenario}`, {
    skip: !process.env["TYPEDB_ADDRESS"] && "requires live TypeDB",
  }, async () => {
    await rawFailure(scenario, process.env["TYPEDB_ADDRESS"]!, `${INTG_DATABASE}_raw_${scenario}`);
  });
}

async function rawFailure(scenario: "stc2" | "cnt5" | "cnt9" | "paused", address: string, database: string, container?: string): Promise<void> {
    const db = Database.connect(address, database, {
      httpPort: Number(process.env["TYPEDB_HTTP_PORT"] ?? "8000"),
    });
    const contexts: RawTransaction[] = [];
    db.createDatabase();
    try {
      const schema = await db.rawTransaction("schema"); contexts.push(schema);
      await schema.executeQuery(fixture.schema);
      await schema.commit();
      const seed = await db.rawTransaction("write"); contexts.push(seed);
      await seed.executeQuery(fixture.seed); await seed.commit();
      const tx = await db.rawTransaction("write"); contexts.push(tx);
      let operation: () => Promise<unknown>;
      if (scenario === "stc2") {
        const other = await db.rawTransaction("write"); contexts.push(other);
        await tx.executeQuery(fixture.purchase);
        await other.executeQuery(fixture.purchase);
        await tx.commit(); operation = () => other.commit();
        const read = await db.rawTransaction("read"); contexts.push(read);
        assert.deepEqual((await read.executeQuery(fixture.stock_read) as { answers: unknown }).answers, [{ stock: 0 }]);
      } else if (scenario === "cnt5") {
        await tx.executeQuery(fixture.cnt5);
        operation = () => tx.commit();
      } else if (scenario === "cnt9") {
        operation = () => tx.executeQuery(fixture.cnt9);
      } else {
        await tx.executeQuery(fixture.paused);
        assert.ok(container);
        execFileSync("docker", ["pause", container]);
        operation = async () => { try { await tx.commit(); } finally { execFileSync("docker", ["unpause", container]); } };
      }
      await assert.rejects(operation, (error: unknown) => {
        assert.ok(error instanceof RawQueryError);
        assert.equal(error.code, { stc2: "STC2", cnt5: "CNT5", cnt9: "CNT9", paused: "commit_outcome_unknown" }[scenario]);
        assert.equal(error.commitOutcome, scenario === "paused" ? "unknown" : scenario === "cnt9" ? null : "definitely_aborted");
        if (scenario === "paused") assert.ok(error.providerCode);
        else assert.equal(error.providerCode, null);
        console.log(scenario, error.code, error.providerCode, error.commitOutcome);
        return true;
      });
    } finally {
      for (const context of contexts) { try { await context.close(); } catch { /* Already terminal. */ } }
      db.deleteDatabase(); db.close();
    }
}
