/**
 * Typed low-level authoring for canonical V2 query plans.
 *
 * This module wraps opaque native handles only. Rust owns builder state,
 * semantic validation, canonical serialization, fingerprints, and capability
 * derivation.
 */
import { QueryV2Authority } from "./index.js";
import { type NativeAuthoredQueryInvocation, type NativeAuthoredQueryPlan, type NativeQueryV2AuthorityIdentity, type NativeQueryV2BindingHandle, type NativeQueryV2DocumentFieldHandle, type NativeQueryV2InputHandle, type NativeQueryV2LocalFunctionHandle, type NativeQueryV2LocalReturnHandle, type NativeQueryV2OperandHandle, type NativeQueryV2OrderHandle, type NativeQueryV2PatternHandle, type NativeQueryV2ReduceAssignmentHandle } from "./query-v2-internals.js";
export { QueryV2Authority };
type QueryV2ValueType = "string" | "long" | "double" | "boolean" | "date" | "datetime" | "datetime_tz" | "decimal" | "duration";
type QueryV2TextValueType = "string" | "date" | "datetime" | "datetime_tz" | "decimal" | "duration";
type QueryV2TypeKind = "entity" | "relation" | "attribute";
type QueryV2Comparator = "equal" | "not_equal" | "less" | "less_or_equal" | "greater" | "greater_or_equal";
type QueryV2OrderDirection = "ascending" | "descending";
type QueryV2Reducer = "count" | "max" | "mean" | "median" | "min" | "std" | "sum";
type QueryV2InputReducer = Exclude<QueryV2Reducer, "count">;
type QueryV2Scalar = string | bigint | number | boolean;
type QueryV2InvocationRows = readonly (readonly (QueryV2Scalar | null)[])[];
declare const AUTHORED_PLAN: unique symbol;
declare const AUTHORED_INVOCATION: unique symbol;
/** One immutable invocation bound to an exact authored-plan fingerprint. */
export declare class AuthoredQueryInvocation {
    #private;
    constructor(native: NativeAuthoredQueryInvocation, token: typeof AUTHORED_INVOCATION);
    get canonicalBytes(): Uint8Array;
    get operation(): "rows" | "count" | "exists";
    get planFingerprint(): string;
    get authorityIdentity(): NativeQueryV2AuthorityIdentity;
    get requiredTransportCapabilities(): readonly string[];
}
/** One immutable, canonical V2 query plan finalized by Rust. */
export declare class AuthoredQueryPlan {
    #private;
    constructor(native: NativeAuthoredQueryPlan, token: typeof AUTHORED_PLAN);
    get canonicalBytes(): Uint8Array;
    get format(): "typebridge.query-plan/v2";
    get fingerprint(): string;
    get requiredCapabilities(): readonly string[];
    get authorityIdentity(): NativeQueryV2AuthorityIdentity;
    rows(rows: QueryV2InvocationRows): AuthoredQueryInvocation;
    documents(rows: QueryV2InvocationRows): AuthoredQueryInvocation;
    count(rows: QueryV2InvocationRows): AuthoredQueryInvocation;
    exists(rows: QueryV2InvocationRows): AuthoredQueryInvocation;
}
/** The only public incremental builder for canonical V2 plans. */
export declare class QueryPlanBuilder {
    #private;
    constructor(authority: QueryV2Authority);
    binding(variable: string): NativeQueryV2BindingHandle;
    input(publicName: string, valueType: QueryV2ValueType, optional: boolean): NativeQueryV2InputHandle;
    bindingOperand(binding: NativeQueryV2BindingHandle): NativeQueryV2OperandHandle;
    literalOperand(valueType: QueryV2TextValueType, value: string): NativeQueryV2OperandHandle;
    literalOperand(valueType: "long", value: bigint): NativeQueryV2OperandHandle;
    literalOperand(valueType: "double", value: number): NativeQueryV2OperandHandle;
    literalOperand(valueType: "boolean", value: boolean): NativeQueryV2OperandHandle;
    inputOperand(input: NativeQueryV2InputHandle): NativeQueryV2OperandHandle;
    isa(binding: NativeQueryV2BindingHandle, typeKind: QueryV2TypeKind, typeLabel: string, includeSubtypes: boolean): NativeQueryV2PatternHandle;
    has(owner: NativeQueryV2BindingHandle, attribute: NativeQueryV2BindingHandle, attributeLabel: string): NativeQueryV2PatternHandle;
    links(relation: NativeQueryV2BindingHandle, relationLabel: string, roles: readonly string[], players: readonly NativeQueryV2BindingHandle[]): NativeQueryV2PatternHandle;
    value(comparator: QueryV2Comparator, left: NativeQueryV2OperandHandle, right: NativeQueryV2OperandHandle): NativeQueryV2PatternHandle;
    not(patterns: readonly NativeQueryV2PatternHandle[]): NativeQueryV2PatternHandle;
    or(branches: readonly (readonly NativeQueryV2PatternHandle[])[]): NativeQueryV2PatternHandle;
    try(patterns: readonly NativeQueryV2PatternHandle[]): NativeQueryV2PatternHandle;
    reachable(source: NativeQueryV2BindingHandle, target: NativeQueryV2BindingHandle, relationLabel: string, roleFrom: string, roleTo: string, minDepth: number, maxDepth: number): NativeQueryV2PatternHandle;
    functionCall(assigned: NativeQueryV2BindingHandle, arguments_: readonly NativeQueryV2OperandHandle[], functionName: string, localFunction?: null): NativeQueryV2PatternHandle;
    functionCall(assigned: NativeQueryV2BindingHandle, arguments_: readonly NativeQueryV2OperandHandle[], functionName: null, localFunction: NativeQueryV2LocalFunctionHandle): NativeQueryV2PatternHandle;
    order(binding: NativeQueryV2BindingHandle, direction: QueryV2OrderDirection): NativeQueryV2OrderHandle;
    reduceAssignment(assigned: NativeQueryV2BindingHandle, reducer: "count", input?: null): NativeQueryV2ReduceAssignmentHandle;
    reduceAssignment(assigned: NativeQueryV2BindingHandle, reducer: QueryV2InputReducer, input: NativeQueryV2BindingHandle): NativeQueryV2ReduceAssignmentHandle;
    localReturn(reducer: "count", input: NativeQueryV2BindingHandle, valueType: "long"): NativeQueryV2LocalReturnHandle;
    localReturn(reducer: "sum", input: NativeQueryV2BindingHandle, valueType: "long" | "double"): NativeQueryV2LocalReturnHandle;
    localFunction(name: string, bindings: readonly NativeQueryV2BindingHandle[], parameterBindings: readonly NativeQueryV2BindingHandle[], parameterLabels: readonly string[], body: readonly NativeQueryV2PatternHandle[], returns: NativeQueryV2LocalReturnHandle): NativeQueryV2LocalFunctionHandle;
    match(patterns: readonly NativeQueryV2PatternHandle[]): void;
    select(bindings: readonly NativeQueryV2BindingHandle[]): void;
    require(bindings: readonly NativeQueryV2BindingHandle[]): void;
    distinct(): void;
    reduce(assignments: readonly NativeQueryV2ReduceAssignmentHandle[], groups: readonly NativeQueryV2BindingHandle[]): void;
    sort(terms: readonly NativeQueryV2OrderHandle[]): void;
    offset(rows: bigint): void;
    limit(rows: bigint): void;
    documentBinding(key: string, binding: NativeQueryV2BindingHandle): NativeQueryV2DocumentFieldHandle;
    documentAttributeList(key: string, owner: NativeQueryV2BindingHandle, attributeLabel: string): NativeQueryV2DocumentFieldHandle;
    finalizeRows(columns: readonly NativeQueryV2BindingHandle[]): AuthoredQueryPlan;
    finalizeDocuments(fields: readonly NativeQueryV2DocumentFieldHandle[]): AuthoredQueryPlan;
}
