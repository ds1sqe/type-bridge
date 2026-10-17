#!/usr/bin/env node
/**
 * The major TypeBridge pipelines through the Node SDK, against a live TypeDB.
 *
 *   node perf/drivers/node/pipelines.cjs --out FILE --workspace DIR
 *       [--iterations N] [--warmup N] [--pipeline NAME]... [--binding]
 *
 * The same data pipelines, seeds and plans as the Rust driver
 * (type-bridge-core/crates/perf/src/bin/pipelines.rs), written as the
 * `type-bridge-perf-pipelines/v1` report. `migration_apply` is a CLI
 * operation with no Node SDK entry, so only the Rust report has it.
 *
 * The package is the built `@type-bridge/node` at TYPE_BRIDGE_NODE_PACKAGE_ROOT;
 * its native module (TYPE_BRIDGE_NODE_NATIVE_PATH) must be built with
 * `--features perf-stages`, which adds the private `_perfStages()` recorder.
 * `--workspace` is a purchase workspace that `perf.drivers.workspace` set
 * up: this driver runs its `apply` and `drop` commands through
 * TB_PERF_SETUP, a command line that takes `apply|drop DATABASE`.
 * TypeDB comes from TYPEDB_ADDRESS, TYPEDB_HTTP_PORT, TYPEDB_USERNAME and
 * TYPEDB_PASSWORD.
 *
 * `--binding` instead times the offline ops of perf/harness/binding.py with
 * no TypeDB (only the workspace's declared schema) and writes a
 * `type-bridge-perf-binding/v1` report.
 */

"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const FORMAT = "type-bridge-perf-pipelines/v1";
const BINDING_FORMAT = "type-bridge-perf-binding/v1";
const SCOPE = "rust-plan-purchase";
const PROFILE = "typedb-3.13.6/v1";
const SEEDED_ITEMS = 1000;
const FILTER_STOCK = 900;
const BATCH_ROWS = 1000;
const MIXED_BATCH_ROWS = 50;
const DOCTORS = 10;
const PIPELINES = [
  ["insert_one", 1],
  ["fetch_by_key", 1],
  ["fetch_filtered", 1],
  ["update", 1],
  ["delete", 1],
  ["batch_given_rows_1k", BATCH_ROWS],
  ["staged_mixed", 1],
];
// The offline `--binding` ops, in report order (perf/harness/binding.py).
const BINDING_OPS = ["build_purchase", "build_rows_1k", "encode_rows_1k"];

const packageRoot = process.env.TYPE_BRIDGE_NODE_PACKAGE_ROOT;
if (!packageRoot) {
  throw new Error("set TYPE_BRIDGE_NODE_PACKAGE_ROOT to the built @type-bridge/node package");
}
const { Plan, QueryV2Authority, RustDatabase } = require(packageRoot);
const native = require(process.env.TYPE_BRIDGE_NODE_NATIVE_PATH);
if (typeof native._perfStages !== "function") {
  throw new Error("the native module has no _perfStages(); build it with --features perf-stages");
}

// ---------------------------------------------------------------- tokens
// The generated token shapes for the purchase schema, hand-made as in
// tests/integration/staged-plan-builder.test.ts.

const model = (kind, label) => ({ typeKey: JSON.stringify({ kind, label }) });
const field = (owner, attribute) => ({
  kind: "field",
  owner: owner.typeKey,
  metadata: { declaring_id: { attribute, owner: JSON.parse(owner.typeKey) } },
});
const role = (owner, label) => ({
  kind: "role",
  owner: owner.typeKey,
  role: JSON.stringify({ declaring_relation: "purchase", label }),
});
const Item = model("entity", "item");
const Account = model("entity", "account");
const Doctor = model("entity", "doctor");
const Purchase = model("relation", "purchase");
const ITEM_ID = field(Item, "item-id");
const STOCK = field(Item, "stock");
const PRICE = field(Item, "price");
const ACCOUNT_ID = field(Account, "account-id");
const BALANCE = field(Account, "balance");
const NAME = field(Doctor, "name");
const ON_CALL = field(Doctor, "on-call");
const ORDER_ID = field(Purchase, "order-id");
const BUYER = role(Purchase, "buyer");
const BOUGHT = role(Purchase, "item");

// ----------------------------------------------------------------- plans

function insertItems(authority, prefix, from, to) {
  const plan = Plan.builder();
  const id = plan.input("item_id", "string");
  const stock = plan.input("stock", "long");
  const price = plan.input("price", "long");
  plan.insert(Item).field(ITEM_ID, id).field(STOCK, stock).field(PRICE, price);
  for (let number = from; number < to; number += 1) {
    plan.row([id.value(`${prefix}${number}`), stock.value(BigInt(number)), price.value(5n)]);
  }
  return plan.build(authority);
}

function fetchByKey(authority, key) {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const id = plan.input("item_id", "string");
  const stock = plan.field(item, STOCK);
  const price = plan.field(item, PRICE);
  plan.where(plan.field(item, ITEM_ID).eq(id)).returning([stock, price]);
  plan.row([id.value(key)]);
  return plan.build(authority);
}

function fetchFiltered(authority) {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const key = plan.field(item, ITEM_ID);
  const stock = plan.field(item, STOCK);
  plan.where(stock.ge(BigInt(FILTER_STOCK))).returning([key, stock]);
  return plan.build(authority);
}

function restock(authority, key) {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const id = plan.input("item_id", "string");
  const stock = plan.field(item, STOCK);
  plan.where(plan.field(item, ITEM_ID).eq(id));
  plan.update(item, STOCK, plan.let("next_stock", stock.add(1n)));
  plan.row([id.value(key)]);
  return plan.build(authority);
}

function deleteItem(authority, key) {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const id = plan.input("item_id", "string");
  plan.where(plan.field(item, ITEM_ID).eq(id)).delete(item);
  plan.row([id.value(key)]);
  return plan.build(authority);
}

function owners(authority) {
  const plan = Plan.builder();
  plan.insert(Item).field(ITEM_ID, "I1").field(STOCK, 1000000n).field(PRICE, 1n);
  plan.insert(Account).field(ACCOUNT_ID, "A1").field(BALANCE, 1000000000n);
  return plan.build(authority);
}

function doctors(authority) {
  const plan = Plan.builder();
  const name = plan.input("name", "string");
  plan.insert(Doctor).field(NAME, name).field(ON_CALL, true);
  for (let number = 0; number < DOCTORS; number += 1) {
    plan.row([name.value(`D${number}`)]);
  }
  return plan.build(authority);
}

function purchase(authority, order) {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const buyer = plan.exact(Account);
  const orderId = plan.input("order_id", "string");
  const stock = plan.field(item, STOCK);
  const price = plan.field(item, PRICE);
  const balance = plan.field(buyer, BALANCE);
  plan
    .where(plan.field(item, ITEM_ID).eq("I1"))
    .where(plan.field(buyer, ACCOUNT_ID).eq("A1"))
    .where(stock.gt(0n))
    .where(balance.ge(price));
  plan.update(item, STOCK, plan.let("next_stock", stock.sub(1n)));
  plan.update(buyer, BALANCE, plan.let("next_balance", balance.sub(price)));
  plan.insert(Purchase).field(ORDER_ID, orderId).player(BUYER, buyer).player(BOUGHT, item);
  plan.row([orderId.value(order)]);
  return plan.build(authority);
}

function touch(authority) {
  const plan = Plan.builder();
  const doctor = plan.exact(Doctor);
  plan.update(doctor, ON_CALL, plan.field(doctor, ON_CALL));
  return plan.build(authority);
}

function orders(authority, prefix, rows) {
  const plan = Plan.builder();
  const item = plan.exact(Item);
  const buyer = plan.exact(Account);
  const orderId = plan.input("order_id", "string");
  plan.where(plan.field(item, ITEM_ID).eq("I1"));
  plan.where(plan.field(buyer, ACCOUNT_ID).eq("A1"));
  plan.insert(Purchase).field(ORDER_ID, orderId).player(BUYER, buyer).player(BOUGHT, item);
  for (let number = 0; number < rows; number += 1) {
    plan.row([orderId.value(`${prefix}${number}`)]);
  }
  return plan.build(authority);
}

// ------------------------------------------------------------ iterations

/** Stage `plans` in one transaction, commit, and decode their answers. */
async function commit(db, kind, plans) {
  const tx = db.stagedTransaction(kind);
  try {
    const pending = plans.map((plan) => tx.stage(plan));
    await tx.commit();
    return pending.map((answer) => answer.get());
  } finally {
    await tx[Symbol.asyncDispose]();
  }
}

function expect(answer, rows) {
  const actual = answer.kind === "rows" ? answer.rows.length : Number(answer.value);
  if (!["rows", "matched", "count"].includes(answer.kind) || actual !== rows) {
    throw new Error(`unexpected answer ${JSON.stringify(answer)}, expected ${rows} rows`);
  }
}

async function seed(name, db, authority, iterations) {
  if (["fetch_by_key", "fetch_filtered", "update"].includes(name)) {
    await commit(db, "write", [insertItems(authority, "item-", 0, SEEDED_ITEMS)]);
  } else if (name === "delete") {
    await commit(db, "write", [insertItems(authority, "doomed-", 0, iterations)]);
  } else if (name === "staged_mixed") {
    await commit(db, "write", [owners(authority), doctors(authority)]);
  }
}

async function iteration(name, db, authority, number) {
  const key = `item-${number % SEEDED_ITEMS}`;
  switch (name) {
    case "insert_one":
      return expect((await commit(db, "write", [insertItems(authority, "new-", number, number + 1)]))[0], 1);
    case "fetch_by_key":
      return expect((await commit(db, "read", [fetchByKey(authority, key)]))[0], 1);
    case "fetch_filtered":
      return expect((await commit(db, "read", [fetchFiltered(authority)]))[0], SEEDED_ITEMS - FILTER_STOCK);
    case "update":
      return expect((await commit(db, "write", [restock(authority, key)]))[0], 1);
    case "delete":
      return expect((await commit(db, "write", [deleteItem(authority, `doomed-${number}`)]))[0], 1);
    case "batch_given_rows_1k": {
      const plan = insertItems(authority, `batch-${number}-`, 0, BATCH_ROWS);
      return expect((await commit(db, "write", [plan]))[0], BATCH_ROWS);
    }
    case "staged_mixed": {
      const plans = [
        purchase(authority, `O${number}`),
        touch(authority),
        orders(authority, `B${number}-`, MIXED_BATCH_ROWS),
      ];
      const [bought, touched, batch] = await commit(db, "write", plans);
      expect(bought, 1);
      expect(touched, DOCTORS);
      return expect(batch, MIXED_BATCH_ROWS);
    }
    default:
      throw new Error(`${name} is not a data pipeline`);
  }
}

// ---------------------------------------------------------------- driver

function parse(argv) {
  const config = { iterations: 30, warmup: 5, only: [], binding: false };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    if (flag === "--binding") {
      config.binding = true;
      continue;
    }
    const value = argv[++index];
    if (flag === "--out") config.out = value;
    else if (flag === "--workspace") config.workspace = value;
    else if (flag === "--iterations") config.iterations = Number(value);
    else if (flag === "--warmup") config.warmup = Number(value);
    else if (flag === "--pipeline") {
      if (!PIPELINES.some(([known]) => known === value)) throw new Error(`unknown pipeline ${value}`);
      config.only.push(value);
    } else throw new Error(`unknown argument ${flag}`);
  }
  if (!config.out || !config.workspace) throw new Error("--out FILE and --workspace DIR are required");
  for (const count of [config.iterations, config.warmup]) {
    if (!Number.isInteger(count) || count < 1) throw new Error("counts must be positive integers");
  }
  for (const name of ["TYPEDB_USERNAME", "TYPEDB_PASSWORD", "TB_PERF_SETUP"]) {
    if (!config.binding && !process.env[name]) throw new Error(`set ${name}`);
  }
  return config;
}

/** Run the harness's workspace setup command: `apply|drop DATABASE`. */
function setup(action, database) {
  const [command, ...prefix] = JSON.parse(process.env.TB_PERF_SETUP);
  execFileSync(command, [...prefix, action, database], { stdio: ["ignore", "ignore", "inherit"] });
}

function connect(database) {
  return RustDatabase.connect(process.env.TYPEDB_ADDRESS ?? "localhost:1729", database, {
    username: process.env.TYPEDB_USERNAME,
    password: process.env.TYPEDB_PASSWORD,
    httpPort: Number(process.env.TYPEDB_HTTP_PORT ?? "8000"),
  });
}

function nanos(started) {
  return Number(process.hrtime.bigint() - started);
}

async function runPipeline(name, config, declared) {
  const database = `tb_perf_node_${process.pid}_${name}`;
  try {
    setup("apply", database);
    const db = connect(database);
    try {
      const authority = QueryV2Authority.queryOnly(db, declared, SCOPE, PROFILE);
      await seed(name, db, authority, config.warmup + config.iterations);
      const samples = [];
      for (let number = 0; number < config.warmup + config.iterations; number += 1) {
        native._perfStages();
        const started = process.hrtime.bigint();
        await iteration(name, db, authority, number);
        const total = nanos(started);
        const stages = native._perfStages();
        if (number >= config.warmup) {
          const sorted = Object.fromEntries(Object.entries(stages).sort(([a], [b]) => a.localeCompare(b)));
          samples.push({ total_ns: total, stages: sorted });
        }
      }
      return samples;
    } finally {
      db.close();
    }
  } finally {
    setup("drop", database);
  }
}

/** Time the offline ops: no database opens, since staging needs a connected one. */
function binding(config, declared) {
  const authority = new QueryV2Authority(declared, SCOPE, PROFILE);
  const rows = insertItems(authority, "batch-", 0, BATCH_ROWS);
  const calls = {
    build_purchase: () => purchase(authority, "O0"),
    build_rows_1k: () => insertItems(authority, "batch-", 0, BATCH_ROWS),
    encode_rows_1k: () => rows.canonicalBytes(),
  };
  const ops = BINDING_OPS.map((name) => {
    const samples = [];
    for (let number = 0; number < config.warmup + config.iterations; number += 1) {
      const started = process.hrtime.bigint();
      calls[name]();
      const elapsed = nanos(started);
      if (number >= config.warmup) samples.push(elapsed);
    }
    return { name, samples_ns: samples };
  });
  return { format: BINDING_FORMAT, binding: "node", warmup: config.warmup, ops };
}
function write(out, report) {
  fs.mkdirSync(path.dirname(path.resolve(out)), { recursive: true });
  fs.writeFileSync(out, `${JSON.stringify(report, null, 2)}\n`);
}
async function main() {
  const config = parse(process.argv.slice(2));
  const declared = fs.readFileSync(path.join(config.workspace, "declared-schema.json"));
  if (config.binding) return write(config.out, binding(config, declared));
  const pipelines = [];
  for (const [name, operations] of PIPELINES) {
    if (config.only.length && !config.only.includes(name)) continue;
    process.stderr.write(`pipeline ${name}\n`);
    pipelines.push({ name, operations, samples: await runPipeline(name, config, declared) });
  }
  write(config.out, { format: FORMAT, binding: "node", warmup: config.warmup, pipelines });
}

main().catch((error) => {
  process.stderr.write(`error: ${error.stack ?? error}\n`);
  process.exit(1);
});
