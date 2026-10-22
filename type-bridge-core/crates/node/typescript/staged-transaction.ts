import type {
  NativePendingPlanAnswer,
  NativeStagedTransaction,
} from "./native.js";
import { ownedByteSnapshot } from "./owned-bytes.js";
import { nativePlan, type Plan } from "./plan.js";
import { queryV2NativeCall, queryV2NativePromise } from "./query-v2-internals.js";

/** Provider transaction kind; an omitted kind is inferred from staged writes. */
export type StagedTransactionKind = "read" | "write" | "schema";

/** The answer a committed `query-plan/v3` program resolves to. */
export type PlanAnswer =
  | { readonly kind: "rows"; readonly rows: readonly unknown[] }
  | { readonly kind: "documents"; readonly documents: readonly Record<string, unknown>[] }
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

  /**
   * Stage one plan from `PlanBuilder.build`; its answer resolves at commit.
   * A `read` transaction refuses write plans and a `schema` transaction
   * refuses every plan.
   */
  stage(plan: Plan): PendingPlanAnswer {
    const native = nativePlan(plan);
    return new PendingPlanAnswer(queryV2NativeCall(() => this.#native.stage(native)));
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

  /** Discard staged work and keep the transaction for reuse. */
  rollback(): void {
    queryV2NativeCall(() => this.#native.rollback());
  }

  /** Discard staged work and make the transaction terminal. */
  close(): void {
    queryV2NativeCall(() => this.#native.close());
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
          return queryV2NativePromise(queryV2NativeCall(() => native.dispose()));
        },
      });
    }
  }
}
