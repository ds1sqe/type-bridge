import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { createRequire } from "node:module";
import { test } from "node:test";
import type {
  PlanFieldToken,
  PlanModelToken,
  PlanRoleToken,
} from "../../typescript/public.js";

// Load the built package like the shared helpers do, so classes are shared.
const packageRoot = process.env["TYPE_BRIDGE_NODE_PACKAGE_ROOT"] ?? process.cwd();
const requirePackage = createRequire(import.meta.url);
const { Plan, QueryV2Authority, QueryV2Error, RustDatabase } = requirePackage(packageRoot) as typeof import(
  "../../typescript/public.js"
);
const { ProxyConnection } = requirePackage(
  path.join(packageRoot, "dist/runtime-projection.js"),
) as typeof import("../../typescript/runtime-projection.js");

type Plan = InstanceType<typeof Plan>;
type QueryV2Authority = InstanceType<typeof QueryV2Authority>;

// A fresh instance of the live fixture `sdk_staged_fixture_serves_until_stopped`
// (crates/server/tests/v3_staged_integration_tests.rs), separate from the one
// staged-proxy.test.ts consumes: every purchase here needs unbought stock.
interface Fixture {
  readonly direct_database: string;
  readonly profile: string;
  readonly declared_hex: string;
  readonly proxies: Record<"purchase" | "retry" | "refused", {
    readonly database: string;
    readonly endpoint: string;
    readonly scope: string;
  }>;
}

const fixtureDir = process.env["TB_STAGED_BUILDER_FIXTURE_DIR"];
const skip = !fixtureDir
  && "requires a fresh staged proxy live fixture (TB_STAGED_BUILDER_FIXTURE_DIR)";

function fixture(): Fixture {
  return JSON.parse(
    fs.readFileSync(path.join(fixtureDir as string, "fixture.json"), "utf8"),
  ) as Fixture;
}

function connect(database: string) {
  return RustDatabase.connect(process.env["TYPEDB_ADDRESS"] ?? "localhost:1730", database, {
    httpPort: Number(process.env["TYPEDB_HTTP_PORT"] ?? "8000"),
  });
}

// The generated token shapes for the race schema.
function model<Id extends string>(kind: "entity" | "relation", label: string): PlanModelToken<Id> {
  return { typeKey: JSON.stringify({ kind, label }) as Id };
}
function field<Id extends string>(owner: PlanModelToken<Id>, attribute: string): PlanFieldToken<Id> {
  return {
    kind: "field",
    owner: owner.typeKey,
    metadata: { declaring_id: { attribute, owner: JSON.parse(owner.typeKey) as unknown } },
  };
}
function role<Id extends string>(owner: PlanModelToken<Id>, label: string): PlanRoleToken<Id> {
  return { kind: "role", owner: owner.typeKey, role: JSON.stringify({ declaring_relation: "purchase", label }) };
}
const Item = model<"item">("entity", "item");
const Account = model<"account">("entity", "account");
const Purchase = model<"purchase">("relation", "purchase");

/** The one-query purchase, built in TypeScript. */
function purchase(authority: QueryV2Authority, order: string): Plan {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const buyer = plan.exact(Account);
  const orderInput = plan.input("order_id", "string");
  const itemInput = plan.input("item_id", "string");
  const accountInput = plan.input("account_id", "string");
  const itemKey = plan.field(item, field(Item, "item-id"));
  const buyerKey = plan.field(buyer, field(Account, "account-id"));
  const stock = plan.field(item, field(Item, "stock"));
  const price = plan.field(item, field(Item, "price"));
  const balance = plan.field(buyer, field(Account, "balance"));
  plan
    .where(itemKey.eq(itemInput))
    .where(buyerKey.eq(accountInput))
    .where(stock.gt(0n))
    .where(balance.ge(price));
  plan.update(item, field(Item, "stock"), plan.let("next_stock", stock.sub(1n)));
  plan.update(buyer, field(Account, "balance"), plan.let("next_balance", balance.sub(price)));
  plan
    .insert(Purchase)
    .field(field(Purchase, "order-id"), orderInput)
    .player(role(Purchase, "buyer"), buyer)
    .player(role(Purchase, "item"), item);
  plan.row([orderInput.value(order), itemInput.value("I1"), accountInput.value("A1")]);
  return plan.build(authority);
}

function authorityFor(live: Fixture): QueryV2Authority {
  return new QueryV2Authority(
    Buffer.from(live.declared_hex, "hex"),
    live.proxies.purchase.scope,
    live.profile,
  );
}

test("the TypeScript purchase commits directly and over the proxy with the same answers", { skip }, async () => {
  const live = fixture();
  const authority = authorityFor(live);
  const plans = [purchase(authority, "O1"), purchase(authority, "O2")];

  const db = connect(live.direct_database);
  const direct = db.stagedTransaction();
  const directAnswers = plans.map((plan) => direct.stage(plan));
  assert.equal(direct.hasUnknownOutcome(), false);
  await direct.commit();

  const connection = await ProxyConnection.connect(authority, live.proxies.purchase.endpoint);
  const remote = await connection.transaction();
  const remoteAnswers = plans.map((plan) => remote.stage(plan));
  assert.throws(() => remoteAnswers[0]?.get(), /not committed/i);
  await remote.commit();

  const expected = [{ kind: "matched", value: 1 }, { kind: "matched", value: 0 }];
  assert.deepEqual(directAnswers.map((answer) => answer.get()), expected);
  assert.deepEqual(remoteAnswers.map((answer) => answer.get()), expected);

  // Dispose closes without committing; a closed transaction refuses work.
  await direct[Symbol.asyncDispose]();
  await direct[Symbol.asyncDispose]();
  assert.throws(() => direct.stage(plans[0] as Plan));
  await remote[Symbol.asyncDispose]();
  db.close();
});

test("await using closes without committing", { skip }, async () => {
  const live = fixture();
  const db = connect(live.proxies.retry.database);
  const plan = purchase(authorityFor(live), "O-disposed");
  {
    await using tx = db.stagedTransaction();
    const answer = tx.stage(plan);
    assert.throws(() => answer.get(), /not committed/i);
  }
  // Nothing was bought: the next purchase still finds the stock.
  const tx = db.stagedTransaction("write");
  const answer = tx.stage(purchase(authorityFor(live), "O-after"));
  await tx.commit();
  assert.deepEqual(answer.get(), { kind: "matched", value: 1 });
  tx.close();
  db.close();
});

test("a read transaction refuses a write plan", { skip }, () => {
  const live = fixture();
  const db = connect(live.direct_database);
  const tx = db.stagedTransaction("read");
  // Lifecycle refusals are plain errors, as in the Rust SDK.
  assert.throws(() => tx.stage(purchase(authorityFor(live), "O-read")), /Read transaction cannot stage a write/);
  tx.close();
  db.close();
});

test("two TypeScript transactions race for the last item and never oversell", { skip }, async () => {
  const live = fixture();
  const authority = authorityFor(live);
  // The refused proxy's database is seeded, and no test here writes to it.
  const db = connect(live.proxies.refused.database);
  const racer = async (order: string) => {
    const tx = db.stagedTransaction();
    const answer = tx.stage(purchase(authority, order));
    try {
      await tx.commit();
      return answer.get();
    } catch (error) {
      return error;
    } finally {
      await tx[Symbol.asyncDispose]();
    }
  };
  const outcomes = await Promise.all([racer("R1"), racer("R2")]);
  const bought = outcomes.filter(
    (outcome) => JSON.stringify(outcome) === JSON.stringify({ kind: "matched", value: 1 }),
  );
  assert.equal(bought.length, 1, `exactly one racer buys: ${JSON.stringify(outcomes)}`);
  for (const outcome of outcomes) {
    if (outcome instanceof Error) {
      assert.ok(outcome instanceof QueryV2Error, String(outcome));
      assert.ok(outcome.failure, `the loser carries a typed failure: ${outcome.code}`);
    } else {
      assert.equal((outcome as { kind: string }).kind, "matched");
    }
  }
  db.close();
});
