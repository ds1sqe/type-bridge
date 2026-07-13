import type { NativeModule, NativeRustDatabase, NativeRustTransactionContext } from "./index.js";
type NativeRegistryHandle = InstanceType<NativeModule["NodeDescriptorRegistry"]>;
type NativeMatchComparison = "equal" | "not_equal" | "less_than" | "less_than_or_equal" | "greater_than" | "greater_than_or_equal" | "contains" | "starts_with" | "ends_with" | "regex";
type NativeMatchDirection = "ascending" | "descending";
type NativeMatchMissingOrder = "reject" | "first" | "last";
type NativeMatchRowCardinality = "exactly_one" | "bounded_many";
declare const nativeMatchHandleKind: unique symbol;
interface NativeMatchSessionHandle {
    readonly [nativeMatchHandleKind]: "session";
    exact(typeName: string): NativeMatchBindingHandle;
    subtypes(typeName: string): NativeMatchBindingHandle;
    positional(selections: NativeMatchSelectionHandle[]): NativeMatchShapeHandle;
    named(names: string[], selections: NativeMatchSelectionHandle[]): NativeMatchShapeHandle;
    query(shape: NativeMatchShapeHandle): NativeMatchQueryHandle;
}
interface NativeMatchBindingHandle {
    readonly [nativeMatchHandleKind]: "binding";
    field(fieldName: string): NativeMatchFieldHandle;
    fieldOwnedBy(ownerType: string, fieldName: string): NativeMatchFieldHandle;
    role(roleName: string): NativeMatchRoleHandle;
    roleOwnedBy(ownerType: string, roleName: string): NativeMatchRoleHandle;
    one(): NativeMatchSelectionHandle;
    collect(): NativeMatchSelectionHandle;
}
interface NativeMatchFieldHandle {
    readonly [nativeMatchHandleKind]: "field";
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
interface NativeMatchModule {
    NodeMatchSessionHandle: new (registry: NativeRegistryHandle) => NativeMatchSessionHandle;
    revalidateMatchDiagnostic(registry: NativeRegistryHandle, diagnosticJson: string): string;
}
type LoadedNativeModule = NativeModule & NativeMatchModule;
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
