/** Typed shop walkthrough; the runner owns schema setup and database cleanup. */
import { Database, Plan, setDebugListener } from "@type-bridge/node";
import type { DebugEvent } from "@type-bridge/node";
import { Account, Document, Item, Memo, OrderLine, OrderId, Price, Quantity, Report, Sku, Stock } from "./index.js";
import type { CompleteFacet } from "./index.js";

declare const process: {
  readonly env: Readonly<Record<string, string | undefined>>;
  readonly stdout: { write(text: string): boolean };
};

let captured: DebugEvent["fields"][] = [];
setDebugListener(event => {
  if (event.name === "type_bridge.io.provider.statement") captured.push(event.fields);
});

async function finish(expected: number): Promise<DebugEvent["fields"][]> {
  const deadline = Date.now() + 10_000;
  while (captured.length < expected && Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  // Let the host queue settle so a late duplicate fails this case.
  if (captured.length === expected) await new Promise(resolve => setTimeout(resolve, 50));
  if (captured.length !== expected) throw new Error(`expected ${expected} provider statements, captured ${captured.length}`);
  const events = captured;
  captured = [];
  return events;
}

function emit(name: string, step: string, value: unknown): void {
  process.stdout.write(`${JSON.stringify({ case: name, step, value })}\n`);
}

function modelRecord(item: CompleteFacet<string>): unknown {
  const identity = { iid: item.iid?.toString() ?? null, deleted: item.deleted };
  switch (item.__typebridgeModel) {
    case Account.typeKey: {
      const value = item as Account;
      return { ...identity, type: "account", account_id: value.accountId.value, balance: value.balance === null ? null : Number(value.balance.value) };
    }
    case Item.typeKey: {
      const value = item as Item;
      return { ...identity, type: "item", sku: value.sku.value, price: value.price === null ? null : Number(value.price.value), stock: value.stock === null ? null : Number(value.stock.value) };
    }
    case OrderLine.typeKey: {
      const value = item as OrderLine;
      return { ...identity, type: "order-line", order_id: value.orderId?.value ?? null, quantity: value.quantity === null ? null : Number(value.quantity.value),
        buyer: value.buyer === null ? null : { type: "account", iid: value.buyer.iid?.toString() ?? null },
        item: value.item === null ? null : { type: "item", iid: value.item.iid?.toString() ?? null } };
    }
    case Report.typeKey: case Memo.typeKey: {
      const value = item as Report | Memo;
      return { ...identity, type: item.__typebridgeModel === Report.typeKey ? "report" : "memo", doc_id: value.docId.value, status: value.status.value };
    }
    default: throw new Error(`unexpected model ${item.__typebridgeModel}`);
  }
}

function trace(name: string, events: DebugEvent["fields"][]): void {
  emit(name, "plan", { source: "TRACE", fingerprints: events.map(event => event["fingerprint"] ?? "") });
  emit(name, "explain", { source: "TRACE", statements: events.map(event => event["statement"]) });
}

async function show<R extends CompleteFacet<string>>(db: Database, name: string, call: string, plan: Plan<R[]>, statements = 1, split = false): Promise<R[]> {
  emit(name, "call", call);
  const explanation = split ? null : plan.explain();
  if (explanation !== null) {
    const shape = JSON.parse(new TextDecoder().decode(plan.canonicalBytes()));
    delete shape.given_rows;
    emit(name, "plan", { source: "canonical_bytes", plan: shape });
    emit(name, "explain", { source: "explain", ...explanation });
  }
  const tx = db.stagedTransaction();
  try {
    const pending = tx.stage(plan);
    await tx.commit();
    const events = await finish(statements);
    if (explanation === null) trace(name, events);
    else if (JSON.stringify(explanation.statements) !== JSON.stringify(events.map(event => event["statement"]))) {
      throw new Error(`${name}: explain differs from provider statements`);
    }
    emit(name, "raw", pending.documents());
    const result = pending.result();
    emit(name, "models", result.map(modelRecord));
    return result;
  } finally { tx.close(); }
}

async function manager<R extends CompleteFacet<string>>(name: string, call: string, execute: () => readonly R[] | { readonly items: readonly R[] }, statements = 1): Promise<readonly R[]> {
  emit(name, "call", call);
  const result = execute();
  trace(name, await finish(statements));
  const values = "items" in result ? result.items : result;
  emit(name, "raw", "raw documents: not exposed by manager result");
  emit(name, "models", values.map(modelRecord));
  return values;
}

function purchase(): Plan<OrderLine[]> {
  return Plan.build(() => {
    const item = Item.match("I1").where(Item.stock.gt(0n));
    let buyer = Account.match("A1");
    const balance = buyer.balance;
    buyer = buyer.where(balance.ge(item.price));
    item.set({ stock: item.stock.sub(1n) });
    buyer.set({ balance: buyer.balance.sub(item.price) });
    return OrderLine.insert({ orderId: "O1", quantity: 1n, buyer, item });
  });
}

const purchaseCall = `Plan.build(() => {
  const item = Item.match("I1").where(Item.stock.gt(0n));
  let buyer = Account.match("A1");
  const balance = buyer.balance;
  buyer = buyer.where(balance.ge(item.price));
  item.set({ stock: item.stock.sub(1n) });
  buyer.set({ balance: buyer.balance.sub(item.price) });
  return OrderLine.insert({ orderId: "O1", quantity: 1n, buyer, item });
});`;

async function main(): Promise<void> {
  const db = Database.connect(process.env.TYPEDB_ADDRESS!, process.env.TYPE_BRIDGE_WALKTHROUGH_DATABASE!, {
    username: process.env.TYPEDB_USERNAME ?? "admin", password: process.env.TYPEDB_PASSWORD ?? "password",
    httpPort: Number(process.env.TYPEDB_HTTP_PORT),
  });
  try {
    const seed = db.stagedTransaction();
    seed.stage(Plan.build(() => Account.insert({ accountId: "A1", balance: 10n })));
    seed.stage(Plan.build(() => Item.insert({ sku: "I1", price: 3n, stock: 5n })));
    seed.stage(Plan.build(() => Report.insert({ docId: "R1", status: "archived" })));
    seed.stage(Plan.build(() => Memo.insert({ docId: "M1", status: "archived" })));
    await seed.commit();
    seed.close();
    await finish(4);

    const bought = await show(db, "purchase", purchaseCall, purchase());
    if (bought.length !== 1 || bought[0]!.orderId?.value !== "O1" || bought[0]!.buyer?.balance?.value !== 7n || bought[0]!.item?.stock?.value !== 4n) {
      throw new Error("purchase result mismatch");
    }
    const gone = Plan.build(() => Document.subtypes().where(Document.status.eq("archived")).deleteReturning());
    const removed = await show(db, "delete_subtypes", 'Plan.build(() => Document.subtypes().where(Document.status.eq("archived")).deleteReturning())', gone, 2);
    if (removed.length !== 2 || !removed.every(item => item.deleted) || new Set(removed.map(item => item.__typebridgeModel)).size !== 2) {
      throw new Error("subtype delete result mismatch");
    }
    const batch = Plan.build(() => Item.insertMany(Array.from({ length: 10_000 }, (_, n) => ({ sku: `B${n}`, price: 3n, stock: 10n }))));
    const items = await show(db, "bulk_insert", 'Plan.build(() => Item.insertMany(Array.from({ length: 10_000 }, (_, n) => ({ sku: `B${n}`, price: 3n, stock: 10n }))))', batch, 3, true);
    if (items.length !== 10_000 || items.some((item, n) => item.sku.value !== `B${n}`)) throw new Error("bulk order mismatch");

    const itemManager = Item.manager(db);
    const made = await manager("insert", 'Item.manager(db).insert(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(5n) }))', () => itemManager.insert(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(5n) })));
    const iid = made[0]!.iid;
    if (iid === null) throw new Error("insert missing IID");
    await manager("put", 'Item.manager(db).put(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(6n) }))', () => itemManager.put(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(6n) })));
    const found = await manager("typed_iid", 'Item.manager(db).filter({ iid }).all()', () => itemManager.filter({ iid }).all());
    if (found.length !== 1 || found[0]!.iid?.toString() !== iid.toString()) throw new Error("IID filter mismatch");
    await manager("filtered_fetch", 'Item.manager(db).filter({ sku: Sku.create("M1") }).all()', () => itemManager.filter({ sku: Sku.create("M1") }).all());
    const updated = await manager("update", 'Item.manager(db).filter({ iid }).update(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(8n) }))', () => itemManager.filter({ iid }).update(Item.create({ sku: Sku.create("M1"), price: Price.create(3n), stock: Stock.create(8n) })));
    if (updated[0]!.stock?.value !== 8n) throw new Error("update result mismatch");
    await manager("insert_many", 'OrderLine.manager(db).insertMany(["L1", "L2"].map(key => OrderLine.create({ orderId: OrderId.create(key), quantity: Quantity.create(1n), buyer: bought[0].buyer, item: bought[0].item })))',
      () => OrderLine.manager(db).insertMany(["L1", "L2"].map(key => OrderLine.create({ orderId: OrderId.create(key), quantity: Quantity.create(1n), buyer: bought[0]!.buyer, item: bought[0]!.item }))));
    const deleted = await manager("delete_returning", 'Item.manager(db).deleteReturning(iid)', () => itemManager.deleteReturning(iid), 2);
    if (!deleted[0]!.deleted) throw new Error("delete result mismatch");

    const newAccount = Plan.build(() => Account.insert({ accountId: "A2", balance: 20n }));
    const newItem = Plan.build(() => Item.insert({ price: 4n, sku: "I2", stock: 3n }));
    const newOrder = Plan.build(() => {
      const buyer = Account.match("A2");
      const item = Item.match("I2");
      return OrderLine.insert({ orderId: "O2", quantity: 1n, buyer, item });
    });
    const plans = [newAccount, newItem, newOrder];
    emit("staged_transaction", "call", `const newAccount = Plan.build(() => Account.insert({ accountId: "A2", balance: 20n }));
const newItem = Plan.build(() => Item.insert({ price: 4n, sku: "I2", stock: 3n }));
const newOrder = Plan.build(() => {
  const buyer = Account.match("A2");
  const item = Item.match("I2");
  return OrderLine.insert({ orderId: "O2", quantity: 1n, buyer, item });
});
const tx = db.stagedTransaction();
const pending = [tx.stage(newAccount), tx.stage(newItem), tx.stage(newOrder)];
await tx.commit();`);
    const shapes = plans.map(plan => {
      const shape = JSON.parse(new TextDecoder().decode(plan.canonicalBytes()));
      delete shape.given_rows;
      return shape;
    });
    const explanations = plans.map(plan => plan.explain());
    emit("staged_transaction", "plan", { source: "canonical_bytes", plans: shapes });
    emit("staged_transaction", "explain", { source: "explain", plans: explanations });
    const tx = db.stagedTransaction();
    try {
      const pending = [tx.stage(newAccount), tx.stage(newItem), tx.stage(newOrder)];
      await tx.commit();
      const events = await finish(3);
      if (JSON.stringify(explanations.flatMap(explanation => explanation.statements)) !== JSON.stringify(events.map(event => event["statement"]))) throw new Error("staged statement mismatch");
      emit("staged_transaction", "raw", pending.flatMap(value => value.documents()));
      const values: CompleteFacet<string>[] = [];
      for (const value of pending) values.push(...value.result());
      emit("staged_transaction", "models", values.map(modelRecord));
      const order = pending[2]!.result()[0] as OrderLine;
      if (order.buyer?.accountId.value !== "A2" || order.item?.sku.value !== "I2") throw new Error("staged dependency mismatch");
    } finally { tx.close(); }

    emit("raw_unchecked", "call", 'db.executeQuery("match $x isa order-line; reduce $n = count;")');
    emit("raw_unchecked", "unchecked", "no plan, no hydration");
    emit("raw_unchecked", "raw", await db.executeQuery("match $x isa order-line; reduce $n = count;"));
  } finally { db.close(); setDebugListener(null); }
}

await main();
