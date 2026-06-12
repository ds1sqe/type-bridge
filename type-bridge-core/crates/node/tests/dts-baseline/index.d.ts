import { loadNative } from "./native.js";
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
    is_ordered: boolean;
    parent_type?: string | null;
    is_abstract?: boolean;
    is_independent?: boolean;
    regex?: string | null;
    allowed_values?: string[] | null;
    range?: [string | null, string | null] | null;
}
export interface EntityDescriptor {
    type_name: string;
    is_abstract: boolean;
    parent_type: string | null;
    owned_attributes: OwnedAttributeDescriptor[];
}
export interface RoleDescriptor {
    role_name: string;
    player_type_names: string[];
    cardinality: [number, number | null] | null;
    overrides: string | null;
    is_abstract: boolean;
    ordered: boolean;
    distinct: boolean;
}
export interface RelationDescriptor {
    type_name: string;
    is_abstract: boolean;
    parent_type: string | null;
    owned_attributes: OwnedAttributeDescriptor[];
    roles: RoleDescriptor[];
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
    is_ordered: boolean;
}
export interface RoleEntry {
    role_name: string;
    player_type_names: string[];
    cardinality: [number, number | null] | null;
    overrides: string | null;
    is_abstract: boolean;
    ordered: boolean;
    distinct: boolean;
}
export interface EntitySchemaEntry {
    type_name: string;
    is_abstract: boolean;
    parent_type: string | null;
    owned_attributes: OwnedAttributeEntry[];
    plays_cardinalities?: Record<string, [number, number | null]>;
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
export { AttributeFlags, Card, Flag, Key, TypeFlags, TypeNameCase, Unique, formatTypeName, resolveFlags, type AttributeFlagsOptions, type CardSpec, type FlagInput, type FlagSpec, type ResolvedAttributeFlags, type ResolvedTypeFlags, type TypeFlagsOptions, } from "./flags.js";
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
    databaseName(): string;
    transaction(transactionType?: TransactionType): NativeRustTransactionContext;
    entityManagerJson(descriptorJson: string): NativeDynamicEntityManager;
    relationManagerJson(descriptorJson: string): NativeDynamicRelationManager;
}
export interface NativeRustTransactionContext {
    queryJson(query: string): string;
    commit(): void;
    rollback(): void;
    close(): void;
    transactionType(): TransactionType;
    entityManagerJson(descriptorJson: string): NativeDynamicEntityManager;
    relationManagerJson(descriptorJson: string): NativeDynamicRelationManager;
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
    ensureRustDatabase(address: string, database: string, username?: string | null, password?: string | null, httpPort?: number | null): void;
    connectRustDatabase(address: string, database: string, username?: string | null, password?: string | null, httpPort?: number | null): NativeRustDatabase;
}
interface NativeSchemaParser {
    parseSchemaJson(input: string): string;
}
export interface NativeModule extends NativeRuntime, NativeMarshalling, NativeSchemaParser {
    NodeDescriptorRegistry: new () => NativeDescriptorRegistry;
    generateDefineBlockJson(schemaInfoJson: string): string;
}
export interface RustDatabaseConnectOptions {
    username?: string | null;
    password?: string | null;
    /** Port of the TypeDB HTTP API used for the connect-time version probe. */
    httpPort?: number;
}
export interface EnsureDatabaseOptions {
    username?: string | null;
    password?: string | null;
    /** Port of the TypeDB HTTP API used for the connect-time version probe. */
    httpPort?: number;
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
    databaseName(): string;
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
export { generateModels, type GenerateModelsOptions, type NamingOptions, } from "./generator/index.js";
