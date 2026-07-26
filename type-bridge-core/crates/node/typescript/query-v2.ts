/**
 * Typed low-level authoring for canonical V2 query plans.
 *
 * This module wraps opaque native handles only. Rust owns builder state,
 * semantic validation, canonical serialization, fingerprints, and capability
 * derivation.
 */

import { QueryV2Authority } from "./index.js";
import { loadNative } from "./native.js";
import {
  queryV2AuthorityHandle,
  queryV2NativeCall,
  type NativeAuthoredQueryInvocation,
  type NativeAuthoredQueryPlan,
  type NativeQueryPlanBuilder,
  type NativeQueryV2AuthorityIdentity,
  type NativeQueryV2BindingHandle,
  type NativeQueryV2DocumentFieldHandle,
  type NativeQueryV2InputHandle,
  type NativeQueryV2LocalFunctionHandle,
  type NativeQueryV2LocalReturnHandle,
  type NativeQueryV2OperandHandle,
  type NativeQueryV2OrderHandle,
  type NativeQueryV2PatternHandle,
  type NativeQueryV2ReduceAssignmentHandle,
} from "./query-v2-internals.js";

export { QueryV2Authority };

type QueryV2ValueType =
  | "string"
  | "long"
  | "double"
  | "boolean"
  | "date"
  | "datetime"
  | "datetime_tz"
  | "decimal"
  | "duration";

type QueryV2TextValueType =
  | "string"
  | "date"
  | "datetime"
  | "datetime_tz"
  | "decimal"
  | "duration";

type QueryV2TypeKind = "entity" | "relation" | "attribute";
type QueryV2Comparator =
  | "equal"
  | "not_equal"
  | "less"
  | "less_or_equal"
  | "greater"
  | "greater_or_equal";
type QueryV2OrderDirection = "ascending" | "descending";
type QueryV2Reducer = "count" | "max" | "mean" | "min" | "sum";
type QueryV2InputReducer = Exclude<QueryV2Reducer, "count">;
type QueryV2Scalar = string | bigint | number | boolean;
type QueryV2InvocationRows = readonly (readonly (QueryV2Scalar | null)[])[];

const AUTHORED_PLAN = Symbol("AuthoredQueryPlan");
const AUTHORED_INVOCATION = Symbol("AuthoredQueryInvocation");

/**
 * N-API reads authoring arrays synchronously and never mutates them. Keep the
 * public readonly contract without copying attacker-sized collections before
 * the native boundary can inspect their lengths.
 */
function nativeArray<T>(values: readonly T[]): T[] {
  return values as T[];
}

function nativeNestedArray<T>(values: readonly (readonly T[])[]): T[][] {
  return values as T[][];
}

/** One immutable invocation bound to an exact authored-plan fingerprint. */
export class AuthoredQueryInvocation {
  readonly #native: NativeAuthoredQueryInvocation;

  constructor(native: NativeAuthoredQueryInvocation, token: typeof AUTHORED_INVOCATION) {
    if (token !== AUTHORED_INVOCATION) {
      throw new TypeError("AuthoredQueryInvocation values are created by AuthoredQueryPlan");
    }
    this.#native = native;
    Object.freeze(this);
  }

  get canonicalBytes(): Uint8Array {
    return new Uint8Array(this.#native.canonicalBytes);
  }

  get operation(): "rows" | "count" | "exists" {
    return this.#native.operation;
  }

  get planFingerprint(): string {
    return this.#native.planFingerprint;
  }

  get authorityIdentity(): NativeQueryV2AuthorityIdentity {
    return this.#native.authorityIdentity;
  }

  get requiredTransportCapabilities(): readonly string[] {
    return Object.freeze([...this.#native.requiredTransportCapabilities]);
  }
}

/** One immutable, canonical V2 query plan finalized by Rust. */
export class AuthoredQueryPlan {
  readonly #native: NativeAuthoredQueryPlan;

  constructor(native: NativeAuthoredQueryPlan, token: typeof AUTHORED_PLAN) {
    if (token !== AUTHORED_PLAN) {
      throw new TypeError("AuthoredQueryPlan values are created by QueryPlanBuilder");
    }
    this.#native = native;
    Object.freeze(this);
  }

  get canonicalBytes(): Uint8Array {
    return new Uint8Array(this.#native.canonicalBytes);
  }

  get format(): "typebridge.query-plan/v2" {
    return this.#native.format;
  }

  get fingerprint(): string {
    return this.#native.fingerprint;
  }

  get requiredCapabilities(): readonly string[] {
    return Object.freeze([...this.#native.requiredCapabilities]);
  }

  get authorityIdentity(): NativeQueryV2AuthorityIdentity {
    return this.#native.authorityIdentity;
  }

  rows(rows: QueryV2InvocationRows): AuthoredQueryInvocation {
    const native = queryV2NativeCall(() => this.#native.rows(nativeNestedArray(rows)));
    return new AuthoredQueryInvocation(native, AUTHORED_INVOCATION);
  }

  documents(rows: QueryV2InvocationRows): AuthoredQueryInvocation {
    const native = queryV2NativeCall(() => this.#native.documents(nativeNestedArray(rows)));
    return new AuthoredQueryInvocation(native, AUTHORED_INVOCATION);
  }

  count(rows: QueryV2InvocationRows): AuthoredQueryInvocation {
    const native = queryV2NativeCall(() => this.#native.count(nativeNestedArray(rows)));
    return new AuthoredQueryInvocation(native, AUTHORED_INVOCATION);
  }

  exists(rows: QueryV2InvocationRows): AuthoredQueryInvocation {
    const native = queryV2NativeCall(() => this.#native.exists(nativeNestedArray(rows)));
    return new AuthoredQueryInvocation(native, AUTHORED_INVOCATION);
  }
}

/** The only public incremental builder for canonical V2 plans. */
export class QueryPlanBuilder {
  readonly #native: NativeQueryPlanBuilder;

  constructor(authority: QueryV2Authority) {
    const nativeAuthority = queryV2AuthorityHandle(authority);
    if (nativeAuthority === undefined) {
      throw new TypeError("QueryPlanBuilder requires a type-bridge QueryV2Authority");
    }
    this.#native = queryV2NativeCall(
      () => new (loadNative().NodeQueryPlanBuilder)(nativeAuthority),
    );
  }

  binding(variable: string): NativeQueryV2BindingHandle {
    return queryV2NativeCall(() => this.#native.binding(variable));
  }

  input(
    publicName: string,
    valueType: QueryV2ValueType,
    optional: boolean,
  ): NativeQueryV2InputHandle {
    return queryV2NativeCall(() => this.#native.input(publicName, valueType, optional));
  }

  bindingOperand(binding: NativeQueryV2BindingHandle): NativeQueryV2OperandHandle {
    return queryV2NativeCall(() => this.#native.bindingOperand(binding));
  }

  literalOperand(
    valueType: QueryV2TextValueType,
    value: string,
  ): NativeQueryV2OperandHandle;
  literalOperand(valueType: "long", value: bigint): NativeQueryV2OperandHandle;
  literalOperand(valueType: "double", value: number): NativeQueryV2OperandHandle;
  literalOperand(valueType: "boolean", value: boolean): NativeQueryV2OperandHandle;
  literalOperand(
    valueType: QueryV2ValueType,
    value: QueryV2Scalar,
  ): NativeQueryV2OperandHandle {
    return queryV2NativeCall(() => this.#native.literalOperand(valueType, value));
  }

  inputOperand(input: NativeQueryV2InputHandle): NativeQueryV2OperandHandle {
    return queryV2NativeCall(() => this.#native.inputOperand(input));
  }

  isa(
    binding: NativeQueryV2BindingHandle,
    typeKind: QueryV2TypeKind,
    typeLabel: string,
    includeSubtypes: boolean,
  ): NativeQueryV2PatternHandle {
    return queryV2NativeCall(
      () => this.#native.isa(binding, typeKind, typeLabel, includeSubtypes),
    );
  }

  has(
    owner: NativeQueryV2BindingHandle,
    attribute: NativeQueryV2BindingHandle,
    attributeLabel: string,
  ): NativeQueryV2PatternHandle {
    return queryV2NativeCall(
      () => this.#native.has(owner, attribute, attributeLabel),
    );
  }

  links(
    relation: NativeQueryV2BindingHandle,
    relationLabel: string,
    roles: readonly string[],
    players: readonly NativeQueryV2BindingHandle[],
  ): NativeQueryV2PatternHandle {
    return queryV2NativeCall(
      () =>
        this.#native.links(
          relation,
          relationLabel,
          nativeArray(roles),
          nativeArray(players),
        ),
    );
  }

  value(
    comparator: QueryV2Comparator,
    left: NativeQueryV2OperandHandle,
    right: NativeQueryV2OperandHandle,
  ): NativeQueryV2PatternHandle {
    return queryV2NativeCall(() => this.#native.value(comparator, left, right));
  }

  not(patterns: readonly NativeQueryV2PatternHandle[]): NativeQueryV2PatternHandle {
    return queryV2NativeCall(() => this.#native.not(nativeArray(patterns)));
  }

  or(
    branches: readonly (readonly NativeQueryV2PatternHandle[])[],
  ): NativeQueryV2PatternHandle {
    return queryV2NativeCall(
      () => this.#native.or(nativeNestedArray(branches)),
    );
  }

  try(patterns: readonly NativeQueryV2PatternHandle[]): NativeQueryV2PatternHandle {
    return queryV2NativeCall(() => this.#native.try(nativeArray(patterns)));
  }

  reachable(
    source: NativeQueryV2BindingHandle,
    target: NativeQueryV2BindingHandle,
    relationLabel: string,
    roleFrom: string,
    roleTo: string,
    minDepth: number,
    maxDepth: number,
  ): NativeQueryV2PatternHandle {
    return queryV2NativeCall(
      () =>
        this.#native.reachable(
          source,
          target,
          relationLabel,
          roleFrom,
          roleTo,
          minDepth,
          maxDepth,
        ),
    );
  }

  functionCall(
    assigned: NativeQueryV2BindingHandle,
    arguments_: readonly NativeQueryV2OperandHandle[],
    functionName: string,
    localFunction?: null,
  ): NativeQueryV2PatternHandle;
  functionCall(
    assigned: NativeQueryV2BindingHandle,
    arguments_: readonly NativeQueryV2OperandHandle[],
    functionName: null,
    localFunction: NativeQueryV2LocalFunctionHandle,
  ): NativeQueryV2PatternHandle;
  functionCall(
    assigned: NativeQueryV2BindingHandle,
    arguments_: readonly NativeQueryV2OperandHandle[],
    functionName: string | null,
    localFunction?: NativeQueryV2LocalFunctionHandle | null,
  ): NativeQueryV2PatternHandle {
    return queryV2NativeCall(
      () =>
        this.#native.functionCall(
          assigned,
          nativeArray(arguments_),
          functionName,
          localFunction,
        ),
    );
  }

  order(
    binding: NativeQueryV2BindingHandle,
    direction: QueryV2OrderDirection,
  ): NativeQueryV2OrderHandle {
    return queryV2NativeCall(() => this.#native.order(binding, direction));
  }

  reduceAssignment(
    assigned: NativeQueryV2BindingHandle,
    reducer: "count",
    input?: null,
  ): NativeQueryV2ReduceAssignmentHandle;
  reduceAssignment(
    assigned: NativeQueryV2BindingHandle,
    reducer: QueryV2InputReducer,
    input: NativeQueryV2BindingHandle,
  ): NativeQueryV2ReduceAssignmentHandle;
  reduceAssignment(
    assigned: NativeQueryV2BindingHandle,
    reducer: QueryV2Reducer,
    input?: NativeQueryV2BindingHandle | null,
  ): NativeQueryV2ReduceAssignmentHandle {
    return queryV2NativeCall(
      () => this.#native.reduceAssignment(assigned, reducer, input),
    );
  }

  localReturn(
    reducer: "count",
    input: NativeQueryV2BindingHandle,
    valueType: "long",
  ): NativeQueryV2LocalReturnHandle;
  localReturn(
    reducer: "sum",
    input: NativeQueryV2BindingHandle,
    valueType: "long" | "double",
  ): NativeQueryV2LocalReturnHandle;
  localReturn(
    reducer: "count" | "sum",
    input: NativeQueryV2BindingHandle,
    valueType: "long" | "double",
  ): NativeQueryV2LocalReturnHandle {
    return queryV2NativeCall(
      () => this.#native.localReturn(reducer, input, valueType),
    );
  }

  localFunction(
    name: string,
    bindings: readonly NativeQueryV2BindingHandle[],
    parameterBindings: readonly NativeQueryV2BindingHandle[],
    parameterLabels: readonly string[],
    body: readonly NativeQueryV2PatternHandle[],
    returns: NativeQueryV2LocalReturnHandle,
  ): NativeQueryV2LocalFunctionHandle {
    return queryV2NativeCall(
      () =>
        this.#native.localFunction(
          name,
          nativeArray(bindings),
          nativeArray(parameterBindings),
          nativeArray(parameterLabels),
          nativeArray(body),
          returns,
        ),
    );
  }

  match(patterns: readonly NativeQueryV2PatternHandle[]): void {
    queryV2NativeCall(() => this.#native.match(nativeArray(patterns)));
  }

  select(bindings: readonly NativeQueryV2BindingHandle[]): void {
    queryV2NativeCall(() => this.#native.select(nativeArray(bindings)));
  }

  require(bindings: readonly NativeQueryV2BindingHandle[]): void {
    queryV2NativeCall(() => this.#native.require(nativeArray(bindings)));
  }

  distinct(): void {
    queryV2NativeCall(() => this.#native.distinct());
  }

  reduce(
    assignments: readonly NativeQueryV2ReduceAssignmentHandle[],
    groups: readonly NativeQueryV2BindingHandle[],
  ): void {
    queryV2NativeCall(() =>
      this.#native.reduce(nativeArray(assignments), nativeArray(groups)),
    );
  }

  sort(terms: readonly NativeQueryV2OrderHandle[]): void {
    queryV2NativeCall(() => this.#native.sort(nativeArray(terms)));
  }

  offset(rows: bigint): void {
    queryV2NativeCall(() => this.#native.offset(rows));
  }

  limit(rows: bigint): void {
    queryV2NativeCall(() => this.#native.limit(rows));
  }

  documentBinding(
    key: string,
    binding: NativeQueryV2BindingHandle,
  ): NativeQueryV2DocumentFieldHandle {
    return queryV2NativeCall(() => this.#native.documentBinding(key, binding));
  }

  documentAttributeList(
    key: string,
    owner: NativeQueryV2BindingHandle,
    attributeLabel: string,
  ): NativeQueryV2DocumentFieldHandle {
    return queryV2NativeCall(
      () => this.#native.documentAttributeList(key, owner, attributeLabel),
    );
  }

  finalizeRows(
    columns: readonly NativeQueryV2BindingHandle[],
  ): AuthoredQueryPlan {
    const native = queryV2NativeCall(() =>
      this.#native.finalizeRows(nativeArray(columns)),
    );
    return new AuthoredQueryPlan(native, AUTHORED_PLAN);
  }

  finalizeDocuments(
    fields: readonly NativeQueryV2DocumentFieldHandle[],
  ): AuthoredQueryPlan {
    const native = queryV2NativeCall(() =>
      this.#native.finalizeDocuments(nativeArray(fields)),
    );
    return new AuthoredQueryPlan(native, AUTHORED_PLAN);
  }
}
