import * as fs from "fs";
import * as path from "path";
import type {
  NativeModule,
  NativeRustDatabase,
  NativeRustTransactionContext,
} from "./index.js";
import { ownedByteSnapshot } from "./owned-bytes.js";
import type { NativePlanAuthority } from "./plan-internals.js";
import type {
  NativeProjectedFacadeProof,
  NativeProjectedManager,
  NativeProjectedValueEnvelope,
  NativeProjectionHandle,
  NativeAuthoredPlanResult,
  RuntimeProjectionInstall,
} from "./runtime-projection.js";

type NativeProjectedBatchAuthority = Parameters<
  NonNullable<RuntimeProjectionInstall["projectedBatchMaterializer"]>
>[3];

type NativeMatchComparison =
  | "equal"
  | "not_equal"
  | "less_than"
  | "less_than_or_equal"
  | "greater_than"
  | "greater_than_or_equal"
  | "contains"
  | "starts_with"
  | "ends_with"
  | "regex";

type NativeMatchDirection = "ascending" | "descending";
type NativeMatchMissingOrder = "reject" | "first" | "last";
type NativeMatchRowCardinality = "exactly_one" | "bounded_many";
type NativeMatchReduction =
  | "count"
  | "sum"
  | "min"
  | "max"
  | "mean"
  | "median"
  | "std";
declare const nativeMatchHandleKind: unique symbol;

interface NativeMatchSessionHandle {
  readonly [nativeMatchHandleKind]: "session";
  readonly isClosed: boolean;
  close(): void;
  exact(typeName: string): NativeMatchBindingHandle;
  subtypes(typeName: string): NativeMatchBindingHandle;
  /** `matchMode` is parsed by the core: `exact` or `subtypes`. */
  binding(typeName: string, matchMode: string): NativeMatchBindingHandle;
  functionById(functionId: string): NativeMatchFunctionHandle;
  functionValueJson(attributeTypeKey: string, valueJson: string): NativeMatchFunctionValueHandle;
  reachable(
    relationType: string,
    roleFrom: string,
    roleTo: string,
    source: NativeMatchBindingHandle,
    target: NativeMatchBindingHandle,
    minDepth: number,
    maxDepth: number,
  ): NativeMatchPredicateHandle;
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
  compareField(
    comparison: NativeMatchComparison,
    field: NativeMatchFieldHandle,
  ): NativeMatchPredicateHandle;
  compareValue(
    comparison: NativeMatchComparison,
    value: NativeMatchFunctionValueHandle,
  ): NativeMatchPredicateHandle;
  compareCall(
    comparison: NativeMatchComparison,
    other: NativeMatchFunctionCallHandle,
  ): NativeMatchPredicateHandle;
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
  checkGroupFields(groups: NativeMatchFieldHandle[]): void;
  fetchRowsDiagnostic(
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): string;
  executeFetchRowsOwned(
    database: NativeRustDatabase,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): Promise<NativeValidatedMatchResultHandle>;
  executeFetchRowsBorrowed(
    transaction: NativeRustTransactionContext,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): Promise<NativeValidatedMatchResultHandle>;
  executePageByOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): Promise<NativeValidatedMatchResultHandle>;
  executePageByBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): Promise<NativeValidatedMatchResultHandle>;
  executeCountByOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
  ): Promise<NativeValidatedMatchResultHandle>;
  executeCountByBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
  ): Promise<NativeValidatedMatchResultHandle>;
  executeExistsByOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
  ): Promise<NativeValidatedMatchResultHandle>;
  executeExistsByBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
  ): Promise<NativeValidatedMatchResultHandle>;
  pageByDiagnostic(
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): string;
  countByDiagnostic(root: NativeMatchBindingHandle): string;
  existsByDiagnostic(root: NativeMatchBindingHandle): string;
  reduceByDiagnostic(
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): string;
  executeReduceByOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;
  executeReduceByBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;
  reduceByFieldDiagnostic(
    root: NativeMatchBindingHandle,
    group: NativeMatchFieldHandle,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): string;
  executeReduceByFieldOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
    group: NativeMatchFieldHandle,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;
  executeReduceByFieldBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
    group: NativeMatchFieldHandle,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;
  reduceByFieldsDiagnostic(
    root: NativeMatchBindingHandle,
    groups: NativeMatchFieldHandle[],
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): string;
  executeReduceByFieldsOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
    groups: NativeMatchFieldHandle[],
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;
  executeReduceByFieldsBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
    groups: NativeMatchFieldHandle[],
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;

  stageFetchRows(
    transaction: NativeStagedTransaction,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): NativePendingPlanAnswer;
  stagePageBy(
    transaction: NativeStagedTransaction,
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): NativePendingPlanAnswer;
  stageCountBy(
    transaction: NativeStagedTransaction,
    root: NativeMatchBindingHandle,
  ): NativePendingPlanAnswer;
  stageExistsBy(
    transaction: NativeStagedTransaction,
    root: NativeMatchBindingHandle,
  ): NativePendingPlanAnswer;
  stageReduceBy(
    transaction: NativeStagedTransaction,
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): NativePendingPlanAnswer;
  stageReduceByField(
    transaction: NativeStagedTransaction,
    root: NativeMatchBindingHandle,
    group: NativeMatchFieldHandle,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): NativePendingPlanAnswer;
  stageReduceByFields(
    transaction: NativeStagedTransaction,
    root: NativeMatchBindingHandle,
    groups: NativeMatchFieldHandle[],
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): NativePendingPlanAnswer;
}

interface NativeValidatedMatchResultHandle {
  readonly [nativeMatchHandleKind]: "validated-result";
  outputSlotCount(query: NativeMatchQueryHandle): number;
  outputSlotIsCollection(query: NativeMatchQueryHandle, slotIndex: number): boolean;
  rowCount(query: NativeMatchQueryHandle): number;
  slotCount(query: NativeMatchQueryHandle, rowIndex: number): number;
  outputNames(query: NativeMatchQueryHandle): string[] | null;
  slotThing(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    slotIndex: number,
  ): NativeValidatedThingHandle;
  pageEntryCount(query: NativeMatchQueryHandle): number;
  pageSlotCount(query: NativeMatchQueryHandle, entryIndex: number): number;
  pageSlotValueCount(
    query: NativeMatchQueryHandle,
    entryIndex: number,
    slotIndex: number,
  ): number;
  pageSlotThing(
    query: NativeMatchQueryHandle,
    entryIndex: number,
    slotIndex: number,
    valueIndex: number,
  ): NativeValidatedThingHandle;
  pageOffset(query: NativeMatchQueryHandle): bigint;
  pageLimit(query: NativeMatchQueryHandle): bigint;
  pageTotal(query: NativeMatchQueryHandle): bigint | null;
  countValue(query: NativeMatchQueryHandle): bigint;
  existsValue(query: NativeMatchQueryHandle): boolean;
  reductionRowCount(query: NativeMatchQueryHandle): number;
  reductionValueCount(query: NativeMatchQueryHandle, rowIndex: number): number;
  reductionValueKind(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    valueIndex: number,
  ): "count" | "long" | "double";
  reductionCountValue(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    valueIndex: number,
  ): bigint;
  reductionLongValue(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    valueIndex: number,
  ): bigint | null;
  reductionDoubleValue(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    valueIndex: number,
  ): number | null;
  reductionGroup(
    query: NativeMatchQueryHandle,
    rowIndex: number,
  ): NativeValidatedThingHandle;
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

interface NativeRemoteModelQueryContext {}

export interface NativeQueryLimits {
  names(): string[];
  get(name: string): bigint;
}

export interface NativeQueryExecutionResources {
  readonly collectPhaseTimestamps: boolean;
  readonly timeoutMilliseconds: bigint;
  readonly items: bigint;
  readonly bytes: bigint;
  readonly graphNodes: bigint;
  readonly attributeValues: bigint;
  readonly collectionMembers: bigint;
  readonly rolePlayers: bigint;
  readonly statements: bigint | null;
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

export interface NativePendingPlanAnswer {
  getQueryResult(): NativeValidatedMatchResultHandle;
  getProjection(): NativeAuthoredPlanResult | null;
  getJson(): string;
}

export interface NativeStagedTransaction {
  stageStack(plans: NativeStagedPlan[]): NativePendingPlanAnswer[];
  stagePlan(planBytes: Uint8Array, schemaBytes: Uint8Array): NativePendingPlanAnswer;
  stage(plan: NativePlan): NativePendingPlanAnswer;
  commit(): Promise<void>;
  rollback(): void;
  close(): void;
  dispose(): Promise<void>;
}

/** One operand: a binding, an input column, or a registered literal. */
export interface NativePlanOperand {
  readonly kind: "binding" | "input" | "literal";
  readonly index: number;
}

/** One validated `query-plan/v3` program. */
export interface NativeStagedPlan { resultRefsJson(): string; }

export interface NativePlan {
  explainJson(): string;
  bindRows(authority: NativePlanAuthority, columns: number[][], rows: unknown[][]): NativePlan;
  scalarRowsJson(answerJson: string): string;
  canonicalBytes(): Uint8Array;
  fingerprint(): string;
  writes(): boolean;
}

/** The shared core plan builder; handles are dense indexes. */
export interface NativePlanBuilder {
  copy(): NativePlanBuilder;
  exact(typeJson: string): number;
  subtypes(typeJson: string): number;
  field(owner: number, ownsJson: string): number;
  fieldValue(owner: number, ownsJson: string): number;
  byIid(owner: number, iid: string): void;
  byIidInput(owner: number, input: number): void;
  bindValue(owner: number, field: string, valueType: string, value: unknown): number;
  bindRows(columns: { owner: number; field: string; valueType: string }[], rows: number[][]): number[];
  letValue(owner: number, field: string, expr: number): number;
  modelRows(thing: number, columns: { field: string; valueType: string }[], rows: unknown[][]): void;
  link(relation: number, roleJson: string, player: number): void;
  links(relation: number, roleJson: string, player: number): void;
  compare(left: NativePlanOperand, comparator: string, right: NativePlanOperand): void;
  input(name: string, valueType: string, optional: boolean): number;
  literal(valueType: string, value: unknown): number;
  exprOperand(operand: NativePlanOperand): number;
  exprArith(op: string, lhs: number, rhs: number): number;
  exprCall(func: string, args: number[]): number;
  let(name: string, expr: number): number;
  update(owner: number, ownsJson: string, value: NativePlanOperand): void;
  updatePlayer(relation: number, roleJson: string, player: number): void;
  insert(typeJson: string): number;
  put(typeJson: string): number;
  thingField(thing: number, ownsJson: string, value: NativePlanOperand): void;
  thingPlayer(thing: number, roleJson: string, player: number): void;
  delete(thing: number): void;
  deleteField(owner: number, ownsJson: string): void;
  deletePlayer(relation: number, roleJson: string, player: number): void;
  row(columns: number[], literals: (number | null)[]): void;
  returning(columns: number[]): void;
  fetch(thing: number, roles: string[], required: boolean[]): void;
  fetchHeld(thing: number, roles: string[], required: boolean[]): void;
  count(): void;
  exists(): void;
  build(authority: NativePlanAuthority, matched?: boolean, models?: boolean): NativePlan;
  buildStaged(authority: NativePlanAuthority, columns: number[][], rows: unknown[][], references: string): NativeStagedPlan;
  buildSelected(authority: NativePlanAuthority, kind: string, thing: number | null, roles: string[], required: boolean[], columns: number[], projection?: NativeProjectionHandle | null, declared?: string | null, modelBindings?: number[] | null, modelTypes?: string[] | null): NativePlan;
}

interface NativePlanBuilderModule {
  setDebugListener(callback: ((json: string) => void) | null): void;
  NodePlanBuilder: new (queryLimits?: NativeQueryLimits) => NativePlanBuilder;
}

/** Native proxy connection over the built-in HTTP route. */
export interface NativeProxyConnection {
  readonly admissionWindowMilliseconds: bigint;
  readonly queryLimits: NativeQueryLimits;
  readonly answerLimits: NativeQueryExecutionResources;
  transaction(
    kind?: string,
    cancellation?: NativeQueryCancellation,
  ): Promise<NativeStagedTransaction>;
  rows(
    query: NativeMatchQueryHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): Promise<NativeValidatedMatchResultHandle>;
  page(
    query: NativeMatchQueryHandle,
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): Promise<NativeValidatedMatchResultHandle>;
  count(
    query: NativeMatchQueryHandle,
    root: NativeMatchBindingHandle,
  ): Promise<NativeValidatedMatchResultHandle>;
  exists(
    query: NativeMatchQueryHandle,
    root: NativeMatchBindingHandle,
  ): Promise<NativeValidatedMatchResultHandle>;
  reduce(
    query: NativeMatchQueryHandle,
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;
  reduceByField(
    query: NativeMatchQueryHandle,
    root: NativeMatchBindingHandle,
    group: NativeMatchFieldHandle,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;
  reduceByFields(
    query: NativeMatchQueryHandle,
    root: NativeMatchBindingHandle,
    groups: NativeMatchFieldHandle[],
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): Promise<NativeValidatedMatchResultHandle>;
}

interface NativeProxyConnectionModule {
  NodeProxyConnection: {
    connect(
      authority: ReturnType<NativeModule["planAuthority"]>,
      endpoint: string,
      tlsMode: string,
      tlsRootCa: string | undefined,
      headerNames: string[],
      headerValues: string[],
      advertisement?: Uint8Array,
      connectionLimits?: NativeQueryExecutionResources,
      answerLimits?: NativeQueryExecutionResources,
      cancellation?: NativeQueryCancellation,
      stagedAdvertisement?: Uint8Array,
      admissionWindowMilliseconds?: bigint,
      queryLimits?: NativeQueryLimits,
    ): Promise<NativeProxyConnection>;
  };
}

/** Core limits the addon exports, so the package holds no copies. */
interface NativeLimitsModule {
  readonly DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS: bigint;
  readonly DEFAULT_DIRECT_QUERY_ITEMS: bigint;
  readonly DEFAULT_DIRECT_QUERY_BYTES: bigint;
  readonly DEFAULT_DIRECT_QUERY_GRAPH_NODES: bigint;
  readonly DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES: bigint;
  readonly DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS: bigint;
  readonly DEFAULT_DIRECT_QUERY_ROLE_PLAYERS: bigint;
  readonly DEFAULT_DIRECT_QUERY_STATEMENTS: bigint;
  readonly DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS: bigint;
  readonly DEFAULT_PROXY_QUERY_ITEMS: bigint;
  readonly DEFAULT_PROXY_QUERY_BYTES: bigint;
  readonly DEFAULT_PROXY_QUERY_GRAPH_NODES: bigint;
  readonly DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES: bigint;
  readonly DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS: bigint;
  readonly DEFAULT_PROXY_QUERY_ROLE_PLAYERS: bigint;
  readonly DEFAULT_PROXY_QUERY_STATEMENTS: bigint;
  readonly MAX_CANONICAL_BYTES: number;
  readonly MAX_REMOTE_ENVELOPE_BYTES: number;
  readonly MAX_DIAGNOSTIC_CODE_BYTES: number;
}

/** The core's one parser per closed enum. */
interface NativeEnumSpellingModule {
  /**
   * The canonical spelling of `value` in the enum named by `kind`; an unknown
   * spelling throws the `unknown_enum_value` diagnostic.
   */
  canonicalEnumSpelling(kind: string, value: string): string;
}

interface NativeRemoteModelQueryModule {
  /** An omitted dimension takes the core default. */
  NodeQueryExecutionResources: new (
    timeoutMilliseconds?: bigint,
    items?: bigint,
    bytes?: bigint,
    graphNodes?: bigint,
    attributeValues?: bigint,
    collectionMembers?: bigint,
    rolePlayers?: bigint,
    statements?: bigint,
    collectPhaseTimestamps?: boolean,
  ) => NativeQueryExecutionResources;
  NodeQueryCancellation: new () => NativeQueryCancellation;
  NodeQueryLimits: new (names: string[], values: bigint[]) => NativeQueryLimits;
  planRemoteModelContextWithResources(
    authority: ReturnType<NativeModule["planAuthority"]>,
    advertisement: Uint8Array,
    resources: NativeQueryExecutionResources,
    cancellation: NativeQueryCancellation,
    answerLimits?: NativeQueryExecutionResources,
  ): NativeRemoteModelQueryContext;
  planPrepareRemoteModelRows(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): NativePendingRemoteModelQuery;
  planPrepareRemoteModelPage(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): NativePendingRemoteModelQuery;
  planPrepareRemoteModelCount(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
  ): NativePendingRemoteModelQuery;
  planPrepareRemoteModelExists(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
  ): NativePendingRemoteModelQuery;
  planPrepareRemoteModelReduce(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): NativePendingRemoteModelQuery;
  planPrepareRemoteModelReduceByField(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
    group: NativeMatchFieldHandle,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): NativePendingRemoteModelQuery;
  planPrepareRemoteModelReduceByFields(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
    groups: NativeMatchFieldHandle[],
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): NativePendingRemoteModelQuery;
}

interface NativeRuntimeProjectionHandle {
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
  managerForProxy(typeKey: string, connection: NativeProxyConnection): NativeProjectedManager;
  managerForDatabase(typeKey: string, database: NativeRustDatabase): NativeProjectedManager;
  managerForTransaction(typeKey: string, transaction: NativeRustTransactionContext): NativeProjectedManager;
  matchSessionWithResources(
    resources: NativeQueryExecutionResources,
    cancellation: NativeQueryCancellation,
    queryLimits?: NativeQueryLimits | null,
  ): NativeMatchSessionHandle;
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
  materializeMatchThingJson(thing: NativeValidatedThingHandle): string;
  materializePlanDocuments(plan: NativePlan, answerJson: string, declaredId: string): unknown[];
  materializeMatchThingProjected(
    thing: NativeValidatedThingHandle,
  ): NativeProjectedValueEnvelope;
}

interface NativeRuntimeProjectionModule {
  projectedBatchDeleted(authority: NativeProjectedBatchAuthority, ordinal: number, roleName?: string, playerIndex?: number): boolean;
  installPlanRuntimeProjection(
    projectionJson: string,
    semanticFingerprintJson: string,
    projectionFingerprintJson: string,
    registrationsJson: string,
    schemaAuthorityJson: string,
    projectedBatchMaterializer: NonNullable<ConstructorParameters<NativeRuntimeProjectionModule["NodeRuntimeProjection"]>[5]>,
  ): NativeRuntimeProjectionHandle;
  NodeRuntimeProjection: new (
    projectionJson: string,
    semanticFingerprintJson: string,
    projectionFingerprintJson: string,
    registrationsJson: string,
    schemaAuthorityJson?: string,
    projectedBatchMaterializer?: (
      typeKey: string,
      ordinal: number,
      json: string,
      authority: NativeProjectedBatchAuthority,
    ) => object,
  ) => NativeRuntimeProjectionHandle;
}

type LoadedNativeModule =
  & NativeModule
  & NativeLimitsModule
  & NativeEnumSpellingModule
  & NativeRemoteModelQueryModule
  & NativeProxyConnectionModule
  & NativePlanBuilderModule
  & NativeRuntimeProjectionModule;
type LoadedNativePendingRemoteModelQuery = ReturnType<
  LoadedNativeModule["planPrepareRemoteModelRows"]
>;

// This module compiles to dist/native.js (CommonJS). __dirname is therefore
// the dist/ directory. The .node artifacts are placed at the package root
// (one level up, i.e. dist/..) by build-native.js, which writes
// type_bridge_node.<triple>.node beside package.json. The candidates list
// probes the package root first (primary), then dist/ itself as a robustness
// fallback for atypical build layouts.

let _cached: LoadedNativeModule | null = null;

function protectedPendingRemoteModelQuery(
  pending: LoadedNativePendingRemoteModelQuery,
  maxEnvelopeBytes: number,
): LoadedNativePendingRemoteModelQuery {
  const requestBytes = pending.requestBytes.bind(pending);
  const decodeReply = pending.decodeReply.bind(pending);
  // Released test adapters and third-party native shims predate the additive
  // pending lifecycle methods. Preserve their byte/one-shot behavior while
  // exposing the new no-op lifecycle view; production native handles always
  // provide the real methods.
  const close = typeof pending.close === "function"
    ? pending.close.bind(pending)
    : (): void => {};
  let decodeStarted = false;
  return Object.freeze({
    get isClosed(): boolean { return pending.isClosed ?? false; },
    close: (): void => close(),
    requestBytes: (): Uint8Array => requestBytes(),
    decodeReply: (response: Uint8Array) => {
      if (decodeStarted) {
        return decodeReply(response);
      }
      decodeStarted = true;
      let snapshot: Buffer;
      try {
        snapshot = ownedByteSnapshot(response, maxEnvelopeBytes);
      } catch {
        return decodeReply(response);
      }
      return decodeReply(snapshot);
    },
  });
}

function protectedMethodDescriptor(
  native: LoadedNativeModule,
  name: keyof LoadedNativeModule,
  value: (...args: never[]) => unknown,
): PropertyDescriptor {
  const original = Object.getOwnPropertyDescriptor(native, name);
  return {
    configurable: original?.configurable ?? true,
    enumerable: original?.enumerable ?? true,
    value,
    writable: original !== undefined && "writable" in original
      ? original.writable
      : false,
  };
}

/**
 * Hide every raw N-API V2 byte boundary behind an owned JavaScript snapshot.
 *
 * A Node Buffer may alias SharedArrayBuffer storage that another Worker can
 * mutate. N-API exposes that storage to Rust as an ordinary borrowed slice,
 * for which concurrent mutation would be undefined behaviour. The public
 * loader therefore returns a facade whose byte-bearing V2 calls take bounded
 * copies before entering the addon. The addon independently rejects shared
 * backing storage so direct artifact loads remain safe; the facade preserves
 * Uint8Array convenience and reflective property access without weakening
 * that native boundary.
 */
function protectNativeV2ByteInputs(native: LoadedNativeModule): LoadedNativeModule {
  const maxCanonicalBytes = native.MAX_CANONICAL_BYTES;
  const maxEnvelopeBytes = native.MAX_REMOTE_ENVELOPE_BYTES;
  const planAuthority = native.planAuthority.bind(native);
  const planRemoteModelContextWithResources =
    native.planRemoteModelContextWithResources.bind(native);
  const planPrepareRemoteModelRows =
    native.planPrepareRemoteModelRows.bind(native);
  const planPrepareRemoteModelPage =
    native.planPrepareRemoteModelPage.bind(native);
  const planPrepareRemoteModelCount =
    native.planPrepareRemoteModelCount.bind(native);
  const planPrepareRemoteModelExists =
    native.planPrepareRemoteModelExists.bind(native);
  const planPrepareRemoteModelReduce =
    native.planPrepareRemoteModelReduce.bind(native);
  const planPrepareRemoteModelReduceByField =
    native.planPrepareRemoteModelReduceByField.bind(native);
  const planPrepareRemoteModelReduceByFields =
    native.planPrepareRemoteModelReduceByFields.bind(native);
  const descriptors = Object.getOwnPropertyDescriptors(native);

  descriptors["planAuthority"] = protectedMethodDescriptor(
    native,
    "planAuthority",
    ((declaredSchema: Uint8Array, scope: string, profile: string) =>
      planAuthority(
        ownedByteSnapshot(declaredSchema, maxCanonicalBytes),
        scope,
        profile,
      )) as (
      ...args: never[]
    ) => unknown,
  );
  descriptors["planRemoteModelContextWithResources"] = protectedMethodDescriptor(
    native,
    "planRemoteModelContextWithResources",
    ((
      authority: Parameters<LoadedNativeModule["planRemoteModelContextWithResources"]>[0],
      advertisement: Uint8Array,
      resources: NativeQueryExecutionResources,
      cancellation: NativeQueryCancellation,
      answerLimits?: NativeQueryExecutionResources,
    ) => planRemoteModelContextWithResources(
      authority,
      ownedByteSnapshot(advertisement, maxEnvelopeBytes),
      resources,
      cancellation,
      answerLimits,
    )) as (...args: never[]) => unknown,
  );
  descriptors["planPrepareRemoteModelRows"] = protectedMethodDescriptor(
    native,
    "planPrepareRemoteModelRows",
    ((...args: Parameters<LoadedNativeModule["planPrepareRemoteModelRows"]>) =>
      protectedPendingRemoteModelQuery(
        planPrepareRemoteModelRows(...args),
        maxEnvelopeBytes,
      )) as (...args: never[]) => unknown,
  );
  descriptors["planPrepareRemoteModelPage"] = protectedMethodDescriptor(
    native,
    "planPrepareRemoteModelPage",
    ((...args: Parameters<LoadedNativeModule["planPrepareRemoteModelPage"]>) =>
      protectedPendingRemoteModelQuery(
        planPrepareRemoteModelPage(...args),
        maxEnvelopeBytes,
      )) as (...args: never[]) => unknown,
  );
  descriptors["planPrepareRemoteModelCount"] = protectedMethodDescriptor(
    native,
    "planPrepareRemoteModelCount",
    ((...args: Parameters<LoadedNativeModule["planPrepareRemoteModelCount"]>) =>
      protectedPendingRemoteModelQuery(
        planPrepareRemoteModelCount(...args),
        maxEnvelopeBytes,
      )) as (...args: never[]) => unknown,
  );
  descriptors["planPrepareRemoteModelExists"] = protectedMethodDescriptor(
    native,
    "planPrepareRemoteModelExists",
    ((...args: Parameters<LoadedNativeModule["planPrepareRemoteModelExists"]>) =>
      protectedPendingRemoteModelQuery(
        planPrepareRemoteModelExists(...args),
        maxEnvelopeBytes,
      )) as (...args: never[]) => unknown,
  );
  descriptors["planPrepareRemoteModelReduce"] = protectedMethodDescriptor(
    native,
    "planPrepareRemoteModelReduce",
    ((...args: Parameters<LoadedNativeModule["planPrepareRemoteModelReduce"]>) =>
      protectedPendingRemoteModelQuery(
        planPrepareRemoteModelReduce(...args),
        maxEnvelopeBytes,
      )) as (...args: never[]) => unknown,
  );
  descriptors["planPrepareRemoteModelReduceByField"] = protectedMethodDescriptor(
    native,
    "planPrepareRemoteModelReduceByField",
    ((...args: Parameters<LoadedNativeModule["planPrepareRemoteModelReduceByField"]>) =>
      protectedPendingRemoteModelQuery(
        planPrepareRemoteModelReduceByField(...args),
        maxEnvelopeBytes,
      )) as (...args: never[]) => unknown,
  );
  descriptors["planPrepareRemoteModelReduceByFields"] = protectedMethodDescriptor(
    native,
    "planPrepareRemoteModelReduceByFields",
    ((...args: Parameters<LoadedNativeModule["planPrepareRemoteModelReduceByFields"]>) =>
      protectedPendingRemoteModelQuery(
        planPrepareRemoteModelReduceByFields(...args),
        maxEnvelopeBytes,
      )) as (...args: never[]) => unknown,
  );

  return Object.create(Object.getPrototypeOf(native), descriptors) as LoadedNativeModule;
}

/**
 * Returns the platform triple used in the built .node filename, or null when
 * the current platform has no recognised triple.
 */
function platformTriple(): string | null {
  const arch = process.arch;
  switch (process.platform) {
    case "darwin":
      return arch === "arm64" ? "darwin-arm64" : "darwin-x64";
    case "linux":
      if (arch === "arm64") {
        return "linux-arm64-gnu";
      }
      return arch === "x64" ? "linux-x64-gnu" : null;
    case "win32":
      return arch === "arm64" ? "win32-arm64-msvc" : "win32-x64-msvc";
    default:
      return null;
  }
}

/**
 * Returns the ordered list of absolute candidate paths to probe for the .node
 * artifact. Platform-triple-specific names are tried first; generic fallbacks
 * follow. Both the package root (dist/..) and dist/ itself are probed so that
 * the loader works whether the artifact sits beside package.json or beside the
 * compiled output.
 */
function nativeCandidates(): string[] {
  const triple = platformTriple();
  const names: string[] = [];

  if (triple) {
    names.push(
      `type_bridge_node.${triple}.node`,
      `type-bridge-node.${triple}.node`,
    );
  }

  names.push(
    "type_bridge_node.node",
    "type-bridge-node.node",
    "index.node",
  );

  // Primary: package root (dist/..) — where build-native.js places the artifact.
  // Secondary: dist/ itself — robustness fallback for atypical build layouts.
  const packageRoot = path.join(__dirname, "..");
  const candidates: string[] = [];
  for (const name of names) {
    candidates.push(path.join(packageRoot, name));
  }
  for (const name of names) {
    candidates.push(path.join(__dirname, name));
  }
  return candidates;
}

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
export function loadNative(): LoadedNativeModule {
  if (_cached !== null) {
    return _cached;
  }

  const explicitPath = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  const candidates: string[] = explicitPath ? [explicitPath] : [];
  candidates.push(...nativeCandidates());

  const tried: string[] = [];
  for (const candidate of candidates) {
    tried.push(candidate);
    if (fs.existsSync(candidate)) {
      // eslint-disable-next-line @typescript-eslint/no-require-imports
      const native = require(candidate) as LoadedNativeModule;
      _cached = protectNativeV2ByteInputs(native);
      return _cached;
    }
  }

  throw new Error(
    [
      "Unable to load the type-bridge native Node module.",
      "Run `npm run build:native`, or set TYPE_BRIDGE_NODE_NATIVE_PATH to the built .node artifact.",
      `Tried: ${tried.join(", ")}`,
    ].join(" "),
  );
}
