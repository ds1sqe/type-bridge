import assert = require("node:assert/strict");
import fs = require("node:fs");
import path = require("node:path");
import test = require("node:test");

import { QueryV2Error } from "../../typescript/index.js";
import {
  AuthoredQueryInvocation,
  AuthoredQueryPlan,
  QueryPlanBuilder,
  QueryV2Authority,
} from "../../typescript/query-v2.js";

type JsonRecord = Readonly<Record<string, unknown>>;
type Scalar = string | bigint | number | boolean;

interface ExpectedPlan {
  readonly canonical_b64: string;
  readonly fingerprint: string;
}

interface ExpectedInvocation {
  readonly canonical_b64: string;
  readonly required_transport_capabilities: readonly string[];
}

interface InvocationCase {
  readonly id: string;
  readonly terminal: string;
  readonly rows: readonly (readonly (JsonRecord | null)[])[];
  readonly expected: ExpectedInvocation;
}

interface PlanCase {
  readonly id: string;
  readonly covers: readonly string[];
  readonly steps: readonly JsonRecord[];
  readonly invocations: readonly InvocationCase[];
  readonly expected: ExpectedPlan;
}

interface DiagnosticCase {
  readonly id: string;
  readonly kind: "builder" | "invocation";
  readonly steps?: readonly JsonRecord[];
  readonly failure?: JsonRecord;
  readonly plan?: string;
  readonly terminal?: string;
  readonly rows?: readonly (readonly (JsonRecord | null)[])[];
  readonly expected: JsonRecord;
}

interface InventoryCorpus {
  readonly authority: {
    readonly declared: string;
    readonly scope: string;
    readonly profile: string;
  };
  readonly inventory: {
    readonly builder_operations: readonly string[];
    readonly coverage: Readonly<Record<string, readonly string[]>>;
    readonly invocation_terminals: readonly string[];
    readonly diagnostic_cases: readonly string[];
  };
  readonly plans: readonly PlanCase[];
  readonly diagnostics: readonly DiagnosticCase[];
}

const fixtureDirectory = path.resolve(process.cwd(), "../../../tests/fixtures");
const fixturePath = path.join(
  fixtureDirectory,
  "query-v2-authoring-inventory.json",
);
const corpus = JSON.parse(
  fs.readFileSync(fixturePath, "utf8"),
) as InventoryCorpus;
const declaredWithNewline = fs.readFileSync(
  path.join(fixtureDirectory, corpus.authority.declared),
);
const declared = declaredWithNewline.at(-1) === 0x0a
  ? declaredWithNewline.subarray(0, -1)
  : declaredWithNewline;

function text(value: JsonRecord, key: string): string {
  const field = value[key];
  assert.equal(typeof field, "string", `${key} must be text`);
  return field as string;
}

function nullableText(value: JsonRecord, key: string): string | null {
  const field = value[key];
  assert.ok(field === null || typeof field === "string", `${key} must be text or null`);
  return field as string | null;
}

function flag(value: JsonRecord, key: string): boolean {
  const field = value[key];
  assert.equal(typeof field, "boolean", `${key} must be boolean`);
  return field as boolean;
}

function integer(value: JsonRecord, key: string): number {
  const field = value[key];
  assert.equal(typeof field, "number", `${key} must be numeric`);
  assert.equal(Number.isInteger(field), true, `${key} must be integral`);
  return field as number;
}

function strings(value: JsonRecord, key: string): readonly string[] {
  const field = value[key];
  if (
    typeof field === "object"
    && field !== null
    && !Array.isArray(field)
  ) {
    const repeated = (field as JsonRecord)["repeat"];
    const count = (field as JsonRecord)["count"];
    assert.equal(typeof repeated, "string", `${key}.repeat must be text`);
    assert.equal(typeof count, "number", `${key}.count must be numeric`);
    assert.equal(Number.isSafeInteger(count), true, `${key}.count must be integral`);
    assert.ok((count as number) >= 0, `${key}.count must be non-negative`);
    return Array.from({ length: count as number }, () => repeated as string);
  }
  assert.ok(Array.isArray(field), `${key} must be an array`);
  for (const item of field) {
    assert.equal(typeof item, "string", `${key} entries must be text`);
  }
  return field as string[];
}

function nestedStrings(
  value: JsonRecord,
  key: string,
): readonly (readonly string[])[] {
  const field = value[key];
  assert.ok(Array.isArray(field), `${key} must be an array`);
  return field.map((branch) => {
    assert.ok(Array.isArray(branch), `${key} branches must be arrays`);
    for (const item of branch) {
      assert.equal(typeof item, "string", `${key} entries must be text`);
    }
    return branch as string[];
  });
}

function handle(handles: ReadonlyMap<string, unknown>, name: string): never {
  assert.equal(handles.has(name), true, `unknown inventory handle: ${name}`);
  return handles.get(name) as never;
}

function handles(
  known: ReadonlyMap<string, unknown>,
  names: readonly string[],
): never[] {
  return names.map((name) => handle(known, name));
}

function scalar(valueType: string, value: unknown): Scalar {
  if (valueType === "long") {
    return BigInt(String(value));
  }
  assert.ok(
    typeof value === "string"
      || typeof value === "number"
      || typeof value === "boolean",
    "inventory scalar must be a public host scalar",
  );
  return value;
}

function invocationRows(
  rows: readonly (readonly (JsonRecord | null)[])[],
): (Scalar | null)[][] {
  return rows.map((row) =>
    row.map((cell) =>
      cell === null
        ? null
        : scalar(text(cell, "type"), cell["value"])
    )
  );
}

function executeStep(
  builder: QueryPlanBuilder,
  known: Map<string, unknown>,
  step: JsonRecord,
): unknown {
  const op = text(step, "op");
  let result: unknown;
  switch (op) {
    case "binding":
      result = builder.binding(text(step, "name"));
      break;
    case "input":
      result = builder.input(
        text(step, "name"),
        text(step, "value_type") as never,
        flag(step, "optional"),
      );
      break;
    case "binding_operand":
      result = builder.bindingOperand(handle(known, text(step, "binding")));
      break;
    case "literal_operand":
      result = builder.literalOperand(
        text(step, "value_type") as never,
        scalar(text(step, "value_type"), step["value"]) as never,
      );
      break;
    case "input_operand":
      result = builder.inputOperand(handle(known, text(step, "input")));
      break;
    case "isa":
      result = builder.isa(
        handle(known, text(step, "binding")),
        text(step, "type_kind") as never,
        text(step, "type_label"),
        flag(step, "include_subtypes"),
      );
      break;
    case "has":
      result = builder.has(
        handle(known, text(step, "owner")),
        handle(known, text(step, "attribute")),
        text(step, "attribute_label"),
      );
      break;
    case "links":
      result = builder.links(
        handle(known, text(step, "relation")),
        text(step, "relation_label"),
        strings(step, "roles"),
        handles(known, strings(step, "players")),
      );
      break;
    case "value":
      result = builder.value(
        text(step, "comparator") as never,
        handle(known, text(step, "left")),
        handle(known, text(step, "right")),
      );
      break;
    case "not": {
      const repeat = step["repeat"] === undefined ? 1 : integer(step, "repeat");
      assert.ok(repeat > 0, "negation repeat must be positive");
      let nested = handles(known, strings(step, "patterns"));
      for (let index = 0; index < repeat; index += 1) {
        result = builder.not(nested);
        nested = [result as never];
      }
      break;
    }
    case "or":
      result = builder.or(
        nestedStrings(step, "branches").map((branch) => handles(known, branch)),
      );
      break;
    case "try":
      result = builder.try(handles(known, strings(step, "patterns")));
      break;
    case "reachable":
      result = builder.reachable(
        handle(known, text(step, "source")),
        handle(known, text(step, "target")),
        text(step, "relation_label"),
        text(step, "role_from"),
        text(step, "role_to"),
        integer(step, "min_depth"),
        integer(step, "max_depth"),
      );
      break;
    case "function_call": {
      const localName = nullableText(step, "local_function");
      const call = builder.functionCall.bind(builder) as (
        assigned: never,
        arguments_: never[],
        functionName: string | null,
        localFunction: never | null,
      ) => unknown;
      result = call(
        handle(known, text(step, "assigned")),
        handles(known, strings(step, "arguments")),
        nullableText(step, "function_name"),
        localName === null ? null : handle(known, localName),
      );
      break;
    }
    case "order":
      result = builder.order(
        handle(known, text(step, "binding")),
        text(step, "direction") as never,
      );
      break;
    case "reduce_assignment": {
      const inputName = step["input"];
      assert.ok(
        inputName === undefined || inputName === null || typeof inputName === "string",
        "input must be an optional handle name",
      );
      const assign = builder.reduceAssignment.bind(builder) as (
        assigned: never,
        reducer: string,
        input: never | null,
      ) => unknown;
      result = assign(
        handle(known, text(step, "assigned")),
        text(step, "reducer"),
        typeof inputName === "string" ? handle(known, inputName) : null,
      );
      break;
    }
    case "local_return": {
      const localReturn = builder.localReturn.bind(builder) as (
        reducer: string,
        input: never,
        valueType: string,
      ) => unknown;
      result = localReturn(
        text(step, "reducer"),
        handle(known, text(step, "input")),
        text(step, "value_type"),
      );
      break;
    }
    case "local_function":
      result = builder.localFunction(
        text(step, "name"),
        handles(known, strings(step, "bindings")),
        handles(known, strings(step, "parameter_bindings")),
        strings(step, "parameter_labels"),
        handles(known, strings(step, "body")),
        handle(known, text(step, "returns")),
      );
      break;
    case "match":
      builder.match(handles(known, strings(step, "patterns")));
      result = undefined;
      break;
    case "select":
      builder.select(handles(known, strings(step, "bindings")));
      result = undefined;
      break;
    case "require":
      builder.require(handles(known, strings(step, "bindings")));
      result = undefined;
      break;
    case "distinct":
      builder.distinct();
      result = undefined;
      break;
    case "reduce":
      builder.reduce(
        handles(known, strings(step, "assignments")),
        handles(known, strings(step, "groups")),
      );
      result = undefined;
      break;
    case "sort":
      builder.sort(handles(known, strings(step, "terms")));
      result = undefined;
      break;
    case "offset":
      builder.offset(BigInt(integer(step, "rows")));
      result = undefined;
      break;
    case "limit":
      builder.limit(BigInt(integer(step, "rows")));
      result = undefined;
      break;
    case "document_binding":
      result = builder.documentBinding(
        text(step, "key"),
        handle(known, text(step, "binding")),
      );
      break;
    case "document_attribute_list":
      result = builder.documentAttributeList(
        text(step, "key"),
        handle(known, text(step, "owner")),
        text(step, "attribute_label"),
      );
      break;
    case "finalize_rows":
      result = builder.finalizeRows(handles(known, strings(step, "bindings")));
      break;
    case "finalize_documents":
      result = builder.finalizeDocuments(handles(known, strings(step, "fields")));
      break;
    default:
      assert.fail(`unknown inventory operation: ${op}`);
  }
  const id = step["id"];
  if (id !== undefined) {
    assert.equal(typeof id, "string", "inventory handle id must be text");
    assert.notEqual(result, undefined, `${op} declared an id without a handle`);
    known.set(id as string, result);
  }
  return result;
}

function executePlan(
  authority: QueryV2Authority,
  planCase: PlanCase,
): AuthoredQueryPlan {
  const builder = new QueryPlanBuilder(authority);
  const known = new Map<string, unknown>();
  for (const step of planCase.steps) {
    executeStep(builder, known, step);
  }
  const plan = known.get("plan");
  assert.ok(plan instanceof AuthoredQueryPlan);
  return plan;
}

function invoke(
  plan: AuthoredQueryPlan,
  terminal: string,
  rows: readonly (readonly (JsonRecord | null)[])[],
): AuthoredQueryInvocation {
  const converted = invocationRows(rows);
  let invocation: AuthoredQueryInvocation;
  switch (terminal) {
    case "rows":
      invocation = plan.rows(converted);
      break;
    case "documents":
      invocation = plan.documents(converted);
      break;
    case "count":
      invocation = plan.count(converted);
      break;
    case "exists":
      invocation = plan.exists(converted);
      break;
    default:
      assert.fail(`unknown inventory terminal: ${terminal}`);
  }
  assert.ok(invocation instanceof AuthoredQueryInvocation);
  return invocation;
}

function authority(): QueryV2Authority {
  return new QueryV2Authority(
    declared,
    corpus.authority.scope,
    corpus.authority.profile,
  );
}

function sorted(values: Iterable<string>): string[] {
  return [...values].sort();
}

test("shared inventory names every public operation and required variant", () => {
  const operations = new Set(
    corpus.plans.flatMap((planCase) =>
      planCase.steps.map((step) => text(step, "op"))
    ),
  );
  assert.deepEqual(
    sorted(operations),
    sorted(corpus.inventory.builder_operations),
  );

  const expectedCoverage = new Set(
    Object.entries(corpus.inventory.coverage).flatMap(([category, variants]) =>
      variants.map((variant) => `${category}:${variant}`)
    ),
  );
  const actualCoverage = new Set(
    corpus.plans.flatMap((planCase) => planCase.covers),
  );
  assert.deepEqual(sorted(actualCoverage), sorted(expectedCoverage));

  const terminals = new Set(
    corpus.plans.flatMap((planCase) =>
      planCase.invocations.map((invocationCase) => invocationCase.terminal)
    ),
  );
  assert.deepEqual(
    sorted(terminals),
    sorted(corpus.inventory.invocation_terminals),
  );
  assert.deepEqual(
    sorted(corpus.diagnostics.map((diagnostic) => diagnostic.id)),
    sorted(corpus.inventory.diagnostic_cases),
  );
});

test("public Node facade matches every fixed Rust authority vector", () => {
  const sharedAuthority = authority();
  for (const planCase of corpus.plans) {
    const plan = executePlan(sharedAuthority, planCase);
    const expectedBytes = Buffer.from(planCase.expected.canonical_b64, "base64");
    const expectedWire = JSON.parse(expectedBytes.toString("utf8")) as {
      readonly required_capabilities: readonly string[];
    };
    assert.deepEqual(Buffer.from(plan.canonicalBytes), expectedBytes, planCase.id);
    assert.equal(plan.fingerprint, planCase.expected.fingerprint, planCase.id);
    assert.deepEqual(
      plan.requiredCapabilities,
      expectedWire.required_capabilities,
      planCase.id,
    );

    for (const invocationCase of planCase.invocations) {
      const invocation = invoke(
        plan,
        invocationCase.terminal,
        invocationCase.rows,
      );
      const expectedInvocationBytes = Buffer.from(
        invocationCase.expected.canonical_b64,
        "base64",
      );
      const expectedInvocationWire = JSON.parse(
        expectedInvocationBytes.toString("utf8"),
      ) as {
        readonly operation: "rows" | "count" | "exists";
        readonly plan_fingerprint: { readonly digest: string };
      };
      assert.deepEqual(
        Buffer.from(invocation.canonicalBytes),
        expectedInvocationBytes,
        `${planCase.id}/${invocationCase.id}`,
      );
      assert.equal(invocation.operation, expectedInvocationWire.operation);
      assert.equal(
        invocation.planFingerprint,
        expectedInvocationWire.plan_fingerprint.digest,
      );
      assert.deepEqual(
        invocation.requiredTransportCapabilities,
        invocationCase.expected.required_transport_capabilities,
      );
    }
  }
});

test("public Node facade preserves every complete inventory diagnostic", () => {
  const sharedAuthority = authority();
  const plans = new Map(
    corpus.plans.map((planCase) => [
      planCase.id,
      executePlan(sharedAuthority, planCase),
    ]),
  );
  for (const diagnostic of corpus.diagnostics) {
    assert.throws(
      () => {
        if (diagnostic.kind === "builder") {
          assert.ok(diagnostic.steps !== undefined);
          assert.ok(diagnostic.failure !== undefined);
          const builder = new QueryPlanBuilder(sharedAuthority);
          const known = new Map<string, unknown>();
          for (const step of diagnostic.steps) {
            executeStep(builder, known, step);
          }
          executeStep(builder, known, diagnostic.failure);
        } else {
          assert.ok(diagnostic.plan !== undefined);
          assert.ok(diagnostic.terminal !== undefined);
          assert.ok(diagnostic.rows !== undefined);
          const plan = plans.get(diagnostic.plan);
          assert.ok(plan !== undefined);
          invoke(plan, diagnostic.terminal, diagnostic.rows);
        }
      },
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
          diagnostic.expected,
          diagnostic.id,
        );
        return true;
      },
      diagnostic.id,
    );
  }
});
