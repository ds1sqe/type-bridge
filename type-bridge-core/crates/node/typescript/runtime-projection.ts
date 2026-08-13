import {
  QueryV2Authority,
  type NativeRustDatabase,
  type NativeRustTransactionContext,
  type RustDatabase,
  type RustTransactionContext,
} from "./index.js";
import { loadNative } from "./native.js";
import type {
  NativeQueryCancellation,
  NativeQueryExecutionResources,
} from "./native.js";
import {
  queryV2AuthorityHandle,
  queryV2NativeCall,
  queryV2NativePromise,
  registerQueryV2AuthorityHandle,
} from "./query-v2-internals.js";
import {
  isRegisteredRustConnection,
  rustDatabaseHandle,
  rustTransactionHandle,
} from "./runtime-handles.js";

type NativeModule = ReturnType<typeof loadNative>;
type NativeRuntimeProjection = InstanceType<NativeModule["NodeRuntimeProjection"]>;
export type RuntimeProjectionMatchSession = ReturnType<NativeRuntimeProjection["matchSession"]>;
export type RuntimeProjectionMatchBinding = ReturnType<RuntimeProjectionMatchSession["exact"]>;
export type RuntimeProjectionMatchFunction = ReturnType<RuntimeProjectionMatchSession["functionById"]>;
export type RuntimeProjectionMatchFunctionValue = ReturnType<RuntimeProjectionMatchSession["functionValueJson"]>;
export type RuntimeProjectionMatchFunctionArgument = ReturnType<RuntimeProjectionMatchBinding["functionArgument"]>;
export type RuntimeProjectionMatchFunctionCall = ReturnType<RuntimeProjectionMatchFunction["call"]>;
export type RuntimeProjectionMatchField = ReturnType<RuntimeProjectionMatchBinding["field"]>;
export type RuntimeProjectionMatchPredicate = ReturnType<RuntimeProjectionMatchField["compareValueJson"]>;
export type RuntimeProjectionMatchOrder = ReturnType<RuntimeProjectionMatchField["order"]>;
export type RuntimeProjectionMatchSelection = ReturnType<RuntimeProjectionMatchBinding["one"]>;
export type RuntimeProjectionMatchShape = ReturnType<RuntimeProjectionMatchSession["positional"]>;
export type RuntimeProjectionMatchQuery = ReturnType<RuntimeProjectionMatchSession["query"]>;
export type RuntimeProjectionMatchResult = ReturnType<RuntimeProjectionMatchQuery["executeFetchRowsOwned"]>;
export type RuntimeProjectionMatchThing = ReturnType<RuntimeProjectionMatchResult["slotThing"]>;
export type RuntimeProjectionReduction =
  | "count"
  | "sum"
  | "min"
  | "max"
  | "mean"
  | "median"
  | "std";
type RuntimeProjectionRemoteContext = ReturnType<NativeModule["queryV2RemoteModelContextWithResources"]>;
type RuntimeProjectionRemotePending = ReturnType<NativeModule["queryV2PrepareRemoteModelRows"]>;

export type RuntimeProjectionConnection = RustDatabase | RustTransactionContext;

export interface RuntimeProjectionBinding {
  readonly typeKey: string;
  readonly targetName: string;
  readonly create: boolean;
  readonly reference: boolean;
}

export interface RuntimeProjectionInstall {
  readonly schemaAuthorityJson?: string;
  readonly projectionJson: string;
  readonly semanticFingerprintJson: string;
  readonly projectionFingerprintJson: string;
  readonly bindings: readonly RuntimeProjectionBinding[];
}

export interface GeneratedSchemaAuthorityInstall {
  readonly schemaAuthorityJson: string;
  readonly semanticFingerprintJson: string;
}

/** Explicit immutable budgets bound into every generated remote query. */
export interface RuntimeProjectionRemoteLimits {
  readonly maxItems: bigint;
  readonly maxBytes: bigint;
  readonly maxCollectionMembers: bigint;
  readonly maxGraphNodes: bigint;
  readonly maxAttributeValues: bigint;
  readonly maxRolePlayers: bigint;
  readonly deadlineMs?: bigint | null;
}

/** Common tighten-only execution policy shared by direct and remote queries. */
export interface QueryExecutionResourceLimitOptions {
  readonly timeoutMilliseconds?: bigint;
  readonly items?: bigint;
  readonly bytes?: bigint;
  readonly graphNodes?: bigint;
  readonly attributeValues?: bigint;
  readonly collectionMembers?: bigint;
  readonly rolePlayers?: bigint;
  readonly statements?: bigint;
}

/** Canonical common resource limits, clamped by the Rust semantic engine. */
export class QueryExecutionResourceLimits {
  readonly #native: NativeQueryExecutionResources;

  constructor(options: QueryExecutionResourceLimitOptions = {}) {
    const native = loadNative();
    this.#native = new native.NodeQueryExecutionResources(
      options.timeoutMilliseconds ?? 30_000n,
      options.items ?? 65_536n,
      options.bytes ?? 33_554_432n,
      options.graphNodes ?? 65_536n,
      options.attributeValues ?? 65_536n,
      options.collectionMembers ?? 65_536n,
      options.rolePlayers ?? 65_536n,
      options.statements ?? 3n,
    );
    Object.freeze(this);
  }

  get timeoutMilliseconds(): bigint { return this.#native.timeoutMilliseconds; }
  get items(): bigint { return this.#native.items; }
  get bytes(): bigint { return this.#native.bytes; }
  get graphNodes(): bigint { return this.#native.graphNodes; }
  get attributeValues(): bigint { return this.#native.attributeValues; }
  get collectionMembers(): bigint { return this.#native.collectionMembers; }
  get rolePlayers(): bigint { return this.#native.rolePlayers; }
  get statements(): bigint { return this.#native.statements; }

  /** @internal Exact native policy owner. */
  nativeHandle(): NativeQueryExecutionResources { return this.#native; }
}

/** Caller-owned cooperative cancellation for one or more query sessions. */
export class QueryCancellation {
  readonly #native: NativeQueryCancellation;
  readonly #cancelled: Promise<void>;
  readonly #listeners = new Set<() => void>();
  readonly #resolveCancelled: () => void;

  constructor() {
    this.#native = new (loadNative().NodeQueryCancellation)();
    let resolveCancelled: (() => void) | undefined;
    this.#cancelled = new Promise((resolve) => {
      resolveCancelled = resolve;
    });
    this.#resolveCancelled = () => resolveCancelled?.();
    Object.freeze(this);
  }

  cancel(): void {
    const notify = !this.#native.isCancelled;
    this.#native.cancel();
    if (!notify) {
      return;
    }
    this.#resolveCancelled();
    for (const listener of this.#listeners) {
      listener();
    }
    this.#listeners.clear();
  }
  get isCancelled(): boolean { return this.#native.isCancelled; }

  /** @internal Exact native cancellation owner. */
  nativeHandle(): NativeQueryCancellation { return this.#native; }

  /** @internal Resolve when caller-owned cancellation is first requested. */
  cancelled(): Promise<void> {
    return this.isCancelled ? Promise.resolve() : this.#cancelled;
  }

  /** @internal Attach an abort side effect without exposing mutable native state. */
  onCancelled(listener: () => void): () => void {
    if (this.isCancelled) {
      listener();
      return () => {};
    }
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }
}

/** One caller-owned request/response exchange. No retry is performed. */
export type RuntimeProjectionRemoteExchange = (
  request: Uint8Array,
  signal?: AbortSignal,
) => Promise<Uint8Array>;

/** Opaque verified remote terminal executor for one generated package. */
export interface RuntimeProjectionRemote {
  rows(
    query: RuntimeProjectionMatchQuery,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    cardinality: "exactly_one" | "bounded_many",
  ): Promise<RuntimeProjectionMatchResult>;
  page(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): Promise<RuntimeProjectionMatchResult>;
  count(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
  ): Promise<RuntimeProjectionMatchResult>;
  exists(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
  ): Promise<RuntimeProjectionMatchResult>;
  reduce(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchBinding | null,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult>;
  reduceByField(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchField,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult>;
  reduceByFields(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    groups: RuntimeProjectionMatchField[],
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult>;
}

export interface NativeProjectedManager {
  insertJson(instanceJson: string): string;
  insertManyJson(batchJson: string): string;
  putJson(instanceJson: string): string;
  putManyJson(batchJson: string): string;
  updateJson(iid: string, instanceJson: string): string;
  deleteByIid(iid: string): void;
  filterJson(filtersJson: string): NativeProjectedManager;
  getByIidJson(iid: string): string;
  allJson(): string;
  firstJson(): string;
  count(): bigint;
  exists(): boolean;
}

interface NativeProjectionHandle {
  managerForDatabase(typeKey: string, database: NativeRustDatabase): NativeProjectedManager;
  managerForTransaction(typeKey: string, transaction: NativeRustTransactionContext): NativeProjectedManager;
  matchSession(): RuntimeProjectionMatchSession;
  matchSessionWithResources(
    resources: NativeQueryExecutionResources,
    cancellation: NativeQueryCancellation,
  ): RuntimeProjectionMatchSession;
  matchModelType(typeKey: string): string;
  validateAttributeValueJson(typeKey: string, valueJson: string): void;
  validateHydratedAttributeValueJson(typeKey: string, valueJson: string): void;
  validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
  validateCreateJson(typeKey: string, valueJson: string): void;
  validateThingJson(typeKey: string, valueJson: string): void;
  rejectGeneratedTokenPackageMismatch(pathJson: string): void;
  revalidateMatchDiagnostic(diagnostic: string): string;
  materializeMatchThingJson(thing: RuntimeProjectionMatchThing): string;
}

/** A verified native projection scoped to one generated package instance. */
export class InstalledRuntimeProjection {
  readonly #native: NativeProjectionHandle;

  constructor(native: NativeProjectionHandle) {
    this.#native = native;
    Object.freeze(this);
  }

  /** @internal Bind one generated token without exposing its native handle. */
  manager(typeKey: string, connection: RuntimeProjectionConnection): NativeProjectedManager {
    if (!isRegisteredRustConnection(connection)) {
      throw new TypeError("projected manager requires a registered RustDatabase or RustTransactionContext");
    }
    const database = rustDatabaseHandle(connection);
    if (database !== undefined) {
      return this.#native.managerForDatabase(typeKey, database);
    }
    const transaction = rustTransactionHandle(connection);
    if (transaction !== undefined) {
      return this.#native.managerForTransaction(typeKey, transaction);
    }
    throw new TypeError("projected manager has no registered native execution handle");
  }

  /** @internal Create an opaque query session from verified projection evidence. */
  matchSession(
    resources?: QueryExecutionResourceLimits,
    cancellation?: QueryCancellation,
  ): RuntimeProjectionMatchSession {
    if (resources === undefined && cancellation === undefined) {
      return this.#native.matchSession();
    }
    resources ??= new QueryExecutionResourceLimits();
    cancellation ??= new QueryCancellation();
    return this.#native.matchSessionWithResources(
      resources.nativeHandle(),
      cancellation.nativeHandle(),
    );
  }

  /** @internal Resolve one exact generated model token to its provider label. */
  matchModelType(typeKey: string): string {
    return this.#native.matchModelType(typeKey);
  }

  /** @internal Validate one generated attribute scalar against projected constraints. */
  validateAttributeValueJson(typeKey: string, valueJson: string): void {
    this.#native.validateAttributeValueJson(typeKey, valueJson);
  }

  /** @internal Validate one provider-hydrated attribute scalar. */
  validateHydratedAttributeValueJson(typeKey: string, valueJson: string): void {
    this.#native.validateHydratedAttributeValueJson(typeKey, valueJson);
  }

  /** @internal Validate one generated owned-field scalar against projected constraints. */
  validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void {
    this.#native.validateFieldValueJson(typeKey, fieldName, valueJson);
  }

  /** @internal Validate one complete generated create payload. */
  validateCreateJson(typeKey: string, valueJson: string): void {
    this.#native.validateCreateJson(typeKey, valueJson);
  }

  /** @internal Validate one complete generated provider result. */
  validateThingJson(typeKey: string, valueJson: string): void {
    this.#native.validateThingJson(typeKey, valueJson);
  }

  /** @internal Surface one exact foreign generated-member package boundary. */
  rejectGeneratedTokenPackageMismatch(pathJson: string): void {
    this.#native.rejectGeneratedTokenPackageMismatch(pathJson);
  }

  /** @internal Reject structural or foreign connection lookalikes. */
  assertConnection(connection: RuntimeProjectionConnection): void {
    if (!isRegisteredRustConnection(connection)) {
      throw new TypeError("generated query requires a registered RustDatabase or RustTransactionContext");
    }
  }

  /** @internal Materialize one native-validated thing as projected private JSON. */
  materializeMatchThingJson(thing: RuntimeProjectionMatchThing): string {
    return this.#native.materializeMatchThingJson(thing);
  }

  /** @internal Execute one selected-row request through the verified projection. */
  executeRows(
    query: RuntimeProjectionMatchQuery,
    connection: RuntimeProjectionConnection,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    cardinality: "exactly_one" | "bounded_many",
  ): RuntimeProjectionMatchResult {
    return queryV2NativeCall(() => {
      this.#native.revalidateMatchDiagnostic(
        query.fetchRowsDiagnostic(orders, offset, limit, cardinality),
      );
      const database = this.#database(connection);
      if (database !== undefined) {
        return query.executeFetchRowsOwned(database, orders, offset, limit, cardinality);
      }
      return query.executeFetchRowsBorrowed(
        this.#transaction(connection),
        orders,
        offset,
        limit,
        cardinality,
      );
    });
  }

  /** @internal Execute one distinct-root page through the verified projection. */
  executePage(
    query: RuntimeProjectionMatchQuery,
    connection: RuntimeProjectionConnection,
    root: RuntimeProjectionMatchBinding,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): RuntimeProjectionMatchResult {
    return queryV2NativeCall(() => {
      this.#native.revalidateMatchDiagnostic(
        query.pageByDiagnostic(root, orders, offset, limit, includeTotal),
      );
      const database = this.#database(connection);
      if (database !== undefined) {
        return query.executePageByOwned(database, root, orders, offset, limit, includeTotal);
      }
      return query.executePageByBorrowed(
        this.#transaction(connection),
        root,
        orders,
        offset,
        limit,
        includeTotal,
      );
    });
  }

  /** @internal Execute one distinct-root count through the verified projection. */
  executeCount(
    query: RuntimeProjectionMatchQuery,
    connection: RuntimeProjectionConnection,
    root: RuntimeProjectionMatchBinding,
  ): bigint {
    return queryV2NativeCall(() => {
      this.#native.revalidateMatchDiagnostic(query.countByDiagnostic(root));
      const database = this.#database(connection);
      const result = database === undefined
        ? query.executeCountByBorrowed(this.#transaction(connection), root)
        : query.executeCountByOwned(database, root);
      return result.countValue(query);
    });
  }

  /** @internal Execute one distinct-root existence request. */
  executeExists(
    query: RuntimeProjectionMatchQuery,
    connection: RuntimeProjectionConnection,
    root: RuntimeProjectionMatchBinding,
  ): boolean {
    return queryV2NativeCall(() => {
      this.#native.revalidateMatchDiagnostic(query.existsByDiagnostic(root));
      const database = this.#database(connection);
      const result = database === undefined
        ? query.executeExistsByBorrowed(this.#transaction(connection), root)
        : query.executeExistsByOwned(database, root);
      return result.existsValue(query);
    });
  }

  /** @internal Execute one typed ungrouped or grouped reduction. */
  executeReduce(
    query: RuntimeProjectionMatchQuery,
    connection: RuntimeProjectionConnection,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchBinding | null,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): RuntimeProjectionMatchResult {
    return queryV2NativeCall(() => {
      this.#native.revalidateMatchDiagnostic(
        query.reduceByDiagnostic(root, group, reducers, inputs),
      );
      const database = this.#database(connection);
      return database === undefined
        ? query.executeReduceByBorrowed(
            this.#transaction(connection),
            root,
            group,
            reducers,
            inputs,
          )
        : query.executeReduceByOwned(database, root, group, reducers, inputs);
    });
  }

  /** @internal Execute one typed reduction grouped by an owned field value. */
  executeReduceByField(
    query: RuntimeProjectionMatchQuery,
    connection: RuntimeProjectionConnection,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchField,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): RuntimeProjectionMatchResult {
    return queryV2NativeCall(() => {
      this.#native.revalidateMatchDiagnostic(
        query.reduceByFieldDiagnostic(root, group, reducers, inputs),
      );
      const database = this.#database(connection);
      return database === undefined
        ? query.executeReduceByFieldBorrowed(
            this.#transaction(connection),
            root,
            group,
            reducers,
            inputs,
          )
        : query.executeReduceByFieldOwned(database, root, group, reducers, inputs);
    });
  }

  /** @internal Execute one typed reduction grouped by an owned-field tuple. */
  executeReduceByFields(
    query: RuntimeProjectionMatchQuery,
    connection: RuntimeProjectionConnection,
    root: RuntimeProjectionMatchBinding,
    groups: RuntimeProjectionMatchField[],
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): RuntimeProjectionMatchResult {
    return queryV2NativeCall(() => {
      this.#native.revalidateMatchDiagnostic(
        query.reduceByFieldsDiagnostic(root, groups, reducers, inputs),
      );
      const database = this.#database(connection);
      return database === undefined
        ? query.executeReduceByFieldsBorrowed(
            this.#transaction(connection),
            root,
            groups,
            reducers,
            inputs,
          )
        : query.executeReduceByFieldsOwned(database, root, groups, reducers, inputs);
    });
  }

  /** @internal Bind remote authority, executor epoch, budgets, and exchange once. */
  remote(
    authority: QueryV2Authority,
    advertisement: Uint8Array,
    exchange: RuntimeProjectionRemoteExchange,
    limits: RuntimeProjectionRemoteLimits | QueryExecutionResourceLimits,
    cancellation?: QueryCancellation,
  ): RuntimeProjectionRemote {
    const nativeAuthority = queryV2AuthorityHandle(authority);
    if (nativeAuthority === undefined) {
      throw new TypeError("generated remote query requires a type-bridge QueryV2Authority");
    }
    if (!(advertisement instanceof Uint8Array)) {
      throw new TypeError("generated remote query advertisement must be a Uint8Array");
    }
    if (typeof exchange !== "function") {
      throw new TypeError("generated remote query exchange must be callable");
    }
    if (typeof limits !== "object" || limits === null) {
      throw new TypeError("generated remote query limits must be an object");
    }
    const commonResources = limits instanceof QueryExecutionResourceLimits;
    const resources = commonResources ? limits : new QueryExecutionResourceLimits({
          timeoutMilliseconds: limits.deadlineMs ?? 30_000n,
          items: limits.maxItems,
          bytes: limits.maxBytes,
          collectionMembers: limits.maxCollectionMembers,
          graphNodes: limits.maxGraphNodes,
          attributeValues: limits.maxAttributeValues,
          rolePlayers: limits.maxRolePlayers,
          statements: 3n,
        });
    const native = loadNative();
    const effectiveCancellation = cancellation ?? new QueryCancellation();
    const context = !commonResources && cancellation === undefined
      ? queryV2NativeCall(() => native.queryV2RemoteModelContext(
          nativeAuthority,
          advertisement,
          limits.maxItems,
          limits.maxBytes,
          limits.maxCollectionMembers,
          limits.maxGraphNodes,
          limits.maxAttributeValues,
          limits.maxRolePlayers,
          limits.deadlineMs,
        ))
      : queryV2NativeCall(() => native.queryV2RemoteModelContextWithResources(
          nativeAuthority,
          advertisement,
          resources.nativeHandle(),
          effectiveCancellation.nativeHandle(),
        ));
    return new InstalledRuntimeProjectionRemote(
      native,
      context,
      exchange,
      effectiveCancellation,
    );
  }

  #database(connection: RuntimeProjectionConnection): NativeRustDatabase | undefined {
    if (!isRegisteredRustConnection(connection)) {
      throw new TypeError("generated query requires a registered Rust connection");
    }
    return rustDatabaseHandle(connection);
  }

  #transaction(connection: RuntimeProjectionConnection): NativeRustTransactionContext {
    const transaction = rustTransactionHandle(connection);
    if (transaction === undefined) {
      throw new TypeError("generated query has no registered native execution handle");
    }
    return transaction;
  }
}

/** Verified remote execution bound to one projection, authority, and executor epoch. */
class InstalledRuntimeProjectionRemote implements RuntimeProjectionRemote {
  readonly #native: NativeModule;
  readonly #context: RuntimeProjectionRemoteContext;
  readonly #exchange: RuntimeProjectionRemoteExchange;
  readonly #cancellation: QueryCancellation;

  constructor(
    native: NativeModule,
    context: RuntimeProjectionRemoteContext,
    exchange: RuntimeProjectionRemoteExchange,
    cancellation: QueryCancellation,
  ) {
    this.#native = native;
    this.#context = context;
    this.#exchange = exchange;
    this.#cancellation = cancellation;
    Object.freeze(this);
  }

  rows(
    query: RuntimeProjectionMatchQuery,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    cardinality: "exactly_one" | "bounded_many",
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#execute(queryV2NativeCall(() => this.#native.queryV2PrepareRemoteModelRows(
      query,
      this.#context,
      orders,
      offset,
      limit,
      cardinality,
    )));
  }

  page(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#execute(queryV2NativeCall(() => this.#native.queryV2PrepareRemoteModelPage(
      query,
      this.#context,
      root,
      orders,
      offset,
      limit,
      includeTotal,
    )));
  }

  count(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#execute(queryV2NativeCall(() =>
      this.#native.queryV2PrepareRemoteModelCount(query, this.#context, root)
    ));
  }

  exists(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#execute(queryV2NativeCall(() =>
      this.#native.queryV2PrepareRemoteModelExists(query, this.#context, root)
    ));
  }

  reduce(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchBinding | null,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#execute(queryV2NativeCall(() =>
      this.#native.queryV2PrepareRemoteModelReduce(
        query,
        this.#context,
        root,
        group,
        reducers,
        inputs,
      )
    ));
  }

  reduceByField(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchField,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#execute(queryV2NativeCall(() =>
      this.#native.queryV2PrepareRemoteModelReduceByField(
        query,
        this.#context,
        root,
        group,
        reducers,
        inputs,
      )
    ));
  }

  reduceByFields(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    groups: RuntimeProjectionMatchField[],
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#execute(queryV2NativeCall(() =>
      this.#native.queryV2PrepareRemoteModelReduceByFields(
        query,
        this.#context,
        root,
        groups,
        reducers,
        inputs,
      )
    ));
  }

  async #execute(pending: RuntimeProjectionRemotePending): Promise<RuntimeProjectionMatchResult> {
    const request = queryV2NativeCall(() => new Uint8Array(pending.requestBytes()));
    const controller = new AbortController();
    const detach = this.#cancellation.onCancelled(() => controller.abort());
    const exchange = Promise.resolve()
      .then(() => this.#exchange(request, controller.signal))
      .then(
        (response) => ({ kind: "response" as const, response }),
        (error: unknown) => ({ kind: "error" as const, error }),
      );
    try {
      const outcome = await Promise.race([
        exchange,
        this.#cancellation.cancelled().then(() => ({ kind: "cancelled" as const })),
      ]);
      if (outcome.kind === "cancelled") {
        // Claim through the neutral one-shot authority after cancellation so
        // the pending request is consumed and the canonical diagnostic wins.
        return await queryV2NativePromise(queryV2NativeCall(() =>
          pending.decodeReply(new Uint8Array())
        ));
      }
      if (outcome.kind === "error") {
        throw outcome.error;
      }
      const response = outcome.response;
      if (!(response instanceof Uint8Array)) {
        throw new TypeError("generated remote query exchange must resolve to a Uint8Array");
      }
      return queryV2NativePromise(queryV2NativeCall(() => pending.decodeReply(response)));
    } finally {
      detach();
    }
  }
}

/** Verify and install one generated package's exact projection evidence. */
export function installRuntimeProjection(input: RuntimeProjectionInstall): InstalledRuntimeProjection {
  const native = loadNative();
  return new InstalledRuntimeProjection(new native.NodeRuntimeProjection(
    input.projectionJson,
    input.semanticFingerprintJson,
    input.projectionFingerprintJson,
    JSON.stringify(input.bindings),
    input.schemaAuthorityJson,
  ));
}

/** @internal Verify an embedded authority before a generated package installs. */
export function installGeneratedSchemaAuthority(
  input: GeneratedSchemaAuthorityInstall,
): QueryV2Authority {
  const authority = Object.create(QueryV2Authority.prototype) as QueryV2Authority;
  registerQueryV2AuthorityHandle(
    authority,
    queryV2NativeCall(() =>
      loadNative().queryV2AuthorityFromSchemaAuthority(
        input.schemaAuthorityJson,
        input.semanticFingerprintJson,
      )
    ),
  );
  Object.freeze(authority);
  return authority;
}
