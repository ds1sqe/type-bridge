/** Shared, package-private machinery for the prepared and authored V2 facades. */

declare const nativeQueryV2AuthorityKind: unique symbol;

/** Opaque N-API authority. Public callers use the stable QueryV2Authority class. */
export interface NativeQueryV2Authority {
  readonly [nativeQueryV2AuthorityKind]: "query-v2-authority";
}

declare const nativeQueryV2BuilderHandleKind: unique symbol;

interface NativeOpaqueClass<Instance> {
  readonly prototype: Instance;
}

type NativeQueryV2Scalar = string | bigint | number | boolean;

export interface NativeQueryV2AuthorityIdentity {
  readonly [nativeQueryV2BuilderHandleKind]: "authority-identity";
  sameAuthority(other: NativeQueryV2AuthorityIdentity): boolean;
}

export interface NativeQueryV2BindingHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "binding";
}

export interface NativeQueryV2InputHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "input";
}

export interface NativeQueryV2OperandHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "operand";
}

export interface NativeQueryV2PatternHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "pattern";
}

export interface NativeQueryV2OrderHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "order";
}

export interface NativeQueryV2ReduceAssignmentHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "reduce-assignment";
}

export interface NativeQueryV2LocalReturnHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "local-return";
}

export interface NativeQueryV2LocalFunctionHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "local-function";
}

export interface NativeQueryV2DocumentFieldHandle {
  readonly [nativeQueryV2BuilderHandleKind]: "document-field";
}

export interface NativeAuthoredQueryInvocation {
  readonly canonicalBytes: Uint8Array;
  readonly operation: "rows" | "count" | "exists";
  readonly planFingerprint: string;
  readonly authorityIdentity: NativeQueryV2AuthorityIdentity;
  readonly requiredTransportCapabilities: string[];
}

export interface NativeAuthoredQueryPlan {
  readonly canonicalBytes: Uint8Array;
  readonly format: "typebridge.query-plan/v2";
  readonly fingerprint: string;
  readonly requiredCapabilities: string[];
  readonly authorityIdentity: NativeQueryV2AuthorityIdentity;
  rows(rows: (NativeQueryV2Scalar | null)[][]): NativeAuthoredQueryInvocation;
  documents(
    rows: (NativeQueryV2Scalar | null)[][],
  ): NativeAuthoredQueryInvocation;
  count(rows: (NativeQueryV2Scalar | null)[][]): NativeAuthoredQueryInvocation;
  exists(rows: (NativeQueryV2Scalar | null)[][]): NativeAuthoredQueryInvocation;
}

export interface NativeQueryPlanBuilder {
  binding(variable: string): NativeQueryV2BindingHandle;
  input(
    publicName: string,
    valueType: string,
    optional: boolean,
  ): NativeQueryV2InputHandle;
  bindingOperand(
    binding: NativeQueryV2BindingHandle,
  ): NativeQueryV2OperandHandle;
  literalOperand(valueType: string, value: unknown): NativeQueryV2OperandHandle;
  inputOperand(input: NativeQueryV2InputHandle): NativeQueryV2OperandHandle;
  isa(
    binding: NativeQueryV2BindingHandle,
    typeKind: string,
    typeLabel: string,
    includeSubtypes: boolean,
  ): NativeQueryV2PatternHandle;
  has(
    owner: NativeQueryV2BindingHandle,
    attribute: NativeQueryV2BindingHandle,
    attributeLabel: string,
  ): NativeQueryV2PatternHandle;
  links(
    relation: NativeQueryV2BindingHandle,
    relationLabel: string,
    roles: string[],
    players: NativeQueryV2BindingHandle[],
  ): NativeQueryV2PatternHandle;
  value(
    comparator: string,
    left: NativeQueryV2OperandHandle,
    right: NativeQueryV2OperandHandle,
  ): NativeQueryV2PatternHandle;
  not(patterns: NativeQueryV2PatternHandle[]): NativeQueryV2PatternHandle;
  or(branches: NativeQueryV2PatternHandle[][]): NativeQueryV2PatternHandle;
  try(patterns: NativeQueryV2PatternHandle[]): NativeQueryV2PatternHandle;
  reachable(
    source: NativeQueryV2BindingHandle,
    target: NativeQueryV2BindingHandle,
    relationLabel: string,
    roleFrom: string,
    roleTo: string,
    minDepth: number,
    maxDepth: number,
  ): NativeQueryV2PatternHandle;
  functionCall(
    assigned: NativeQueryV2BindingHandle,
    arguments_: NativeQueryV2OperandHandle[],
    functionName?: string | null,
    localFunction?: NativeQueryV2LocalFunctionHandle | null,
  ): NativeQueryV2PatternHandle;
  order(
    binding: NativeQueryV2BindingHandle,
    direction: string,
  ): NativeQueryV2OrderHandle;
  reduceAssignment(
    assigned: NativeQueryV2BindingHandle,
    reducer: string,
    input?: NativeQueryV2BindingHandle | null,
  ): NativeQueryV2ReduceAssignmentHandle;
  localReturn(
    reducer: string,
    input: NativeQueryV2BindingHandle,
    valueType: string,
  ): NativeQueryV2LocalReturnHandle;
  localFunction(
    name: string,
    bindings: NativeQueryV2BindingHandle[],
    parameterBindings: NativeQueryV2BindingHandle[],
    parameterLabels: string[],
    body: NativeQueryV2PatternHandle[],
    returns: NativeQueryV2LocalReturnHandle,
  ): NativeQueryV2LocalFunctionHandle;
  match(patterns: NativeQueryV2PatternHandle[]): void;
  select(bindings: NativeQueryV2BindingHandle[]): void;
  require(bindings: NativeQueryV2BindingHandle[]): void;
  distinct(): void;
  reduce(
    assignments: NativeQueryV2ReduceAssignmentHandle[],
    groups: NativeQueryV2BindingHandle[],
  ): void;
  sort(terms: NativeQueryV2OrderHandle[]): void;
  offset(rows: bigint): void;
  limit(rows: bigint): void;
  documentBinding(
    key: string,
    binding: NativeQueryV2BindingHandle,
  ): NativeQueryV2DocumentFieldHandle;
  documentAttributeList(
    key: string,
    owner: NativeQueryV2BindingHandle,
    attributeLabel: string,
  ): NativeQueryV2DocumentFieldHandle;
  finalizeRows(columns: NativeQueryV2BindingHandle[]): NativeAuthoredQueryPlan;
  finalizeDocuments(
    fields: NativeQueryV2DocumentFieldHandle[],
  ): NativeAuthoredQueryPlan;
}

export interface NativeQueryV2BuilderRuntime {
  NodeQueryPlanBuilder: new (
    authority: NativeQueryV2Authority,
  ) => NativeQueryPlanBuilder;
  NodeAuthoredQueryPlan: NativeOpaqueClass<NativeAuthoredQueryPlan>;
  NodeAuthoredQueryInvocation: NativeOpaqueClass<NativeAuthoredQueryInvocation>;
  NodeQueryV2AuthorityIdentity: NativeOpaqueClass<NativeQueryV2AuthorityIdentity>;
  NodeQueryV2BindingHandle: NativeOpaqueClass<NativeQueryV2BindingHandle>;
  NodeQueryV2InputHandle: NativeOpaqueClass<NativeQueryV2InputHandle>;
  NodeQueryV2OperandHandle: NativeOpaqueClass<NativeQueryV2OperandHandle>;
  NodeQueryV2PatternHandle: NativeOpaqueClass<NativeQueryV2PatternHandle>;
  NodeQueryV2OrderHandle: NativeOpaqueClass<NativeQueryV2OrderHandle>;
  NodeQueryV2ReduceAssignmentHandle: NativeOpaqueClass<NativeQueryV2ReduceAssignmentHandle>;
  NodeQueryV2LocalReturnHandle: NativeOpaqueClass<NativeQueryV2LocalReturnHandle>;
  NodeQueryV2LocalFunctionHandle: NativeOpaqueClass<NativeQueryV2LocalFunctionHandle>;
  NodeQueryV2DocumentFieldHandle: NativeOpaqueClass<NativeQueryV2DocumentFieldHandle>;
}

const queryV2AuthorityHandles = new WeakMap<object, NativeQueryV2Authority>();

export function registerQueryV2AuthorityHandle(
  authority: object,
  native: NativeQueryV2Authority,
): void {
  queryV2AuthorityHandles.set(authority, native);
}

export function queryV2AuthorityHandle(
  authority: object,
): NativeQueryV2Authority | undefined {
  return queryV2AuthorityHandles.get(authority);
}

/** Stable V2 contract diagnostic categories. */
export type QueryV2ErrorCategory =
  | "invalid_contract"
  | "invalid_input"
  | "invalid_plan"
  | "cardinality"
  | "unsupported_capability"
  | "stale_schema"
  | "resource_limit"
  | "integrity"
  | "cancelled"
  | "provider"
  | "result_decode"
  | "transaction"
  | "internal";

/** One typed location inside a rejected V2 contract. */
export type QueryV2ErrorPathSegment =
  | Readonly<{
      kind:
        | "request"
        | "plan"
        | "operation"
        | "predicate"
        | "output"
        | "provider_evidence"
        | "result"
        | "unknown";
    }>
  | Readonly<{
      kind:
        | "argument"
        | "identifier"
        | "type"
        | "contract_field"
        | "contract_identity";
      value: string | Readonly<Record<string, unknown>>;
    }>
  | Readonly<{
      kind: "field" | "role";
      value: string | Readonly<Record<string, unknown>>;
    }>
  | Readonly<{
      kind: "index" | "binding" | "role_edge" | "output_slot";
      value: number;
    }>
  | Readonly<{ kind: "output_name"; value: string }>;

/** One deterministic structured V2 diagnostic detail. */
export type QueryV2ErrorDetail =
  | Readonly<{
      kind:
        | "text"
        | "long"
        | "signed"
        | "unsigned"
        | "count"
        | "byte_count"
        | "query_identity";
      value: string;
    }>
  | Readonly<{ kind: "boolean"; value: boolean }>
  | Readonly<{
      kind: "text_list" | "query_identity_list";
      value: readonly string[];
    }>;

/** Structured diagnostic preserved from the Rust V2 semantic engine. */
export class QueryV2Error extends Error {
  readonly name = "QueryV2Error";

  constructor(
    readonly category: QueryV2ErrorCategory,
    readonly code: string,
    readonly diagnosticMessage: string,
    readonly path: readonly QueryV2ErrorPathSegment[],
    readonly details: Readonly<Record<string, QueryV2ErrorDetail>>,
    readonly sdkCategory: QueryV2ErrorCategory = category,
    readonly queryCategory: QueryV2ErrorCategory | null = category,
  ) {
    super(`${code}: ${diagnosticMessage}`);
  }
}

interface NativeQueryV2ErrorPayload {
  readonly category: QueryV2ErrorCategory;
  readonly sdkCategory: QueryV2ErrorCategory;
  readonly queryCategory: QueryV2ErrorCategory | null;
  readonly code: string;
  readonly message: string;
  readonly path: readonly QueryV2ErrorPathSegment[];
  readonly details: Readonly<Record<string, QueryV2ErrorDetail>>;
}

interface LegacyNativeQueryV2ErrorPayload {
  readonly category: QueryV2ErrorCategory;
  readonly code: string;
  readonly message: string;
  readonly path: readonly QueryV2ErrorPathSegment[];
  readonly details: Readonly<Record<string, QueryV2ErrorDetail>>;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(
  value: Readonly<Record<string, unknown>>,
  expected: readonly string[],
): boolean {
  const keys = Object.keys(value);
  return (
    keys.length === expected.length &&
    expected.every((key) => keys.includes(key))
  );
}

function isQueryV2ErrorCategory(value: unknown): value is QueryV2ErrorCategory {
  return [
    "invalid_contract",
    "invalid_input",
    "invalid_plan",
    "cardinality",
    "unsupported_capability",
    "stale_schema",
    "resource_limit",
    "integrity",
    "cancelled",
    "provider",
    "result_decode",
    "transaction",
    "internal",
  ].includes(value as string);
}

function isAdmissibleNativeQueryV2ErrorCategory(
  value: unknown,
): value is QueryV2ErrorCategory {
  // Native internal failures are deliberately not projected as stable public
  // diagnostics: their text is not part of the redacted query contract.
  return isQueryV2ErrorCategory(value) && value !== "internal";
}

function isQueryV2ErrorPathSegment(
  value: unknown,
): value is QueryV2ErrorPathSegment {
  if (!isRecord(value) || typeof value.kind !== "string") {
    return false;
  }
  if (
    [
      "request",
      "plan",
      "operation",
      "predicate",
      "output",
      "provider_evidence",
      "result",
      "unknown",
    ].includes(value.kind)
  ) {
    return hasExactKeys(value, ["kind"]);
  }
  if (!hasExactKeys(value, ["kind", "value"])) {
    return false;
  }
  if (
    [
      "argument",
      "identifier",
      "type",
      "field",
      "role",
      "contract_field",
      "contract_identity",
    ].includes(value.kind)
  ) {
    return typeof value.value === "string" || isRecord(value.value);
  }
  if (value.kind === "output_name") {
    return typeof value.value === "string";
  }
  return (
    ["index", "binding", "role_edge", "output_slot"].includes(value.kind) &&
    typeof value.value === "number" &&
    Number.isSafeInteger(value.value) &&
    value.value >= 0
  );
}

const MIN_I64 = -(1n << 63n);
const MAX_I64 = (1n << 63n) - 1n;

function isCanonicalI64(value: string): boolean {
  if (!/^(?:0|-?[1-9][0-9]*)$/.test(value)) {
    return false;
  }
  const parsed = BigInt(value);
  return parsed >= MIN_I64 && parsed <= MAX_I64;
}

function isQueryV2ErrorDetail(value: unknown): value is QueryV2ErrorDetail {
  if (!isRecord(value) || !hasExactKeys(value, ["kind", "value"])) {
    return false;
  }
  switch (value.kind) {
    case "text":
    case "query_identity":
      return typeof value.value === "string";
    case "long":
    case "signed":
      return typeof value.value === "string" && isCanonicalI64(value.value);
    case "unsigned":
    case "count":
    case "byte_count":
      return (
        typeof value.value === "string" &&
        /^(?:0|[1-9][0-9]*)$/.test(value.value)
      );
    case "boolean":
      return typeof value.value === "boolean";
    case "text_list":
    case "query_identity_list":
      return (
        Array.isArray(value.value) &&
        value.value.every((item) => typeof item === "string")
      );
    default:
      return false;
  }
}

function isNativeQueryV2ErrorPayload(
  value: unknown,
): value is NativeQueryV2ErrorPayload {
  return (
    isRecord(value) &&
    hasExactKeys(value, [
      "category",
      "sdkCategory",
      "queryCategory",
      "code",
      "message",
      "path",
      "details",
    ]) &&
    isAdmissibleNativeQueryV2ErrorCategory(value.category) &&
    isAdmissibleNativeQueryV2ErrorCategory(value.sdkCategory) &&
    (value.queryCategory === null ||
      isQueryV2ErrorCategory(value.queryCategory)) &&
    typeof value.code === "string" &&
    /^[a-z][a-z0-9_]{0,127}$/.test(value.code) &&
    typeof value.message === "string" &&
    Array.isArray(value.path) &&
    value.path.every(isQueryV2ErrorPathSegment) &&
    isRecord(value.details) &&
    Object.values(value.details).every(isQueryV2ErrorDetail)
  );
}

function isLegacyNativeQueryV2ErrorPayload(
  value: unknown,
): value is LegacyNativeQueryV2ErrorPayload {
  return (
    isRecord(value) &&
    hasExactKeys(value, ["category", "code", "message", "path", "details"]) &&
    isAdmissibleNativeQueryV2ErrorCategory(value.category) &&
    typeof value.code === "string" &&
    /^[a-z][a-z0-9_]{0,127}$/.test(value.code) &&
    typeof value.message === "string" &&
    Array.isArray(value.path) &&
    value.path.every(isQueryV2ErrorPathSegment) &&
    isRecord(value.details) &&
    Object.values(value.details).every(isQueryV2ErrorDetail)
  );
}

function structuredQueryV2Error(error: unknown): unknown {
  if (!(error instanceof Error)) {
    return error;
  }
  try {
    const payload: unknown = JSON.parse(error.message);
    if (isNativeQueryV2ErrorPayload(payload)) {
      return new QueryV2Error(
        payload.category,
        payload.code,
        payload.message,
        payload.path,
        payload.details,
        payload.sdkCategory,
        payload.queryCategory,
      );
    }
    if (isLegacyNativeQueryV2ErrorPayload(payload)) {
      // Retain source compatibility for the released low-level V2 N-API,
      // whose contract diagnostics predate the additive SDK/query category
      // split. New generated match facades always emit the complete form.
      return new QueryV2Error(
        payload.category,
        payload.code,
        payload.message,
        payload.path,
        payload.details,
      );
    }
  } catch {
    // Preserve non-diagnostic native errors unchanged.
  }
  return error;
}

export function queryV2NativeCall<Result>(operation: () => Result): Result {
  try {
    return operation();
  } catch (error) {
    throw structuredQueryV2Error(error);
  }
}

export async function queryV2NativePromise<Result>(
  promise: Promise<Result>,
): Promise<Result> {
  try {
    return await promise;
  } catch (error) {
    throw structuredQueryV2Error(error);
  }
}
