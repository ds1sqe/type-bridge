/**
 * The purchase program of the `writes` SDK conformance suite, in TypeScript.
 *
 * It builds every plan from the generated package's tokens and authority,
 * then, when a live target is configured, runs the same program directly and
 * over the proxy: seed, stage the purchase, a touch and a batch of 50 orders
 * in one transaction, commit, and read the state back. It prints one JSON
 * report. It compiles as part of the generated package (`./index.js`); see
 * tests/contracts/sdk_conformance/writes/purchase.
 */
import { Plan, RustDatabase } from "@type-bridge/node";
import type { PendingPlanAnswer, QueryV2Authority, StagedTransaction } from "@type-bridge/node";
import { ProxyConnection } from "@type-bridge/node/runtime-projection";
import { Account, Doctor, Item, Purchase, schemaAuthority } from "./index.js";

// The generated package carries no Node typings; this is all the program uses.
declare const process: {
  readonly env: Readonly<Record<string, string | undefined>>;
  readonly stdout: { write(text: string): boolean };
};

type Built = Plan;

const BATCH_ORDERS = Array.from(
  { length: 50 },
  (_, index) => `B${String(index + 1).padStart(2, "0")}`,
);

/** One item in stock, one funded account and one on-call doctor. */
function seed(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const itemId = plan.input("item_id", "string");
  const stock = plan.input("stock", "long");
  const price = plan.input("price", "long");
  const accountId = plan.input("account_id", "string");
  const balance = plan.input("balance", "long");
  const doctor = plan.input("doctor", "string");
  const onCall = plan.input("on_call", "boolean");
  plan
    .insert(Item)
    .field(Item.fields.itemId, itemId)
    .field(Item.fields.stock, stock)
    .field(Item.fields.price, price);
  plan.insert(Account).field(Account.fields.accountId, accountId).field(Account.fields.balance, balance);
  plan.insert(Doctor).field(Doctor.fields.name_, doctor).field(Doctor.fields.onCall, onCall);
  plan.row([
    itemId.value("I1"),
    stock.value(1n),
    price.value(40n),
    accountId.value("A1"),
    balance.value(100n),
    doctor.value("D1"),
    onCall.value(true),
  ]);
  return plan.build(authority);
}

// --8<-- [start:purchase]
/** The one-query purchase: take one item and its price, record the order. */
function purchase(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const buyer = plan.exact(Account);
  const orderInput = plan.input("order_id", "string");
  const itemInput = plan.input("item_id", "string");
  const accountInput = plan.input("account_id", "string");
  const itemKey = plan.field(item, Item.fields.itemId);
  const buyerKey = plan.field(buyer, Account.fields.accountId);
  const stock = plan.field(item, Item.fields.stock);
  const price = plan.field(item, Item.fields.price);
  const balance = plan.field(buyer, Account.fields.balance);
  plan
    .where(itemKey.eq(itemInput))
    .where(buyerKey.eq(accountInput))
    .where(stock.gt(0n))
    .where(balance.ge(price));
  plan.update(item, Item.fields.stock, plan.let("next_stock", stock.sub(1n)));
  plan.update(buyer, Account.fields.balance, plan.let("next_balance", balance.sub(price)));
  plan
    .insert(Purchase)
    .field(Purchase.fields.orderId, orderInput)
    .player(Purchase.roles.buyer, buyer)
    .player(Purchase.roles.item, item);
  plan.row([orderInput.value("O1"), itemInput.value("I1"), accountInput.value("A1")]);
  return plan.build(authority);
}
// --8<-- [end:purchase]

/** Rewrite every doctor's on-call value with itself, taking its lock. */
function touch(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const doctor = plan.exact(Doctor);
  plan.update(doctor, Doctor.fields.onCall, plan.field(doctor, Doctor.fields.onCall));
  return plan.build(authority);
}

/** Fifty orders of the item by the account, one given row each. */
function batch(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const buyer = plan.exact(Account);
  const orderInput = plan.input("order_id", "string");
  plan.where(plan.field(item, Item.fields.itemId).eq("I1"));
  plan.where(plan.field(buyer, Account.fields.accountId).eq("A1"));
  plan
    .insert(Purchase)
    .field(Purchase.fields.orderId, orderInput)
    .player(Purchase.roles.buyer, buyer)
    .player(Purchase.roles.item, item);
  for (const order of BATCH_ORDERS) plan.row([orderInput.value(order)]);
  return plan.build(authority);
}

function soldOut(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  plan.where(plan.field(item, Item.fields.itemId).eq("I1"));
  plan.where(plan.field(item, Item.fields.stock).eq(0n));
  return plan.count().build(authority);
}

function charged(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const account = plan.exact(Account);
  plan.where(plan.field(account, Account.fields.accountId).eq("A1"));
  plan.where(plan.field(account, Account.fields.balance).eq(60n));
  return plan.count().build(authority);
}

function orders(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  plan.exact(Purchase);
  return plan.count().build(authority);
}

function onCall(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const doctor = plan.exact(Doctor);
  plan.where(plan.field(doctor, Doctor.fields.onCall).eq(true));
  return plan.exists().build(authority);
}

const BUILDERS: Record<string, (authority: QueryV2Authority) => Built> = {
  seed,
  purchase,
  touch,
  batch,
  sold_out: soldOut,
  charged,
  orders,
  on_call: onCall,
};
const STEPS: ReadonlyArray<readonly [string, readonly string[]]> = [
  ["seed", ["seed"]],
  ["program", ["purchase", "touch", "batch"]],
  ["read", ["sold_out", "charged", "orders", "on_call"]],
];

type Answer = readonly [string, number];

/** One answer as [kind, value]; rows report their count, exists 0 or 1. */
function answer(pending: PendingPlanAnswer): Answer {
  const value = pending.get();
  switch (value.kind) {
    case "rows":
      return ["rows", value.rows.length];
    case "exists":
      return ["exists", value.value ? 1 : 0];
    default:
      return [value.kind, value.value];
  }
}

/** Stage `names` in one transaction, commit, and read their answers. */
async function run(
  tx: StagedTransaction,
  plans: ReadonlyMap<string, Built>,
  names: readonly string[],
): Promise<Record<string, Answer>> {
  try {
    const pending = names.map((name) => [name, tx.stage(plans.get(name) as Built)] as const);
    await tx.commit();
    return Object.fromEntries(pending.map(([name, item]) => [name, answer(item)]));
  } finally {
    tx.close();
  }
}

async function program(
  open: (kind: "read" | "write") => StagedTransaction | Promise<StagedTransaction>,
  plans: ReadonlyMap<string, Built>,
): Promise<Record<string, Record<string, Answer>>> {
  const results: Record<string, Record<string, Answer>> = {};
  for (const [step, names] of STEPS) {
    results[step] = await run(await open(step === "read" ? "read" : "write"), plans, names);
  }
  return results;
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function main(): Promise<void> {
  const authority = schemaAuthority();
  const plans = new Map(Object.entries(BUILDERS).map(([name, build]) => [name, build(authority)]));
  const results: Record<string, unknown> = {};
  const database = process.env["TYPE_BRIDGE_SDK_WRITES_DIRECT_DATABASE"];
  const endpoint = process.env["TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT"];
  if (database) {
    const db = RustDatabase.connect(process.env["TYPEDB_ADDRESS"] ?? "localhost:1729", database, {
      httpPort: Number(process.env["TYPEDB_HTTP_PORT"] ?? "8000"),
      username: process.env["TYPEDB_USERNAME"] ?? "admin",
      password: process.env["TYPEDB_PASSWORD"] ?? "password",
    });
    try {
      results["direct"] = await program((kind) => db.stagedTransaction(kind), plans);
    } finally {
      db.close();
    }
  }
  if (endpoint) {
    const proxy = await ProxyConnection.connect(authority, endpoint);
    results["proxy"] = await program((kind) => proxy.transaction(kind), plans);
  }
  const report = {
    binding: "node",
    plans: Object.fromEntries([...plans].map(([name, plan]) => [name, hex(plan.canonicalBytes())])),
    results,
  };
  process.stdout.write(`${JSON.stringify(report)}\n`);
}

await main();
