/**
 * The shop checkout of the `writes` SDK conformance suite, in TypeScript.
 *
 * A checkout takes a whole cart at once: every line must be in stock and the
 * account must cover the total, and then every stock, the balance and the
 * order lines change together. So a cart is one plan, and one plan is one
 * TypeQL query: the checks and the writes run in the same query, and the
 * database decides. If any check fails the match is empty, nothing is
 * written, and the plan answers `Matched(0)`: check the answer. Plans staged
 * in one transaction commit together, and each sees the writes staged before
 * it. In the race step several buyers commit the same cart at once, each in
 * its own transaction; the losers get a retry-safe conflict, run again on a
 * fresh snapshot, and find the lamp gone.
 *
 * When a live target is configured, it runs the steps of the program directly
 * and over the proxy and prints one JSON report. It compiles as part of the
 * generated package (`./index.js`); see
 * tests/contracts/sdk_conformance/writes/shop.
 */
import { Plan, QueryV2Error, RustDatabase } from "@type-bridge/node";
import type { PendingPlanAnswer, QueryV2Authority, StagedTransaction } from "@type-bridge/node";
import { ProxyConnection } from "@type-bridge/node/runtime-projection";
import { Account, Item, OrderLine, schemaAuthority } from "./index.js";

// The generated package carries no Node typings; this is all the program uses.
declare const process: {
  readonly env: Readonly<Record<string, string | undefined>>;
  readonly stdout: { write(text: string): boolean };
  readonly stderr: { write(text: string): boolean };
};

type Built = Plan;
type Builder = (authority: QueryV2Authority) => Built;
/** One cart line: an item's sku and the quantity taken. */
type CartLine = readonly [string, bigint];

/** Three accounts, one given row each. */
function seedAccounts(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const accountId = plan.input("account_id", "string");
  const balance = plan.input("balance", "long");
  plan.insert(Account).field(Account.fields.accountId, accountId).field(Account.fields.balance, balance);
  const accounts = [
    ["A1", 100n],
    ["A2", 30n],
    ["R1", 100n],
  ] as const;
  for (const [key, value] of accounts) {
    plan.row([accountId.value(key), balance.value(value)]);
  }
  return plan.build(authority);
}

/** Four items with a price and a stock, one given row each. */
function seedItems(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  const sku = plan.input("sku", "string");
  const price = plan.input("price", "long");
  const stock = plan.input("stock", "long");
  plan
    .insert(Item)
    .field(Item.fields.sku, sku)
    .field(Item.fields.price, price)
    .field(Item.fields.stock, stock);
  const items = [
    ["pen", 3n, 10n],
    ["book", 40n, 1n],
    ["mug", 12n, 5n],
    ["lamp", 10n, 1n],
  ] as const;
  for (const [key, cost, count] of items) {
    plan.row([sku.value(key), price.value(cost), stock.value(count)]);
  }
  return plan.build(authority);
}

// --8<-- [start:checkout]
/** The whole cart in one query: check every line and the total, then write. */
function checkout(
  authority: QueryV2Authority,
  accountId: string,
  orderId: string,
  cart: readonly CartLine[],
): Built {
  const plan = Plan.builder();
  const buyer = plan.exact(Account);
  const balance = plan.field(buyer, Account.fields.balance);
  plan.where(plan.field(buyer, Account.fields.accountId).eq(accountId));

  const lines = cart.map(([sku, quantity]) => {
    const item = plan.exact(Item);
    const stock = plan.field(item, Item.fields.stock);
    plan.where(plan.field(item, Item.fields.sku).eq(sku)).where(stock.ge(quantity));
    const cost = plan.field(item, Item.fields.price).mul(quantity);
    return { item, stock, quantity, cost };
  });
  // Expressions are built lazily, so summing after the loop is the same plan.
  const total = plan.let(
    "total",
    lines.map((line) => line.cost).reduce((sum, cost) => sum.add(cost)),
  );
  plan.where(balance.ge(total));

  lines.forEach(({ item, stock, quantity }, index) => {
    plan.update(item, Item.fields.stock, plan.let(`stock_${index + 1}`, stock.sub(quantity)));
  });
  plan.update(buyer, Account.fields.balance, plan.let("balance_after", balance.sub(total)));
  for (const { item, quantity } of lines) {
    plan
      .insert(OrderLine)
      .field(OrderLine.fields.orderId, orderId)
      .field(OrderLine.fields.quantity, quantity)
      .player(OrderLine.roles.buyer, buyer)
      .player(OrderLine.roles.item, item);
  }
  return plan.build(authority);
}
// --8<-- [end:checkout]

/** Count the accounts whose id and balance hold the given values. */
function balanceIs(accountId: string, value: bigint): Builder {
  return (authority) => {
    const plan = Plan.builder();
    const account = plan.exact(Account);
    plan.where(plan.field(account, Account.fields.accountId).eq(accountId));
    plan.where(plan.field(account, Account.fields.balance).eq(value));
    return plan.count().build(authority);
  };
}

/** Count the items whose sku and stock hold the given values. */
function stockIs(sku: string, value: bigint): Builder {
  return (authority) => {
    const plan = Plan.builder();
    const item = plan.exact(Item);
    plan.where(plan.field(item, Item.fields.sku).eq(sku));
    plan.where(plan.field(item, Item.fields.stock).eq(value));
    return plan.count().build(authority);
  };
}

function orderLines(authority: QueryV2Authority): Built {
  const plan = Plan.builder();
  plan.exact(OrderLine);
  return plan.count().build(authority);
}

function cart(accountId: string, orderId: string, ...lines: readonly CartLine[]): Builder {
  return (authority) => checkout(authority, accountId, orderId, lines);
}

const BUILDERS: Record<string, Builder> = {
  seed_accounts: seedAccounts,
  seed_items: seedItems,
  checkout_a1: cart("A1", "O1", ["pen", 2n], ["book", 1n]),
  a1_paid: balanceIs("A1", 54n),
  book_sold_out: stockIs("book", 0n),
  pens_left: stockIs("pen", 8n),
  lines: orderLines,
  checkout_a2: cart("A2", "O2", ["mug", 1n], ["book", 1n]),
  checkout_short: cart("A2", "O3", ["mug", 3n]),
  mugs_kept: stockIs("mug", 5n),
  a2_kept: balanceIs("A2", 30n),
  checkout_a2_mug: cart("A2", "O4", ["mug", 1n]),
  checkout_a2_pens: cart("A2", "O5", ["pen", 6n]),
  a2_spent: balanceIs("A2", 0n),
  pens_final: stockIs("pen", 2n),
  mugs_final: stockIs("mug", 4n),
  race_lamp: cart("R1", "L1", ["lamp", 1n]),
  lamp_sold_out: stockIs("lamp", 0n),
  r1_paid_once: balanceIs("R1", 90n),
};
const STEPS: ReadonlyArray<readonly [string, readonly string[]]> = [
  ["seed", ["seed_accounts", "seed_items"]],
  ["checkout", ["checkout_a1"]],
  ["checked_out", ["a1_paid", "book_sold_out", "pens_left", "lines"]],
  ["sold_out", ["checkout_a2"]],
  ["short", ["checkout_short"]],
  ["unchanged", ["mugs_kept", "a2_kept", "lines"]],
  ["two_carts", ["checkout_a2_mug", "checkout_a2_pens"]],
  ["final", ["a2_spent", "pens_final", "mugs_final", "lines"]],
  ["race", ["race_lamp"]],
  ["raced", ["lamp_sold_out", "r1_paid_once", "lines"]],
];
const READS: ReadonlySet<string> = new Set(["checked_out", "unchanged", "final", "raced"]);
// The race step: this many buyers commit its one plan at once.
const BUYERS = 8;
const ATTEMPTS = 20;

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

// --8<-- [start:stage]
/** Stage `names` in one transaction, commit once, and read their answers. */
async function run(
  tx: StagedTransaction,
  plans: ReadonlyMap<string, Built>,
  names: readonly string[],
): Promise<Record<string, Answer>> {
  try {
    const pending = names.map((name) => [name, tx.stage(plans.get(name) as Built)] as const);
    await tx.commit();
    // The transaction committed; each answer is known only now.
    return Object.fromEntries(pending.map(([name, item]) => [name, answer(item)]));
  } finally {
    tx.close();
  }
}
// --8<-- [end:stage]

/** A typed commit conflict that a fresh transaction may retry. */
function retrySafeConflict(error: unknown): boolean {
  return (
    error instanceof QueryV2Error &&
    error.failure !== undefined &&
    error.failure.reason === "conflict" &&
    error.failure.retry_safe
  );
}

// --8<-- [start:race]
/**
 * BUYERS buyers commit `plan` at once; answer the sum of their matched counts.
 *
 * Each buyer stages the plan in its own transaction and waits until every
 * buyer has staged, so the commits overlap. A buyer that loses the race gets
 * a retry-safe conflict and nothing it staged is stored; it runs the whole
 * transaction again on a fresh snapshot, where the lamp is gone.
 */
async function race(
  open: () => StagedTransaction | Promise<StagedTransaction>,
  plan: Built,
): Promise<Answer> {
  let staged = 0;
  let release: () => void = () => {};
  let abort: (error: unknown) => void = () => {};
  const ready = new Promise<void>((resolve, reject) => {
    release = () => resolve();
    abort = (error) => reject(error);
  });
  // A buyer that fails aborts the wait; the failure itself is rethrown below.
  ready.catch(() => {});
  let conflicts = 0;

  async function buyer(): Promise<number> {
    try {
      for (let attempt = 0; ; attempt += 1) {
        const tx = await open();
        try {
          const bought = tx.stage(plan);
          if (attempt === 0) {
            staged += 1;
            if (staged === BUYERS) release();
            await ready;
          }
          await tx.commit();
          return answer(bought)[1];
        } catch (error) {
          if (!retrySafeConflict(error) || attempt + 1 === ATTEMPTS) throw error;
          conflicts += 1;
        } finally {
          tx.close();
        }
      }
    } catch (error) {
      abort(error);
      throw error;
    }
  }

  // Wait for every buyer, so all transactions are closed before a failure
  // propagates to the caller, which closes the database.
  const settled = await Promise.allSettled(Array.from({ length: BUYERS }, () => buyer()));
  const matched = settled.map((outcome) => {
    if (outcome.status === "rejected") throw outcome.reason;
    return outcome.value;
  });
  // Diagnostic only: how many commits lost and were retried varies by run.
  process.stderr.write(`race: ${conflicts} conflicts retried\n`);
  return ["matched", matched.reduce((sum, count) => sum + count, 0)];
}
// --8<-- [end:race]

/** Run every step; the race step runs its plan once per buyer at once. */
async function program(
  open: (kind: "read" | "write") => StagedTransaction | Promise<StagedTransaction>,
  plans: ReadonlyMap<string, Built>,
): Promise<Record<string, Record<string, Answer>>> {
  const results: Record<string, Record<string, Answer>> = {};
  for (const [step, names] of STEPS) {
    if (step === "race") {
      const [name] = names;
      if (name === undefined || names.length !== 1) {
        throw new Error("the race step stages exactly one plan");
      }
      results[step] = { [name]: await race(() => open("write"), plans.get(name) as Built) };
    } else {
      results[step] = await run(await open(READS.has(step) ? "read" : "write"), plans, names);
    }
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
