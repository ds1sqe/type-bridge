import { loadNative } from "./native.js";
import { type NativeQueryV2Authority, type NativeQueryV2BuilderRuntime } from "./query-v2-internals.js";
export { QueryV2Error } from "./query-v2-internals.js";
export type { QueryV2ErrorCategory, QueryV2ErrorDetail, QueryV2ErrorPathSegment, } from "./query-v2-internals.js";
export type ValueType = "string" | "long" | "double" | "boolean" | "date" | "datetime" | "datetime-tz" | "decimal" | "duration";
export type Annotation = "Key" | "Unique" | "Distinct" | {
    Card: [number, number | null];
};
export interface OwnedAttributeDescriptor {
    field_name: string;
    attr_name: string;
    value_type: ValueType;
    annotations: Annotation[];
    is_optional: boolean;
    /** Whether this ownership is declared as an ordered list (`owns name[]`).
     * Instance-level list semantics are engine-unimplemented (REP256); this is a
     * schema-emission marker only. */
    is_ordered: boolean;
    parent_type?: string | null;
    is_abstract?: boolean;
    is_independent?: boolean;
    regex?: string | null;
    allowed_values?: string[] | null;
    range?: [string | null, string | null] | null;
    /** TypeDB 3.12+ `@doc("...")` annotation. Omitted when not declared. */
    doc?: string;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, keyed by meta key. */
    meta?: Record<string, string>;
}
export interface EntityDescriptor {
    type_name: string;
    is_abstract: boolean;
    parent_type: string | null;
    owned_attributes: OwnedAttributeDescriptor[];
    /** TypeDB 3.12+ `@doc("...")` annotation. Omitted when not declared. */
    doc?: string;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, keyed by meta key. */
    meta?: Record<string, string>;
}
export interface RoleDescriptor {
    role_name: string;
    player_type_names: string[];
    cardinality: [number, number | null] | null;
    /** Plays-side cardinality for this role's players. Authoring datum consumed
     * by `SchemaInfo.from_descriptors` to build the per-player `plays_cardinalities`
     * overlay. `null` when no plays-side constraint is declared. */
    plays_cardinality: [number, number | null] | null;
    overrides: string | null;
    is_abstract: boolean;
    /** Whether this role is declared as an ordered list (`relates name[]`).
     * Instance-level list semantics are engine-unimplemented (REP256); this is a
     * schema-emission marker only. */
    ordered: boolean;
    /** Whether this role carries `@distinct`. Valid only when `ordered` is true. */
    distinct: boolean;
    /** TypeDB 3.12+ `@doc("...")` annotation. Omitted when not declared. */
    doc?: string;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, keyed by meta key. */
    meta?: Record<string, string>;
}
export interface RelationDescriptor {
    type_name: string;
    is_abstract: boolean;
    parent_type: string | null;
    owned_attributes: OwnedAttributeDescriptor[];
    roles: RoleDescriptor[];
    /** TypeDB 3.12+ `@doc("...")` annotation. Omitted when not declared. */
    doc?: string;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, keyed by meta key. */
    meta?: Record<string, string>;
}
export type TypeDescriptor = {
    kind: "entity";
    descriptor: EntityDescriptor;
} | {
    kind: "relation";
    descriptor: RelationDescriptor;
};
export interface OwnedAttributeEntry {
    attr_name: string;
    value_type: ValueType;
    annotations: Annotation[];
    /** Whether this ownership is declared as an ordered list (`owns name[]`).
     * Instance-level list semantics are engine-unimplemented (REP256); this is a
     * schema-emission marker only. */
    is_ordered: boolean;
    /** TypeDB 3.12+ `@doc("...")` annotation. Omitted when not declared. */
    doc?: string;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, keyed by meta key. */
    meta?: Record<string, string>;
}
export interface RoleEntry {
    role_name: string;
    player_type_names: string[];
    cardinality: [number, number | null] | null;
    /** Plays-side cardinality authoring datum. Mirrors `RoleDescriptor.plays_cardinality`
     * at the SchemaInfo (info-level) role entry. `null` when not declared. */
    plays_cardinality: [number, number | null] | null;
    overrides: string | null;
    is_abstract: boolean;
    /** Whether this role is declared as an ordered list (`relates name[]`).
     * Instance-level list semantics are engine-unimplemented (REP256); this is a
     * schema-emission marker only. */
    ordered: boolean;
    /** Whether this role carries `@distinct`. Valid only when `ordered` is true. */
    distinct: boolean;
    /** TypeDB 3.12+ `@doc("...")` annotation. Omitted when not declared. */
    doc?: string;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, keyed by meta key. */
    meta?: Record<string, string>;
}
export interface EntitySchemaEntry {
    type_name: string;
    is_abstract: boolean;
    parent_type: string | null;
    owned_attributes: OwnedAttributeEntry[];
    plays_cardinalities?: Record<string, [number, number | null]>;
    /** TypeDB 3.12+ `@doc("...")` annotation. Omitted when not declared. */
    doc?: string;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, keyed by meta key. */
    meta?: Record<string, string>;
}
export interface RelationSchemaEntry extends EntitySchemaEntry {
    roles: RoleEntry[];
}
export interface AttributeSchemaEntry {
    attr_name: string;
    value_type: ValueType;
    parent_type?: string | null;
    is_abstract?: boolean;
    is_independent?: boolean;
    regex?: string | null;
    allowed_values?: string[] | null;
    range?: [string | null, string | null] | null;
    /** TypeDB 3.12+ `@doc("...")` annotation. Omitted when not declared. */
    doc?: string;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, keyed by meta key. */
    meta?: Record<string, string>;
}
export interface SchemaInfo {
    entities: Record<string, EntitySchemaEntry>;
    relations: Record<string, RelationSchemaEntry>;
    attributes: Record<string, AttributeSchemaEntry>;
}
export type TransactionType = "read" | "write" | "schema";
export type AttributeValue = {
    value_type: "string";
    value: string;
} | {
    value_type: "long";
    value: string;
} | {
    value_type: "double";
    value: number;
} | {
    value_type: "boolean";
    value: boolean;
} | {
    value_type: "date";
    value: string;
} | {
    value_type: "datetime";
    value: string;
} | {
    value_type: "datetime-tz";
    value: string;
} | {
    value_type: "decimal";
    value: string;
} | {
    value_type: "duration";
    value: string;
};
export type AttributeInput = Record<string, AttributeValue | AttributeValue[] | null | undefined>;
export interface FilterInput {
    attr_name: string;
    operator?: string;
    value: AttributeValue;
}
export interface AggregateInput {
    result_key: string;
    function: string;
    attr_name?: string | null;
}
export interface RolePlayerInput {
    role_name: string;
    /**
     * The concrete type of the player. The binding validates only that the role
     * exists; whether this type may actually play the role (including via
     * subtyping of an abstract declared player type) is enforced by TypeDB at
     * insert time, so an incompatible type surfaces as a TypeDB error, not a
     * binding error.
     */
    player_type_name: string;
    iid?: string | null;
    key_attr?: string | null;
    key_value?: AttributeValue | null;
}
export interface RelationWriteInput {
    attributes?: AttributeInput | null;
    role_players: RolePlayerInput[];
}
export type RuntimeAttributeValue = {
    String: string;
} | {
    Long: string;
} | {
    Double: number;
} | {
    Boolean: boolean;
} | {
    Date: string;
} | {
    DateTime: string;
} | {
    DateTimeTZ: string;
} | {
    Decimal: string;
} | {
    Duration: string;
};
export { Attribute, attr, type AttributeBase, type AttributeTypeOptions, type AttributeTypeParent, type ComparableAttributeBase, type NumericAttributeBase, type StringAttributeBase, } from "./attribute.js";
export { AggregateSpec, BooleanExpr, ComparisonExpr, NotExpr, QueryExpr, SortExpr, TypedGroupByQuery, TypedQuery, TypedQueryError, agg, } from "./query.js";
export { AttributeFlags, Card, Doc, Flag, Key, Meta, TypeFlags, TypeNameCase, Unique, formatTypeName, resolveFlags, type AttributeFlagsOptions, type CardSpec, type DocSpec, type FlagInput, type FlagSpec, type MetaSpec, type ResolvedAttributeFlags, type ResolvedTypeFlags, type TypeFlagsOptions, } from "./flags.js";
export { Entity, FieldSpec, ListFieldSpec, Relation, RoleSpec, field, role, type AttributeClass, type EntitySchema, type FieldValue, type IidBearing, type InstanceDict, type InstanceFields, type MergedSchema, type ModelClass, type ModelInstance, type ParentModelClass, type ParentOption, type PlainFieldValue, type RelationSchema, type SchemaSpec, } from "./model.js";
export { TypedCodecError, attributeToPlain, hydrateAttributeEntries, hydrateAttributes, keyAttributeDescriptor, lowerAttributes, lowerAttributeValue, lowerFilters, plainToAttribute, runtimeAttributeValueFromUnknown, } from "./codec.js";
export { TypedEntityManager, TypedRelationManager, buildRolePlayers, entityManagerFor, relationManagerFor, type ExactFilters, type ManagerConnection, } from "./manager.js";
export { parseSchema, type SchemaParserNative, type AttributeType as SchemaAttributeType, type Cardinality as SchemaCardinality, type EntityType as SchemaEntityType, type FunctionType as SchemaFunctionType, type OwnedAttribute as SchemaOwnedAttribute, type Parameter as SchemaParameter, type PlayedRole as SchemaPlayedRole, type RelationType as SchemaRelationType, type ReturnType as SchemaReturnType, type ReturnTypeItem as SchemaReturnTypeItem, type RoleSpec as SchemaRoleSpec, type StructField as SchemaStructField, type StructType as SchemaStructType, type TypeSchema, } from "./parser.js";
export interface DynamicEntityRow {
    iid: string | null;
    type_name: string | null;
    attributes: [string, RuntimeAttributeValue][];
}
export interface DynamicRolePlayer {
    role_name: string;
    player_iid: string | null;
    player_type_name: string | null;
    attributes: [string, unknown][];
}
export interface DynamicRelationRow extends DynamicEntityRow {
    role_players: DynamicRolePlayer[];
}
/**
 * Wire shape of the Rust `DynamicComparisonOp`. `starts_with`/`ends_with` carry
 * the raw literal only — Rust owns regex anchoring and escaping.
 */
export type DynamicComparisonOp = "eq" | "neq" | "gt" | "gte" | "lt" | "lte" | "contains" | "like" | "starts_with" | "ends_with";
/**
 * Wire shape of the Rust `DynamicExpr` expression tree. Comparison values use the
 * same precision-safe {@link AttributeValue} `{ value_type, value }` encoding as
 * CRUD filters (`long` carried as a string) — the Node binding decodes them
 * through its shared value convention, so `long` keeps full i64 precision rather
 * than being capped at the JS safe-integer range.
 */
export type DynamicExpr = {
    kind: "compare";
    attr_name: string;
    operator: DynamicComparisonOp;
    value: AttributeValue;
} | {
    kind: "iid";
    iid: string;
} | {
    kind: "is_null";
    attr_name: string;
    is_null: boolean;
} | {
    kind: "and";
    exprs: DynamicExpr[];
} | {
    kind: "or";
    exprs: DynamicExpr[];
} | {
    kind: "not";
    expr: DynamicExpr;
} | {
    kind: "role_player";
    role_name: string;
    expr: DynamicExpr;
};
/** Wire shape of the Rust `SortDir` (bare PascalCase variant names). */
export type DynamicSortDir = "Asc" | "Desc";
/** Wire shape of the Rust `DynamicSort`. */
export type DynamicSort = {
    kind: "attribute";
    attr_name: string;
    direction: DynamicSortDir;
} | {
    kind: "role_player_attribute";
    role_name: string;
    attr_name: string;
    direction: DynamicSortDir;
};
/**
 * Wire shape of the Rust `DynamicQuerySpecJson`. All fields are optional; an empty
 * `expr` matches every row. `limit`/`offset` apply only to `query`, not to
 * `queryCount`/`queryAggregate`/`queryGroupByAggregate`.
 */
export interface DynamicQuerySpec {
    expr?: DynamicExpr[];
    sort?: DynamicSort[];
    limit?: number | null;
    offset?: number | null;
}
export declare function string(value: string): AttributeValue;
export declare function long(value: bigint): AttributeValue;
export declare function longFromNumberUnsafe(value: number): AttributeValue;
export declare function double(value: number): AttributeValue;
export declare function boolean(value: boolean): AttributeValue;
export declare function date(value: string): AttributeValue;
export declare function datetime(value: string): AttributeValue;
export declare function datetimetz(value: string): AttributeValue;
export declare function decimal(value: string): AttributeValue;
export declare function duration(value: string): AttributeValue;
interface NativeDescriptorRegistry {
    registerEntityJson(descriptorJson: string): string;
    registerRelationJson(descriptorJson: string): string;
    entityJson(typeName: string): string;
    relationJson(typeName: string): string;
    snapshotJson(): string;
    schemaInfoJson(): string;
}
interface NativeMarshalling {
    normalizeAttributeValueJson(valueJson: string): string;
    normalizeEntityAttributesJson(descriptorJson: string, attributesJson: string): string;
    normalizeRelationAttributesJson(descriptorJson: string, attributesJson: string): string;
    normalizeFiltersJson(descriptorJson: string, filtersJson: string): string;
    normalizeRelationFiltersJson(descriptorJson: string, filtersJson: string): string;
    normalizeAggregatesJson(descriptorJson: string, aggregatesJson: string): string;
    normalizeRolePlayersJson(descriptorJson: string, rolePlayersJson: string): string;
    normalizeRelationWriteBatchJson(descriptorJson: string, batchJson: string): string;
}
export interface NativeRustDatabase {
    isConnected(): boolean;
    serverDeprecationNotice?(): TypeDBServerDeprecationNotice | null;
    close(): void;
    databaseName(): string;
    databaseExists(): boolean;
    createDatabase(): void;
    deleteDatabase(): void;
    resetDatabase(): void;
    transaction(transactionType?: TransactionType): NativeRustTransactionContext;
    entityManagerJson(descriptorJson: string): NativeDynamicEntityManager;
    relationManagerJson(descriptorJson: string): NativeDynamicRelationManager;
}
/** Structured metadata for one legacy TypeDB server warning. */
export interface TypeDBServerDeprecationNotice {
    readonly code: typeof TYPE_DB_SERVER_DEPRECATION_CODE;
    readonly message: string;
}
/** TypeBridge-specific machine-readable identity for the server notice. */
export declare const TYPE_DB_SERVER_DEPRECATION_CODE = "TYPE_BRIDGE_TYPEDB_LEGACY_SERVER";
/** Standard Node warning type used so `--no-deprecation` remains effective.
 * Inspect {@link TYPE_DB_SERVER_DEPRECATION_CODE} for the TypeBridge-specific
 * machine-readable identity.
 */
export declare const TYPE_DB_SERVER_DEPRECATION_WARNING = "DeprecationWarning";
export interface NativeRustTransactionContext {
    queryJson(query: string): string;
    commit(): void;
    rollback(): void;
    close(): void;
    transactionType(): TransactionType;
    entityManagerJson(descriptorJson: string): NativeDynamicEntityManager;
    relationManagerJson(descriptorJson: string): NativeDynamicRelationManager;
}
interface NativePendingQueryV2Remote {
    requestBytes(): Uint8Array;
    decodeReply(response: Uint8Array): Promise<string>;
}
/** Opaque one-shot decoder for one exact prepared V2 remote request. */
export interface PendingQueryV2Remote {
    /** Exact canonical bytes to send to the executor's `/v2/query` route. */
    requestBytes(): Uint8Array;
    /** Atomically consume and decode the only accepted request-bound reply off-thread. */
    decodeReply(response: Uint8Array): Promise<string>;
}
interface NativeQueryV2Runtime {
    queryV2Authority(declaredSchema: Uint8Array, scope: string, profile: string): NativeQueryV2Authority;
    queryV2QueryOnlyAuthority(database: NativeRustDatabase, declaredSchema: Uint8Array, scope: string, profile: string): NativeQueryV2Authority;
    queryV2ExecuteLocal(database: NativeRustDatabase, authority: NativeQueryV2Authority, plan: Uint8Array, invocationJson: string, deadlineMs?: bigint | null): Promise<string>;
    queryV2RemoteCapabilities(advertisement: Uint8Array): string[];
    queryV2PrepareRemote(authority: NativeQueryV2Authority, plan: Uint8Array, invocationJson: string, advertisement: Uint8Array, maxItems: bigint, maxBytes: bigint, maxCollectionMembers: bigint, deadlineMs?: bigint | null): NativePendingQueryV2Remote;
}
export interface NativeDynamicEntityManager {
    insertJson(attributesJson: string): string;
    insertManyJson(batchJson: string): string;
    putJson(attributesJson: string): string;
    putManyJson(batchJson: string): string;
    updateJson(attributesJson: string, iid?: string | null): void;
    getJson(filtersJson?: string | null): string;
    getByIidJson(iid: string): string;
    allJson(): string;
    countJson(filtersJson?: string | null): string;
    aggregateJson(aggregatesJson: string, filtersJson?: string | null): string;
    groupByAggregateJson(groupFieldsJson: string, aggregatesJson: string, filtersJson?: string | null): string;
    deleteByIid(iid: string): void;
    queryJson(specJson: string): string;
    queryCountJson(specJson: string): string;
    queryAggregateJson(specJson: string, aggregatesJson: string): string;
    queryGroupByAggregateJson(specJson: string, groupFieldsJson: string, aggregatesJson: string): string;
}
export interface NativeDynamicRelationManager {
    insertJson(attributesJson: string, rolePlayersJson: string): string;
    insertManyJson(batchJson: string): string;
    putJson(attributesJson: string, rolePlayersJson: string): string;
    putManyJson(batchJson: string): string;
    updateJson(attributesJson: string, rolePlayersJson: string, iid?: string | null): void;
    getJson(filtersJson?: string | null): string;
    getWithRolePlayersJson(filtersJson?: string | null, rolePlayersJson?: string | null): string;
    getByIidJson(iid: string): string;
    allJson(): string;
    countJson(filtersJson?: string | null): string;
    aggregateJson(aggregatesJson: string, filtersJson?: string | null): string;
    groupByAggregateJson(groupFieldsJson: string, aggregatesJson: string, filtersJson?: string | null): string;
    deleteByIid(iid: string): void;
    queryJson(specJson: string): string;
    queryCountJson(specJson: string): string;
    queryAggregateJson(specJson: string, aggregatesJson: string): string;
    queryGroupByAggregateJson(specJson: string, groupFieldsJson: string, aggregatesJson: string): string;
}
export interface NativeRuntime {
    ensureRustDatabase(address: string, database: string, username?: string | null, password?: string | null, httpPort?: number | null, serverVersion?: string | null, tlsEnabled?: boolean | null, tlsRootCa?: string | null): void;
    connectRustDatabase(address: string, database: string, username?: string | null, password?: string | null, httpPort?: number | null, serverVersion?: string | null, tlsEnabled?: boolean | null, tlsRootCa?: string | null): NativeRustDatabase;
}
interface NativeSchemaParser {
    parseSchemaJson(input: string): string;
    renderModelsJson(input: string, target: string, optionsJson?: string | null): string;
}
export interface NativeModule extends NativeRuntime, NativeMarshalling, NativeSchemaParser, NativeQueryV2Runtime, NativeQueryV2BuilderRuntime {
    readonly TYPE_DB_SERVER_DEPRECATION_CODE: string;
    NodeDescriptorRegistry: new () => NativeDescriptorRegistry;
    generateDefineBlockJson(schemaInfoJson: string): string;
}
export interface RustDatabaseConnectOptions {
    username?: string | null;
    password?: string | null;
    /** Port of the TypeDB HTTP API used for the connect-time version probe. */
    httpPort?: number;
    /** Exact TypeDB server version; skips HTTP probing for gRPC-only deployments. */
    serverVersion?: string | null;
    /** Enable TLS using native trust roots, or a custom root when tlsRootCa is set. */
    tlsEnabled?: boolean;
    /** PEM root-CA path. Requires an explicit tlsEnabled: true. */
    tlsRootCa?: string;
}
export interface EnsureDatabaseOptions {
    username?: string | null;
    password?: string | null;
    /** Port of the TypeDB HTTP API used for the connect-time version probe. */
    httpPort?: number;
    /** Exact TypeDB server version; skips HTTP probing for gRPC-only deployments. */
    serverVersion?: string | null;
    /** Enable TLS using native trust roots, or a custom root when tlsRootCa is set. */
    tlsEnabled?: boolean;
    /** PEM root-CA path. Requires an explicit tlsEnabled: true. */
    tlsRootCa?: string;
}
/**
 * Ensure the named TypeDB database exists, creating it if absent.
 *
 * Fails hard when TypeDB is unreachable — callers should let the error
 * propagate so that a missing server shows up as a clear failure, not a
 * silent skip.
 */
export declare function ensureDatabase(address: string, database: string, options?: EnsureDatabaseOptions): void;
export { loadNative };
/** Opaque declared-schema authority for prepared V2 plan execution. */
export declare class QueryV2Authority {
    #private;
    constructor(declaredSchema: Uint8Array, scope: string, profile: string);
    /** Build a local-only authority for an exact database with no migration controls. */
    static queryOnly(database: RustDatabase, declaredSchema: Uint8Array, scope: string, profile: string): QueryV2Authority;
}
/** Caller ceilings bound into one prepared V2 remote request.
 * `deadlineMs` is resolved once into an absolute expiry (30 seconds by
 * default, maximum five minutes).
 */
export interface QueryV2RemoteLimits {
    readonly maxItems: bigint;
    /** Maximum signed bytes for a successful typed response. Authenticated
     * failure envelopes use the protocol hard ceiling so their diagnostic is
     * still available when this success budget is zero or otherwise tiny.
     */
    readonly maxBytes: bigint;
    readonly maxCollectionMembers: bigint;
    readonly deadlineMs?: bigint | null;
}
/** Execute canonical prepared-plan bytes against a local Rust database. */
export declare function queryV2ExecuteLocal(database: RustDatabase, authority: QueryV2Authority, plan: Uint8Array, invocationJson: string, deadlineMs?: bigint | null): Promise<string>;
/** Decode the executor's exact prepared-query capability advertisement. */
export declare function queryV2RemoteCapabilities(advertisement: Uint8Array): readonly string[];
/** Prepare one request bound to the exact advertised executor epoch and expiry. */
export declare function queryV2PrepareRemote(authority: QueryV2Authority, plan: Uint8Array, invocationJson: string, advertisement: Uint8Array, limits: QueryV2RemoteLimits): PendingQueryV2Remote;
export declare function generateDefineBlock(info: SchemaInfo): string;
export declare class DescriptorRegistry {
    #private;
    constructor(nativeRegistry?: NativeDescriptorRegistry | null);
    registerEntity(descriptor: EntityDescriptor): EntityDescriptor;
    registerRelation(descriptor: RelationDescriptor): RelationDescriptor;
    entity(typeName: string): EntityDescriptor;
    relation(typeName: string): RelationDescriptor;
    snapshot(): TypeDescriptor[];
    schemaInfo(): SchemaInfo;
}
export declare class Marshalling {
    #private;
    constructor(nativeMarshalling?: NativeMarshalling | null);
    attributeValue(value: AttributeValue): unknown;
    entityAttributes(descriptor: EntityDescriptor, attributes: AttributeInput): unknown;
    relationAttributes(descriptor: RelationDescriptor, attributes: AttributeInput): unknown;
    filters(descriptor: EntityDescriptor, filters: Record<string, AttributeValue> | FilterInput[]): unknown;
    relationFilters(descriptor: RelationDescriptor, filters: Record<string, AttributeValue> | FilterInput[]): unknown;
    aggregates(descriptor: EntityDescriptor, aggregates: AggregateInput[]): unknown;
    rolePlayers(descriptor: RelationDescriptor, rolePlayers: RolePlayerInput[]): unknown;
    relationWriteBatch(descriptor: RelationDescriptor, batch: RelationWriteInput[]): unknown;
}
export declare class RustDatabase {
    #private;
    private constructor();
    static connect(address: string, database: string, options?: RustDatabaseConnectOptions): RustDatabase;
    static connect(native: NativeRuntime, address: string, database: string, options?: RustDatabaseConnectOptions): RustDatabase;
    isConnected(): boolean;
    close(): void;
    databaseName(): string;
    databaseExists(): boolean;
    createDatabase(): void;
    deleteDatabase(): void;
    resetDatabase(): void;
    transaction(transactionType?: TransactionType): RustTransactionContext;
    entityManager(descriptor: EntityDescriptor): RustDynamicEntityManager;
    relationManager(descriptor: RelationDescriptor): RustDynamicRelationManager;
}
export declare class RustTransactionContext {
    #private;
    constructor(native: NativeRustTransactionContext);
    query(query: string): unknown[];
    commit(): void;
    rollback(): void;
    close(): void;
    transactionType(): TransactionType;
    entityManager(descriptor: EntityDescriptor): RustDynamicEntityManager;
    relationManager(descriptor: RelationDescriptor): RustDynamicRelationManager;
}
export declare class RustDynamicEntityManager {
    #private;
    constructor(native: NativeDynamicEntityManager);
    insert(attributes: AttributeInput): string;
    insertMany(batch: AttributeInput[]): string[];
    put(attributes: AttributeInput): string;
    putMany(batch: AttributeInput[]): string[];
    update(attributes: AttributeInput, iid?: string | null): void;
    get(filters?: Record<string, AttributeValue> | FilterInput[] | null): DynamicEntityRow[];
    getByIid(iid: string): DynamicEntityRow | null;
    all(): DynamicEntityRow[];
    count(filters?: Record<string, AttributeValue> | FilterInput[] | null): bigint;
    aggregate(aggregates: AggregateInput[], filters?: Record<string, AttributeValue> | FilterInput[] | null): unknown[];
    groupByAggregate(groupFields: string[], aggregates: AggregateInput[], filters?: Record<string, AttributeValue> | FilterInput[] | null): unknown[];
    query(spec: DynamicQuerySpec): DynamicEntityRow[];
    queryCount(spec: DynamicQuerySpec): bigint;
    queryAggregate(spec: DynamicQuerySpec, aggregates: AggregateInput[]): unknown[];
    queryGroupByAggregate(spec: DynamicQuerySpec, groupFields: string[], aggregates: AggregateInput[]): unknown[];
    deleteByIid(iid: string): void;
}
export declare class RustDynamicRelationManager {
    #private;
    constructor(native: NativeDynamicRelationManager);
    insert(attributes: AttributeInput, rolePlayers: RolePlayerInput[]): string;
    insertMany(batch: RelationWriteInput[]): string[];
    put(attributes: AttributeInput, rolePlayers: RolePlayerInput[]): string;
    putMany(batch: RelationWriteInput[]): string[];
    update(attributes: AttributeInput, rolePlayers: RolePlayerInput[], iid?: string | null): void;
    get(filters?: Record<string, AttributeValue> | FilterInput[] | null): DynamicRelationRow[];
    getWithRolePlayers(filters?: Record<string, AttributeValue> | FilterInput[] | null, rolePlayers?: RolePlayerInput[] | null): DynamicRelationRow[];
    getByIid(iid: string): DynamicRelationRow[];
    all(): DynamicRelationRow[];
    count(filters?: Record<string, AttributeValue> | FilterInput[] | null): bigint;
    aggregate(aggregates: AggregateInput[], filters?: Record<string, AttributeValue> | FilterInput[] | null): unknown[];
    groupByAggregate(groupFields: string[], aggregates: AggregateInput[], filters?: Record<string, AttributeValue> | FilterInput[] | null): unknown[];
    query(spec: DynamicQuerySpec): DynamicRelationRow[];
    queryCount(spec: DynamicQuerySpec): bigint;
    queryAggregate(spec: DynamicQuerySpec, aggregates: AggregateInput[]): unknown[];
    queryGroupByAggregate(spec: DynamicQuerySpec, groupFields: string[], aggregates: AggregateInput[]): unknown[];
    deleteByIid(iid: string): void;
}
export { generateModels, generateModelsForTarget, type BindgenRenderOptions, type BindgenTarget, type GenerateModelsOptions, type GenerateTargetModelsOptions, type NamingOptions, } from "./generator/index.js";
