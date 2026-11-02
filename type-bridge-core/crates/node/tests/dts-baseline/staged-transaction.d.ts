import type { NativePendingPlanAnswer, NativeStagedTransaction } from "./native.js";
import type { NativeAuthoredPlanResult, RuntimeProjectionMatchResult } from "./runtime-projection.js";
import { type Plan } from "./plan.js";
/** Provider transaction kind; an omitted kind is inferred from staged writes. */
export type StagedTransactionKind = "read" | "write" | "schema";
/** Per-commit configuration. Omission keeps the 60000 ms default and collection off. */
export interface StagedTransactionOptions {
    /** Zero or negative disables the execution deadline. */
    readonly timeoutMilliseconds?: bigint;
    readonly collectPhaseTimestamps?: boolean;
}
/** The answer a committed `query-plan/v3` program resolves to. */
export type PlanAnswer = {
    readonly kind: "rows";
    readonly rows: readonly unknown[];
} | {
    readonly kind: "documents";
    readonly documents: readonly Record<string, unknown>[];
} | {
    readonly kind: "count";
    readonly value: number;
} | {
    readonly kind: "exists";
    readonly value: boolean;
} | {
    readonly kind: "matched";
    readonly value: number;
};
/** One staged plan or typed query answer, visible only after commit succeeds. */
export declare class PendingPlanAnswer<R = PlanAnswer> {
    #private;
    /** @internal */
    constructor(native: NativePendingPlanAnswer, decode?: (answerJson: string, projection?: NativeAuthoredPlanResult, query?: RuntimeProjectionMatchResult) => R, typedQuery?: boolean);
    /**
     * The resolved answer. Throws the lifecycle failure before commit, or the
     * typed failure every answer of a failed commit shares.
     */
    get(): R;
    /** The typed result after a successful commit. */
    result(): R;
}
type AsyncDisposeSymbol = SymbolConstructor extends {
    readonly asyncDispose: infer Key extends symbol;
} ? Key : never;
type AsyncDisposable = {
    readonly [Key in AsyncDisposeSymbol]: () => Promise<void>;
};
export interface StagedTransaction extends AsyncDisposable {
}
/**
 * A logical transaction, direct or over the proxy. Nothing opens until
 * `commit`; each commit is one atomic provider transaction, and over the
 * proxy one signed `query-remote/v3` exchange. A lost proxy reply rejects
 * with `commit_outcome_unknown`: the writes may have applied, and the caller
 * reads the database to find out.
 *
 * TypeScript never commits implicitly: `await using tx = ...` closes the
 * transaction at scope exit without committing.
 */
export declare class StagedTransaction {
    #private;
    /** @internal */
    constructor(native: NativeStagedTransaction);
    /** @internal Exact native transaction used by generated typed terminals. */
    nativeHandle(): NativeStagedTransaction;
    /**
     * Stage one plan from `PlanBuilder.build`; its answer resolves at commit.
     * A `read` transaction refuses write plans and a `schema` transaction
     * refuses every plan.
     */
    stage<R>(plan: Plan<R>): PendingPlanAnswer<R>;
    /** Stage one canonical `query-plan/v3` program validated under `declaredSchema`. */
    stagePlan(plan: Uint8Array, declaredSchema: Uint8Array): PendingPlanAnswer;
    /** Fire the staged stack; answers become visible only on success. */
    commit(): Promise<void>;
    /** Discard staged work and keep the transaction for reuse. */
    rollback(): void;
    /** Discard staged work and make the transaction terminal. */
    close(): void;
}
export {};
