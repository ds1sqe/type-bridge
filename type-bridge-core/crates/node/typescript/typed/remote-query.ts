import { loadNative } from "../native.js";
import {
  queryV2NativeCall,
  queryV2NativePromise,
} from "../query-v2-internals.js";
import type { Page } from "./page.js";
import {
  prepareCountTerminal,
  prepareExistsTerminal,
  prepareOneTerminal,
  preparePageTerminal,
  prepareRowsTerminal,
  nativeOrderArray,
  type PageOptions,
  type PageRootFor,
  type Query,
  type QueryContainsCollection,
  type QueryRow,
  type QuerySlots,
  type RowsOptions,
  type SingularOutputsFor,
} from "./query.js";
import {
  nativeCall,
  type BoundVar,
  type Predicate,
} from "./references.js";
import {
  materializeValidatedCount,
  materializeValidatedExists,
  materializeValidatedOne,
  materializeValidatedPage,
  materializeValidatedRows,
} from "./results.js";

type NativeModule = ReturnType<typeof loadNative>;
type NativeRemoteContext = ReturnType<
  NativeModule["queryV2RemoteModelContext"]
>;
type NativePending = ReturnType<
  NativeModule["queryV2PrepareRemoteModelRows"]
>;
type NativeResult = Awaited<ReturnType<NativePending["decodeReply"]>>;

/** One caller-owned request/response exchange. No retry is performed. */
export type RemoteQueryExchange = (
  request: Uint8Array,
) => Promise<Uint8Array>;

/** @internal Immutable transport and native context snapshot. */
export interface RemoteQueryRuntime {
  readonly context: NativeRemoteContext;
  readonly exchange: RemoteQueryExchange;
}

const remoteQueryStates = new WeakMap<
  object,
  Readonly<{ direct: object; runtime: RemoteQueryRuntime }>
>();

/**
 * Immutable typed-query composition over one caller-owned remote exchange.
 *
 * Every builder delegates to the released direct Query. Only terminal
 * execution differs: Rust prepares one authenticated V2 request and consumes
 * exactly one reply into the ordinary opaque validated-result proof.
 */
export class RemoteQuery<Slots extends QuerySlots> {
  private constructor(direct: Query<Slots>, runtime: RemoteQueryRuntime) {
    remoteQueryStates.set(this, { direct, runtime });
    Object.freeze(this);
  }

  match<const Models extends readonly [object, ...object[]]>(
    ...bindings: {
      readonly [Index in keyof Models]: BoundVar<Models[Index]>;
    }
  ): RemoteQuery<Slots> {
    const state = remoteQueryState(this);
    let direct = state.direct;
    for (const binding of bindings) {
      direct = direct.match(binding);
    }
    return createRemoteQuery(direct, state.runtime);
  }

  where(
    ...predicates: readonly [Predicate, ...Predicate[]]
  ): RemoteQuery<Slots> {
    const state = remoteQueryState(this);
    return createRemoteQuery(state.direct.where(...predicates), state.runtime);
  }

  allowCrossJoin<Left extends object, Right extends object>(
    left: BoundVar<Left>,
    right: BoundVar<Right>,
  ): RemoteQuery<Slots> {
    const state = remoteQueryState(this);
    return createRemoteQuery(
      state.direct.allowCrossJoin(left, right),
      state.runtime,
    );
  }

  async one(
    this: QueryContainsCollection<Slots> extends true
      ? never
      : RemoteQuery<Slots>,
  ): Promise<QueryRow<Slots>> {
    const remote = remoteQueryState(this);
    const state = prepareOneTerminal(remote.direct);
    const pending = modelNativeCall(() =>
      loadNative().queryV2PrepareRemoteModelRows(
        state.handle,
        remote.runtime.context,
        [],
        0n,
        1n,
        "exactly_one",
      ),
    );
    const result = await executeRemoteExchange(
      pending,
      remote.runtime.exchange,
    );
    return materializeValidatedOne(
      state.handle,
      result,
      state.context.models,
    ) as QueryRow<Slots>;
  }

  async rows(
    this: QueryContainsCollection<Slots> extends true
      ? never
      : RemoteQuery<Slots>,
    options: RowsOptions,
  ): Promise<readonly QueryRow<Slots>[]> {
    const remote = remoteQueryState(this);
    const { state, orders, window } = prepareRowsTerminal(
      remote.direct,
      options,
    );
    const pending = modelNativeCall(() =>
      loadNative().queryV2PrepareRemoteModelRows(
        state.handle,
        remote.runtime.context,
        nativeOrderArray(orders),
        window.offset,
        window.limit,
        "bounded_many",
      ),
    );
    const result = await executeRemoteExchange(
      pending,
      remote.runtime.exchange,
    );
    return materializeValidatedRows(
      state.handle,
      result,
      state.context.models,
    ) as readonly QueryRow<Slots>[];
  }

  async pageBy(
    this: [PageRootFor<Slots>] extends [never]
      ? never
      : RemoteQuery<Slots>,
    root: BoundVar<Extract<PageRootFor<Slots>, object>>,
    options: PageOptions,
  ): Promise<Page<QueryRow<Slots>>> {
    const remote = remoteQueryState(this);
    const {
      state,
      nativeRoot,
      orders,
      window,
      includeTotal,
    } = preparePageTerminal(remote.direct, root, options);
    const pending = modelNativeCall(() =>
      loadNative().queryV2PrepareRemoteModelPage(
        state.handle,
        remote.runtime.context,
        nativeRoot,
        nativeOrderArray(orders),
        window.offset,
        window.limit,
        includeTotal,
      ),
    );
    const result = await executeRemoteExchange(
      pending,
      remote.runtime.exchange,
    );
    return materializeValidatedPage(
      state.handle,
      result,
      state.context.models,
      window.offset,
      window.limit,
      includeTotal,
    ) as Page<QueryRow<Slots>>;
  }

  async countBy<Root extends Extract<SingularOutputsFor<Slots>, object>>(
    root: BoundVar<Root>,
  ): Promise<bigint> {
    const remote = remoteQueryState(this);
    const { state, nativeRoot } = prepareCountTerminal(
      remote.direct,
      root,
    );
    const pending = modelNativeCall(() =>
      loadNative().queryV2PrepareRemoteModelCount(
        state.handle,
        remote.runtime.context,
        nativeRoot,
      ),
    );
    const result = await executeRemoteExchange(
      pending,
      remote.runtime.exchange,
    );
    return materializeValidatedCount(state.handle, result);
  }

  async existsBy<Root extends Extract<SingularOutputsFor<Slots>, object>>(
    root: BoundVar<Root>,
  ): Promise<boolean> {
    const remote = remoteQueryState(this);
    const { state, nativeRoot } = prepareExistsTerminal(
      remote.direct,
      root,
    );
    const pending = modelNativeCall(() =>
      loadNative().queryV2PrepareRemoteModelExists(
        state.handle,
        remote.runtime.context,
        nativeRoot,
      ),
    );
    const result = await executeRemoteExchange(
      pending,
      remote.runtime.exchange,
    );
    return materializeValidatedExists(state.handle, result);
  }
}

/** @internal Wrap an ordinary immutable Query without copying its semantics. */
export function createRemoteQuery<Slots extends QuerySlots>(
  direct: Query<Slots>,
  runtime: RemoteQueryRuntime,
): RemoteQuery<Slots> {
  const Constructor = RemoteQuery as unknown as new (
    direct: Query<Slots>,
    runtime: RemoteQueryRuntime,
  ) => RemoteQuery<Slots>;
  return new Constructor(direct, runtime);
}

function remoteQueryState<Slots extends QuerySlots>(
  query: RemoteQuery<Slots>,
): Readonly<{ direct: Query<Slots>; runtime: RemoteQueryRuntime }> {
  const state = remoteQueryStates.get(query);
  if (state === undefined) {
    throw new TypeError(
      "RemoteQuery was not constructed by RemoteQuerySession.query",
    );
  }
  return state as unknown as Readonly<{
    direct: Query<Slots>;
    runtime: RemoteQueryRuntime;
  }>;
}

/** @internal Execute exactly one caller exchange for one prepared request. */
export async function executeRemoteExchange(
  pending: NativePending,
  exchange: RemoteQueryExchange,
): Promise<NativeResult> {
  const request = modelNativeCall(
    () => new Uint8Array(pending.requestBytes()),
  );
  const response = await exchange(request);
  if (!(response instanceof Uint8Array)) {
    throw new TypeError(
      "remote query exchange must resolve to a Uint8Array",
    );
  }
  const decode = modelNativeCall(() => pending.decodeReply(response));
  return modelNativePromise(decode);
}

function modelNativeCall<Result>(operation: () => Result): Result {
  return nativeCall(() => queryV2NativeCall(operation));
}

async function modelNativePromise<Result>(
  promise: Promise<Result>,
): Promise<Result> {
  try {
    return await queryV2NativePromise(promise);
  } catch (error) {
    return nativeCall(() => {
      throw error;
    });
  }
}
