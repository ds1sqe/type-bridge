import assert = require("node:assert/strict");
import test = require("node:test");

type NativeCall = (...args: unknown[]) => unknown;

const validDiagnostic = Object.freeze({
  category: "invalid_contract",
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

test("QueryV2Error accepts only the complete canonical Rust diagnostic shape", async (context) => {
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

  const { QueryV2Authority, QueryV2Error } = await import("../../typescript/index.js");
  const invoke = () => new QueryV2Authority(Buffer.from([0]), "scope", "profile");

  assert.throws(invoke, (error: unknown) => {
    assert.ok(error instanceof QueryV2Error);
    assert.equal(error.category, validDiagnostic.category);
    assert.equal(error.code, validDiagnostic.code);
    assert.equal(error.diagnosticMessage, validDiagnostic.message);
    assert.deepEqual(error.path, validDiagnostic.path);
    assert.deepEqual(error.details, validDiagnostic.details);
    return true;
  });

  const malformed = [
    { ...validDiagnostic, category: "internal" },
    { ...validDiagnostic, code: "Not_Canonical" },
    { ...validDiagnostic, extra: true },
    { ...validDiagnostic, path: [{ kind: "index", value: -1 }] },
    { ...validDiagnostic, path: [{ kind: "index", value: Number.MAX_SAFE_INTEGER + 1 }] },
    { ...validDiagnostic, path: [{ kind: "field", value: "x", extra: true }] },
    {
      ...validDiagnostic,
      details: { value: { kind: "long", value: "-0" } },
    },
    {
      ...validDiagnostic,
      details: { value: { kind: "long", value: "9223372036854775808" } },
    },
    {
      ...validDiagnostic,
      details: { value: { kind: "text_list", value: ["ok", 1] } },
    },
    {
      ...validDiagnostic,
      details: { value: { kind: "text", value: "ok", extra: true } },
    },
  ];

  for (const payload of malformed) {
    diagnostic = payload;
    assert.throws(invoke, (error: unknown) => {
      assert.ok(error instanceof Error);
      assert.ok(!(error instanceof QueryV2Error));
      assert.equal(error.message, JSON.stringify(payload));
      return true;
    });
  }
});
