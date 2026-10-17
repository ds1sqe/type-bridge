#!/usr/bin/env node
/**
 * The release CRUD ops through one released `@type-bridge/node`, against a
 * live TypeDB.
 *
 *   node perf/released/node/workload.mjs --database NAME --models DIR --out FILE
 *       [--skip OP]... [--iterations N] [--warmup N]
 *
 * The same six ops as perf/released/python/workload.py, on the generated
 * `Item.manager(db)` of the package `--models` names (compiled, with a
 * `dist/index.js`), on a `RustDatabase`:
 *
 * - insert_one: `insert` one item;
 * - fetch_by_key: `filter({ itemId }).first()` of one of 1,000 items;
 * - fetch_filtered: `filter({ stock__gte }).all()` with stock 900, the 100 of 1,000;
 * - update: `update(iid, replacement)` with its stock plus one;
 * - delete: `delete` one fetched item;
 * - batch_insert_1k: `insertMany` of 1,000 items.
 *
 * `--skip` drops an op the version's generated manager does not have; the
 * harness (perf/harness/released_node.py) is the one place that names a
 * version. Each op is timed around the manager call alone. The database
 * gets the item schema in one schema transaction, is seeded with 1,000
 * items (one `insert` each when `insertMany` is skipped), and is deleted at
 * the end. `@type-bridge/node` resolves from the models directory, so the
 * package must sit beneath the npm prefix that holds the version. TypeDB
 * comes from TYPEDB_ADDRESS, TYPEDB_HTTP_PORT, TYPEDB_USERNAME and
 * TYPEDB_PASSWORD.
 */

import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { pathToFileURL } from "node:url";

const FORMAT = "type-bridge-perf-release/v1";
const SEEDED_ITEMS = 1000;
const FILTER_STOCK = 900;
const BATCH_ROWS = 1000;
const OPS = [
  ["insert_one", 1],
  ["fetch_by_key", 1],
  ["fetch_filtered", 1],
  ["update", 1],
  ["delete", 1],
  ["batch_insert_1k", BATCH_ROWS],
];
const SCHEMA = `define
  attribute item-id, value string;
  attribute stock, value integer;
  attribute price, value integer;
  entity item, owns item-id @key, owns stock, owns price;`;

function parse(argv) {
  const options = { skip: [], iterations: 30, warmup: 5 };
  for (let index = 0; index < argv.length; index += 2) {
    const [flag, value] = [argv[index], argv[index + 1]];
    if (value === undefined) throw new Error(`${flag} needs a value`);
    if (flag === "--database") options.database = value;
    else if (flag === "--models") options.models = path.resolve(value);
    else if (flag === "--out") options.out = path.resolve(value);
    else if (flag === "--skip") options.skip.push(value);
    else if (flag === "--iterations") options.iterations = Number(value);
    else if (flag === "--warmup") options.warmup = Number(value);
    else throw new Error(`unknown option ${flag}`);
  }
  for (const key of ["database", "models", "out"]) {
    if (!options[key]) throw new Error(`--${key} is required`);
  }
  const names = OPS.map(([name]) => name);
  for (const name of options.skip) {
    if (!names.includes(name)) throw new Error(`unknown op ${name}`);
  }
  return options;
}

function expect(condition, op) {
  if (!condition) throw new Error(`${op} returned an unexpected result`);
}

function run(m, db, skip, iterations, warmup) {
  const manager = m.Item.manager(db);
  const item = (key, stock) =>
    m.Item.create({
      itemId: m.ItemId.create(key),
      stock: m.Stock.create(BigInt(stock)),
      price: m.Price.create(5n),
    });
  const fetch = (key) => {
    const found = manager.filter({ itemId: m.ItemId.create(key) }).first();
    if (found === null) throw new Error(`item ${key} is missing`);
    return found;
  };

  const seed = Array.from({ length: SEEDED_ITEMS }, (_, number) => item(`seed-${number}`, number));
  if (skip.includes("batch_insert_1k")) seed.forEach((row) => manager.insert(row));
  else manager.insertMany(seed);

  const prepare = {
    // Stock 0 keeps the filtered op at the 100 seeded items.
    insert_one: (index) => () => manager.insert(item(`one-${index}`, 0)),
    fetch_by_key: (index) => {
      const key = m.ItemId.create(`seed-${index % SEEDED_ITEMS}`);
      return () => expect(manager.filter({ itemId: key }).first() !== null, "fetch_by_key");
    },
    fetch_filtered: () => {
      const stock = m.Stock.create(BigInt(FILTER_STOCK));
      const expected = SEEDED_ITEMS - FILTER_STOCK;
      return () =>
        expect(manager.filter({ stock__gte: stock }).all().length === expected, "fetch_filtered");
    },
    update: (index) => {
      // Items below the filter bound, so the filtered count stays fixed.
      const found = fetch(`seed-${index % FILTER_STOCK}`);
      const replacement = item(found.itemId.value, Number(found.stock.value) + 1);
      return () => manager.update(found.iid, replacement);
    },
    delete: (index) => {
      const found = fetch(`one-${index}`);
      return () => manager.delete(found);
    },
    batch_insert_1k: (index) => {
      const rows = Array.from({ length: BATCH_ROWS }, (_, n) => item(`batch-${index}-${n}`, 0));
      return () => manager.insertMany(rows);
    },
  };
  const samples = {};
  for (const [name] of OPS) {
    if (skip.includes(name)) continue;
    samples[name] = [];
    for (let index = 0; index < warmup + iterations; index += 1) {
      const call = prepare[name](index);
      const started = process.hrtime.bigint();
      call();
      const elapsed = process.hrtime.bigint() - started;
      if (index >= warmup) samples[name].push(Number(elapsed));
    }
  }
  return samples;
}

async function main() {
  const options = parse(process.argv.slice(2));
  const models = await import(pathToFileURL(path.join(options.models, "dist", "index.js")).href);
  const require = createRequire(path.join(options.models, "package.json"));
  const nodePackage = require.resolve("@type-bridge/node");
  const { RustDatabase } = require(nodePackage);
  const manifest = JSON.parse(
    fs.readFileSync(path.join(nodePackage.split(`${path.sep}dist${path.sep}`)[0], "package.json")),
  );
  const env = process.env;
  const db = RustDatabase.connect(env.TYPEDB_ADDRESS || "localhost:1729", options.database, {
    username: env.TYPEDB_USERNAME || "admin",
    password: env.TYPEDB_PASSWORD || "password",
    httpPort: Number(env.TYPEDB_HTTP_PORT || "8000"),
  });
  let samples;
  try {
    if (!db.databaseExists()) db.createDatabase();
    const tx = db.transaction("schema");
    tx.query(SCHEMA);
    tx.commit();
    samples = run(models, db, options.skip, options.iterations, options.warmup);
  } finally {
    if (db.databaseExists()) db.deleteDatabase();
    db.close();
  }
  const report = {
    format: FORMAT,
    version: manifest.version,
    node: process.versions.node,
    ops: OPS.filter(([name]) => !options.skip.includes(name)).map(([name, operations]) => ({
      name,
      operations,
      samples: samples[name],
    })),
  };
  fs.mkdirSync(path.dirname(options.out), { recursive: true });
  fs.writeFileSync(options.out, `${JSON.stringify(report, null, 2)}\n`);
}

main().catch((error) => {
  console.error(`error: ${error.message}`);
  process.exit(1);
});
