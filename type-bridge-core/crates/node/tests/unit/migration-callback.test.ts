import assert from "node:assert/strict";
import test from "node:test";
import { MigrationCatalog, MigrationTransaction } from "../../typescript/index.js";
import { isRegisteredRustConnection, rustTransactionHandle } from "../../typescript/runtime-handles.js";

test("migration registration wraps the same native transaction without lifecycle methods", async () => {
  type NativeCatalog = ConstructorParameters<typeof MigrationCatalog>[0];
  type Register = NativeCatalog["register"];
  let registered: Parameters<Register> | undefined;
  const catalog = new MigrationCatalog({
    register(...args: Parameters<Register>) { registered = args; },
  } as NativeCatalog);
  const answer = { answers: [{ value: 9223372036854775807n }] };
  let escaped: MigrationTransaction | undefined;
  catalog.register("derive-gender", async (tx) => {
    escaped = tx;
    assert.ok(tx instanceof MigrationTransaction);
    assert.ok(isRegisteredRustConnection(tx));
    assert.equal(await tx.executeQuery("match exact;"), answer);
    for (const method of ["commit", "rollback", "close"]) assert.equal(method in tx, false);
  });
  assert.equal(registered?.[0], "derive-gender");
  assert.equal(registered?.[2], undefined);
  const native = {
    commit() { throw new Error("engine owned"); },
    rollback() { throw new Error("engine owned"); },
    close() { throw new Error("engine owned"); },
    transactionType() { return "write" as const; },
    async executeQuery(text: string) {
      assert.equal(text, "match exact;");
      return answer;
    },
  } as Parameters<Parameters<Register>[1]>[0];
  await registered![1](native);
  assert.equal(rustTransactionHandle(escaped!), native);
});

test("migration transaction constructors cannot forge generated execution connections", () => {
  const Constructor = MigrationTransaction as unknown as new (native: object) => MigrationTransaction;
  assert.throws(() => new Constructor({}), /created by migration callbacks/);
  assert.equal(isRegisteredRustConnection(Object.create(MigrationTransaction.prototype)), false);
});
