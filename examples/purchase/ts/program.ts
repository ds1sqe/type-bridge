/**
 * The purchase program of the `writes` SDK conformance suite, in TypeScript.
 *
 * It builds every plan from the generated package's tokens and authority,
 * then, when a live target is configured, runs the same program directly and
 * over the proxy: seed, stage the purchase, a touch and a batch of 50 orders
 * in one transaction, commit, and read the state back; then the journeys (a
 * put, an update answering rows, an insert whose players it inserts, a
 * delete) in one transaction, and a second read. It prints one JSON
 * report. It compiles as part of the generated package (`./index.js`); see
 * tests/contracts/sdk_conformance/writes/purchase.
 */
import { Matched, Plan, PlanError, Database } from "@type-bridge/node";
import type {
  Pending,
  PlanAnswer,
  ProviderFailure,   PlanAuthority,
  StagedTransaction,
} from "@type-bridge/node";
import { ProxyConnection } from "@type-bridge/node/runtime-projection";
import { Account, Doctor, Item, Purchase, schemaAuthority } from "./index.js";

// The generated package carries no Node typings; this is all the program uses.
declare const process: {
  readonly env: Readonly<Record<string, string | undefined>>;
  readonly stdout: { write(text: string): boolean };
};

import { purchaseExplicit, touchExplicit, doctorExplicit } from "./program_explicit.js";

type Built = Plan<PlanAnswer | Matched>;

const BATCH_ORDERS = Array.from(
  { length: 50 },
  (_, index) => `B${String(index + 1).padStart(2, "0")}`,
);

/** One item in stock, one funded account and one on-call doctor. */
function seed(authority: PlanAuthority): Built {
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
function purchase(_authority: PlanAuthority): Built {
  return Plan.build(() => {
    const item = Item.match({ itemId: "I1" }).where(Item.stock.gt(0n));
    const buyer = Account.match({ accountId: "A1" });
    buyer.where(buyer.balance.ge(item.price));
    item.set({ stock: item.stock.sub(1n) });
    buyer.set({ balance: buyer.balance.sub(item.price) });
    Purchase.insert({ orderId: "O1", buyer, item });
  });
}
// --8<-- [end:purchase]

/** Rewrite every doctor's on-call value with itself, taking its lock. */
function touch(_authority: PlanAuthority): Built {
  return Plan.build(() => {
    const doctor = Doctor.match({});
    doctor.set({ onCall: doctor.onCall });
  });
}

/** Fifty orders of the item by the account, one given row each. */
function batch(authority: PlanAuthority): Built {
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

function soldOut(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  plan.where(plan.field(item, Item.fields.itemId).eq("I1"));
  plan.where(plan.field(item, Item.fields.stock).eq(0n));
  return plan.count().build(authority);
}

function charged(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const account = plan.exact(Account);
  plan.where(plan.field(account, Account.fields.accountId).eq("A1"));
  plan.where(plan.field(account, Account.fields.balance).eq(60n));
  return plan.count().build(authority);
}

function orders(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  plan.exact(Purchase);
  return plan.count().build(authority);
}

function onCall(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const doctor = plan.exact(Doctor);
  plan.where(plan.field(doctor, Doctor.fields.onCall).eq(true));
  return plan.exists().build(authority);
}

/** Put a second doctor, off call: created, since none matches. */
function putDoctor(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  plan.put(Doctor).field(Doctor.fields.name_, "D2").field(Doctor.fields.onCall, false);
  return plan.build(authority);
}

/** Put the second doctor on call and answer its name (`Write{returns}`). */
function returns(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const doctor = plan.exact(Doctor);
  const name = plan.field(doctor, Doctor.fields.name_);
  plan.where(name.eq("D2"));
  plan.update(doctor, Doctor.fields.onCall, true);
  return plan.returning([name]).build(authority);
}

/** A purchase whose players are the item and account this write inserts. */
function refer(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const item = plan
    .insert(Item)
    .field(Item.fields.itemId, "I2")
    .field(Item.fields.stock, 3n)
    .field(Item.fields.price, 10n);
  const buyer = plan.insert(Account).field(Account.fields.accountId, "A2").field(Account.fields.balance, 50n);
  plan
    .insert(Purchase)
    .field(Purchase.fields.orderId, "R1")
    .player(Purchase.roles.buyer, buyer.thing())
    .player(Purchase.roles.item, item.thing());
  return plan.build(authority);
}

/** Delete the first purchase. */
function remove(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const order = plan.exact(Purchase);
  plan.where(plan.field(order, Purchase.fields.orderId).eq("O1"));
  return plan.delete(order).build(authority);
}

// --8<-- [start:read_back]
function readBack(authority: PlanAuthority): Built {
  const plan = Plan.builder(); // PlanBuilder
  const purchase = plan.exact(Purchase); // PlanThing
  const orderId = plan.input("order_id", "string");
  plan.where(plan.field(purchase, Purchase.fields.orderId).eq(orderId));
  plan.fetch(purchase, [Purchase.roles.buyer, Purchase.roles.item]);
  plan.row([orderId.value("R1")]);
  return plan.build(authority);
}
// --8<-- [end:read_back]

function doctorsOnCall(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const doctor = plan.exact(Doctor);
  plan.where(plan.field(doctor, Doctor.fields.onCall).eq(true));
  return plan.count().build(authority);
}

function referred(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const order = plan.exact(Purchase);
  const buyer = plan.exact(Account);
  plan.where(plan.field(order, Purchase.fields.orderId).eq("R1"));
  plan.where(plan.field(buyer, Account.fields.accountId).eq("A2"));
  plan.links(order, Purchase.roles.buyer, buyer);
  return plan.count().build(authority);
}

function removed(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  const order = plan.exact(Purchase);
  plan.where(plan.field(order, Purchase.fields.orderId).eq("O1"));
  return plan.exists().build(authority);
}

/** A third doctor, on call: staged, then rolled back. */
function ghost(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  plan.insert(Doctor).field(Doctor.fields.name_, "D3").field(Doctor.fields.onCall, true);
  return plan.build(authority);
}

/** A fourth doctor, off call: staged after the rollback and committed. */
function kept(authority: PlanAuthority): Built {
  const plan = Plan.builder();
  plan.insert(Doctor).field(Doctor.fields.name_, "D4").field(Doctor.fields.onCall, false);
  return plan.build(authority);
}

/** The doctor whose name the constraints suite inserts twice. */
function seedDoctor(_authority: PlanAuthority): Built {
  return Plan.build(() => { Doctor.insert({ name_: "D1", onCall: true }); });
}

/** A second doctor named D1: the key rejects the commit. */
function duplicateDoctor(_authority: PlanAuthority): Built {
  return Plan.build(() => { Doctor.insert({ name_: "D1", onCall: false }); });
}

const PURCHASE_BUILDERS: Record<string, (authority: PlanAuthority) => Built> = {
  seed,
  purchase,
  touch,
  batch,
  sold_out: soldOut,
  charged,
  orders,
  on_call: onCall,
  put_doctor: putDoctor,
  returns,
  refer,
  remove,
  read_back: readBack,
  doctors_on_call: doctorsOnCall,
  referred,
  removed,
  ghost,
  kept,
};
const PURCHASE_STEPS: ReadonlyArray<readonly [string, readonly string[]]> = [
  ["seed", ["seed"]],
  ["program", ["purchase", "touch", "batch"]],
  ["read", ["sold_out", "charged", "orders", "on_call"]],
  ["journeys", ["put_doctor", "returns", "refer", "remove"]],
  ["read_back", ["read_back"]],
  ["after", ["doctors_on_call", "referred", "removed"]],
  ["rollback", ["kept"]],
];
/** Plans a step stages and rolls back before staging its own. */
const PURCHASE_ROLLED_BACK: ReadonlyMap<string, readonly string[]> = new Map([
  ["rollback", ["ghost"]],
]);
const PURCHASE_READS: ReadonlySet<string> = new Set(["read", "after", "read_back"]);

interface Suite {
  readonly builders: Record<string, (authority: PlanAuthority) => Built>;
  readonly steps: ReadonlyArray<readonly [string, readonly string[]]>;
  readonly rolledBack: ReadonlyMap<string, readonly string[]>;
  readonly reads: ReadonlySet<string>;
}

/** The suite a run reports, named by TYPE_BRIDGE_SDK_WRITES_SUITE. */
const SUITES: Record<string, Suite> = {
  purchase: {
    builders: PURCHASE_BUILDERS,
    steps: PURCHASE_STEPS,
    rolledBack: PURCHASE_ROLLED_BACK,
    reads: PURCHASE_READS,
  },
  constraints: {
    builders: { seed_doctor: seedDoctor, duplicate_doctor: duplicateDoctor },
    steps: [
      ["seed", ["seed_doctor"]],
      ["reject", ["duplicate_doctor"]],
    ],
    rolledBack: new Map(),
    reads: new Set(),
  },
};
const SUITE = SUITES[process.env["TYPE_BRIDGE_SDK_WRITES_SUITE"] ?? "purchase"] as Suite;

type Answer =
  | readonly [string, number]
  | readonly ["failure", Pick<ProviderFailure, "phase" | "reason" | "code" | "retry_safe">];

/** One answer as [kind, value]; rows report their count, exists 0 or 1. */
function answer(pending: Pending<PlanAnswer | Matched>): Answer {
  const value = pending.result();
  if (value instanceof Matched) return ["matched", Number(value.count)];
  switch (value.kind) {
    case "rows":
      return ["rows", value.rows.length];
    case "documents":
      return ["documents", value.documents.length];
    case "exists":
      return ["exists", value.value ? 1 : 0];
    default:
      return [value.kind, value.value];
  }
}

/**
 * Stage `names` in one transaction, commit, and read their answers.
 * `discarded` plans are staged first and rolled back; the transaction stays
 * usable for `names`.
 */
async function run(
  tx: StagedTransaction,
  plans: ReadonlyMap<string, Built>,
  names: readonly string[],
  discarded: readonly string[],
  snapshot: Record<string, unknown>,
): Promise<Record<string, Answer>> {
  try {
    for (const name of discarded) {
      tx.stage(plans.get(name) as Built);
    }
    if (discarded.length > 0) {
      tx.rollback();
    }
    const pending = names.map((name) => [name, tx.stage(plans.get(name) as Built)] as const);
    try {
      await tx.commit();
    } catch (error) {
      // A rejected commit answers every plan with its typed failure.
      if (!(error instanceof PlanError) || error.failure === undefined) {
        throw error;
      }
      const { phase, reason, code, retry_safe } = error.failure;
      const failed: Answer = ["failure", { phase, reason, code, retry_safe }];
      return Object.fromEntries(names.map((name) => [name, failed]));
    }
    const fetched = pending.find(([name]) => name === "read_back")?.[1].result();
    if (fetched !== undefined) {
      if (fetched instanceof Matched || fetched.kind !== "documents" || fetched.documents.length !== 1) throw new Error("expected one purchase document");
      const doc = fetched.documents[0] as Record<string, unknown>;
      const scalar = (attributes: unknown, key: string): unknown => {
        const value = (attributes as Record<string, unknown>)[key];
        return Array.isArray(value) ? value[0] : value;
      };
      Object.assign(snapshot, { type: doc["_type"], order_id: scalar(doc["attributes"], "order-id"),
        buyer: scalar(doc["_role_0_attributes"], "account-id"), item: scalar(doc["_role_1_attributes"], "item-id") });
      if (doc["_role_0_type"] !== "account" || doc["_role_1_type"] !== "item" ||
          snapshot["type"] !== "purchase" || snapshot["order_id"] !== "R1" || snapshot["buyer"] !== "A2" || snapshot["item"] !== "I2") throw new Error("purchase read-back differs");
    }
    return Object.fromEntries(pending.map(([name, item]) => [name, answer(item)]));
  } finally {
    tx.close();
  }
}

async function program(
  open: (kind: "read" | "write") => StagedTransaction | Promise<StagedTransaction>,
  plans: ReadonlyMap<string, Built>,
  snapshot: Record<string, unknown>,
): Promise<Record<string, Record<string, Answer>>> {
  const results: Record<string, Record<string, Answer>> = {};
  for (const [step, names] of SUITE.steps) {
    const kind = SUITE.reads.has(step) ? "read" : "write";
    results[step] = await run(await open(kind), plans, names, SUITE.rolledBack.get(step) ?? [], snapshot);
  }
  return results;
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function main(): Promise<void> {
  const authority = schemaAuthority();
  const plans = new Map(Object.entries(SUITE.builders).map(([name, build]) => [name, build(authority)]));
  for (const [name, oracle] of Object.entries({ purchase: purchaseExplicit(authority), touch: touchExplicit(authority), seed_doctor: doctorExplicit(authority, true), duplicate_doctor: doctorExplicit(authority, false) })) {
    const plan = plans.get(name);
    if (plan !== undefined && plan.fingerprint() !== oracle.fingerprint()) throw new Error(`${name}: explicit bytes differ`);
  }

  const results: Record<string, unknown> = {};
  const snapshots: Record<string, Record<string, unknown>> = {};
  const database = process.env["TYPE_BRIDGE_SDK_WRITES_DIRECT_DATABASE"];
  const endpoint = process.env["TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT"];
  if (database) {
    const db = Database.connect(process.env["TYPEDB_ADDRESS"] ?? "localhost:1729", database, {
      httpPort: Number(process.env["TYPEDB_HTTP_PORT"] ?? "8000"),
      username: process.env["TYPEDB_USERNAME"] ?? "admin",
      password: process.env["TYPEDB_PASSWORD"] ?? "password",
    });
    try {
      snapshots["direct"] = {};
      results["direct"] = await program((kind) => db.stagedTransaction(kind), plans, snapshots["direct"]);
    } finally {
      db.close();
    }
  }
  if (endpoint) {
    const proxy = await ProxyConnection.connect(authority, endpoint);
    snapshots["proxy"] = {};
    results["proxy"] = await program((kind) => proxy.transaction(kind), plans, snapshots["proxy"]);
  }
  const report = {
    binding: "node",
    plans: Object.fromEntries([...plans].map(([name, plan]) => [name, hex(plan.canonicalBytes())])),
    results,
    ...("read_back" in SUITE.builders ? { read_back: snapshots } : {}),
    ...(plans.has("purchase") ? { explanation: plans.get("purchase")!.explain() } : {}),
  };
  process.stdout.write(`${JSON.stringify(report)}\n`);
}

await main();
