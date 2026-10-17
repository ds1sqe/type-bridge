import { QueryV2Authority, type NativeRustDatabase, type NativeRustTransactionContext, type RustDatabase, type RustTransactionContext } from "./index.js";
import { loadNative } from "./native.js";
import type { NativeQueryCancellation, NativeQueryExecutionResources, NativeQueryLimits } from "./native.js";
import { StagedTransaction, type StagedTransactionKind } from "./staged-transaction.js";
type NativeModule = ReturnType<typeof loadNative>;
type NativeRuntimeProjection = InstanceType<NativeModule["NodeRuntimeProjection"]>;
export type RuntimeProjectionMatchSession = ReturnType<NativeRuntimeProjection["matchSessionWithResources"]>;
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
export type RuntimeProjectionMatchResult = Awaited<ReturnType<RuntimeProjectionMatchQuery["executeFetchRowsOwned"]>>;
export type RuntimeProjectionMatchThing = ReturnType<RuntimeProjectionMatchResult["slotThing"]>;
export type RuntimeProjectionReduction = "count" | "sum" | "min" | "max" | "mean" | "median" | "std";
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
/**
 * The answer policy shared by direct and remote queries. An omitted
 * dimension takes the core default; a given one may tighten or raise it.
 */
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
export declare class QueryExecutionResourceLimits {
    #private;
    constructor(options?: QueryExecutionResourceLimitOptions);
    get timeoutMilliseconds(): bigint;
    get items(): bigint;
    get bytes(): bigint;
    get graphNodes(): bigint;
    get attributeValues(): bigint;
    get collectionMembers(): bigint;
    get rolePlayers(): bigint;
    get statements(): bigint;
    /** @internal Exact native policy owner. */
    nativeHandle(): NativeQueryExecutionResources;
}
/** Stable names of the query limits a session can override. */
export type QueryLimitName = "canonical_bytes" | "canonical_depth" | "canonical_collection_len" | "canonical_string_bytes" | "selected_slots" | "bindings" | "predicate_nodes" | "predicate_depth" | "boolean_terms" | "allowed_cross_joins" | "order_terms" | "collection_order_terms" | "input_rows" | "input_bytes" | "output_name_bytes" | "semantic_id_bytes" | "diagnostic_bytes" | "remote_request_bytes";
/** Overrides of the canonical query limits, keyed by stable limit name. */
export type QueryLimitOverrides = Readonly<Partial<Record<QueryLimitName, bigint>>>;
/**
 * Query limits one generated session validates, encodes, and executes under.
 *
 * Rust admits the overrides: an unknown name or an unsupported value throws
 * here, before any session exists. A limit left out keeps its default.
 */
export declare class QueryLimits {
    #private;
    constructor(overrides?: QueryLimitOverrides);
    /** Stable names of every query limit. */
    names(): readonly string[];
    /** Effective value of one named limit. */
    get(name: QueryLimitName): bigint;
    /** @internal Exact native limits owner. */
    nativeHandle(): NativeQueryLimits;
}
/** Caller-owned cooperative cancellation for one or more query sessions. */
export declare class QueryCancellation {
    #private;
    constructor();
    cancel(): void;
    get isCancelled(): boolean;
    /** @internal Exact native cancellation owner. */
    nativeHandle(): NativeQueryCancellation;
    /** @internal Resolve when caller-owned cancellation is first requested. */
    cancelled(): Promise<void>;
    /** @internal Attach an abort side effect without exposing mutable native state. */
    onCancelled(listener: () => void): () => void;
}
/** One caller-owned request/response exchange. No retry is performed. */
export type RuntimeProjectionRemoteExchange = (request: Uint8Array, signal?: AbortSignal) => Promise<Uint8Array>;
/** Opaque verified remote terminal executor for one generated package. */
export interface RuntimeProjectionRemote {
    rows(query: RuntimeProjectionMatchQuery, orders: RuntimeProjectionMatchOrder[], offset: bigint, limit: bigint, cardinality: "exactly_one" | "bounded_many"): Promise<RuntimeProjectionMatchResult>;
    page(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding, orders: RuntimeProjectionMatchOrder[], offset: bigint, limit: bigint, includeTotal: boolean): Promise<RuntimeProjectionMatchResult>;
    count(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding): Promise<RuntimeProjectionMatchResult>;
    exists(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding): Promise<RuntimeProjectionMatchResult>;
    reduce(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding, group: RuntimeProjectionMatchBinding | null, reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): Promise<RuntimeProjectionMatchResult>;
    reduceByField(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding, group: RuntimeProjectionMatchField, reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): Promise<RuntimeProjectionMatchResult>;
    reduceByFields(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding, groups: RuntimeProjectionMatchField[], reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): Promise<RuntimeProjectionMatchResult>;
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
type NativeProjectedBatchMaterializer = (typeKey: string, ordinal: number, json: string, authority: NativeProjectedBatchAuthority) => object;
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
type NativeProjectedInputProof = NativeProjectedFacadeProof | NativeProjectedBatchRootProof | NativeProjectedBatchRoleProof;
interface NativeProjectedCreateBatchRow {
    readonly instanceJson: string;
    readonly proofs: readonly (NativeProjectedInputProof | null)[];
}
interface NativeProjectedUpdateBatchRow extends NativeProjectedCreateBatchRow {
    readonly iid: string;
}
export interface NativeProjectedManager {
    insertProjected(instanceJson: string, proofs: (NativeProjectedInputProof | null)[]): NativeProjectedValueEnvelope;
    insertJson(instanceJson: string): string;
    insertManyJson(batchJson: string): string;
    insertManyProjected<Complete>(rowCount: number, rowAt: (ordinal: number) => NativeProjectedCreateBatchRow): readonly Complete[];
    putProjected(instanceJson: string, proofs: (NativeProjectedInputProof | null)[]): NativeProjectedValueEnvelope;
    putJson(instanceJson: string): string;
    putManyJson(batchJson: string): string;
    putManyProjected<Complete>(rowCount: number, rowAt: (ordinal: number) => NativeProjectedCreateBatchRow): readonly Complete[];
    updateProjected(iid: string, instanceJson: string, proofs: (NativeProjectedInputProof | null)[]): NativeProjectedValueEnvelope;
    updateJson(iid: string, instanceJson: string): string;
    updateManyProjected<Complete>(rowCount: number, rowAt: (ordinal: number) => NativeProjectedUpdateBatchRow): readonly Complete[];
    deleteByIid(iid: string): void;
    deleteManyProjected(rowCount: number, rowAt: (ordinal: number) => string): void;
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
    andProjected(fieldOwnerTypeKey: string, fieldAttributeKey: string, comparison: "eq" | "ne" | "lt" | "lte" | "gt" | "gte", valueJson: string): NativeProjectedManagerFilter;
    rejectForeignFieldToken(): never;
    allProjected(): readonly NativeProjectedValueEnvelope[];
    firstProjected(): NativeProjectedValueEnvelope | null;
    count(): bigint;
    exists(): boolean;
}
/** @internal Preserve structured SDK diagnostics from generated-manager N-API calls. */
export declare function projectedManagerNativeCall<Result>(operation: () => Result): Result;
interface NativeProjectionHandle {
    connectDirect(endpoint: string, database: string, username: string, password: string, httpPort: number, tlsMode: string, tlsRootCa?: string, connectionLimits?: NativeQueryExecutionResources, answerLimits?: NativeQueryExecutionResources, cancellation?: NativeQueryCancellation): NativeRustDatabase;
    managerForDatabase(typeKey: string, database: NativeRustDatabase): NativeProjectedManager;
    managerForTransaction(typeKey: string, transaction: NativeRustTransactionContext): NativeProjectedManager;
    matchSessionWithResources(resources: NativeQueryExecutionResources, cancellation: NativeQueryCancellation, queryLimits?: NativeQueryLimits | null): RuntimeProjectionMatchSession;
    matchModelType(typeKey: string): string;
    validateAttributeValueJson(typeKey: string, valueJson: string): void;
    validateHydratedAttributeValueJson(typeKey: string, valueJson: string): void;
    validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
    validateMemberCardinality(typeKey: string, memberName: string, count: number): void;
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
    encodeArchiveControlled(records: readonly Uint8Array[], cancellation?: NativeQueryCancellation, timeoutMilliseconds?: number, maxInputBytes?: number, maxOutputBytes?: number, maxDepth?: number, maxRecords?: number, maxMembers?: number): Uint8Array;
    decodeArchiveControlled(bytes: Uint8Array, cancellation?: NativeQueryCancellation, timeoutMilliseconds?: number, maxInputBytes?: number, maxOutputBytes?: number, maxDepth?: number, maxRecords?: number, maxMembers?: number): Uint8Array[];
    encodeRecordJsonControlled(recordKind: string, typeKey: string, valueJson: string, cancellation?: NativeQueryCancellation, timeoutMilliseconds?: number, maxInputBytes?: number, maxOutputBytes?: number, maxDepth?: number, maxMembers?: number): Uint8Array;
    decodeRecordJsonControlled(recordKind: string, typeKey: string, bytes: Uint8Array, cancellation?: NativeQueryCancellation, timeoutMilliseconds?: number, maxInputBytes?: number, maxOutputBytes?: number, maxDepth?: number, maxMembers?: number): string;
    validateThingJson(typeKey: string, valueJson: string): void;
    rejectGeneratedTokenPackageMismatch(pathJson: string): void;
    revalidateMatchDiagnostic(diagnostic: string): string;
    materializeMatchThingJson(thing: RuntimeProjectionMatchThing): string;
    materializeMatchThingProjected(thing: RuntimeProjectionMatchThing): NativeProjectedValueEnvelope;
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
export declare class InstalledRuntimeProjection {
    #private;
    constructor(native: NativeProjectionHandle);
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
    }): RustDatabase;
    /** @internal Bind one generated token without exposing its native handle. */
    manager(typeKey: string, connection: RuntimeProjectionConnection): NativeProjectedManager;
    /** @internal Create an opaque query session from verified projection evidence. */
    matchSession(resources?: QueryExecutionResourceLimits, cancellation?: QueryCancellation, queryLimits?: QueryLimits): RuntimeProjectionMatchSession;
    /** @internal Resolve one exact generated model token to its provider label. */
    matchModelType(typeKey: string): string;
    /** @internal Validate one generated attribute scalar against projected constraints. */
    validateAttributeValueJson(typeKey: string, valueJson: string): void;
    /** @internal Validate one provider-hydrated attribute scalar. */
    validateHydratedAttributeValueJson(typeKey: string, valueJson: string): void;
    /** @internal Validate one generated owned-field scalar against projected constraints. */
    validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
    /** @internal Check one generated create member's value count against its projected cardinality. */
    validateMemberCardinality(typeKey: string, memberName: string, count: number): void;
    /** @internal Validate one complete generated create payload. */
    validateCreateJson(typeKey: string, valueJson: string): void;
    /** @internal Encode one exact generated attribute value to canonical bytes. */
    encodeAttributeJson(typeKey: string, valueJson: string): Uint8Array;
    /** @internal Decode canonical attribute bytes through exact package authority. */
    decodeAttributeJson(typeKey: string, bytes: Uint8Array): string;
    /** @internal Encode one exact generated create value to canonical bytes. */
    encodeCreateJson(typeKey: string, valueJson: string): Uint8Array;
    /** @internal Decode canonical bytes through this package's exact authority. */
    decodeCreateJson(typeKey: string, bytes: Uint8Array): string;
    /** @internal Encode one exact generated detached reference. */
    encodeReferenceJson(typeKey: string, valueJson: string): Uint8Array;
    /** @internal Decode one exact generated detached reference. */
    decodeReferenceJson(typeKey: string, bytes: Uint8Array): string;
    /** @internal Encode one exact generated detached snapshot. */
    encodeSnapshotJson(typeKey: string, valueJson: string): Uint8Array;
    /** @internal Decode one exact generated detached snapshot. */
    decodeSnapshotJson(typeKey: string, bytes: Uint8Array): string;
    /** @internal Return an opaque mutation fence for one decoded snapshot facade. */
    detachedSnapshotProof(): NativeProjectedFacadeProof;
    /** @internal Encode one exact generated struct. */
    encodeStructJson(typeKey: string, valueJson: string): Uint8Array;
    /** @internal Decode one exact generated struct. */
    decodeStructJson(typeKey: string, bytes: Uint8Array): string;
    /** @internal Compose verified canonical records into one archive. */
    encodeArchive(records: readonly Uint8Array[]): Uint8Array;
    /** @internal Split and verify one canonical archive. */
    decodeArchive(bytes: Uint8Array): readonly Uint8Array[];
    /** @internal Compose records with cancellation, deadline, and resource limits. */
    encodeArchiveControlled(records: readonly Uint8Array[], options?: CanonicalCodecOptions): Uint8Array;
    /** @internal Split records with cancellation, deadline, and resource limits. */
    decodeArchiveControlled(bytes: Uint8Array, options?: CanonicalCodecOptions): readonly Uint8Array[];
    /** @internal Encode one exact nominal record under managed controls. */
    encodeRecordJsonControlled(recordKind: "attribute" | "create" | "reference" | "snapshot" | "struct", typeKey: string, valueJson: string, options?: CanonicalCodecOptions): Uint8Array;
    /** @internal Decode one exact nominal record under managed controls. */
    decodeRecordJsonControlled(recordKind: "attribute" | "create" | "reference" | "snapshot" | "struct", typeKey: string, bytes: Uint8Array, options?: CanonicalCodecOptions): string;
    /** @internal Validate one complete generated provider result. */
    validateThingJson(typeKey: string, valueJson: string): void;
    /** @internal Surface one exact foreign generated-member package boundary. */
    rejectGeneratedTokenPackageMismatch(pathJson: string): void;
    /** @internal Reject structural or foreign connection lookalikes. */
    assertConnection(connection: RuntimeProjectionConnection): void;
    /** @internal Materialize one native-validated thing as projected private JSON. */
    materializeMatchThingJson(thing: RuntimeProjectionMatchThing): string;
    /** @internal Materialize one successor query thing with its exact opaque proof. */
    materializeMatchThingProjected(thing: RuntimeProjectionMatchThing): NativeProjectedValueEnvelope;
    /**
     * @internal Bind one registered `RustDatabase` (owned) or transaction
     * (borrowed) as a terminal executor. Terminals run on the provider runtime
     * and resolve without blocking the event loop.
     */
    direct(connection: RuntimeProjectionConnection): RuntimeProjectionRemote;
    /**
     * @internal Bind remote authority, executor epoch, the session's resources
     * and cancellation, and one host exchange. `answerLimits` is the
     * connection's ceiling; left out, the session's resources are the ceiling.
     */
    remote(authority: QueryV2Authority, advertisement: Uint8Array, exchange: RuntimeProjectionRemoteExchange, resources: QueryExecutionResourceLimits, cancellation: QueryCancellation, answerLimits?: QueryExecutionResourceLimits): RuntimeProjectionRemote;
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
    /**
     * Exact staged (`/v3/capabilities`) advertisement bytes provisioned out of
     * band; fetched on the first staged transaction when omitted. Either way
     * it must name the same executor and reply key as `advertisement`.
     */
    readonly stagedAdvertisement?: Uint8Array;
    /** Bounds connecting, which fetches the advertisement. */
    readonly connectionLimits?: QueryExecutionResourceLimits;
    /** The ceiling every terminal on this connection is constrained by. */
    readonly answerLimits?: QueryExecutionResourceLimits;
    /** Cancels connecting. */
    readonly cancellation?: QueryCancellation;
}
/**
 * A verified connection to a TypeBridge server over the built-in HTTP route.
 *
 * Each terminal is one exchange, run off the JavaScript thread under the
 * query session's policy constrained by this connection's `answerLimits`.
 */
export declare class ProxyConnection implements RuntimeProjectionRemote {
    #private;
    private constructor();
    /** Open `endpoint` under `authority`, fetch or pin its advertisement, and verify it. */
    static connect(authority: QueryV2Authority, endpoint: string, options?: ProxyConnectionOptions): Promise<ProxyConnection>;
    /**
     * Open a staged transaction whose commit is one signed `query-remote/v3`
     * exchange to `POST /v3/staged`. The staged advertisement is fetched from
     * `/v3/capabilities` now; nothing else is sent until commit. An omitted
     * kind is inferred from the staged writes.
     */
    transaction(kind?: StagedTransactionKind, options?: {
        readonly cancellation?: QueryCancellation;
    }): Promise<StagedTransaction>;
    /** The ceiling every terminal on this connection is constrained by. */
    get answerLimits(): QueryExecutionResourceLimits;
    rows(query: RuntimeProjectionMatchQuery, orders: RuntimeProjectionMatchOrder[], offset: bigint, limit: bigint, cardinality: "exactly_one" | "bounded_many"): Promise<RuntimeProjectionMatchResult>;
    page(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding, orders: RuntimeProjectionMatchOrder[], offset: bigint, limit: bigint, includeTotal: boolean): Promise<RuntimeProjectionMatchResult>;
    count(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding): Promise<RuntimeProjectionMatchResult>;
    exists(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding): Promise<RuntimeProjectionMatchResult>;
    reduce(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding, group: RuntimeProjectionMatchBinding | null, reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): Promise<RuntimeProjectionMatchResult>;
    reduceByField(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding, group: RuntimeProjectionMatchField, reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): Promise<RuntimeProjectionMatchResult>;
    reduceByFields(query: RuntimeProjectionMatchQuery, root: RuntimeProjectionMatchBinding, groups: RuntimeProjectionMatchField[], reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): Promise<RuntimeProjectionMatchResult>;
}
/** Verify and install one generated package's exact projection evidence. */
export declare function installRuntimeProjection(input: RuntimeProjectionInstall): InstalledRuntimeProjection;
/** @internal Verify an embedded authority before a generated package installs. */
export declare function installGeneratedSchemaAuthority(input: GeneratedSchemaAuthorityInstall): QueryV2Authority;
export {};
