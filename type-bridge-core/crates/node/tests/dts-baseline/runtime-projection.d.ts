import { QueryV2Authority, type NativeRustDatabase, type NativeRustTransactionContext, type RustDatabase, type RustTransactionContext } from "./index.js";
import { loadNative } from "./native.js";
import type { NativeQueryCancellation, NativeQueryExecutionResources } from "./native.js";
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
    managerForDatabase(typeKey: string, database: NativeRustDatabase): NativeProjectedManager;
    managerForTransaction(typeKey: string, transaction: NativeRustTransactionContext): NativeProjectedManager;
    matchSession(): RuntimeProjectionMatchSession;
    matchSessionWithResources(resources: NativeQueryExecutionResources, cancellation: NativeQueryCancellation): RuntimeProjectionMatchSession;
    matchModelType(typeKey: string): string;
    validateAttributeValueJson(typeKey: string, valueJson: string): void;
    validateHydratedAttributeValueJson(typeKey: string, valueJson: string): void;
    validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
    validateCreateJson(typeKey: string, valueJson: string): void;
    validateThingJson(typeKey: string, valueJson: string): void;
    rejectGeneratedTokenPackageMismatch(pathJson: string): void;
    revalidateMatchDiagnostic(diagnostic: string): string;
    materializeMatchThingJson(thing: RuntimeProjectionMatchThing): string;
    materializeMatchThingProjected(thing: RuntimeProjectionMatchThing): NativeProjectedValueEnvelope;
}
/** A verified native projection scoped to one generated package instance. */
export declare class InstalledRuntimeProjection {
    #private;
    constructor(native: NativeProjectionHandle);
    /** @internal Bind one generated token without exposing its native handle. */
    manager(typeKey: string, connection: RuntimeProjectionConnection): NativeProjectedManager;
    /** @internal Create an opaque query session from verified projection evidence. */
    matchSession(resources?: QueryExecutionResourceLimits, cancellation?: QueryCancellation): RuntimeProjectionMatchSession;
    /** @internal Resolve one exact generated model token to its provider label. */
    matchModelType(typeKey: string): string;
    /** @internal Validate one generated attribute scalar against projected constraints. */
    validateAttributeValueJson(typeKey: string, valueJson: string): void;
    /** @internal Validate one provider-hydrated attribute scalar. */
    validateHydratedAttributeValueJson(typeKey: string, valueJson: string): void;
    /** @internal Validate one generated owned-field scalar against projected constraints. */
    validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
    /** @internal Validate one complete generated create payload. */
    validateCreateJson(typeKey: string, valueJson: string): void;
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
    /** @internal Execute one selected-row request through the verified projection. */
    executeRows(query: RuntimeProjectionMatchQuery, connection: RuntimeProjectionConnection, orders: RuntimeProjectionMatchOrder[], offset: bigint, limit: bigint, cardinality: "exactly_one" | "bounded_many"): RuntimeProjectionMatchResult;
    /** @internal Execute one distinct-root page through the verified projection. */
    executePage(query: RuntimeProjectionMatchQuery, connection: RuntimeProjectionConnection, root: RuntimeProjectionMatchBinding, orders: RuntimeProjectionMatchOrder[], offset: bigint, limit: bigint, includeTotal: boolean): RuntimeProjectionMatchResult;
    /** @internal Execute one distinct-root count through the verified projection. */
    executeCount(query: RuntimeProjectionMatchQuery, connection: RuntimeProjectionConnection, root: RuntimeProjectionMatchBinding): bigint;
    /** @internal Execute one distinct-root existence request. */
    executeExists(query: RuntimeProjectionMatchQuery, connection: RuntimeProjectionConnection, root: RuntimeProjectionMatchBinding): boolean;
    /** @internal Execute one typed ungrouped or grouped reduction. */
    executeReduce(query: RuntimeProjectionMatchQuery, connection: RuntimeProjectionConnection, root: RuntimeProjectionMatchBinding, group: RuntimeProjectionMatchBinding | null, reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): RuntimeProjectionMatchResult;
    /** @internal Execute one typed reduction grouped by an owned field value. */
    executeReduceByField(query: RuntimeProjectionMatchQuery, connection: RuntimeProjectionConnection, root: RuntimeProjectionMatchBinding, group: RuntimeProjectionMatchField, reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): RuntimeProjectionMatchResult;
    /** @internal Execute one typed reduction grouped by an owned-field tuple. */
    executeReduceByFields(query: RuntimeProjectionMatchQuery, connection: RuntimeProjectionConnection, root: RuntimeProjectionMatchBinding, groups: RuntimeProjectionMatchField[], reducers: RuntimeProjectionReduction[], inputs: (RuntimeProjectionMatchField | null)[]): RuntimeProjectionMatchResult;
    /** @internal Bind remote authority, executor epoch, budgets, and exchange once. */
    remote(authority: QueryV2Authority, advertisement: Uint8Array, exchange: RuntimeProjectionRemoteExchange, limits: RuntimeProjectionRemoteLimits | QueryExecutionResourceLimits, cancellation?: QueryCancellation): RuntimeProjectionRemote;
}
/** Verify and install one generated package's exact projection evidence. */
export declare function installRuntimeProjection(input: RuntimeProjectionInstall): InstalledRuntimeProjection;
/** @internal Verify an embedded authority before a generated package installs. */
export declare function installGeneratedSchemaAuthority(input: GeneratedSchemaAuthorityInstall): QueryV2Authority;
export {};
