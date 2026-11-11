import type {
  NativePendingPlanAnswer,
  NativeStagedTransaction,
} from "./native.js";
import type { NativeAuthoredPlanResult, RuntimeProjectionMatchResult } from "./runtime-projection.js";
import { ownedByteSnapshot } from "./owned-bytes.js";
import { decodePlan, nativePlan, type Plan, type StagedPlan } from "./plan.js";
import { planNativeCall, planNativePromise } from "./plan-internals.js";

/** Provider transaction kind; an omitted kind is inferred from staged writes. */
export type StagedTransactionKind = "read" | "write" | "schema";

/** Per-commit configuration. Omission keeps the 60000 ms default and collection off. */
export interface StagedTransactionOptions {
  /** Zero or negative disables the execution deadline. */
  readonly timeoutMilliseconds?: bigint;
  readonly collectPhaseTimestamps?: boolean;
}

/** The answer a committed `query-plan/v3` program resolves to. */
export type PlanAnswer =
  | { readonly kind: "rows"; readonly rows: readonly unknown[] }
  | { readonly kind: "documents"; readonly documents: readonly Record<string, unknown>[] }
  | { readonly kind: "count"; readonly value: number }
  | { readonly kind: "exists"; readonly value: boolean }
  | { readonly kind: "matched"; readonly value: number };

// Plans and declared schemas are canonical contract bytes, far below this.
const MAX_STAGED_INPUT_BYTES = 16 * 1024 * 1024;

/** One staged plan or typed query answer, visible only after commit succeeds. */
export class Pending<R = PlanAnswer> {
  readonly #native: NativePendingPlanAnswer;
  readonly #typedQuery: boolean;
  readonly #decode: (answerJson: string, projection?: NativeAuthoredPlanResult, query?: RuntimeProjectionMatchResult) => R;

  /** @internal */
  constructor(native: NativePendingPlanAnswer, decode: (answerJson: string, projection?: NativeAuthoredPlanResult, query?: RuntimeProjectionMatchResult) => R = (answerJson) => JSON.parse(answerJson) as R, typedQuery = false) {
    this.#native = native;
    this.#decode = decode;
    this.#typedQuery = typedQuery;
    Object.freeze(this);
  }

  /**
   * The resolved answer. Throws the lifecycle failure before commit, or the
   * typed failure every answer of a failed commit shares.
   */
  result(): R {
    if (this.#typedQuery) {
      return this.#decode("", undefined, planNativeCall(() => this.#native.getQueryResult()));
    }
    const projection = planNativeCall(() => this.#native.getProjection());
    return this.#decode(projection === null ? planNativeCall(() => this.#native.getJson()) : "", projection ?? undefined);
  }

  /**
   * Raw documents from this execution after commit, without another query or
   * hydration. Throws the pending failure or rejects a non-document answer.
   * Mutating the returned JSON cannot change the retained native answer.
   */
  documents(): readonly Readonly<Record<string, unknown>>[] {
    if (this.#typedQuery) {
      throw new TypeError("plan_result_shape: a typed query has no document plan answer");
    }
    const answer = JSON.parse(planNativeCall(() => this.#native.getJson())) as PlanAnswer;
    if (answer.kind !== "documents") {
      throw new TypeError("plan_result_shape: the committed answer is not documents");
    }
    return answer.documents;
  }

}

// `Symbol.asyncDispose` exists in the type only where the consumer's lib
// declares it (TypeScript 5.2+ `esnext.disposable`, or `@types/node`), so
// older consumers still compile these declarations.
type AsyncDisposeSymbol = SymbolConstructor extends { readonly asyncDispose: infer Key extends symbol }
  ? Key
  : never;
type AsyncDisposable = { readonly [Key in AsyncDisposeSymbol]: () => Promise<void> };

export interface StagedTransaction extends AsyncDisposable {}

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
export class StagedTransaction {
  readonly #native: NativeStagedTransaction;

  /** @internal */
  constructor(native: NativeStagedTransaction) {
    this.#native = native;
    Object.freeze(this);
  }

  /** @internal Exact native transaction used by generated typed terminals. */
  nativeHandle(): NativeStagedTransaction { return this.#native; }

  /**
   * Stage one plan from `PlanBuilder.build`; its answer resolves at commit.
   * A `read` transaction refuses write plans and a `schema` transaction
   * refuses every plan.
   */
  stage<R>(plan: Plan<R>): Pending<R> {
    const native = nativePlan(plan);
    return new Pending(planNativeCall(() => this.#native.stage(native)), (answer, projection) => decodePlan(plan, answer, projection));
  }

  /** Queue a complete invocation stack with block-local IID references. */
  stageStack(plans: readonly StagedPlan[]): Pending[] {
    return planNativeCall(() => this.#native.stageStack(plans.map(plan => plan.nativeHandle()))).map(pending => new Pending(pending));
  }

  /** Stage one canonical `query-plan/v3` program validated under `declaredSchema`. */
  stagePlan(plan: Uint8Array, declaredSchema: Uint8Array): Pending {
    const planBytes = ownedByteSnapshot(plan, MAX_STAGED_INPUT_BYTES);
    const schemaBytes = ownedByteSnapshot(declaredSchema, MAX_STAGED_INPUT_BYTES);
    return new Pending(
      planNativeCall(() => this.#native.stagePlan(planBytes, schemaBytes)),
    );
  }

  /** Fire the staged stack; answers become visible only on success. */
  commit(): Promise<void> {
    return planNativePromise(planNativeCall(() => this.#native.commit()));
  }

  /** Discard staged work and keep the transaction for reuse. */
  rollback(): void {
    planNativeCall(() => this.#native.rollback());
  }

  /** Discard staged work and make the transaction terminal. */
  close(): void {
    planNativeCall(() => this.#native.close());
  }

  // `await using` closes without committing once a running commit settles;
  // an already closed transaction stays closed. Installed only where the
  // runtime has `Symbol.asyncDispose` (Node 18.18+).
  static {
    const key = (Symbol as { readonly asyncDispose?: symbol }).asyncDispose;
    if (key !== undefined) {
      Object.defineProperty(this.prototype, key, {
        configurable: true,
        writable: true,
        value(this: StagedTransaction): Promise<void> {
          const native = this.#native;
          return planNativePromise(planNativeCall(() => native.dispose()));
        },
      });
    }
  }
}
