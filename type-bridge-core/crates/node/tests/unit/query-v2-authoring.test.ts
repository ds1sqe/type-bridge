import assert = require("node:assert/strict");
import fs = require("node:fs");
import os = require("node:os");
import path = require("node:path");
import test = require("node:test");
import vm = require("node:vm");
import type ts = require("typescript");

import { QueryV2Error } from "../../typescript/index.js";
import * as queryV2Authoring from "../../typescript/query-v2.js";
import {
  QueryPlanBuilder,
  QueryV2Authority,
} from "../../typescript/query-v2.js";
import { loadNative } from "../../typescript/native.js";

const typescript = require(
  path.join(process.cwd(), "node_modules/typescript"),
) as typeof ts;

interface FailureCorpus {
  readonly declared_b64: string;
  readonly scope: string;
  readonly profile: string;
}

const corpusPath = path.resolve(
  process.cwd(),
  "../../../tests/fixtures/query-v2-remote-failures.json",
);
const corpus = JSON.parse(fs.readFileSync(corpusPath, "utf8")) as FailureCorpus;
const declared = Buffer.from(corpus.declared_b64, "base64");
const parityDeclared = fs.readFileSync(path.resolve(
  process.cwd(),
  "../../../tests/fixtures/query-v2-model-remote-parity-declared.json",
)).subarray(0, -1);
const builderOperations = new Set([
  "binding",
  "input",
  "binding_operand",
  "literal_operand",
  "input_operand",
  "isa",
  "has",
  "links",
  "value",
  "not",
  "or",
  "try",
  "reachable",
  "function_call",
  "order",
  "reduce_assignment",
  "local_return",
  "local_function",
  "match",
  "select",
  "require",
  "distinct",
  "reduce",
  "sort",
  "offset",
  "limit",
  "document_binding",
  "document_attribute_list",
  "finalize_rows",
  "finalize_documents",
]);

function documentedExample(after: string, language: string): string {
  const guidePath = path.resolve(
    process.cwd(),
    "../../../docs/guide/typed-queries.md",
  );
  const guide = fs.readFileSync(guidePath, "utf8");
  const tail = guide.split(after, 2)[1];
  assert.notEqual(tail, undefined, `missing documentation marker: ${after}`);
  const fenced = tail.split(`\`\`\`${language}\n`, 2)[1];
  assert.notEqual(fenced, undefined, `missing ${language} fence after ${after}`);
  return `${fenced.split("\n```", 1)[0]}\n`;
}

function formatTypeScriptDiagnostics(
  diagnostics: readonly ts.Diagnostic[],
): string {
  return typescript.formatDiagnosticsWithColorAndContext(diagnostics, {
    getCanonicalFileName: (fileName) => fileName,
    getCurrentDirectory: () => process.cwd(),
    getNewLine: () => "\n",
  });
}

function publicPlan(authority: QueryV2Authority) {
  const builder = new QueryPlanBuilder(authority);
  const person = builder.binding("person");
  const name = builder.binding("name");
  const patterns = [
    builder.isa(person, "entity", "smoke-person", true),
    builder.has(person, name, "smoke-name"),
  ];
  builder.match(patterns);
  builder.sort([builder.order(name, "ascending")]);
  return { builder, plan: builder.finalizeRows([person, name]) };
}

test("public authoring is byte-identical to the direct Rust N-API builder", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const { plan } = publicPlan(authority);

  const native = loadNative();
  const nativeAuthority = native.queryV2Authority(
    declared,
    corpus.scope,
    corpus.profile,
  );
  const nativeBuilder = new native.NodeQueryPlanBuilder(nativeAuthority);
  const person = nativeBuilder.binding("person");
  const name = nativeBuilder.binding("name");
  nativeBuilder.match([
    nativeBuilder.isa(person, "entity", "smoke-person", true),
    nativeBuilder.has(person, name, "smoke-name"),
  ]);
  nativeBuilder.sort([nativeBuilder.order(name, "ascending")]);
  const nativePlan = nativeBuilder.finalizeRows([person, name]);

  assert.deepEqual(Buffer.from(plan.canonicalBytes), Buffer.from(nativePlan.canonicalBytes));
  assert.equal(plan.format, "typebridge.query-plan/v2");
  assert.equal(plan.fingerprint, nativePlan.fingerprint);
  assert.deepEqual(plan.requiredCapabilities, nativePlan.requiredCapabilities);
  assert.deepEqual(
    [...plan.requiredCapabilities],
    [...plan.requiredCapabilities].sort(),
  );

  const invocation = plan.rows([]);
  assert.equal(invocation.operation, "rows");
  assert.equal(invocation.planFingerprint, plan.fingerprint);
  assert.equal(
    plan.authorityIdentity.sameAuthority(invocation.authorityIdentity),
    true,
  );
  assert.deepEqual(invocation.requiredTransportCapabilities, []);
});

test("public facade preserves structured ownership and terminal diagnostics", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const foreignAuthority = new QueryV2Authority(
    declared,
    corpus.scope,
    corpus.profile,
  );
  const first = new QueryPlanBuilder(authority);
  const second = new QueryPlanBuilder(authority);
  const foreign = new QueryPlanBuilder(foreignAuthority);
  const sameAuthorityHandle = first.binding("same_authority");
  const foreignAuthorityHandle = foreign.binding("foreign_authority");

  assert.throws(
    () => second.bindingOperand(sameAuthorityHandle),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_builder_cross_builder_handle");
      return true;
    },
  );
  assert.throws(
    () => second.bindingOperand(foreignAuthorityHandle),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_builder_cross_authority_handle");
      return true;
    },
  );

  const local = second.binding("local");
  second.match([second.isa(local, "entity", "smoke-person", false)]);
  assert.equal(
    second.finalizeRows([local]).format,
    "typebridge.query-plan/v2",
  );

  const { builder, plan } = publicPlan(authority);
  assert.throws(
    () => builder.binding("after_finalize"),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_builder_finalized");
      return true;
    },
  );
  assert.throws(
    () => plan.rows([[]]),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_invocation_unexpected_inputs");
      return true;
    },
  );
});

test("public strings reject lone surrogates instead of replacement-encoding identity or values", () => {
  const mixedAstralVariable = "😀".repeat(64) + "\ud800";
  const mixedAstralLabel = "😀".repeat(128) + "\ud800";
  const mixedAstralCanonical = "😀".repeat(Math.floor(1_048_576 / 2)) +
    "\ud800";

  assert.throws(
    () => new QueryV2Authority(declared, "binding-\ud800", corpus.profile),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_v2_host_string_unicode");
      return true;
    },
  );
  assert.throws(
    () => new QueryV2Authority(
      declared,
      mixedAstralCanonical,
      corpus.profile,
    ),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_v2_host_string_unicode");
      return true;
    },
  );

  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const builder = new QueryPlanBuilder(authority);
  assert.throws(
    () => builder.literalOperand("string", "\ud800"),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_builder_scalar_unicode");
      return true;
    },
  );

  const person = builder.binding("person");
  const relation = builder.binding("relation");
  const assigned = builder.binding("assigned");
  assert.throws(
    () => builder.binding(1 as unknown as string),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_v2_host_string_type");
      return true;
    },
  );
  for (const operation of [
    () => builder.binding("\ud800"),
    () => builder.binding(mixedAstralVariable),
    () => builder.isa(person, "entity", "\ud800", false),
    () => builder.isa(person, "entity", mixedAstralLabel, false),
    () => builder.links(
      relation,
      "smoke-relation",
      ["\ud800"],
      [person],
    ),
    () => builder.functionCall(assigned, [], "\ud800"),
    () => builder.documentBinding("\ud800", person),
  ]) {
    assert.throws(
      operation,
      (error: unknown) => {
        assert.ok(error instanceof QueryV2Error);
        assert.equal(error.code, "query_v2_host_string_unicode");
        return true;
      },
    );
  }
  assert.throws(
    () => builder.literalOperand("string", mixedAstralCanonical),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_builder_scalar_unicode");
      return true;
    },
  );

  builder.match([builder.isa(person, "entity", "smoke-person", false)]);
  assert.equal(
    builder.finalizeRows([person]).format,
    "typebridge.query-plan/v2",
  );
});

function rawDiagnosticCode(operation: () => unknown): string {
  try {
    operation();
  } catch (error) {
    assert.ok(error instanceof Error);
    const payload = JSON.parse(error.message) as { readonly code?: unknown };
    if (typeof payload.code !== "string") {
      assert.fail("native diagnostic code must be a string");
    }
    return payload.code;
  }
  assert.fail("expected native diagnostic");
}

test("raw N-API integer conversion rejects hostile values without consuming the builder", () => {
  const native = loadNative();
  const authority = native.queryV2Authority(declared, corpus.scope, corpus.profile);
  const builder = new native.NodeQueryPlanBuilder(authority);
  const source = builder.binding("source");
  const target = builder.binding("target");

  for (const depth of [
    true,
    -1,
    1.5,
    Number.POSITIVE_INFINITY,
    256,
  ]) {
    assert.equal(
      rawDiagnosticCode(() =>
        builder.reachable(
          source,
          target,
          "friendship",
          "friend",
          "friend",
          depth as unknown as number,
          1,
        ),
      ),
      "query_builder_depth_range",
    );
  }

  for (const window of [
    true as unknown as bigint,
    1 as unknown as bigint,
    -1n,
    1n << 64n,
  ]) {
    assert.equal(
      rawDiagnosticCode(() => builder.offset(window)),
      "query_builder_unsigned_integer_range",
    );
  }

  for (const scalar of [1n << 63n, -(1n << 63n) - 1n]) {
    assert.equal(
      rawDiagnosticCode(() => builder.literalOperand("long", scalar)),
      "query_builder_scalar_integer_range",
    );
  }

  const name = builder.binding("name");
  const isa = builder.isa(source, "entity", "smoke-person", false);
  const has = builder.has(source, name, "smoke-name");
  builder.match([isa, has]);
  builder.select([name]);
  builder.sort([builder.order(name, "ascending")]);
  builder.offset(0n);
  const plan = builder.finalizeRows([name]);
  assert.equal(plan.format, "typebridge.query-plan/v2");
});

test("raw N-API flags, text scalars, and labels fail canonically and atomically", () => {
  const native = loadNative();
  const authority = native.queryV2Authority(declared, corpus.scope, corpus.profile);

  const inputBuilder = new native.NodeQueryPlanBuilder(authority);
  assert.equal(
    rawDiagnosticCode(() =>
      inputBuilder.input(
        "prefix",
        "string",
        1 as unknown as boolean,
      ),
    ),
    "query_builder_boolean_host_type",
  );
  inputBuilder.input("prefix", "string", true);

  const builder = new native.NodeQueryPlanBuilder(authority);
  const person = builder.binding("person");
  assert.equal(
    rawDiagnosticCode(() =>
      builder.isa(
        person,
        "entity",
        "smoke-person",
        1 as unknown as boolean,
      ),
    ),
    "query_builder_boolean_host_type",
  );
  assert.equal(
    rawDiagnosticCode(() =>
      builder.literalOperand(
        "string",
        1 as unknown as string,
      ),
    ),
    "query_builder_scalar_host_type",
  );
  assert.equal(
    rawDiagnosticCode(() => builder.binding("")),
    "migration_assertion_invalid_variable",
  );
  assert.equal(
    rawDiagnosticCode(() =>
      builder.isa(person, "entity", "person name", false),
    ),
    "malformed_id",
  );

  const name = builder.binding("name");
  builder.match([
    builder.isa(person, "entity", "smoke-person", false),
    builder.has(person, name, "smoke-name"),
  ]);
  const plan = builder.finalizeRows([person, name]);
  assert.equal(plan.format, "typebridge.query-plan/v2");
});

test("public authoring rejects oversized arrays before element access", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const builder = new QueryPlanBuilder(authority);
  const person = builder.binding("person");
  const personIsa = builder.isa(person, "entity", "smoke-person", false);
  let accessed = false;
  const hostilePatterns = new Array(257);
  Object.defineProperty(hostilePatterns, 0, {
    get() {
      accessed = true;
      throw new Error("element access must not occur");
    },
  });

  assert.throws(
    () => builder.match(hostilePatterns as never),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_plan_pattern_limit");
      return true;
    },
  );
  assert.equal(accessed, false);

  builder.match([personIsa]);
  const plan = builder.finalizeRows([person]);
  assert.equal(plan.format, "typebridge.query-plan/v2");
});

test("public operation-specific collection limits and node budget are canonical", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const builder = new QueryPlanBuilder(authority);
  const person = builder.binding("person");
  const relation = builder.binding("relation");
  const assigned = builder.binding("assigned");
  const personIsa = builder.isa(person, "entity", "smoke-person", false);
  const operand = builder.bindingOperand(person);
  const assignment = builder.reduceAssignment(assigned, "count");
  const order = builder.order(person, "ascending");
  const tooManyPatterns = Array.from({ length: 257 }, () => personIsa);
  const tooManyBindings = Array.from({ length: 257 }, () => person);
  const cases: readonly [string, () => unknown][] = [
    [
      "query_plan_role_player_limit",
      () => builder.links(
        relation,
        "smoke-relation",
        Array.from({ length: 257 }, () => "role"),
        [person],
      ),
    ],
    [
      "query_plan_role_player_limit",
      () => builder.links(
        relation,
        "smoke-relation",
        ["role"],
        tooManyBindings,
      ),
    ],
    [
      "query_plan_negation_term_limit",
      () => builder.not(tooManyPatterns),
    ],
    [
      "query_plan_disjunction_term_limit",
      () => builder.or(
        Array.from({ length: 257 }, () => [personIsa]),
      ),
    ],
    [
      "query_plan_disjunction_term_limit",
      () => builder.or([tooManyPatterns]),
    ],
    [
      "query_plan_try_term_limit",
      () => builder.try(tooManyPatterns),
    ],
    [
      "query_plan_function_argument_limit",
      () => builder.functionCall(
        assigned,
        Array.from({ length: 257 }, () => operand),
        "unknown_function",
      ),
    ],
    [
      "query_plan_pattern_limit",
      () => builder.match(tooManyPatterns),
    ],
    [
      "query_plan_binding_limit",
      () => builder.select(tooManyBindings),
    ],
    [
      "query_plan_binding_limit",
      () => builder.require(tooManyBindings),
    ],
    [
      "query_plan_reduce_term_limit",
      () => builder.reduce(
        Array.from({ length: 257 }, () => assignment),
        [],
      ),
    ],
    [
      "query_plan_binding_limit",
      () => builder.reduce([assignment], tooManyBindings),
    ],
    [
      "query_plan_sort_term_limit",
      () => builder.sort(Array.from({ length: 65 }, () => order)),
    ],
  ];
  for (const [code, operation] of cases) {
    assert.throws(
      operation,
      (error: unknown) => {
        assert.ok(error instanceof QueryV2Error);
        assert.equal(error.code, code);
        return true;
      },
    );
  }

  const wide = builder.not(Array.from({ length: 256 }, () => personIsa));
  assert.throws(
    () => builder.not(Array.from({ length: 16 }, () => wide)),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_plan_pattern_node_limit");
      return true;
    },
  );

  builder.match([personIsa]);
  assert.equal(
    builder.finalizeRows([person]).format,
    "typebridge.query-plan/v2",
  );

  const inputBuilder = new QueryPlanBuilder(authority);
  inputBuilder.input("input", "string", true);
  const inputPerson = inputBuilder.binding("person");
  inputBuilder.match([
    inputBuilder.isa(inputPerson, "entity", "smoke-person", false),
  ]);
  const inputPlan = inputBuilder.finalizeRows([inputPerson]);
  assert.throws(
    () => inputPlan.exists([
      Array.from({ length: 257 }, () => null),
    ] as never),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_invocation_row_arity");
      return true;
    },
  );
});

test("public groupby accepts sixty-five groups and rejects the 257th", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const builder = new QueryPlanBuilder(authority);
  const owner = builder.binding("group_owner");
  const attributes = Array.from(
    { length: 64 },
    (_, index) => builder.binding(`group_${index}`),
  );
  const groups = [owner, ...attributes];
  const assigned = builder.binding("count");
  builder.match([
    builder.isa(owner, "entity", "smoke-person", false),
    ...attributes.map((attribute) =>
      builder.has(owner, attribute, "smoke-name")
    ),
  ]);
  const count = builder.reduceAssignment(assigned, "count");
  assert.throws(
    () => builder.reduce(
      [count],
      Array.from({ length: 257 }, () => groups[0]),
    ),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_plan_binding_limit");
      return true;
    },
  );
  builder.reduce([count], groups);
  assert.equal(
    builder.finalizeRows([groups[0], assigned]).format,
    "typebridge.query-plan/v2",
  );
});

test("public invocation rows are count and aggregate-byte bounded before materialization", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const { plan: noInputPlan } = publicPlan(authority);
  let accessed = false;
  const hostileRows = new Array(4_097);
  Object.defineProperty(hostileRows, 0, {
    get() {
      accessed = true;
      throw new Error("row access must not occur");
    },
  });

  assert.throws(
    () => noInputPlan.rows(hostileRows as never),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_invocation_row_limit");
      return true;
    },
  );
  assert.equal(accessed, false);
  assert.equal(noInputPlan.rows([]).operation, "rows");

  const inputBuilder = new QueryPlanBuilder(authority);
  inputBuilder.input("supplied_text", "string", false);
  const person = inputBuilder.binding("person");
  inputBuilder.match([
    inputBuilder.isa(person, "entity", "smoke-person", false),
  ]);
  const inputPlan = inputBuilder.finalizeRows([person]);

  const maxInputBytes = 4 * 1_024 * 1_024;
  const oversizedChunk = "x".repeat(Math.floor(maxInputBytes / 5) + 32);
  assert.throws(
    () => inputPlan.exists(Array.from({ length: 5 }, () => [oversizedChunk])),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_invocation_input_byte_limit");
      return true;
    },
  );

  const baseChunk = "x".repeat(Math.floor(maxInputBytes / 5) - 128);
  const exactChunks = Array.from({ length: 5 }, () => baseChunk);
  const inputWireLength = Buffer.byteLength(
    JSON.stringify(
      exactChunks.map((chunk) => [
        { kind: "string", value: chunk },
      ]),
    ),
  );
  exactChunks[exactChunks.length - 1] += "x".repeat(
    maxInputBytes - inputWireLength,
  );
  const exactRows = exactChunks.map((chunk) => [chunk]);
  assert.equal(
    Buffer.byteLength(
      JSON.stringify(
        exactChunks.map((chunk) => [
          { kind: "string", value: chunk },
        ]),
      ),
    ),
    maxInputBytes,
  );
  assert.equal(
    inputPlan.exists(exactRows).canonicalBytes.byteLength,
    maxInputBytes + 234,
  );
});

test("public collection container type is canonical across bindings", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const builder = new QueryPlanBuilder(authority);
  const person = builder.binding("person");
  const relation = builder.binding("relation");

  assert.throws(
    () => builder.links(
      relation,
      "smoke-relation",
      "role" as never,
      [person],
    ),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_builder_host_collection_type");
      return true;
    },
  );
  assert.throws(
    () => builder.match({} as never),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_builder_host_collection_type");
      return true;
    },
  );
});

test("public invocation row arity preflights before cell access", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const inputBuilder = new QueryPlanBuilder(authority);
  inputBuilder.input("supplied_text", "string", true);
  const inputPerson = inputBuilder.binding("person");
  inputBuilder.match([
    inputBuilder.isa(inputPerson, "entity", "smoke-person", false),
  ]);
  const inputPlan = inputBuilder.finalizeRows([inputPerson]);

  let wrongArityAccessed = false;
  const wrongArity = new Array(2);
  Object.defineProperty(wrongArity, 0, {
    get() {
      wrongArityAccessed = true;
      throw new Error("cell access must not occur");
    },
  });
  assert.throws(
    () => inputPlan.exists([wrongArity] as never),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_invocation_row_arity");
      return true;
    },
  );
  assert.equal(wrongArityAccessed, false);

  const { plan: noInputPlan } = publicPlan(authority);
  let noInputAccessed = false;
  const hostileNoInput = new Array(1);
  Object.defineProperty(hostileNoInput, 0, {
    get() {
      noInputAccessed = true;
      throw new Error("row access must not occur");
    },
  });
  assert.throws(
    () => noInputPlan.rows(hostileNoInput as never),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_invocation_unexpected_inputs");
      return true;
    },
  );
  assert.equal(noInputAccessed, false);
});

test("optional invocation cells require explicit null rather than undefined or holes", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);
  const builder = new QueryPlanBuilder(authority);
  builder.input("optional_name", "string", true);
  const person = builder.binding("person");
  builder.match([
    builder.isa(person, "entity", "smoke-person", false),
  ]);
  const plan = builder.finalizeRows([person]);
  const sparse: unknown[] = new Array(1);

  for (const rows of [[[undefined]], [sparse]]) {
    assert.throws(
      () => plan.exists(rows as never),
      (error: unknown) => {
        assert.ok(error instanceof QueryV2Error);
        assert.equal(error.code, "query_builder_scalar_host_type");
        return true;
      },
    );
  }

  const invocation = plan.exists([[null]]);
  assert.equal(invocation.operation, "exists");
  assert.deepEqual(
    invocation.requiredTransportCapabilities,
    ["query.input.given-rows"],
  );
});

test("public declaration and output boundaries use stable binding diagnostics", () => {
  const authority = new QueryV2Authority(declared, corpus.scope, corpus.profile);

  const bindingBuilder = new QueryPlanBuilder(authority);
  for (let index = 0; index < 256; index += 1) {
    bindingBuilder.binding(`binding_${index}`);
  }
  assert.throws(
    () => bindingBuilder.binding("binding_256"),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_builder_authored_binding_limit");
      return true;
    },
  );

  const inputBuilder = new QueryPlanBuilder(authority);
  for (let index = 0; index < 256; index += 1) {
    inputBuilder.input(`input_${index}`, "string", true);
  }
  assert.throws(
    () => inputBuilder.input("input_256", "string", true),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_plan_input_limit");
      return true;
    },
  );

  const rowBuilder = new QueryPlanBuilder(authority);
  const rowOwner = rowBuilder.binding("row_owner");
  const rowAttributes = Array.from(
    { length: 16 },
    (_, index) => rowBuilder.binding(`row_${index}`),
  );
  const rowBindings = [rowOwner, ...rowAttributes];
  rowBuilder.match([
    rowBuilder.isa(rowOwner, "entity", "smoke-person", false),
    ...rowAttributes.map((attribute) =>
      rowBuilder.has(rowOwner, attribute, "smoke-name")
    ),
  ]);
  assert.throws(
    () => rowBuilder.finalizeRows(rowBindings),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_plan_output_limit");
      return true;
    },
  );
  assert.equal(
    rowBuilder.finalizeRows(rowBindings.slice(0, 16)).format,
    "typebridge.query-plan/v2",
  );

  const documentBuilder = new QueryPlanBuilder(authority);
  const person = documentBuilder.binding("document_person");
  const name = documentBuilder.binding("document_name");
  documentBuilder.match([
    documentBuilder.isa(person, "entity", "smoke-person", false),
    documentBuilder.has(person, name, "smoke-name"),
  ]);
  const fields = Array.from(
    { length: 17 },
    (_, index) => documentBuilder.documentBinding(`field_${index}`, name),
  );
  assert.throws(
    () => documentBuilder.finalizeDocuments(fields),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.equal(error.code, "query_plan_output_limit");
      return true;
    },
  );
  assert.equal(
    documentBuilder.finalizeDocuments(fields.slice(0, 16)).format,
    "typebridge.query-plan/v2",
  );
});

test("every builder operation executes with cross-binding canonical parity", () => {
  const authority = new QueryV2Authority(
    parityDeclared,
    "model-remote-parity",
    "typedb-3.12.1/v1",
  );
  const invoked = new Set<string>();
  function invoke<T>(name: string, operation: () => T): T {
    invoked.add(name);
    return operation();
  }

  const builder = new QueryPlanBuilder(authority);
  const localPerson = invoke("binding", () => builder.binding("lp"));
  const localName = builder.binding("ln");
  const localIsa = invoke(
    "isa",
    () => builder.isa(localPerson, "entity", "parity-person", true),
  );
  const localHas = invoke(
    "has",
    () => builder.has(localPerson, localName, "parity-person-name"),
  );
  const localReturn = invoke(
    "local_return",
    () => builder.localReturn("count", localName, "long"),
  );
  const localFunction = invoke(
    "local_function",
    () => builder.localFunction(
      "local_name_count",
      [localName, localPerson],
      [localPerson],
      ["parity-person"],
      [localIsa, localHas],
      localReturn,
    ),
  );

  const person = builder.binding("person");
  const name = builder.binding("name");
  const optionalName = builder.binding("optional_name");
  const localResult = builder.binding("local_result");
  const countResult = builder.binding("count_result");
  const wantedName = invoke(
    "input",
    () => builder.input("wanted_name", "string", false),
  );
  const personIsa = builder.isa(person, "entity", "parity-person", true);
  const nameHas = builder.has(person, name, "parity-person-name");
  const nameOperand = invoke(
    "binding_operand",
    () => builder.bindingOperand(name),
  );
  const inputOperand = invoke(
    "input_operand",
    () => builder.inputOperand(wantedName),
  );
  const nobody = invoke(
    "literal_operand",
    () => builder.literalOperand("string", "nobody"),
  );
  const equal = invoke(
    "value",
    () => builder.value("equal", nameOperand, inputOperand),
  );
  const notEqual = builder.value("not_equal", nameOperand, nobody);
  const disjunction = invoke(
    "or",
    () => builder.or([[equal], [notEqual]]),
  );
  const negation = invoke(
    "not",
    () => builder.not([builder.value("equal", nameOperand, nobody)]),
  );
  const optional = invoke(
    "try",
    () => builder.try([
      builder.has(person, optionalName, "parity-person-name"),
    ]),
  );
  const localCall = invoke(
    "function_call",
    () => builder.functionCall(
      localResult,
      [builder.bindingOperand(person)],
      null,
      localFunction,
    ),
  );
  invoke(
    "match",
    () => builder.match([
      personIsa,
      nameHas,
      disjunction,
      negation,
      optional,
      localCall,
    ]),
  );
  invoke("select", () => builder.select([person, name, localResult]));
  invoke("require", () => builder.require([name]));
  invoke("distinct", () => builder.distinct());
  const count = invoke(
    "reduce_assignment",
    () => builder.reduceAssignment(countResult, "count"),
  );
  invoke("reduce", () => builder.reduce([count], [name]));
  const nameOrder = invoke(
    "order",
    () => builder.order(name, "ascending"),
  );
  const countOrder = builder.order(countResult, "descending");
  invoke("sort", () => builder.sort([nameOrder, countOrder]));
  invoke("offset", () => builder.offset(0n));
  invoke("limit", () => builder.limit(10n));
  const advanced = invoke(
    "finalize_rows",
    () => builder.finalizeRows([name, countResult]),
  );

  const relationBuilder = new QueryPlanBuilder(authority);
  const source = relationBuilder.binding("source");
  const target = relationBuilder.binding("target");
  const assignment = relationBuilder.binding("assignment");
  const sourceIsa = relationBuilder.isa(
    source,
    "entity",
    "parity-person",
    true,
  );
  const targetIsa = relationBuilder.isa(
    target,
    "entity",
    "parity-project",
    false,
  );
  const links = invoke(
    "links",
    () => relationBuilder.links(
      assignment,
      "parity-assignment",
      ["employee", "project"],
      [source, target],
    ),
  );
  const reachable = invoke(
    "reachable",
    () => relationBuilder.reachable(
      source,
      target,
      "parity-assignment",
      "employee",
      "project",
      1,
      1,
    ),
  );
  relationBuilder.match([sourceIsa, targetIsa, links, reachable]);
  const relation = relationBuilder.finalizeRows([source, target, assignment]);

  const documentBuilder = new QueryPlanBuilder(authority);
  const documentPerson = documentBuilder.binding("person");
  const documentName = documentBuilder.binding("name");
  documentBuilder.match([
    documentBuilder.isa(
      documentPerson,
      "entity",
      "parity-person",
      true,
    ),
    documentBuilder.has(
      documentPerson,
      documentName,
      "parity-person-name",
    ),
  ]);
  const scalar = invoke(
    "document_binding",
    () => documentBuilder.documentBinding("primary_name", documentName),
  );
  const attributeList = invoke(
    "document_attribute_list",
    () => documentBuilder.documentAttributeList(
      "all_names",
      documentPerson,
      "parity-person-name",
    ),
  );
  const documents = invoke(
    "finalize_documents",
    () => documentBuilder.finalizeDocuments([scalar, attributeList]),
  );

  assert.deepEqual(invoked, builderOperations);
  assert.equal(
    advanced.fingerprint,
    "85c9504dca956286b46336510af3b24980bba1a72e79465069b7a24e7d52e26f",
  );
  assert.equal(
    relation.fingerprint,
    "0c955b27ba7df589499245fcc8d47f1a14e555a34c15fe8177c07bb8c4293aa8",
  );
  assert.equal(
    documents.fingerprint,
    "e25be2c81dd1c2252967d889e001a713942d8850af3ae232086bac295752f731",
  );
  assert.equal(
    advanced.rows([["Alice"]]).planFingerprint,
    advanced.fingerprint,
  );
  assert.equal(
    documents.documents([]).planFingerprint,
    documents.fingerprint,
  );
});

test("reducer error preserves the complete shared diagnostic", () => {
  const authority = new QueryV2Authority(
    parityDeclared,
    "model-remote-parity",
    "typedb-3.12.1/v1",
  );
  const builder = new QueryPlanBuilder(authority);
  const assigned = builder.binding("assigned");
  assert.throws(
    () => builder.reduceAssignment(assigned, "max", null as never),
    (error: unknown) => {
      assert.ok(error instanceof QueryV2Error);
      assert.deepEqual(
        {
          category: error.category,
          code: error.code,
          message: error.diagnosticMessage,
          path: error.path,
          details: error.details,
        },
        {
          category: "invalid_contract",
          code: "query_plan_reduce_missing_input",
          message: "count takes no input and every other reducer requires one",
          path: [],
          details: {},
        },
      );
      return true;
    },
  );
});

test("documented Node low-level example typechecks and executes verbatim", () => {
  const snippet = documentedExample(
    "The equivalent Node authoring uses the same operation names and order:",
    "typescript",
  );
  const temporary = fs.mkdtempSync(
    path.join(os.tmpdir(), "type-bridge-query-v2-docs-"),
  );
  try {
    const examplePath = path.join(temporary, "query-v2-authoring.ts");
    fs.writeFileSync(examplePath, snippet, "utf8");
    const nodeRoot = process.cwd();
    const program = typescript.createProgram(
      [examplePath],
      {
        baseUrl: nodeRoot,
        module: typescript.ModuleKind.NodeNext,
        moduleResolution: typescript.ModuleResolutionKind.NodeNext,
        noEmit: true,
        paths: {
          "@type-bridge/node/query-v2": [
            path.join(nodeRoot, "typescript/query-v2.ts"),
          ],
        },
        strict: true,
        target: typescript.ScriptTarget.ES2022,
        typeRoots: [path.join(nodeRoot, "node_modules/@types")],
        types: ["node"],
      },
    );
    const diagnostics = typescript.getPreEmitDiagnostics(program);
    assert.equal(
      diagnostics.length,
      0,
      formatTypeScriptDiagnostics(diagnostics),
    );

    const transpiled = typescript.transpileModule(snippet, {
      compilerOptions: {
        module: typescript.ModuleKind.CommonJS,
        target: typescript.ScriptTarget.ES2022,
      },
      fileName: examplePath,
      reportDiagnostics: true,
    });
    assert.equal(
      transpiled.diagnostics?.length ?? 0,
      0,
      formatTypeScriptDiagnostics(transpiled.diagnostics ?? []),
    );

    const sandbox: {
      readonly exports: Record<string, unknown>;
      readonly module: { exports: Record<string, unknown> };
      readonly require: (request: string) => unknown;
      docsResult?: {
        readonly plan: { readonly format: string; readonly fingerprint: string };
        readonly invocation: { readonly planFingerprint: string };
      };
    } = {
      exports: {},
      module: { exports: {} },
      require(request: string): unknown {
        if (request === "node:fs") {
          return { readFileSync: () => declared };
        }
        if (request === "@type-bridge/node/query-v2") {
          return queryV2Authoring;
        }
        throw new Error(`unexpected documentation example import: ${request}`);
      },
    };
    vm.runInNewContext(
      `${transpiled.outputText}\nglobalThis.docsResult = { plan, invocation };\n`,
      sandbox,
      { filename: examplePath },
    );
    assert.equal(sandbox.docsResult?.plan.format, "typebridge.query-plan/v2");
    assert.equal(
      sandbox.docsResult?.invocation.planFingerprint,
      sandbox.docsResult?.plan.fingerprint,
    );
  } finally {
    fs.rmSync(temporary, { force: true, recursive: true });
  }
});
