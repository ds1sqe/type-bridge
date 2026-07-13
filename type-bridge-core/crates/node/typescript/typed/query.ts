import type { RustDatabase, RustTransactionContext } from "../index.js";
import type { ModelOwnerToken } from "../model.js";
import { loadNative } from "../native.js";
import {
  rustDatabaseHandle,
  rustTransactionHandle,
} from "./runtime-handles.js";
import type { Page } from "./page.js";
import {
  TypedMatchError,
  nativeBindingHandle,
  nativeCall,
  nativePredicateHandle,
  nativeQueryOrderHandle,
  type BoundVar,
  type Predicate,
  type QueryModelClass,
  type QueryOrder,
  type Selection,
} from "./references.js";
import {
  materializeValidatedCount,
  materializeValidatedExists,
  materializeValidatedOne,
  materializeValidatedPage,
  materializeValidatedRows,
} from "./results.js";

type NativeModule = ReturnType<typeof loadNative>;
type NativeRegistry = InstanceType<NativeModule["NodeDescriptorRegistry"]>;
type NativeSession = InstanceType<NativeModule["NodeMatchSessionHandle"]>;
type NativeQueryHandle = ReturnType<NativeSession["query"]>;
type NativeOrderHandle = ReturnType<typeof nativeQueryOrderHandle>;
type NativeBindingHandle = ReturnType<typeof nativeBindingHandle>;

/** The canonical selected-output arity accepted by every typed binding. */
export type QuerySlotCount =
  | 1
  | 2
  | 3
  | 4
  | 5
  | 6
  | 7
  | 8
  | 9
  | 10
  | 11
  | 12
  | 13
  | 14
  | 15
  | 16;

/** A non-empty readonly output tuple within the canonical arity cap. */
export type QuerySlots = readonly [unknown, ...unknown[]] & {
  readonly length: QuerySlotCount;
};

/** A non-empty readonly selection tuple within the canonical arity cap. */
export type QuerySelections = readonly [Selection<unknown>, ...Selection<unknown>[]] & {
  readonly length: QuerySlotCount;
};

type SelectionOutput<Selected> = Selected extends Selection<infer Output> ? Output : never;

/** Map an inferred tuple of selections to its exact readonly output tuple. */
export type SelectedOutputs<Selections extends QuerySelections> = {
  readonly [Index in keyof Selections]: SelectionOutput<Selections[Index]>;
} extends infer Outputs extends QuerySlots
  ? Outputs
  : never;

/** Scalarize one output slot and preserve the exact readonly tuple otherwise. */
export type QueryRow<Slots extends QuerySlots> = Slots extends readonly [infer Only]
  ? Only
  : Slots;

/** Exact readonly object output inferred from a named selection declaration. */
export type NamedQueryRow<Shape extends object> = Readonly<{
  [Name in keyof Shape]: SelectionOutput<Shape[Name]>;
}>;

type UnionToIntersection<Union> = (
  Union extends unknown ? (value: Union) => void : never
) extends (value: infer Intersection) => void
  ? Intersection
  : never;

type LastUnionMember<Union> = UnionToIntersection<
  Union extends unknown ? () => Union : never
> extends () => infer Last
  ? Last
  : never;

type UnionToTuple<Union, Last = LastUnionMember<Union>> = [Union] extends [never]
  ? []
  : [...UnionToTuple<Exclude<Union, Last>>, Last];

type NamedSelectionKeys<Shape extends object> = Extract<keyof Shape, string>;

/** @internal Exact-key constraint used by QuerySession.queryNamed. */
export type NamedSelectionInput<Shape extends object> =
  keyof Shape extends never
    ? never
    : string extends keyof Shape
      ? never
      : Exclude<keyof Shape, string> extends never
        ? "" extends keyof Shape
          ? never
          : UnionToTuple<NamedSelectionKeys<Shape>>["length"] extends QuerySlotCount
            ? Shape & Readonly<{
                [Name in keyof Shape]: Selection<unknown>;
              }>
            : never
        : never;

type IsCollection<Value> = Value extends readonly unknown[] ? true : false;
type IsModel<Value> = [ModelOwnerToken<Value>] extends [never] ? false : true;
type IsNamedRow<Value> = Value extends object
  ? IsCollection<Value> extends true
    ? false
    : IsModel<Value> extends true
      ? false
      : true
  : false;

type NamedRowContainsCollection<Row extends object> = true extends {
  [Name in keyof Row]: IsCollection<Row[Name]>;
}[keyof Row]
  ? true
  : false;

type SlotContainsCollection<Slot> = IsCollection<Slot> extends true
  ? true
  : IsNamedRow<Slot> extends true
    ? NamedRowContainsCollection<Extract<Slot, object>>
    : false;

type QueryContainsCollection<Slots extends QuerySlots> = true extends {
  [Index in keyof Slots]: SlotContainsCollection<Slots[Index]>;
}[number]
  ? true
  : false;

type PositionalSingularValues<
  Slots extends readonly unknown[],
  Found extends readonly unknown[] = [],
> = Slots extends readonly [infer First, ...infer Rest]
  ? IsCollection<First> extends true
    ? PositionalSingularValues<Rest, Found>
    : PositionalSingularValues<Rest, readonly [...Found, First]>
  : Found;

type NamedSingularKeys<Row extends object> = {
  [Name in keyof Row]-?: IsCollection<Row[Name]> extends true ? never : Name;
}[keyof Row];

type IsUnion<Value, Whole = Value> = Value extends Whole
  ? [Whole] extends [Value]
    ? false
    : true
  : never;

type OnlyNamedSingular<Row extends object> = NamedSingularKeys<Row> extends infer Keys
  ? [Keys] extends [never]
    ? never
    : true extends IsUnion<Keys>
      ? never
      : Row[Extract<Keys, keyof Row>]
  : never;

type PageRootFor<Slots extends QuerySlots> = Slots extends readonly [infer Only]
  ? IsNamedRow<Only> extends true
    ? OnlyNamedSingular<Extract<Only, object>>
    : IsCollection<Only> extends true
      ? never
      : Only
  : PositionalSingularValues<Slots> extends readonly [infer Root]
    ? Root
    : never;

type SingularOutputsFor<Slots extends QuerySlots> = Slots extends readonly [infer Only]
  ? IsNamedRow<Only> extends true
    ? Extract<Only, object>[NamedSingularKeys<Extract<Only, object>>]
    : IsCollection<Only> extends true
      ? never
      : Only
  : PositionalSingularValues<Slots>[number];

/** Bounds and operation-local ordering for a distinct selected-row fetch. */
export interface RowsOptions {
  readonly limit: number;
  readonly offset?: number;
  readonly orderBy?: readonly QueryOrder[];
}

/** Bounds and operation-local ordering for a distinct-root page. */
export interface PageOptions extends RowsOptions {
  readonly includeTotal?: boolean;
}

export type QueryConnection = RustDatabase | RustTransactionContext;

/** @internal Runtime ownership passed by QuerySession without semantic plan state. */
export interface QueryRuntimeContext {
  readonly registry: NativeRegistry;
  readonly connection: QueryConnection | undefined;
  readonly models: ReadonlyMap<string, QueryModelClass>;
}

interface QueryState {
  readonly handle: NativeQueryHandle;
  readonly context: QueryRuntimeContext;
}

interface TerminalWindow {
  readonly offset: bigint;
  readonly limit: bigint;
}

interface QueryTerminalAdapter {
  one(state: QueryState): unknown;
  rows(
    state: QueryState,
    orders: readonly NativeOrderHandle[],
    window: TerminalWindow,
  ): readonly unknown[];
  pageBy(
    state: QueryState,
    root: NativeBindingHandle,
    orders: readonly NativeOrderHandle[],
    window: TerminalWindow,
    includeTotal: boolean,
  ): Page<unknown>;
  countBy(state: QueryState, root: NativeBindingHandle): bigint;
  existsBy(state: QueryState, root: NativeBindingHandle): boolean;
}

const queryStates = new WeakMap<object, QueryState>();

/**
 * The single immutable positional/named typed-query facade.
 *
 * Runtime instances retain only an opaque native query handle and their
 * session's connection context. Each builder delegates to Rust and returns a
 * fresh frozen facade, leaving its ancestors unchanged.
 */
export class Query<Slots extends QuerySlots> {
  private constructor(handle: NativeQueryHandle, context: QueryRuntimeContext) {
    queryStates.set(this, { handle, context });
    Object.freeze(this);
  }

  match<const Models extends readonly [object, ...object[]]>(
    ...bindings: { readonly [Index in keyof Models]: BoundVar<Models[Index]> }
  ): Query<Slots> {
    const state = queryState(this);
    let handle = state.handle;
    for (const binding of bindings) {
      const nativeBinding = nativeBindingHandle(binding);
      handle = nativeCall(() => handle.addHidden(nativeBinding));
    }
    return createQuery(handle, state.context);
  }

  where(
    ...predicates: readonly [Predicate, ...Predicate[]]
  ): Query<Slots> {
    const state = queryState(this);
    let handle = state.handle;
    for (const predicate of predicates) {
      const nativePredicate = nativePredicateHandle(predicate);
      handle = nativeCall(() => handle.wherePredicate(nativePredicate));
    }
    return createQuery(handle, state.context);
  }

  allowCrossJoin<Left extends object, Right extends object>(
    left: BoundVar<Left>,
    right: BoundVar<Right>,
  ): Query<Slots> {
    const state = queryState(this);
    const handle = nativeCall(() =>
      state.handle.allowCrossJoin(nativeBindingHandle(left), nativeBindingHandle(right)),
    );
    return createQuery(handle, state.context);
  }

  one(
    this: QueryContainsCollection<Slots> extends true ? never : Query<Slots>,
  ): QueryRow<Slots> {
    const state = queryState(this);
    prepareDiagnostic(state, () =>
      state.handle.fetchRowsDiagnostic([], 0n, 1n, "exactly_one"),
    );
    return terminalAdapter.one(state) as QueryRow<Slots>;
  }

  rows(
    this: QueryContainsCollection<Slots> extends true ? never : Query<Slots>,
    options: RowsOptions,
  ): readonly QueryRow<Slots>[] {
    const state = queryState(this);
    const window = terminalWindow(options);
    const orders = nativeOrders(options.orderBy);
    prepareDiagnostic(state, () =>
      state.handle.fetchRowsDiagnostic(
        Array.from(orders),
        window.offset,
        window.limit,
        "bounded_many",
      ),
    );
    return terminalAdapter.rows(state, orders, window) as readonly QueryRow<Slots>[];
  }

  pageBy(
    this: [PageRootFor<Slots>] extends [never] ? never : Query<Slots>,
    root: BoundVar<Extract<PageRootFor<Slots>, object>>,
    options: PageOptions,
  ): Page<QueryRow<Slots>> {
    const state = queryState(this);
    const window = terminalWindow(options);
    const orders = nativeOrders(options.orderBy);
    const nativeRoot = nativeBindingHandle(root);
    prepareDiagnostic(state, () =>
      state.handle.pageByDiagnostic(
        nativeRoot,
        Array.from(orders),
        window.offset,
        window.limit,
        options.includeTotal ?? false,
      ),
    );
    return terminalAdapter.pageBy(
      state,
      nativeRoot,
      orders,
      window,
      options.includeTotal ?? false,
    ) as Page<QueryRow<Slots>>;
  }

  countBy<Root extends Extract<SingularOutputsFor<Slots>, object>>(
    root: BoundVar<Root>,
  ): bigint {
    const state = queryState(this);
    const nativeRoot = nativeBindingHandle(root);
    prepareDiagnostic(state, () => state.handle.countByDiagnostic(nativeRoot));
    return terminalAdapter.countBy(state, nativeRoot);
  }

  existsBy<Root extends Extract<SingularOutputsFor<Slots>, object>>(
    root: BoundVar<Root>,
  ): boolean {
    const state = queryState(this);
    const nativeRoot = nativeBindingHandle(root);
    prepareDiagnostic(state, () => state.handle.existsByDiagnostic(nativeRoot));
    return terminalAdapter.existsBy(state, nativeRoot);
  }
}

/** @internal Construct a facade around one native persistent query handle. */
export function createQuery<Slots extends QuerySlots>(
  handle: NativeQueryHandle,
  context: QueryRuntimeContext,
): Query<Slots> {
  const Constructor = Query as unknown as new (
    handle: NativeQueryHandle,
    context: QueryRuntimeContext,
  ) => Query<Slots>;
  return new Constructor(handle, context);
}

/** One adapter boundary from immutable query facades to opaque native proofs. */
const terminalAdapter: QueryTerminalAdapter = Object.freeze<QueryTerminalAdapter>({
  one(state) {
    const result = executeFetchRows(state, [], { offset: 0n, limit: 1n }, "exactly_one");
    return materializeValidatedOne(state.handle, result, state.context.models);
  },

  rows(state, orders, window) {
    const result = executeFetchRows(state, orders, window, "bounded_many");
    return materializeValidatedRows(state.handle, result, state.context.models);
  },

  pageBy(state, root, orders, window, includeTotal) {
    const result = executePageBy(state, root, orders, window, includeTotal);
    return materializeValidatedPage(
      state.handle,
      result,
      state.context.models,
      window.offset,
      window.limit,
      includeTotal,
    );
  },

  countBy(state, root) {
    const result = executeCountBy(state, root);
    return materializeValidatedCount(state.handle, result);
  },

  existsBy(state, root) {
    const result = executeExistsBy(state, root);
    return materializeValidatedExists(state.handle, result);
  },
});

function queryState(query: object): QueryState {
  const state = queryStates.get(query);
  if (state === undefined) {
    throw new TypeError("Query was not constructed by QuerySession.query");
  }
  return state;
}

function nativeOrders(orders: readonly QueryOrder[] | undefined): readonly NativeOrderHandle[] {
  return orders?.map(nativeQueryOrderHandle) ?? [];
}

function terminalWindow(options: RowsOptions): TerminalWindow {
  const limit = windowBound(options.limit, "limit", false);
  const offset = windowBound(options.offset ?? 0, "offset", true);
  if (!Number.isSafeInteger(options.limit + (options.offset ?? 0))) {
    throw windowError(
      "window_overflow",
      "window offset plus limit must remain a safe integer",
      options.limit + (options.offset ?? 0),
    );
  }
  return { offset, limit };
}

function windowBound(value: number, name: "limit" | "offset", allowZero: boolean): bigint {
  if (
    !Number.isSafeInteger(value) ||
    value < 0 ||
    (!allowZero && value === 0)
  ) {
    throw windowError(
      name === "limit" ? "invalid_window_limit" : "invalid_window_offset",
      name === "limit"
        ? "row and page limits must be positive safe integers"
        : "row and page offsets must be non-negative safe integers",
      value,
    );
  }
  return BigInt(value);
}

function windowError(code: string, message: string, value: number): TypedMatchError {
  return new TypedMatchError(
    "invalid_plan",
    code,
    message,
    Object.freeze([{ kind: "operation" }]),
    Object.freeze({
      value: Object.freeze({ kind: "text", value: String(value) }),
    }),
  );
}

function prepareDiagnostic(state: QueryState, construct: () => string): void {
  const diagnostic = nativeCall(construct);
  const native = loadNative();
  nativeCall(() => native.revalidateMatchDiagnostic(state.context.registry, diagnostic));
}

function executionUnavailable(state: QueryState): never {
  void state;
  throw new TypedMatchError(
    "invalid_plan",
    "execution_connection_required",
    "typed match execution requires a RustDatabase or RustTransactionContext",
    Object.freeze([{ kind: "operation" }]),
    Object.freeze({}),
  );
}

function executePageBy(
  state: QueryState,
  root: NativeBindingHandle,
  orders: readonly NativeOrderHandle[],
  window: TerminalWindow,
  includeTotal: boolean,
): ReturnType<NativeQueryHandle["executePageByOwned"]> {
  const connection = state.context.connection;
  if (connection === undefined) {
    return executionUnavailable(state);
  }
  const database = rustDatabaseHandle(connection);
  if (database !== undefined) {
    return nativeCall(() =>
      state.handle.executePageByOwned(
        database,
        root,
        Array.from(orders),
        window.offset,
        window.limit,
        includeTotal,
      ),
    );
  }
  const transaction = rustTransactionHandle(connection);
  if (transaction !== undefined) {
    return nativeCall(() =>
      state.handle.executePageByBorrowed(
        transaction,
        root,
        Array.from(orders),
        window.offset,
        window.limit,
        includeTotal,
      ),
    );
  }
  return invalidQueryConnection();
}

function executeCountBy(
  state: QueryState,
  root: NativeBindingHandle,
): ReturnType<NativeQueryHandle["executeCountByOwned"]> {
  const connection = state.context.connection;
  if (connection === undefined) {
    return executionUnavailable(state);
  }
  const database = rustDatabaseHandle(connection);
  if (database !== undefined) {
    return nativeCall(() => state.handle.executeCountByOwned(database, root));
  }
  const transaction = rustTransactionHandle(connection);
  if (transaction !== undefined) {
    return nativeCall(() => state.handle.executeCountByBorrowed(transaction, root));
  }
  return invalidQueryConnection();
}

function executeExistsBy(
  state: QueryState,
  root: NativeBindingHandle,
): ReturnType<NativeQueryHandle["executeExistsByOwned"]> {
  const connection = state.context.connection;
  if (connection === undefined) {
    return executionUnavailable(state);
  }
  const database = rustDatabaseHandle(connection);
  if (database !== undefined) {
    return nativeCall(() => state.handle.executeExistsByOwned(database, root));
  }
  const transaction = rustTransactionHandle(connection);
  if (transaction !== undefined) {
    return nativeCall(() => state.handle.executeExistsByBorrowed(transaction, root));
  }
  return invalidQueryConnection();
}

function invalidQueryConnection(): never {
  throw new TypedMatchError(
    "invalid_plan",
    "invalid_query_connection",
    "QuerySession connection was not constructed by the root Node package",
    Object.freeze([{ kind: "operation" }]),
    Object.freeze({}),
  );
}

function executeFetchRows(
  state: QueryState,
  orders: readonly NativeOrderHandle[],
  window: TerminalWindow,
  cardinality: "exactly_one" | "bounded_many",
): ReturnType<NativeQueryHandle["executeFetchRowsOwned"]> {
  const connection = state.context.connection;
  if (connection === undefined) {
    return executionUnavailable(state);
  }
  const database = rustDatabaseHandle(connection);
  if (database !== undefined) {
    return nativeCall(() =>
      state.handle.executeFetchRowsOwned(
        database,
        Array.from(orders),
        window.offset,
        window.limit,
        cardinality,
      ),
    );
  }
  const transaction = rustTransactionHandle(connection);
  if (transaction !== undefined) {
    return nativeCall(() =>
      state.handle.executeFetchRowsBorrowed(
        transaction,
        Array.from(orders),
        window.offset,
        window.limit,
        cardinality,
      ),
    );
  }
  return invalidQueryConnection();
}
