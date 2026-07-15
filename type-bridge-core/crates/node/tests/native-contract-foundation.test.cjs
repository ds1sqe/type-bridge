"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

function nativeLibraryName() {
  switch (process.platform) {
    case "linux":
      return "libtype_bridge_node.so";
    case "darwin":
      return "libtype_bridge_node.dylib";
    case "win32":
      return "type_bridge_node.dll";
    default:
      throw new Error(`unsupported native test platform: ${process.platform}`);
  }
}

let cachedNative = null;

function loadNative() {
  if (cachedNative !== null) {
    return cachedNative;
  }

  const source = path.resolve(
    __dirname,
    "../../../target/debug",
    nativeLibraryName(),
  );
  assert.ok(
    fs.existsSync(source),
    `native artifact missing at ${source}; run cargo build -p type-bridge-node --features contract-test-adapter`,
  );
  const tempDir = path.resolve(__dirname, "../../../../tmp/node-native-contract");
  fs.mkdirSync(tempDir, { recursive: true });
  const loadable = path.join(tempDir, "type_bridge_node.node");
  fs.copyFileSync(source, loadable);
  cachedNative = require(loadable);
  return cachedNative;
}

function foundationBytes() {
  const fixture = fs.readFileSync(
    path.resolve(
      __dirname,
      "../../contract/tests/fixtures/foundation-probe-v1.json",
    ),
    "utf8",
  ).trimEnd();
  return Buffer.from(fixture, "utf8");
}

test("contract foundation bytes round-trip without JS numeric coercion", () => {
  const native = loadNative();
  const input = foundationBytes();
  const output = native.__roundTripContractFoundation(input);

  assert.ok(Buffer.isBuffer(output));
  assert.deepEqual(output, input);
  const decoded = JSON.parse(output.toString("utf8"));
  assert.equal(decoded.long.value, "9007199254740993");
  assert.equal(typeof decoded.long.value, "string");
  assert.deepEqual(decoded.cardinality, {
    kind: "cardinality",
    max: "unbounded",
    min: "0",
  });
  assert.deepEqual(decoded.capabilities, [
    "query.given-multi-row",
    "schema.annotations",
  ]);
  assert.deepEqual(decoded.type_id, { kind: "entity", label: "person" });
  assert.equal(decoded.fingerprint.algorithm, "sha256");
});

test("contract adapter rejects noncanonical and numeric-long inputs", () => {
  const native = loadNative();
  const canonical = foundationBytes().toString("utf8");

  assert.throws(
    () => native.__roundTripContractFoundation(Buffer.from(` ${canonical}`)),
    /non_canonical_json/,
  );
  assert.throws(
    () => native.__roundTripContractFoundation(Buffer.from(
      canonical.replace('"value":"9007199254740993"', '"value":9007199254740993'),
    )),
    /invalid_canonical_value/,
  );
});
