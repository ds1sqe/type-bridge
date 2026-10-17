import type { NativePendingPlanAnswer, NativeStagedTransaction } from "./native.js";
import { type Plan } from "./plan.js";
/** Provider transaction kind; an omitted kind is inferred from staged writes. */
export type StagedTransactionKind = "read" | "write" | "schema";
/** The answer a committed `query-plan/v3` program resolves to. */
export type PlanAnswer = {
    readonly kind: "rows";
    readonly rows: readonly unknown[];
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
/** One staged plan's answer, visible only after its commit succeeds. */
export declare class PendingPlanAnswer {
    #private;
    /** @internal */
    constructor(native: NativePendingPlanAnswer);
    /**
     * The resolved answer. Throws the lifecycle failure before commit, or the
     * typed failure every answer of a failed commit shares.
     */
    get(): PlanAnswer;
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
 * proxy one signed `query-remote/v3` exchange with a fresh idempotency key.
 *
 * TypeScript never commits implicitly: `await using tx = ...` closes the
 * transaction at scope exit without committing.
 */
export declare class StagedTransaction {
    #private;
    /** @internal */
    constructor(native: NativeStagedTransaction);
    /**
     * Stage one plan from `PlanBuilder.build`; its answer resolves at commit.
     * A `read` transaction refuses write plans and a `schema` transaction
     * refuses every plan.
     */
    stage(plan: Plan): PendingPlanAnswer;
    /** Stage one canonical `query-plan/v3` program validated under `declaredSchema`. */
    stagePlan(plan: Uint8Array, declaredSchema: Uint8Array): PendingPlanAnswer;
    /** Fire the staged stack; answers become visible only on success. */
    commit(): Promise<void>;
    /**
     * Resend the last proxy commit with the same idempotency key after an
     * unknown outcome (`commit_outcome_unknown` or a lost reply); the server
     * replays its stored reply, so the writes apply at most once. Any other
     * state, and every direct transaction, rejects with a lifecycle error.
     */
    commitRetry(): Promise<void>;
    /**
     * Whether a proxy commit's outcome is unknown, so its writes may have
     * applied and only {@link commitRetry} can tell.
     */
    hasUnknownOutcome(): boolean;
    /** Discard staged work and keep the transaction for reuse. */
    rollback(): void;
    /** Discard staged work and make the transaction terminal. */
    close(): void;
}
export {};
