import type { NativePendingPlanAnswer, NativeStagedTransaction } from "./native.js";
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
/**
 * A logical transaction, direct or over the proxy. Nothing opens until
 * `commit`; each commit is one atomic provider transaction, and over the
 * proxy one signed `query-remote/v3` exchange with a fresh idempotency key.
 */
export declare class StagedTransaction {
    #private;
    /** @internal */
    constructor(native: NativeStagedTransaction);
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
    /** Discard staged work and keep the transaction for reuse. */
    rollback(): void;
    /** Discard staged work and make the transaction terminal. */
    close(): void;
}
