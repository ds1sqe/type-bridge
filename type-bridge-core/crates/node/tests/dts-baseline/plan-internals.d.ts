import type { NativeQueryLimits } from "./native.js";
/** Shared, package-private machinery for plan facades. */
declare const nativePlanAuthorityKind: unique symbol;
/** Opaque N-API authority. Public callers use the stable PlanAuthority class. */
export interface NativePlanAuthority {
    readonly [nativePlanAuthorityKind]: "plan-authority";
    withQueryLimits(queryLimits: NativeQueryLimits): NativePlanAuthority;
}
export declare function registerPlanAuthorityHandle(authority: object, native: NativePlanAuthority): void;
export declare function planAuthorityHandle(authority: object): NativePlanAuthority | undefined;
/** Stable V2 contract diagnostic categories. */
export type PlanErrorCategory = "invalid_contract" | "invalid_input" | "invalid_plan" | "cardinality" | "unsupported_capability" | "stale_schema" | "resource_limit" | "integrity" | "cancelled" | "provider" | "result_decode" | "transaction" | "internal";
/** One typed location inside a rejected V2 contract. */
export type PlanErrorPathSegment = Readonly<{
    kind: "request" | "plan" | "operation" | "predicate" | "output" | "provider_evidence" | "result" | "unknown";
}> | Readonly<{
    kind: "argument" | "identifier" | "type" | "contract_field" | "contract_identity";
    value: string | Readonly<Record<string, unknown>>;
}> | Readonly<{
    kind: "field" | "role";
    value: string | Readonly<Record<string, unknown>>;
}> | Readonly<{
    kind: "index" | "binding" | "role_edge" | "output_slot";
    value: number;
}> | Readonly<{
    kind: "output_name";
    value: string;
}>;
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
export type PlanErrorDetail = Readonly<{
    kind: "execution_timeline";
    value: ExecutionTimeline;
}> | Readonly<{
    kind: "text" | "long" | "signed" | "unsigned" | "count" | "byte_count" | "query_identity";
    value: string;
}> | Readonly<{
    kind: "boolean";
    value: boolean;
}> | Readonly<{
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
export declare class PlanError extends Error {
    readonly category: PlanErrorCategory;
    readonly code: string;
    readonly diagnosticMessage: string;
    readonly path: readonly PlanErrorPathSegment[];
    readonly details: Readonly<Record<string, PlanErrorDetail>>;
    readonly sdkCategory: PlanErrorCategory;
    readonly queryCategory: PlanErrorCategory | null;
    readonly failure?: Readonly<ProviderFailure> | undefined;
    readonly name = "PlanError";
    constructor(category: PlanErrorCategory, code: string, diagnosticMessage: string, path: readonly PlanErrorPathSegment[], details: Readonly<Record<string, PlanErrorDetail>>, sdkCategory?: PlanErrorCategory, queryCategory?: PlanErrorCategory | null, failure?: Readonly<ProviderFailure> | undefined);
}
export declare function planNativeCall<Result>(operation: () => Result): Result;
export declare function planNativePromise<Result>(promise: Promise<Result>): Promise<Result>;
export {};
