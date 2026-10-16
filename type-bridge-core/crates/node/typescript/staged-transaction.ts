import type {
  NativePendingPlanAnswer,
  NativeStagedTransaction,
} from "./native.js";
import { ownedByteSnapshot } from "./owned-bytes.js";
import { queryV2NativeCall, queryV2NativePromise } from "./query-v2-internals.js";

/** Provider transaction kind; an omitted kind is inferred from staged writes. */
export type StagedTransactionKind = "read" | "write" | "schema";

/** The answer a committed `query-plan/v3` program resolves to. */
export type PlanAnswer =
  | { readonly kind: "rows"; readonly rows: readonly unknown[] }
  | { readonly kind: "count"; readonly value: number }
  | { readonly kind: "exists"; readonly value: boolean }
  | { readonly kind: "matched"; readonly value: number };

// Plans and declared schemas are canonical contract bytes, far below this.
const MAX_STAGED_INPUT_BYTES = 16 * 1024 * 1024;

/** One staged plan's answer, visible only after its commit succeeds. */
export class PendingPlanAnswer {
  readonly #native: NativePendingPlanAnswer;

  /** @internal */
  constructor(native: NativePendingPlanAnswer) {
    this.#native = native;
    Object.freeze(this);
  }

  /**
   * The resolved answer. Throws the lifecycle failure before commit, or the
   * typed failure every answer of a failed commit shares.
   */
  get(): PlanAnswer {
    return JSON.parse(queryV2NativeCall(() => this.#native.getJson())) as PlanAnswer;
  }
}

/**
 * A logical transaction, direct or over the proxy. Nothing opens until
 * `commit`; each commit is one atomic provider transaction, and over the
 * proxy one signed `query-remote/v3` exchange with a fresh idempotency key.
 */
export class StagedTransaction {
  readonly #native: NativeStagedTransaction;

  /** @internal */
  constructor(native: NativeStagedTransaction) {
    this.#native = native;
    Object.freeze(this);
  }

  /** Stage one canonical `query-plan/v3` program validated under `declaredSchema`. */
  stagePlan(plan: Uint8Array, declaredSchema: Uint8Array): PendingPlanAnswer {
    const planBytes = ownedByteSnapshot(plan, MAX_STAGED_INPUT_BYTES);
    const schemaBytes = ownedByteSnapshot(declaredSchema, MAX_STAGED_INPUT_BYTES);
    return new PendingPlanAnswer(
      queryV2NativeCall(() => this.#native.stagePlan(planBytes, schemaBytes)),
    );
  }

  /** Fire the staged stack; answers become visible only on success. */
  commit(): Promise<void> {
    return queryV2NativePromise(queryV2NativeCall(() => this.#native.commit()));
  }

  /**
   * Resend the last proxy commit with the same idempotency key after an
   * unknown outcome (`commit_outcome_unknown` or a lost reply); the server
   * replays its stored reply, so the writes apply at most once. Any other
   * state, and every direct transaction, rejects with a lifecycle error.
   */
  commitRetry(): Promise<void> {
    return queryV2NativePromise(queryV2NativeCall(() => this.#native.commitRetry()));
  }

  /** Discard staged work and keep the transaction for reuse. */
  rollback(): void {
    queryV2NativeCall(() => this.#native.rollback());
  }

  /** Discard staged work and make the transaction terminal. */
  close(): void {
    queryV2NativeCall(() => this.#native.close());
  }
}
