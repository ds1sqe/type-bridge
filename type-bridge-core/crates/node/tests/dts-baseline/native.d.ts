import type { NativeModule, NativeRustDatabase, NativeRustTransactionContext } from "./index.js";
import type { NativeProjectedManager, NativeProjectedValueEnvelope, RuntimeProjectionInstall } from "./runtime-projection.js";
type NativeProjectedBatchAuthority = Parameters<NonNullable<RuntimeProjectionInstall["projectedBatchMaterializer"]>>[3];
type NativeMatchComparison = "equal" | "not_equal" | "less_than" | "less_than_or_equal" | "greater_than" | "greater_than_or_equal" | "contains" | "starts_with" | "ends_with" | "regex";
type NativeMatchDirection = "ascending" | "descending";
type NativeMatchMissingOrder = "reject" | "first" | "last";
type NativeMatchRowCardinality = "exactly_one" | "bounded_many";
type NativeMatchReduction = "count" | "sum" | "min" | "max" | "mean" | "median" | "std";
declare const nativeMatchHandleKind: unique symbol;
interface NativeMatchSessionHandle {
    readonly [nativeMatchHandleKind]: "session";
    readonly isClosed: boolean;
    close(): void;
    exact(typeName: string): NativeMatchBindingHandle;
    subtypes(typeName: string): NativeMatchBindingHandle;
    functionById(functionId: string): NativeMatchFunctionHandle;
    functionValueJson(attributeTypeKey: string, valueJson: string): NativeMatchFunctionValueHandle;
    reachable(relationType: string, roleFrom: string, roleTo: string, source: NativeMatchBindingHandle, target: NativeMatchBindingHandle, minDepth: number, maxDepth: number): NativeMatchPredicateHandle;
    positional(selections: NativeMatchSelectionHandle[]): NativeMatchShapeHandle;
    named(names: string[], selections: NativeMatchSelectionHandle[]): NativeMatchShapeHandle;
    query(shape: NativeMatchShapeHandle): NativeMatchQueryHandle;
}
interface NativeMatchBindingHandle {
    readonly [nativeMatchHandleKind]: "binding";
    iid(iid: string): NativeMatchPredicateHandle;
    iidIn(iids: string[]): NativeMatchPredicateHandle;
    field(fieldName: string): NativeMatchFieldHandle;
    fieldOwnedBy(ownerType: string, fieldName: string): NativeMatchFieldHandle;
    role(roleName: string): NativeMatchRoleHandle;
    roleOwnedBy(ownerType: string, roleName: string): NativeMatchRoleHandle;
    one(): NativeMatchSelectionHandle;
    collect(): NativeMatchSelectionHandle;
    functionArgument(): NativeMatchFunctionArgumentHandle;
}
interface NativeMatchFunctionHandle {
    readonly [nativeMatchHandleKind]: "function";
    call(arguments_: NativeMatchFunctionArgumentHandle[]): NativeMatchFunctionCallHandle;
}
interface NativeMatchFunctionValueHandle {
    readonly [nativeMatchHandleKind]: "function-value";
    functionArgument(): NativeMatchFunctionArgumentHandle;
}
interface NativeMatchFunctionArgumentHandle {
    readonly [nativeMatchHandleKind]: "function-argument";
}
interface NativeMatchFunctionCallHandle {
    readonly [nativeMatchHandleKind]: "function-call";
    functionArgument(): NativeMatchFunctionArgumentHandle;
    compareField(comparison: NativeMatchComparison, field: NativeMatchFieldHandle): NativeMatchPredicateHandle;
    compareValue(comparison: NativeMatchComparison, value: NativeMatchFunctionValueHandle): NativeMatchPredicateHandle;
    compareCall(comparison: NativeMatchComparison, other: NativeMatchFunctionCallHandle): NativeMatchPredicateHandle;
}
interface NativeMatchFieldHandle {
    readonly [nativeMatchHandleKind]: "field";
    presence(present: boolean): NativeMatchPredicateHandle;
    compareValueJson(comparison: NativeMatchComparison, valueJson: string): NativeMatchPredicateHandle;
    compareField(comparison: NativeMatchComparison, other: NativeMatchFieldHandle): NativeMatchPredicateHandle;
    order(direction: NativeMatchDirection, missing: NativeMatchMissingOrder): NativeMatchOrderHandle;
}
interface NativeMatchRoleHandle {
    readonly [nativeMatchHandleKind]: "role";
    connects(player: NativeMatchBindingHandle): NativeMatchPredicateHandle;
}
interface NativeMatchPredicateHandle {
    readonly [nativeMatchHandleKind]: "predicate";
    and(other: NativeMatchPredicateHandle): NativeMatchPredicateHandle;
    or(other: NativeMatchPredicateHandle): NativeMatchPredicateHandle;
    not(): NativeMatchPredicateHandle;
}
interface NativeMatchOrderHandle {
    readonly [nativeMatchHandleKind]: "order";
}
interface NativeMatchSelectionHandle {
    readonly [nativeMatchHandleKind]: "selection";
    distinct(distinct: boolean): NativeMatchSelectionHandle;
    orderBy(order: NativeMatchOrderHandle): NativeMatchSelectionHandle;
}
interface NativeMatchShapeHandle {
    readonly [nativeMatchHandleKind]: "shape";
}
interface NativeMatchQueryHandle {
    readonly [nativeMatchHandleKind]: "query";
    readonly isClosed: boolean;
    close(): void;
    fork(): NativeMatchQueryHandle;
    addHidden(binding: NativeMatchBindingHandle): NativeMatchQueryHandle;
    wherePredicate(predicate: NativeMatchPredicateHandle): NativeMatchQueryHandle;
    allowCrossJoin(left: NativeMatchBindingHandle, right: NativeMatchBindingHandle): NativeMatchQueryHandle;
    fetchRowsDiagnostic(orders: NativeMatchOrderHandle[], offset: bigint, limit: bigint, cardinality: NativeMatchRowCardinality): string;
    executeFetchRowsOwned(database: NativeRustDatabase, orders: NativeMatchOrderHandle[], offset: bigint, limit: bigint, cardinality: NativeMatchRowCardinality): NativeValidatedMatchResultHandle;
    executeFetchRowsBorrowed(transaction: NativeRustTransactionContext, orders: NativeMatchOrderHandle[], offset: bigint, limit: bigint, cardinality: NativeMatchRowCardinality): NativeValidatedMatchResultHandle;
    executePageByOwned(database: NativeRustDatabase, root: NativeMatchBindingHandle, orders: NativeMatchOrderHandle[], offset: bigint, limit: bigint, includeTotal: boolean): NativeValidatedMatchResultHandle;
    executePageByBorrowed(transaction: NativeRustTransactionContext, root: NativeMatchBindingHandle, orders: NativeMatchOrderHandle[], offset: bigint, limit: bigint, includeTotal: boolean): NativeValidatedMatchResultHandle;
    executeCountByOwned(database: NativeRustDatabase, root: NativeMatchBindingHandle): NativeValidatedMatchResultHandle;
    executeCountByBorrowed(transaction: NativeRustTransactionContext, root: NativeMatchBindingHandle): NativeValidatedMatchResultHandle;
    executeExistsByOwned(database: NativeRustDatabase, root: NativeMatchBindingHandle): NativeValidatedMatchResultHandle;
    executeExistsByBorrowed(transaction: NativeRustTransactionContext, root: NativeMatchBindingHandle): NativeValidatedMatchResultHandle;
    pageByDiagnostic(root: NativeMatchBindingHandle, orders: NativeMatchOrderHandle[], offset: bigint, limit: bigint, includeTotal: boolean): string;
    countByDiagnostic(root: NativeMatchBindingHandle): string;
    existsByDiagnostic(root: NativeMatchBindingHandle): string;
    reduceByDiagnostic(root: NativeMatchBindingHandle, group: NativeMatchBindingHandle | null, reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): string;
    executeReduceByOwned(database: NativeRustDatabase, root: NativeMatchBindingHandle, group: NativeMatchBindingHandle | null, reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativeValidatedMatchResultHandle;
    executeReduceByBorrowed(transaction: NativeRustTransactionContext, root: NativeMatchBindingHandle, group: NativeMatchBindingHandle | null, reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativeValidatedMatchResultHandle;
    reduceByFieldDiagnostic(root: NativeMatchBindingHandle, group: NativeMatchFieldHandle, reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): string;
    executeReduceByFieldOwned(database: NativeRustDatabase, root: NativeMatchBindingHandle, group: NativeMatchFieldHandle, reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativeValidatedMatchResultHandle;
    executeReduceByFieldBorrowed(transaction: NativeRustTransactionContext, root: NativeMatchBindingHandle, group: NativeMatchFieldHandle, reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativeValidatedMatchResultHandle;
    reduceByFieldsDiagnostic(root: NativeMatchBindingHandle, groups: NativeMatchFieldHandle[], reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): string;
    executeReduceByFieldsOwned(database: NativeRustDatabase, root: NativeMatchBindingHandle, groups: NativeMatchFieldHandle[], reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativeValidatedMatchResultHandle;
    executeReduceByFieldsBorrowed(transaction: NativeRustTransactionContext, root: NativeMatchBindingHandle, groups: NativeMatchFieldHandle[], reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativeValidatedMatchResultHandle;
}
interface NativeValidatedMatchResultHandle {
    readonly [nativeMatchHandleKind]: "validated-result";
    outputSlotCount(query: NativeMatchQueryHandle): number;
    outputSlotIsCollection(query: NativeMatchQueryHandle, slotIndex: number): boolean;
    rowCount(query: NativeMatchQueryHandle): number;
    slotCount(query: NativeMatchQueryHandle, rowIndex: number): number;
    outputNames(query: NativeMatchQueryHandle): string[] | null;
    slotThing(query: NativeMatchQueryHandle, rowIndex: number, slotIndex: number): NativeValidatedThingHandle;
    pageEntryCount(query: NativeMatchQueryHandle): number;
    pageSlotCount(query: NativeMatchQueryHandle, entryIndex: number): number;
    pageSlotValueCount(query: NativeMatchQueryHandle, entryIndex: number, slotIndex: number): number;
    pageSlotThing(query: NativeMatchQueryHandle, entryIndex: number, slotIndex: number, valueIndex: number): NativeValidatedThingHandle;
    pageOffset(query: NativeMatchQueryHandle): bigint;
    pageLimit(query: NativeMatchQueryHandle): bigint;
    pageTotal(query: NativeMatchQueryHandle): bigint | null;
    countValue(query: NativeMatchQueryHandle): bigint;
    existsValue(query: NativeMatchQueryHandle): boolean;
    reductionRowCount(query: NativeMatchQueryHandle): number;
    reductionValueCount(query: NativeMatchQueryHandle, rowIndex: number): number;
    reductionValueKind(query: NativeMatchQueryHandle, rowIndex: number, valueIndex: number): "count" | "long" | "double";
    reductionCountValue(query: NativeMatchQueryHandle, rowIndex: number, valueIndex: number): bigint;
    reductionLongValue(query: NativeMatchQueryHandle, rowIndex: number, valueIndex: number): bigint | null;
    reductionDoubleValue(query: NativeMatchQueryHandle, rowIndex: number, valueIndex: number): number | null;
    reductionGroup(query: NativeMatchQueryHandle, rowIndex: number): NativeValidatedThingHandle;
    reductionGroupValueJson(query: NativeMatchQueryHandle, rowIndex: number): string;
    reductionGroupValuesJson(query: NativeMatchQueryHandle, rowIndex: number): string;
}
interface NativeValidatedThingHandle {
    readonly [nativeMatchHandleKind]: "validated-thing";
    iid(): string;
    concreteDescriptor(): string;
    thingKind(): "entity" | "relation";
    fieldNames(): string[];
    fieldValuesJson(fieldName: string): string | null;
    roleDataComplete(): boolean;
    roleNames(): string[];
    rolePlayerCount(roleName: string): number;
    rolePlayer(roleName: string, playerIndex: number): NativeValidatedThingHandle;
}
interface NativeRemoteModelQueryContext {
}
export interface NativeQueryExecutionResources {
    readonly timeoutMilliseconds: bigint;
    readonly items: bigint;
    readonly bytes: bigint;
    readonly graphNodes: bigint;
    readonly attributeValues: bigint;
    readonly collectionMembers: bigint;
    readonly rolePlayers: bigint;
    readonly statements: bigint;
}
export interface NativeQueryCancellation {
    readonly isCancelled: boolean;
    cancel(): void;
}
interface NativePendingRemoteModelQuery {
    readonly isClosed: boolean;
    close(): void;
    requestBytes(): Uint8Array;
    decodeReply(response: Uint8Array): Promise<NativeValidatedMatchResultHandle>;
}
interface NativeRemoteModelQueryModule {
    NodeQueryExecutionResources: new (timeoutMilliseconds: bigint, items: bigint, bytes: bigint, graphNodes: bigint, attributeValues: bigint, collectionMembers: bigint, rolePlayers: bigint, statements: bigint) => NativeQueryExecutionResources;
    NodeQueryCancellation: new () => NativeQueryCancellation;
    queryV2RemoteModelContext(authority: ReturnType<NativeModule["queryV2Authority"]>, advertisement: Uint8Array, maxItems: bigint, maxBytes: bigint, maxCollectionMembers: bigint, maxGraphNodes: bigint, maxAttributeValues: bigint, maxRolePlayers: bigint, deadlineMs?: bigint | null): NativeRemoteModelQueryContext;
    queryV2RemoteModelContextWithResources(authority: ReturnType<NativeModule["queryV2Authority"]>, advertisement: Uint8Array, resources: NativeQueryExecutionResources, cancellation: NativeQueryCancellation): NativeRemoteModelQueryContext;
    queryV2PrepareRemoteModelRows(query: NativeMatchQueryHandle, context: NativeRemoteModelQueryContext, orders: NativeMatchOrderHandle[], offset: bigint, limit: bigint, cardinality: NativeMatchRowCardinality): NativePendingRemoteModelQuery;
    queryV2PrepareRemoteModelPage(query: NativeMatchQueryHandle, context: NativeRemoteModelQueryContext, root: NativeMatchBindingHandle, orders: NativeMatchOrderHandle[], offset: bigint, limit: bigint, includeTotal: boolean): NativePendingRemoteModelQuery;
    queryV2PrepareRemoteModelCount(query: NativeMatchQueryHandle, context: NativeRemoteModelQueryContext, root: NativeMatchBindingHandle): NativePendingRemoteModelQuery;
    queryV2PrepareRemoteModelExists(query: NativeMatchQueryHandle, context: NativeRemoteModelQueryContext, root: NativeMatchBindingHandle): NativePendingRemoteModelQuery;
    queryV2PrepareRemoteModelReduce(query: NativeMatchQueryHandle, context: NativeRemoteModelQueryContext, root: NativeMatchBindingHandle, group: NativeMatchBindingHandle | null, reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativePendingRemoteModelQuery;
    queryV2PrepareRemoteModelReduceByField(query: NativeMatchQueryHandle, context: NativeRemoteModelQueryContext, root: NativeMatchBindingHandle, group: NativeMatchFieldHandle, reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativePendingRemoteModelQuery;
    queryV2PrepareRemoteModelReduceByFields(query: NativeMatchQueryHandle, context: NativeRemoteModelQueryContext, root: NativeMatchBindingHandle, groups: NativeMatchFieldHandle[], reducers: NativeMatchReduction[], inputs: (NativeMatchFieldHandle | null)[]): NativePendingRemoteModelQuery;
}
interface NativeRuntimeProjectionHandle {
    connectDirect(endpoint: string, database: string, username: string, password: string, httpPort: number, tlsMode: string, tlsRootCa?: string, connectionLimits?: NativeQueryExecutionResources, answerLimits?: NativeQueryExecutionResources, cancellation?: NativeQueryCancellation): NativeRustDatabase;
    managerForDatabase(typeKey: string, database: NativeRustDatabase): NativeProjectedManager;
    managerForTransaction(typeKey: string, transaction: NativeRustTransactionContext): NativeProjectedManager;
    matchSession(): NativeMatchSessionHandle;
    matchSessionWithResources(resources: NativeQueryExecutionResources, cancellation: NativeQueryCancellation): NativeMatchSessionHandle;
    matchModelType(typeKey: string): string;
    validateAttributeValueJson(typeKey: string, valueJson: string): void;
    validateHydratedAttributeValueJson(typeKey: string, valueJson: string): void;
    validateFieldValueJson(typeKey: string, fieldName: string, valueJson: string): void;
    validateCreateJson(typeKey: string, valueJson: string): void;
    encodeCreateJson(typeKey: string, valueJson: string): Uint8Array;
    decodeCreateJson(typeKey: string, bytes: Uint8Array): string;
    encodeReferenceJson(typeKey: string, valueJson: string): Uint8Array;
    decodeReferenceJson(typeKey: string, bytes: Uint8Array): string;
    encodeSnapshotJson(typeKey: string, valueJson: string): Uint8Array;
    decodeSnapshotJson(typeKey: string, bytes: Uint8Array): string;
    encodeStructJson(typeKey: string, valueJson: string): Uint8Array;
    decodeStructJson(typeKey: string, bytes: Uint8Array): string;
    encodeArchive(records: readonly Uint8Array[]): Uint8Array;
    decodeArchive(bytes: Uint8Array): Uint8Array[];
    validateThingJson(typeKey: string, valueJson: string): void;
    rejectGeneratedTokenPackageMismatch(pathJson: string): void;
    revalidateMatchDiagnostic(diagnostic: string): string;
    materializeMatchThingJson(thing: NativeValidatedThingHandle): string;
    materializeMatchThingProjected(thing: NativeValidatedThingHandle): NativeProjectedValueEnvelope;
}
interface NativeRuntimeProjectionModule {
    NodeRuntimeProjection: new (projectionJson: string, semanticFingerprintJson: string, projectionFingerprintJson: string, registrationsJson: string, schemaAuthorityJson?: string, projectedBatchMaterializer?: (typeKey: string, ordinal: number, json: string, authority: NativeProjectedBatchAuthority) => object) => NativeRuntimeProjectionHandle;
}
type LoadedNativeModule = NativeModule & NativeRemoteModelQueryModule & NativeRuntimeProjectionModule;
/**
 * Loads and returns the native .node module. The result is cached after the
 * first successful load; subsequent calls return the same object.
 *
 * Resolution order:
 *   1. TYPE_BRIDGE_NODE_NATIVE_PATH env var (explicit override).
 *   2. Platform-triple candidates at the package root (dist/..).
 *   3. Generic-name candidates at the package root.
 *   4. Same set probed inside dist/ as a robustness fallback.
 *
 * Throws an actionable error listing all tried paths when no candidate exists.
 */
export declare function loadNative(): LoadedNativeModule;
export {};
