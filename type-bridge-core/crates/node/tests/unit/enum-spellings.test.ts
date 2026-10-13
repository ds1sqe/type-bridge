import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

import { loadNative } from "../../typescript/native.js";
import { QueryV2Error, queryV2NativeCall } from "../../typescript/query-v2-internals.js";

// Node half of the enum-spelling parity corpus: the same cases, codes, and
// details as Python and Rust, from the core's one parser per enum.
type EnumSpellingCase = Readonly<{
  id: string;
  kind: string;
  value: string;
  expected: Readonly<{
    category: string;
    code: string;
    details: Readonly<Record<string, unknown>>;
  }>;
}>;

const repositoryRoot = path.resolve(process.cwd(), "../../..");
const corpus = JSON.parse(
  fs.readFileSync(path.join(repositoryRoot, "tests/contracts/enum-spellings-v1.json"), "utf8"),
) as Readonly<{ cases: readonly EnumSpellingCase[] }>;

for (const entry of corpus.cases) {
  test(`enum spelling corpus: ${entry.id}`, () => {
    const native = loadNative();
    assert.throws(
      () => queryV2NativeCall(() => native.canonicalEnumSpelling(entry.kind, entry.value)),
      (error: unknown) => {
        assert.ok(error instanceof QueryV2Error);
        assert.deepEqual(
          {
            category: error.category,
            code: error.code,
            details: error.details,
          },
          entry.expected,
        );
        return true;
      },
    );
  });
}

test("aliases resolve to their canonical spelling", () => {
  const native = loadNative();
  assert.equal(native.canonicalEnumSpelling("manager_lookup", "exact"), "eq");
  assert.equal(native.canonicalEnumSpelling("manager_lookup", "like"), "regex");
  assert.equal(native.canonicalEnumSpelling("scalar_domain", "datetime_tz"), "datetime_tz");
  assert.equal(native.canonicalEnumSpelling("scalar_domain", "datetime-tz"), "datetime_tz");
  assert.equal(native.canonicalEnumSpelling("transaction_type", " READ "), "read");
});
