import assert = require("node:assert/strict");
import test = require("node:test");

type NativeCall = (...args: unknown[]) => unknown;

const validDiagnostic = Object.freeze({
  version: 1,
  category: "invalid_contract",
  sdk_category: "invalid_input",
  query_category: "invalid_contract",
  code: "query_v2_fixture",
  message: "fixture diagnostic",
  path: [
    { kind: "field", value: "patterns" },
    { kind: "index", value: 2 },
    { kind: "identifier", value: "person" },
  ],
  details: {
    boolean: { kind: "boolean", value: true },
    long: { kind: "long", value: "-9223372036854775808" },
    text: { kind: "text", value: "context" },
    text_list: { kind: "text_list", value: ["a", "b"] },
  },
});

test("QueryV2Error takes the Rust diagnostic JSON forms as given", async (context) => {
  const nativePath = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  assert.ok(nativePath, "native test path");

  // Each unit file runs in its own process. Patch before importing the public
  // facade so this test can exercise its native-error projection directly.
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const raw = require(nativePath) as Record<string, NativeCall>;
  const original = raw["queryV2Authority"];
  assert.ok(original, "raw queryV2Authority");
  let diagnostic: unknown = validDiagnostic;
  raw["queryV2Authority"] = (): never => {
    throw new Error(JSON.stringify(diagnostic));
  };
  context.after(() => {
    raw["queryV2Authority"] = original;
  });

  const { QueryV2Authority, QueryV2Error } =
    await import("../../typescript/index.js");
  const invoke = () =>
    new QueryV2Authority(Buffer.from([0]), "scope", "profile");

  assert.throws(invoke, (error: unknown) => {
    assert.ok(error instanceof QueryV2Error);
    assert.equal(error.category, validDiagnostic.category);
    assert.equal(error.sdkCategory, validDiagnostic.sdk_category);
    assert.equal(error.queryCategory, validDiagnostic.query_category);
    assert.equal(error.code, validDiagnostic.code);
    assert.equal(error.diagnosticMessage, validDiagnostic.message);
    assert.deepEqual(error.path, validDiagnostic.path);
    assert.deepEqual(error.details, validDiagnostic.details);
    return true;
  });

  // The contract `Diagnostic` serde form has no SDK or query category.
  const { version: _version, sdk_category: _sdk, query_category: _query, ...contract } =
    validDiagnostic;
  diagnostic = contract;
  assert.throws(invoke, (error: unknown) => {
    assert.ok(error instanceof QueryV2Error);
    assert.equal(error.category, contract.category);
    assert.equal(error.sdkCategory, contract.category);
    assert.equal(error.queryCategory, contract.category);
    assert.deepEqual(error.details, contract.details);
    return true;
  });

  // Internal failures and messages that are not diagnostics stay plain.
  for (const payload of [
    { ...validDiagnostic, category: "internal", sdk_category: "internal" },
    { message: "not a diagnostic" },
    ["category", "code"],
  ]) {
    diagnostic = payload;
    assert.throws(invoke, (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.ok(!(error instanceof QueryV2Error));
      assert.equal(error.message, JSON.stringify(payload));
      return true;
    });
  }
});
