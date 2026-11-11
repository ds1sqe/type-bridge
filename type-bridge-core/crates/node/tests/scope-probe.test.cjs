"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const { inspectFile } = require("./scope-probe.cjs");

const FIXTURE_ROOT = path.resolve(__dirname, "fixtures/scope-probe");

function fixtureFiles(kind) {
  const root = path.join(FIXTURE_ROOT, kind);
  return fs
    .readdirSync(root)
    .sort()
    .map((name) => path.join(root, name));
}

test("schema TypeQL importer, generator, strings, and docs remain allowed", () => {
  for (const file of fixtureFiles("allowed")) {
    assert.deepEqual(
      inspectFile(file, fs.readFileSync(file, "utf8")),
      [],
      path.relative(FIXTURE_ROOT, file),
    );
  }
});

test("hostile dependencies and executable query policy are rejected structurally", () => {
  const actual = new Map();
  for (const file of fixtureFiles("hostile")) {
    actual.set(
      path.basename(file),
      inspectFile(file, fs.readFileSync(file, "utf8")).map(
        ({ code }) => code,
      ),
    );
  }

  assert.deepEqual(
    actual,
    new Map([
      [
        "Cargo.toml",
        [
          "direct_driver_dependency",
          "direct_driver_dependency",
          "direct_driver_dependency",
          "direct_driver_dependency",
        ],
      ],
      ["driver.ts", ["direct_driver_module"]],
      [
        "package.json",
        ["direct_driver_dependency", "direct_driver_dependency"],
      ],
      [
        "policy.rs",
        [
          "direct_driver_identifier",
          "host_query_policy",
          "host_query_policy",
        ],
      ],
      [
        "policy.ts",
        ["host_query_policy", "host_query_policy", "host_query_policy"],
      ],
    ]),
  );
});

const CRATE_ROOT = path.resolve(__dirname, "..");
const inspected = (relative, source) => inspectFile(path.join(CRATE_ROOT, relative), source);
const forwardingTs = `export class RawTransaction {
  executeQuery(query: string): Promise<RawValue> {
    return rawNativePromise(rawNativeCall(() => this.#native.executeQuery(query)));
  }
}`;
const forwardingRust = `impl NodeRawTransaction {
  pub fn execute_query(&self, query: String) {
    let bytes = transaction.execute_query(&query).await;
  }
}`;

test("raw forwarding exemptions bind the file, owner, receiver and unchanged query", () => {
  assert.deepEqual(inspected("typescript/raw-query.ts", forwardingTs), []);
  assert.deepEqual(inspected("src/raw_runtime.rs", forwardingRust), []);
  assert.deepEqual(inspected("src/lib.rs", forwardingRust
    .replace("NodeRawTransaction", "NodeRustTransactionContext")
    .replace("transaction.execute_query", "tx.execute_query")), []);
  assert.deepEqual(inspected("src/raw_runtime.rs", `async fn execute_one(mut transaction: RawTransaction, query: String) {
    let bytes = transaction.execute_query(&query).await;
  }`), []);

  for (const [file, source] of [
    ["typescript/unrelated.ts", forwardingTs],
    ["typescript/raw-query.ts", forwardingTs.replace("RawTransaction", "OtherClass")],
    ["typescript/raw-query.ts", forwardingTs.replace("this.#native.executeQuery", "other.executeQuery")],
    ["typescript/raw-query.ts", forwardingTs.replace("executeQuery(query: string)", "newMethod(query: string)")],
    ["typescript/raw-query.ts", forwardingTs.replace("executeQuery(query))", 'executeQuery(query + " limit 1"))')],
    ["src/unrelated.rs", forwardingRust],
    ["src/raw_runtime.rs", forwardingRust.replace("NodeRawTransaction", "OtherClass")],
    ["src/raw_runtime.rs", forwardingRust.replace("transaction.execute_query", "other.execute_query")],
    ["src/raw_runtime.rs", forwardingRust.replace("transaction.execute_query", "other.transaction.execute_query")],
    ["src/raw_runtime.rs", forwardingRust.replace("fn execute_query", "fn new_method")],
    ["src/raw_runtime.rs", forwardingRust.replace("execute_query(&query)", "execute_query(&rewritten)")],
  ]) {
    assert.ok(inspected(file, source).some(({ code }) => code === "host_query_policy"), `${file}: ${source}`);
  }
});

test("raw facade files retain compiler and direct-driver bans", () => {
  for (const [file, source] of [
    ["typescript/raw-query.ts", `${forwardingTs}\nconst compiler = new QueryCompiler(); const driver = typedbDriver;`],
    ["src/raw_runtime.rs", `${forwardingRust}\nfn policy() { QueryCompiler::new(); typedb_driver::connect(); }`],
  ]) {
    const codes = inspected(file, source).map(({ code }) => code);
    assert.ok(codes.includes("host_query_policy"));
    assert.ok(codes.includes("direct_driver_identifier"));
  }
});
