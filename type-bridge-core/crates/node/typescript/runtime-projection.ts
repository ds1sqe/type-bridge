import {
  createRustDatabaseFromNative,
  QueryV2Authority,
  type NativeRustDatabase,
  type NativeRustTransactionContext,
  type RustDatabase,
  type RustTransactionContext,
} from "./index.js";
import { loadNative } from "./native.js";
import type {
  NativeProxyConnection,
  NativeQueryCancellation,
  NativeQueryExecutionResources,
  NativeQueryLimits,
} from "./native.js";
import { ownedByteSnapshot } from "./owned-bytes.js";
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
export type RuntimeProjectionMatchSession = ReturnType<
  NativeRuntimeProjection["matchSessionWithResources"]
>;
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
export type RuntimeProjectionMatchResult = Awaited<
  ReturnType<RuntimeProjectionMatchQuery["executeFetchRowsOwned"]>
>;
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
  /**
   * @internal Generated-package-only authority installed exactly once.
   * Manual callbacks, global or prototype mutation, and provider re-entry are
   * unsupported and outside the runtime projection contract.
   */
  readonly projectedBatchMaterializer?: NativeProjectedBatchMaterializer;
}

export interface GeneratedSchemaAuthorityInstall {
  readonly schemaAuthorityJson: string;
  readonly semanticFingerprintJson: string;
}

/** The answer policy shared by direct and remote queries; a dimension may tighten or raise its default. */
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

/** The answer policy, validated by the Rust semantic engine: a value past a supported maximum is refused. */
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

/** Stable names of the query limits a session can override. */
export type QueryLimitName =
  | "canonical_bytes"
  | "canonical_depth"
  | "canonical_collection_len"
  | "canonical_string_bytes"
  | "selected_slots"
  | "bindings"
  | "predicate_nodes"
  | "predicate_depth"
  | "boolean_terms"
  | "allowed_cross_joins"
  | "order_terms"
  | "collection_order_terms"
  | "input_rows"
  | "input_bytes"
  | "output_name_bytes"
  | "semantic_id_bytes"
  | "diagnostic_bytes"
  | "remote_request_bytes";

/** Overrides of the canonical query limits, keyed by stable limit name. */
export type QueryLimitOverrides = Readonly<Partial<Record<QueryLimitName, bigint>>>;

/**
 * Query limits one generated session validates, encodes, and executes under.
 *
 * Rust admits the overrides: an unknown name or an unsupported value throws
 * here, before any session exists. A limit left out keeps its default.
 */
export class QueryLimits {
  readonly #native: NativeQueryLimits;

  constructor(overrides: QueryLimitOverrides = {}) {
    const entries = Object.entries(overrides) as [string, bigint][];
    this.#native = new (loadNative().NodeQueryLimits)(
      entries.map(([name]) => name),
      entries.map(([, value]) => value),
    );
    Object.freeze(this);
  }

  /** Stable names of every query limit. */
  names(): readonly string[] { return this.#native.names(); }

  /** Effective value of one named limit. */
  get(name: QueryLimitName): bigint { return this.#native.get(name); }

  /** @internal Exact native limits owner. */
  nativeHandle(): NativeQueryLimits { return this.#native; }
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

declare const nativeProjectedFacadeProof: unique symbol;

/** @internal Opaque non-serializable proof retained only by generated successor facades. */
export interface NativeProjectedFacadeProof {
  readonly [nativeProjectedFacadeProof]: never;
}

/** @internal Private wire plus exact root and role-player proofs. */
export interface NativeProjectedValueEnvelope {
  readonly json: string;
  rootProof(): NativeProjectedFacadeProof;
  roleProof(roleName: string, playerIndex: number): NativeProjectedFacadeProof;
}

type NativeProjectedBatchMaterializer = (
  typeKey: string,
  ordinal: number,
  json: string,
  authority: NativeProjectedBatchAuthority,
) => object;

declare const nativeProjectedBatchAuthority: unique symbol;

/** @internal One opaque pending/active authority shared by a projected batch. */
interface NativeProjectedBatchAuthority {
  readonly [nativeProjectedBatchAuthority]: never;
}

/** @internal Exact root selector backed by one batch publication authority. */
interface NativeProjectedBatchRootProof {
  readonly kind: "root";
  readonly authority: NativeProjectedBatchAuthority;
  readonly row: number;
}

/** @internal Exact role-player selector backed by one batch publication authority. */
interface NativeProjectedBatchRoleProof {
  readonly kind: "role";
  readonly authority: NativeProjectedBatchAuthority;
  readonly row: number;
  readonly roleName: string;
  readonly playerIndex: number;
}

/** @internal Proof input accepted from a single envelope or a batch selector. */
type NativeProjectedInputProof =
  | NativeProjectedFacadeProof
  | NativeProjectedBatchRootProof
  | NativeProjectedBatchRoleProof;

interface NativeProjectedCreateBatchRow {
  readonly instanceJson: string;
  readonly proofs: readonly (NativeProjectedInputProof | null)[];
}

interface NativeProjectedUpdateBatchRow extends NativeProjectedCreateBatchRow {
  readonly iid: string;
}

export interface NativeProjectedManager {
  insertProjected(
    instanceJson: string,
    proofs: (NativeProjectedInputProof | null)[],
  ): NativeProjectedValueEnvelope;
  insertJson(instanceJson: string): string;
  insertManyJson(batchJson: string): string;
  insertManyProjected<Complete>(
    rowCount: number,
    rowAt: (ordinal: number) => NativeProjectedCreateBatchRow,
  ): readonly Complete[];
  putProjected(
    instanceJson: string,
    proofs: (NativeProjectedInputProof | null)[],
  ): NativeProjectedValueEnvelope;
  putJson(instanceJson: string): string;
  putManyJson(batchJson: string): string;
  putManyProjected<Complete>(
    rowCount: number,
    rowAt: (ordinal: number) => NativeProjectedCreateBatchRow,
  ): readonly Complete[];
  updateProjected(
    iid: string,
    instanceJson: string,
    proofs: (NativeProjectedInputProof | null)[],
  ): NativeProjectedValueEnvelope;
  updateJson(iid: string, instanceJson: string): string;
  updateManyProjected<Complete>(
    rowCount: number,
    rowAt: (ordinal: number) => NativeProjectedUpdateBatchRow,
  ): readonly Complete[];
  deleteByIid(iid: string): void;
  deleteManyProjected(
    rowCount: number,
    rowAt: (ordinal: number) => string,
  ): void;
  managerFilter(): NativeProjectedManagerFilter;
  filterEntriesJson(filtersJson: string): NativeProjectedManager;
  filterJson(filtersJson: string): NativeProjectedManager;
  getByIidProjected(iid: string): NativeProjectedValueEnvelope | null;
  getByIidJson(iid: string): string;
  allJson(): string;
  firstJson(): string;
  count(): bigint;
  exists(): boolean;
}

/** @internal Immutable common field-token filter for ordered generated managers. */
export interface NativeProjectedManagerFilter {
  andProjected(
    fieldOwnerTypeKey: string,
    fieldAttributeKey: string,
    comparison: "eq" | "ne" | "lt" | "lte" | "gt" | "gte",
    valueJson: string,
  ): NativeProjectedManagerFilter;
  rejectForeignFieldToken(): never;
  allProjected(): readonly NativeProjectedValueEnvelope[];
  firstProjected(): NativeProjectedValueEnvelope | null;
  count(): bigint;
  exists(): boolean;
}

/** @internal Preserve structured SDK diagnostics from generated-manager N-API calls. */
export function projectedManagerNativeCall<Result>(operation: () => Result): Result {
  return queryV2NativeCall(operation);
}

interface NativeProjectionHandle {
  connectDirect(
    endpoint: string,
    database: string,
    username: string,
    password: string,
    httpPort: number,
    tlsMode: string,
    tlsRootCa?: string,
    connectionLimits?: NativeQueryExecutionResources,
    answerLimits?: NativeQueryExecutionResources,
    cancellation?: NativeQueryCancellation,
  ): NativeRustDatabase;
  managerForDatabase(typeKey: string, database: NativeRustDatabase): NativeProjectedManager;
  managerForTransaction(typeKey: string, transaction: NativeRustTransactionContext): NativeProjectedManager;
  matchSessionWithResources(
    resources: NativeQueryExecutionResources,
    cancellation: NativeQueryCancellation,
    queryLimits?: NativeQueryLimits | null,
  ): RuntimeProjectionMatchSession;
  matchModelType(typeKey: string): string;
  validateAttributeValueJson(typeKey: string, valueJson: string): void;
  validateHydratedAttributeValueJson(typeKey: string, valueJson: string): void;
  validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
  validateCreateJson(typeKey: string, valueJson: string): void;
  encodeAttributeJson(typeKey: string, valueJson: string): Uint8Array;
  decodeAttributeJson(typeKey: string, bytes: Uint8Array): string;
  encodeCreateJson(typeKey: string, valueJson: string): Uint8Array;
  decodeCreateJson(typeKey: string, bytes: Uint8Array): string;
  encodeReferenceJson(typeKey: string, valueJson: string): Uint8Array;
  decodeReferenceJson(typeKey: string, bytes: Uint8Array): string;
  encodeSnapshotJson(typeKey: string, valueJson: string): Uint8Array;
  decodeSnapshotJson(typeKey: string, bytes: Uint8Array): string;
  detachedSnapshotProof(): NativeProjectedFacadeProof;
  encodeStructJson(typeKey: string, valueJson: string): Uint8Array;
  decodeStructJson(typeKey: string, bytes: Uint8Array): string;
  encodeArchive(records: readonly Uint8Array[]): Uint8Array;
  decodeArchive(bytes: Uint8Array): Uint8Array[];
  encodeArchiveControlled(
    records: readonly Uint8Array[],
    cancellation?: NativeQueryCancellation,
    timeoutMilliseconds?: number,
    maxInputBytes?: number,
    maxOutputBytes?: number,
    maxDepth?: number,
    maxRecords?: number,
    maxMembers?: number,
  ): Uint8Array;
  decodeArchiveControlled(
    bytes: Uint8Array,
    cancellation?: NativeQueryCancellation,
    timeoutMilliseconds?: number,
    maxInputBytes?: number,
    maxOutputBytes?: number,
    maxDepth?: number,
    maxRecords?: number,
    maxMembers?: number,
  ): Uint8Array[];
  encodeRecordJsonControlled(
    recordKind: string,
    typeKey: string,
    valueJson: string,
    cancellation?: NativeQueryCancellation,
    timeoutMilliseconds?: number,
    maxInputBytes?: number,
    maxOutputBytes?: number,
    maxDepth?: number,
    maxMembers?: number,
  ): Uint8Array;
  decodeRecordJsonControlled(
    recordKind: string,
    typeKey: string,
    bytes: Uint8Array,
    cancellation?: NativeQueryCancellation,
    timeoutMilliseconds?: number,
    maxInputBytes?: number,
    maxOutputBytes?: number,
    maxDepth?: number,
    maxMembers?: number,
  ): string;
  validateThingJson(typeKey: string, valueJson: string): void;
  rejectGeneratedTokenPackageMismatch(pathJson: string): void;
  revalidateMatchDiagnostic(diagnostic: string): string;
  materializeMatchThingJson(thing: RuntimeProjectionMatchThing): string;
  materializeMatchThingProjected(
    thing: RuntimeProjectionMatchThing,
  ): NativeProjectedValueEnvelope;
}

/** Tighten-only controls for canonical record/archive work. */
export interface CanonicalCodecOptions {
  readonly cancellation?: QueryCancellation;
  readonly timeoutMilliseconds?: number;
  readonly maxInputBytes?: number;
  readonly maxOutputBytes?: number;
  readonly maxDepth?: number;
  readonly maxRecords?: number;
  readonly maxMembers?: number;
}

/** A verified native projection scoped to one generated package instance. */
export class InstalledRuntimeProjection {
  readonly #native: NativeProjectionHandle;

  constructor(native: NativeProjectionHandle) {
    this.#native = native;
    Object.freeze(this);
  }

  /** @internal Open through this installed generated package's authority. */
  connectDirect(input: {
    endpoint: string;
    database: string;
    username: string;
    password: string;
    httpPort: number;
    tlsMode: "disabled" | "native_roots" | "custom_root";
    tlsRootCa?: string;
    connectionLimits?: QueryExecutionResourceLimits;
    answerLimits?: QueryExecutionResourceLimits;
    cancellation?: QueryCancellation;
  }): RustDatabase {
    return createRustDatabaseFromNative(this.#native.connectDirect(
      input.endpoint,
      input.database,
      input.username,
      input.password,
      input.httpPort,
      input.tlsMode,
      input.tlsRootCa,
      input.connectionLimits?.nativeHandle(),
      input.answerLimits?.nativeHandle(),
      input.cancellation?.nativeHandle(),
    ));
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
    queryLimits?: QueryLimits,
  ): RuntimeProjectionMatchSession {
    resources ??= new QueryExecutionResourceLimits();
    cancellation ??= new QueryCancellation();
    return this.#native.matchSessionWithResources(
      resources.nativeHandle(),
      cancellation.nativeHandle(),
      queryLimits?.nativeHandle() ?? null,
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

  /** @internal Encode one exact generated attribute value to canonical bytes. */
  encodeAttributeJson(typeKey: string, valueJson: string): Uint8Array {
    return this.#native.encodeAttributeJson(typeKey, valueJson);
  }

  /** @internal Decode canonical attribute bytes through exact package authority. */
  decodeAttributeJson(typeKey: string, bytes: Uint8Array): string {
    return this.#native.decodeAttributeJson(typeKey, bytes);
  }

  /** @internal Encode one exact generated create value to canonical bytes. */
  encodeCreateJson(typeKey: string, valueJson: string): Uint8Array {
    return this.#native.encodeCreateJson(typeKey, valueJson);
  }

  /** @internal Decode canonical bytes through this package's exact authority. */
  decodeCreateJson(typeKey: string, bytes: Uint8Array): string {
    return this.#native.decodeCreateJson(typeKey, bytes);
  }

  /** @internal Encode one exact generated detached reference. */
  encodeReferenceJson(typeKey: string, valueJson: string): Uint8Array {
    return this.#native.encodeReferenceJson(typeKey, valueJson);
  }

  /** @internal Decode one exact generated detached reference. */
  decodeReferenceJson(typeKey: string, bytes: Uint8Array): string {
    return this.#native.decodeReferenceJson(typeKey, bytes);
  }

  /** @internal Encode one exact generated detached snapshot. */
  encodeSnapshotJson(typeKey: string, valueJson: string): Uint8Array {
    return this.#native.encodeSnapshotJson(typeKey, valueJson);
  }

  /** @internal Decode one exact generated detached snapshot. */
  decodeSnapshotJson(typeKey: string, bytes: Uint8Array): string {
    return this.#native.decodeSnapshotJson(typeKey, bytes);
  }

  /** @internal Return an opaque mutation fence for one decoded snapshot facade. */
  detachedSnapshotProof(): NativeProjectedFacadeProof {
    return this.#native.detachedSnapshotProof();
  }

  /** @internal Encode one exact generated struct. */
  encodeStructJson(typeKey: string, valueJson: string): Uint8Array {
    return this.#native.encodeStructJson(typeKey, valueJson);
  }

  /** @internal Decode one exact generated struct. */
  decodeStructJson(typeKey: string, bytes: Uint8Array): string {
    return this.#native.decodeStructJson(typeKey, bytes);
  }

  /** @internal Compose verified canonical records into one archive. */
  encodeArchive(records: readonly Uint8Array[]): Uint8Array {
    return this.#native.encodeArchive(records);
  }

  /** @internal Split and verify one canonical archive. */
  decodeArchive(bytes: Uint8Array): readonly Uint8Array[] {
    return Object.freeze(this.#native.decodeArchive(bytes));
  }

  /** @internal Compose records with cancellation, deadline, and resource limits. */
  encodeArchiveControlled(
    records: readonly Uint8Array[],
    options: CanonicalCodecOptions = {},
  ): Uint8Array {
    return this.#native.encodeArchiveControlled(
      records,
      options.cancellation?.nativeHandle(),
      options.timeoutMilliseconds,
      options.maxInputBytes,
      options.maxOutputBytes,
      options.maxDepth,
      options.maxRecords,
      options.maxMembers,
    );
  }

  /** @internal Split records with cancellation, deadline, and resource limits. */
  decodeArchiveControlled(
    bytes: Uint8Array,
    options: CanonicalCodecOptions = {},
  ): readonly Uint8Array[] {
    return Object.freeze(this.#native.decodeArchiveControlled(
      bytes,
      options.cancellation?.nativeHandle(),
      options.timeoutMilliseconds,
      options.maxInputBytes,
      options.maxOutputBytes,
      options.maxDepth,
      options.maxRecords,
      options.maxMembers,
    ));
  }

  /** @internal Encode one exact nominal record under managed controls. */
  encodeRecordJsonControlled(
    recordKind: "attribute" | "create" | "reference" | "snapshot" | "struct",
    typeKey: string,
    valueJson: string,
    options: CanonicalCodecOptions = {},
  ): Uint8Array {
    return this.#native.encodeRecordJsonControlled(
      recordKind,
      typeKey,
      valueJson,
      options.cancellation?.nativeHandle(),
      options.timeoutMilliseconds,
      options.maxInputBytes,
      options.maxOutputBytes,
      options.maxDepth,
      options.maxMembers,
    );
  }

  /** @internal Decode one exact nominal record under managed controls. */
  decodeRecordJsonControlled(
    recordKind: "attribute" | "create" | "reference" | "snapshot" | "struct",
    typeKey: string,
    bytes: Uint8Array,
    options: CanonicalCodecOptions = {},
  ): string {
    return this.#native.decodeRecordJsonControlled(
      recordKind,
      typeKey,
      bytes,
      options.cancellation?.nativeHandle(),
      options.timeoutMilliseconds,
      options.maxInputBytes,
      options.maxOutputBytes,
      options.maxDepth,
      options.maxMembers,
    );
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

  /** @internal Materialize one successor query thing with its exact opaque proof. */
  materializeMatchThingProjected(
    thing: RuntimeProjectionMatchThing,
  ): NativeProjectedValueEnvelope {
    return this.#native.materializeMatchThingProjected(thing);
  }

  /**
   * @internal Bind one registered `RustDatabase` (owned) or transaction
   * (borrowed) as a terminal executor. Terminals run on the provider runtime
   * and resolve without blocking the event loop.
   */
  direct(connection: RuntimeProjectionConnection): RuntimeProjectionRemote {
    if (!isRegisteredRustConnection(connection)) {
      throw new TypeError("generated query requires a registered Rust connection");
    }
    const database = rustDatabaseHandle(connection);
    if (database !== undefined) {
      return new InstalledRuntimeProjectionDirect(this.#native, { kind: "owned", database });
    }
    const transaction = rustTransactionHandle(connection);
    if (transaction === undefined) {
      throw new TypeError("generated query has no registered native execution handle");
    }
    return new InstalledRuntimeProjectionDirect(this.#native, { kind: "borrowed", transaction });
  }

  /**
   * @internal Bind remote authority, executor epoch, the session's resources
   * and cancellation, and one host exchange. `answerLimits` is the
   * connection's ceiling; left out, the session's resources are the ceiling.
   */
  remote(
    authority: QueryV2Authority,
    advertisement: Uint8Array,
    exchange: RuntimeProjectionRemoteExchange,
    resources: QueryExecutionResourceLimits,
    cancellation: QueryCancellation,
    answerLimits?: QueryExecutionResourceLimits,
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
    if (!(resources instanceof QueryExecutionResourceLimits)) {
      throw new TypeError("generated remote query resources must be QueryExecutionResourceLimits");
    }
    if (answerLimits !== undefined && !(answerLimits instanceof QueryExecutionResourceLimits)) {
      throw new TypeError("generated remote query answerLimits must be QueryExecutionResourceLimits");
    }
    const native = loadNative();
    const context = queryV2NativeCall(() => native.queryV2RemoteModelContextWithResources(
      nativeAuthority,
      advertisement,
      resources.nativeHandle(),
      cancellation.nativeHandle(),
      answerLimits?.nativeHandle(),
    ));
    return new InstalledRuntimeProjectionRemote(native, context, exchange, cancellation);
  }
}

type DirectTarget =
  | { readonly kind: "owned"; readonly database: NativeRustDatabase }
  | { readonly kind: "borrowed"; readonly transaction: NativeRustTransactionContext };

/** Verified direct execution bound to one registered Rust database or transaction. */
class InstalledRuntimeProjectionDirect implements RuntimeProjectionRemote {
  readonly #native: NativeRuntimeProjection;
  readonly #target: DirectTarget;

  constructor(native: NativeRuntimeProjection, target: DirectTarget) {
    this.#native = native;
    this.#target = target;
    Object.freeze(this);
  }

  rows(
    query: RuntimeProjectionMatchQuery,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    cardinality: "exactly_one" | "bounded_many",
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(
      () => query.fetchRowsDiagnostic(orders, offset, limit, cardinality),
      (database) => query.executeFetchRowsOwned(database, orders, offset, limit, cardinality),
      (transaction) =>
        query.executeFetchRowsBorrowed(transaction, orders, offset, limit, cardinality),
    );
  }

  page(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(
      () => query.pageByDiagnostic(root, orders, offset, limit, includeTotal),
      (database) =>
        query.executePageByOwned(database, root, orders, offset, limit, includeTotal),
      (transaction) =>
        query.executePageByBorrowed(transaction, root, orders, offset, limit, includeTotal),
    );
  }

  count(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(
      () => query.countByDiagnostic(root),
      (database) => query.executeCountByOwned(database, root),
      (transaction) => query.executeCountByBorrowed(transaction, root),
    );
  }

  exists(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(
      () => query.existsByDiagnostic(root),
      (database) => query.executeExistsByOwned(database, root),
      (transaction) => query.executeExistsByBorrowed(transaction, root),
    );
  }

  reduce(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchBinding | null,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(
      () => query.reduceByDiagnostic(root, group, reducers, inputs),
      (database) => query.executeReduceByOwned(database, root, group, reducers, inputs),
      (transaction) =>
        query.executeReduceByBorrowed(transaction, root, group, reducers, inputs),
    );
  }

  reduceByField(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchField,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(
      () => query.reduceByFieldDiagnostic(root, group, reducers, inputs),
      (database) => query.executeReduceByFieldOwned(database, root, group, reducers, inputs),
      (transaction) =>
        query.executeReduceByFieldBorrowed(transaction, root, group, reducers, inputs),
    );
  }

  reduceByFields(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    groups: RuntimeProjectionMatchField[],
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(
      () => query.reduceByFieldsDiagnostic(root, groups, reducers, inputs),
      (database) => query.executeReduceByFieldsOwned(database, root, groups, reducers, inputs),
      (transaction) =>
        query.executeReduceByFieldsBorrowed(transaction, root, groups, reducers, inputs),
    );
  }

  /** Revalidate the request, then run it on the owned or borrowed entry point. */
  async #run(
    diagnostic: () => string,
    owned: (database: NativeRustDatabase) => Promise<RuntimeProjectionMatchResult>,
    borrowed: (
      transaction: NativeRustTransactionContext,
    ) => Promise<RuntimeProjectionMatchResult>,
  ): Promise<RuntimeProjectionMatchResult> {
    const target = this.#target;
    return queryV2NativePromise(queryV2NativeCall(() => {
      this.#native.revalidateMatchDiagnostic(diagnostic());
      return target.kind === "owned" ? owned(target.database) : borrowed(target.transaction);
    }));
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

/** TLS for a proxy endpoint; `custom_root` trusts the `tlsRootCa` bundle only. */
export type ProxyTlsMode = "disabled" | "native_roots" | "custom_root";

/** How a {@link ProxyConnection} reaches its server and the policy it holds terminals to. */
export interface ProxyConnectionOptions {
  /** Defaults to `custom_root` when `tlsRootCa` is given, else `disabled`. */
  readonly tls?: ProxyTlsMode;
  /** Path of the PEM bundle a `custom_root` connection trusts. */
  readonly tlsRootCa?: string;
  /** Headers sent with every request, such as `Authorization`. */
  readonly headers?: readonly (readonly [string, string])[];
  /** Exact advertisement bytes provisioned out of band; fetched when omitted. */
  readonly advertisement?: Uint8Array;
  /** Bounds connecting, which fetches the advertisement. */
  readonly connectionLimits?: QueryExecutionResourceLimits;
  /** The ceiling every terminal on this connection is constrained by. */
  readonly answerLimits?: QueryExecutionResourceLimits;
  /** Cancels connecting. */
  readonly cancellation?: QueryCancellation;
}

const MAX_PROXY_ADVERTISEMENT_BYTES = 32 * 1024 * 1024;

/**
 * A verified connection to a TypeBridge server over the built-in HTTP route.
 *
 * Each terminal is one exchange, run off the JavaScript thread under the
 * query session's policy constrained by this connection's `answerLimits`.
 */
export class ProxyConnection implements RuntimeProjectionRemote {
  readonly #native: NativeProxyConnection;

  private constructor(native: NativeProxyConnection) {
    this.#native = native;
    Object.freeze(this);
  }

  /** Open `endpoint` under `authority`, fetch or pin its advertisement, and verify it. */
  static async connect(
    authority: QueryV2Authority,
    endpoint: string,
    options: ProxyConnectionOptions = {},
  ): Promise<ProxyConnection> {
    const nativeAuthority = queryV2AuthorityHandle(authority);
    if (nativeAuthority === undefined) {
      throw new TypeError("proxy connection requires a type-bridge QueryV2Authority");
    }
    if (typeof endpoint !== "string") {
      throw new TypeError("proxy endpoint must be a string");
    }
    const headers = options.headers ?? [];
    if (
      !Array.isArray(headers)
      || !headers.every((header) =>
        Array.isArray(header)
        && header.length === 2
        && typeof header[0] === "string"
        && typeof header[1] === "string"
      )
    ) {
      throw new TypeError("proxy headers must be [name, value] string pairs");
    }
    const advertisement = options.advertisement === undefined
      ? undefined
      : ownedByteSnapshot(options.advertisement, MAX_PROXY_ADVERTISEMENT_BYTES);
    const native = await queryV2NativePromise(queryV2NativeCall(() =>
      loadNative().NodeProxyConnection.connect(
        nativeAuthority,
        endpoint,
        options.tls ?? (options.tlsRootCa === undefined ? "disabled" : "custom_root"),
        options.tlsRootCa,
        headers.map(([name]) => name),
        headers.map(([, value]) => value),
        advertisement,
        options.connectionLimits?.nativeHandle(),
        options.answerLimits?.nativeHandle(),
        options.cancellation?.nativeHandle(),
      )
    ));
    return new ProxyConnection(native);
  }

  rows(
    query: RuntimeProjectionMatchQuery,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    cardinality: "exactly_one" | "bounded_many",
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(() => this.#native.rows(query, orders, offset, limit, cardinality));
  }

  page(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    orders: RuntimeProjectionMatchOrder[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(() =>
      this.#native.page(query, root, orders, offset, limit, includeTotal)
    );
  }

  count(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(() => this.#native.count(query, root));
  }

  exists(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(() => this.#native.exists(query, root));
  }

  reduce(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchBinding | null,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(() => this.#native.reduce(query, root, group, reducers, inputs));
  }

  reduceByField(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    group: RuntimeProjectionMatchField,
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(() =>
      this.#native.reduceByField(query, root, group, reducers, inputs)
    );
  }

  reduceByFields(
    query: RuntimeProjectionMatchQuery,
    root: RuntimeProjectionMatchBinding,
    groups: RuntimeProjectionMatchField[],
    reducers: RuntimeProjectionReduction[],
    inputs: (RuntimeProjectionMatchField | null)[],
  ): Promise<RuntimeProjectionMatchResult> {
    return this.#run(() =>
      this.#native.reduceByFields(query, root, groups, reducers, inputs)
    );
  }

  async #run(
    terminal: () => Promise<RuntimeProjectionMatchResult>,
  ): Promise<RuntimeProjectionMatchResult> {
    return queryV2NativePromise(queryV2NativeCall(terminal));
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
    input.projectedBatchMaterializer,
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
