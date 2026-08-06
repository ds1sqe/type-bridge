import type { NativeRustDatabase, NativeRustTransactionContext, QueryV2Authority, RustDatabase, RustTransactionContext } from "./index.js";
import { loadNative } from "./native.js";
type NativeModule = ReturnType<typeof loadNative>;
type NativeRuntimeProjection = InstanceType<NativeModule["NodeRuntimeProjection"]>;
export type RuntimeProjectionMatchSession = ReturnType<NativeRuntimeProjection["matchSession"]>;
export type RuntimeProjectionMatchBinding = ReturnType<RuntimeProjectionMatchSession["exact"]>;
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
    readonly projectionJson: string;
    readonly semanticFingerprintJson: string;
    readonly projectionFingerprintJson: string;
    readonly bindings: readonly RuntimeProjectionBinding[];
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
/** One caller-owned request/response exchange. No retry is performed. */
export type RuntimeProjectionRemoteExchange = (request: Uint8Array) => Promise<Uint8Array>;
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
    matchModelType(typeKey: string): string;
    validateAttributeValueJson(typeKey: string, valueJson: string): void;
    validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
    revalidateMatchDiagnostic(diagnostic: string): string;
    materializeMatchThingJson(thing: RuntimeProjectionMatchThing): string;
}
/** A verified native projection scoped to one generated package instance. */
export declare class InstalledRuntimeProjection {
    #private;
    constructor(native: NativeProjectionHandle);
    /** @internal Bind one generated token without exposing its native handle. */
    manager(typeKey: string, connection: RuntimeProjectionConnection): NativeProjectedManager;
    /** @internal Create an opaque query session from verified projection evidence. */
    matchSession(): RuntimeProjectionMatchSession;
    /** @internal Resolve one exact generated model token to its provider label. */
    matchModelType(typeKey: string): string;
    /** @internal Validate one generated attribute scalar against projected constraints. */
    validateAttributeValueJson(typeKey: string, valueJson: string): void;
    /** @internal Validate one generated owned-field scalar against projected constraints. */
    validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
    /** @internal Reject structural or foreign connection lookalikes. */
    assertConnection(connection: RuntimeProjectionConnection): void;
    /** @internal Materialize one native-validated thing as projected private JSON. */
    materializeMatchThingJson(thing: RuntimeProjectionMatchThing): string;
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
    remote(authority: QueryV2Authority, advertisement: Uint8Array, exchange: RuntimeProjectionRemoteExchange, limits: RuntimeProjectionRemoteLimits): RuntimeProjectionRemote;
}
/** Verify and install one generated package's exact projection evidence. */
export declare function installRuntimeProjection(input: RuntimeProjectionInstall): InstalledRuntimeProjection;
export {};
