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
