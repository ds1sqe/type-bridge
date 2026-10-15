import type { NativeQueryLimits } from "./native.js";
/** Shared, package-private machinery for the prepared and authored V2 facades. */

declare const nativeQueryV2AuthorityKind: unique symbol;

/** Opaque N-API authority. Public callers use the stable QueryV2Authority class. */
export interface NativeQueryV2Authority {
  readonly [nativeQueryV2AuthorityKind]: "query-v2-authority";
  withQueryLimits(queryLimits: NativeQueryLimits): NativeQueryV2Authority;
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

/** Value-free outcome classified once by the Rust provider runtime. */
export interface ProviderFailure {
  readonly phase: "query" | "commit";
  readonly reason: "conflict" | "cardinality" | "unique" | "constraint" | "other" | null;
  readonly lock_kind: "exclusive" | "require_deleted_key" | "deleting_required_key" | "unknown" | null;
  readonly annotation: string | null;
  readonly op_index: number | null;
  readonly retry_safe: boolean;
  readonly code: string | null;
}

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
    readonly failure?: Readonly<ProviderFailure>,
  ) {
    super(`${code}: ${diagnosticMessage}`);
    if (failure) Object.freeze(failure);
  }
}

/**
 * The fields of a native error message that is one of the Rust core's two
 * diagnostic JSON forms: the SDK execution form (`SdkDiagnosticJson`, with
 * `version`, `sdk_category`, and `query_category`) or the contract
 * `Diagnostic` serde form. Rust validates and encodes both, including the
 * decimal-string integer details; TypeScript only selects the fields.
 */
interface NativeDiagnostic {
  readonly failure?: ProviderFailure;
  readonly category: QueryV2ErrorCategory;
  readonly sdk_category?: QueryV2ErrorCategory;
  readonly query_category?: QueryV2ErrorCategory | null;
  readonly code: string;
  readonly message: string;
  readonly path: readonly QueryV2ErrorPathSegment[];
  readonly details: Readonly<Record<string, QueryV2ErrorDetail>>;
}

function nativeDiagnostic(message: string): NativeDiagnostic | null {
  let payload: unknown;
  try {
    payload = JSON.parse(message);
  } catch {
    return null;
  }
  if (typeof payload !== "object" || payload === null) {
    return null;
  }
  const candidate = payload as Partial<NativeDiagnostic>;
  return typeof candidate.category === "string" &&
    typeof candidate.code === "string" &&
    typeof candidate.message === "string" &&
    Array.isArray(candidate.path) &&
    typeof candidate.details === "object" &&
    candidate.details !== null
    ? (candidate as NativeDiagnostic)
    : null;
}

function structuredQueryV2Error(error: unknown): unknown {
  if (!(error instanceof Error)) {
    return error;
  }
  const diagnostic = nativeDiagnostic(error.message);
  const sdkCategory = diagnostic?.sdk_category ?? diagnostic?.category;
  // Native internal failures stay plain errors: their text is not part of
  // the redacted query contract.
  if (diagnostic === null || sdkCategory === undefined || sdkCategory === "internal") {
    return error;
  }
  return new QueryV2Error(
    diagnostic.category,
    diagnostic.code,
    diagnostic.message,
    diagnostic.path,
    diagnostic.details,
    sdkCategory,
    // The contract form has no query category; it is its own category.
    diagnostic.query_category === undefined
      ? diagnostic.category
      : diagnostic.query_category,
    diagnostic.failure,
  );
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
