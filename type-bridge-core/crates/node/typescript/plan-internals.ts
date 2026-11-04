import type { NativeQueryLimits } from "./native.js";
/** Shared, package-private machinery for plan facades. */

declare const nativePlanAuthorityKind: unique symbol;

/** Opaque N-API authority. Public callers use the stable PlanAuthority class. */
export interface NativePlanAuthority {
  readonly [nativePlanAuthorityKind]: "plan-authority";
  withQueryLimits(queryLimits: NativeQueryLimits): NativePlanAuthority;
}


const planAuthorityHandles = new WeakMap<object, NativePlanAuthority>();

export function registerPlanAuthorityHandle(
  authority: object,
  native: NativePlanAuthority,
): void {
  planAuthorityHandles.set(authority, native);
}

export function planAuthorityHandle(
  authority: object,
): NativePlanAuthority | undefined {
  return planAuthorityHandles.get(authority);
}

/** Stable V2 contract diagnostic categories. */
export type PlanErrorCategory =
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
export type PlanErrorPathSegment =
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

/** Named execution boundary, recorded on one process clock. */
export type ExecutionPhase = "fired" | "accepted" | "execution_started" | "db_write" | "before_commit" | "committed" | "failed";

/** Diagnostic-only timestamps; server and client anchors remain separate. */
export interface ExecutionTimeline {
  readonly anchor_unix_milliseconds: number;
  readonly stamps: readonly Readonly<{
    phase: ExecutionPhase;
    offset_milliseconds: number;
    statement_index?: number;
    failing_phase?: ExecutionPhase;
  }>[];
}

/** One deterministic structured V2 diagnostic detail. */
export type PlanErrorDetail =
  | Readonly<{ kind: "execution_timeline"; value: ExecutionTimeline }>
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

/** Structured diagnostic preserved from the Rust plan engine. */
export class PlanError extends Error {
  readonly name = "PlanError";

  constructor(
    readonly category: PlanErrorCategory,
    readonly code: string,
    readonly diagnosticMessage: string,
    readonly path: readonly PlanErrorPathSegment[],
    readonly details: Readonly<Record<string, PlanErrorDetail>>,
    readonly sdkCategory: PlanErrorCategory = category,
    readonly queryCategory: PlanErrorCategory | null = category,
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
  readonly category: PlanErrorCategory;
  readonly sdk_category?: PlanErrorCategory;
  readonly query_category?: PlanErrorCategory | null;
  readonly code: string;
  readonly message: string;
  readonly path: readonly PlanErrorPathSegment[];
  readonly details: Readonly<Record<string, PlanErrorDetail>>;
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

function structuredPlanError(error: unknown): unknown {
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
  return new PlanError(
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

export function planNativeCall<Result>(operation: () => Result): Result {
  try {
    return operation();
  } catch (error) {
    throw structuredPlanError(error);
  }
}

export async function planNativePromise<Result>(
  promise: Promise<Result>,
): Promise<Result> {
  try {
    return await promise;
  } catch (error) {
    throw structuredPlanError(error);
  }
}
