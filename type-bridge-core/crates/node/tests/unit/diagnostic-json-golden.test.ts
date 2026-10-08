import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

import { loadNative } from "../../typescript/native.js";

// Node half of tests/contracts/diagnostic-json-v1.json: a native error's
// message is the core's diagnostic JSON, byte-for-byte the form Python
// exposes as exception attributes. (The group-key case needs a generated
// package; the projection-integration tier checks it.)
type GoldenCase = Readonly<{ id: string; diagnostic: unknown }>;

const repositoryRoot = path.resolve(process.cwd(), "../../..");
const golden = JSON.parse(
  fs.readFileSync(path.join(repositoryRoot, "tests/contracts/diagnostic-json-v1.json"), "utf8"),
) as Readonly<{ cases: readonly GoldenCase[] }>;

function expected(id: string): unknown {
  const entry = golden.cases.find((candidate) => candidate.id === id);
  assert.ok(entry, id);
  return entry.diagnostic;
}

function nativeJson(operation: () => unknown): unknown {
  try {
    operation();
  } catch (error) {
    assert.ok(error instanceof Error);
    return JSON.parse(error.message);
  }
  assert.fail("operation was not refused");
}

test("an unknown enum spelling is the golden SDK diagnostic JSON", () => {
  const native = loadNative();
  assert.deepEqual(
    nativeJson(() => native.canonicalEnumSpelling("sort_direction", "sideways")),
    expected("unknown_enum_value.sort_direction"),
  );
});

test("a malformed schema authority is the golden contract diagnostic JSON", () => {
  const native = loadNative();
  assert.deepEqual(
    nativeJson(() => native.queryV2Authority(Buffer.from("{"), "scope", "profile")),
    expected("contract.malformed_canonical_json"),
  );
});
