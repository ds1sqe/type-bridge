import { loadNative } from "./native.js";

export type ValueType =
  | "string"
  | "long"
  | "double"
  | "boolean"
  | "date"
  | "datetime"
  | "datetime-tz"
  | "decimal"
  | "duration";

export type Annotation = "Key" | "Unique" | "Distinct" | { Card: [number, number | null] };

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
}

export interface RelationDescriptor {
  type_name: string;
  is_abstract: boolean;
  parent_type: string | null;
  owned_attributes: OwnedAttributeDescriptor[];
  roles: RoleDescriptor[];
}

export type TypeDescriptor =
  | { kind: "entity"; descriptor: EntityDescriptor }
  | { kind: "relation"; descriptor: RelationDescriptor };

export interface OwnedAttributeEntry {
  attr_name: string;
  value_type: ValueType;
  annotations: Annotation[];
  /** Whether this ownership is declared as an ordered list (`owns name[]`).
   * Instance-level list semantics are engine-unimplemented (REP256); this is a
   * schema-emission marker only. */
  is_ordered: boolean;
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

const ATTRIBUTE_SCHEMA_METADATA = Symbol.for("@type-bridge/node.attributeSchemaMetadata");

export type TransactionType = "read" | "write" | "schema";

export type AttributeValue =
  | { value_type: "string"; value: string }
  | { value_type: "long"; value: string }
  | { value_type: "double"; value: number }
  | { value_type: "boolean"; value: boolean }
  | { value_type: "date"; value: string }
  | { value_type: "datetime"; value: string }
  | { value_type: "datetime-tz"; value: string }
  | { value_type: "decimal"; value: string }
  | { value_type: "duration"; value: string };

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

export type RuntimeAttributeValue =
  | { String: string }
  | { Long: string }
  | { Double: number }
  | { Boolean: boolean }
  | { Date: string }
  | { DateTime: string }
  | { DateTimeTZ: string }
  | { Decimal: string }
  | { Duration: string };

export {
  Attribute,
  attr,
  type AttributeBase,
  type AttributeTypeOptions,
  type AttributeTypeParent,
  type ComparableAttributeBase,
  type NumericAttributeBase,
  type StringAttributeBase,
} from "./attribute.js";
export {
  AggregateSpec,
  BooleanExpr,
  ComparisonExpr,
  NotExpr,
  QueryExpr,
  SortExpr,
  TypedGroupByQuery,
  TypedQuery,
  TypedQueryError,
  agg,
} from "./query.js";
export {
  AttributeFlags,
  Card,
  Flag,
  Key,
  TypeFlags,
  TypeNameCase,
  Unique,
  formatTypeName,
  resolveFlags,
  type AttributeFlagsOptions,
  type CardSpec,
  type FlagInput,
  type FlagSpec,
  type ResolvedAttributeFlags,
  type ResolvedTypeFlags,
  type TypeFlagsOptions,
} from "./flags.js";
export {
  Entity,
  FieldSpec,
  ListFieldSpec,
  Relation,
  RoleSpec,
  field,
  role,
  type AttributeClass,
  type EntitySchema,
  type FieldValue,
  type IidBearing,
  type InstanceDict,
  type InstanceFields,
  type MergedSchema,
  type ModelClass,
  type ModelInstance,
  type ParentModelClass,
  type ParentOption,
  type PlainFieldValue,
  type RelationSchema,
  type SchemaSpec,
} from "./model.js";
export {
  TypedCodecError,
  attributeToPlain,
  hydrateAttributeEntries,
  hydrateAttributes,
  keyAttributeDescriptor,
  lowerAttributes,
  lowerAttributeValue,
  lowerFilters,
  plainToAttribute,
  runtimeAttributeValueFromUnknown,
} from "./codec.js";
export {
  TypedEntityManager,
  TypedRelationManager,
  buildRolePlayers,
  entityManagerFor,
  relationManagerFor,
  type ExactFilters,
  type ManagerConnection,
} from "./manager.js";
export {
  parseSchema,
  type SchemaParserNative,
  type AttributeType as SchemaAttributeType,
  type Cardinality as SchemaCardinality,
  type EntityType as SchemaEntityType,
  type FunctionType as SchemaFunctionType,
  type OwnedAttribute as SchemaOwnedAttribute,
  type Parameter as SchemaParameter,
  type PlayedRole as SchemaPlayedRole,
  type RelationType as SchemaRelationType,
  type ReturnType as SchemaReturnType,
  type ReturnTypeItem as SchemaReturnTypeItem,
  type RoleSpec as SchemaRoleSpec,
  type StructField as SchemaStructField,
  type StructType as SchemaStructType,
  type TypeSchema,
} from "./parser.js";

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
export type DynamicComparisonOp =
  | "eq"
  | "neq"
  | "gt"
  | "gte"
  | "lt"
  | "lte"
  | "contains"
  | "like"
  | "starts_with"
  | "ends_with";

/**
 * Wire shape of the Rust `DynamicExpr` expression tree. Comparison values use the
 * same precision-safe {@link AttributeValue} `{ value_type, value }` encoding as
 * CRUD filters (`long` carried as a string) — the Node binding decodes them
 * through its shared value convention, so `long` keeps full i64 precision rather
 * than being capped at the JS safe-integer range.
 */
export type DynamicExpr =
  | { kind: "compare"; attr_name: string; operator: DynamicComparisonOp; value: AttributeValue }
  | { kind: "iid"; iid: string }
  | { kind: "is_null"; attr_name: string; is_null: boolean }
  | { kind: "and"; exprs: DynamicExpr[] }
  | { kind: "or"; exprs: DynamicExpr[] }
  | { kind: "not"; expr: DynamicExpr }
  | { kind: "role_player"; role_name: string; expr: DynamicExpr };

/** Wire shape of the Rust `SortDir` (bare PascalCase variant names). */
export type DynamicSortDir = "Asc" | "Desc";

/** Wire shape of the Rust `DynamicSort`. */
export type DynamicSort =
  | { kind: "attribute"; attr_name: string; direction: DynamicSortDir }
  | { kind: "role_player_attribute"; role_name: string; attr_name: string; direction: DynamicSortDir };

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

export function string(value: string): AttributeValue {
  return { value_type: "string", value };
}

export function long(value: bigint): AttributeValue {
  // Runtime guard for JavaScript callers (the static `bigint` type is erased):
  // a non-bigint would silently stringify to a wrong wire value otherwise.
  if (typeof value !== "bigint") {
    throw new TypeError(
      "long requires a bigint; use longFromNumberUnsafe for explicit number conversion",
    );
  }
  return { value_type: "long", value: value.toString() };
}

export function longFromNumberUnsafe(value: number): AttributeValue {
  if (!Number.isFinite(value) || !Number.isInteger(value)) {
    throw new TypeError("longFromNumberUnsafe requires a finite integer number");
  }
  return { value_type: "long", value: value.toString() };
}

export function double(value: number): AttributeValue {
  if (!Number.isFinite(value)) {
    throw new TypeError("double requires a finite number");
  }
  return { value_type: "double", value };
}

export function boolean(value: boolean): AttributeValue {
  return { value_type: "boolean", value };
}

export function date(value: string): AttributeValue {
  return { value_type: "date", value };
}

export function datetime(value: string): AttributeValue {
  return { value_type: "datetime", value };
}

export function datetimetz(value: string): AttributeValue {
  return { value_type: "datetime-tz", value };
}

export function decimal(value: string): AttributeValue {
  return { value_type: "decimal", value };
}

export function duration(value: string): AttributeValue {
  return { value_type: "duration", value };
}

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
  databaseExists(): boolean;
  createDatabase(): void;
  deleteDatabase(): void;
  resetDatabase(): void;
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
  ensureRustDatabase(
    address: string,
    database: string,
    username?: string | null,
    password?: string | null,
    httpPort?: number | null,
    serverVersion?: string | null,
  ): void;
  connectRustDatabase(
    address: string,
    database: string,
    username?: string | null,
    password?: string | null,
    httpPort?: number | null,
    serverVersion?: string | null,
  ): NativeRustDatabase;
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
  /** Exact TypeDB server version; skips HTTP probing for gRPC-only deployments. */
  serverVersion?: string | null;
}

export interface EnsureDatabaseOptions {
  username?: string | null;
  password?: string | null;
  /** Port of the TypeDB HTTP API used for the connect-time version probe. */
  httpPort?: number;
  /** Exact TypeDB server version; skips HTTP probing for gRPC-only deployments. */
  serverVersion?: string | null;
}

/**
 * Ensure the named TypeDB database exists, creating it if absent.
 *
 * Fails hard when TypeDB is unreachable — callers should let the error
 * propagate so that a missing server shows up as a clear failure, not a
 * silent skip.
 */
export function ensureDatabase(
  address: string,
  database: string,
  options?: EnsureDatabaseOptions,
): void {
  loadNative().ensureRustDatabase(
    address,
    database,
    options?.username ?? null,
    options?.password ?? null,
    options?.httpPort ?? null,
    options?.serverVersion ?? null,
  );
}

export { loadNative };

export function generateDefineBlock(info: SchemaInfo): string {
  return loadNative().generateDefineBlockJson(JSON.stringify(info));
}

export class DescriptorRegistry {
  readonly #native: NativeDescriptorRegistry;
  readonly #attributeSchemas = new Map<string, AttributeSchemaEntry>();

  constructor(nativeRegistry?: NativeDescriptorRegistry | null) {
    this.#native = nativeRegistry ?? new (loadNative().NodeDescriptorRegistry)();
  }

  registerEntity(descriptor: EntityDescriptor): EntityDescriptor {
    this.#rememberAttributeSchemas(descriptor);
    return parseJson(this.#native.registerEntityJson(JSON.stringify(descriptor)));
  }

  registerRelation(descriptor: RelationDescriptor): RelationDescriptor {
    this.#rememberAttributeSchemas(descriptor);
    return parseJson<RelationDescriptor>(
      this.#native.registerRelationJson(JSON.stringify(descriptor)),
    );
  }

  entity(typeName: string): EntityDescriptor {
    return parseJson(this.#native.entityJson(typeName));
  }

  relation(typeName: string): RelationDescriptor {
    return parseJson(this.#native.relationJson(typeName));
  }

  snapshot(): TypeDescriptor[] {
    return parseJson(this.#native.snapshotJson());
  }

  schemaInfo(): SchemaInfo {
    // Rust from_descriptors builds plays_cardinalities overlays and nulls foreign
    // parent_types; the Python attributes-section merge remains the only Python-side
    // projection.
    const info = parseJson<SchemaInfo>(this.#native.schemaInfoJson());
    for (const [attrName, entry] of this.#attributeSchemas) {
      info.attributes[attrName] = {
        ...(info.attributes[attrName] ?? { attr_name: attrName, value_type: entry.value_type }),
        ...copyAttributeSchemaEntry(entry),
      };
    }
    return info;
  }

  #rememberAttributeSchemas(descriptor: EntityDescriptor | RelationDescriptor): void {
    for (const entry of descriptorAttributeSchemaMetadata(descriptor)) {
      this.#attributeSchemas.set(entry.attr_name, copyAttributeSchemaEntry(entry));
    }
    for (const attribute of descriptor.owned_attributes) {
      const entry = ownedAttributeSchemaEntry(attribute);
      if (entry !== null) {
        this.#attributeSchemas.set(entry.attr_name, entry);
      }
    }
  }
}

function descriptorAttributeSchemaMetadata(
  descriptor: EntityDescriptor | RelationDescriptor,
): AttributeSchemaEntry[] {
  const metadata = (descriptor as unknown as Record<PropertyKey, unknown>)[
    ATTRIBUTE_SCHEMA_METADATA
  ];
  if (metadata === null || typeof metadata !== "object") {
    return [];
  }
  return Object.values(metadata as Record<string, AttributeSchemaEntry>).map(copyAttributeSchemaEntry);
}

function ownedAttributeSchemaEntry(
  attribute: OwnedAttributeDescriptor,
): AttributeSchemaEntry | null {
  if (!hasAttributeTypeMetadata(attribute)) {
    return null;
  }
  const entry: AttributeSchemaEntry = {
    attr_name: attribute.attr_name,
    value_type: attribute.value_type,
  };
  if (attribute.parent_type !== undefined) entry.parent_type = attribute.parent_type;
  if (attribute.is_abstract !== undefined) entry.is_abstract = attribute.is_abstract;
  if (attribute.is_independent !== undefined) entry.is_independent = attribute.is_independent;
  if (attribute.regex !== undefined) entry.regex = attribute.regex;
  if (attribute.allowed_values !== undefined) {
    entry.allowed_values =
      attribute.allowed_values == null ? null : [...attribute.allowed_values];
  }
  if (attribute.range !== undefined) {
    entry.range = attribute.range == null ? null : [attribute.range[0], attribute.range[1]];
  }
  return entry;
}

function hasAttributeTypeMetadata(attribute: OwnedAttributeDescriptor): boolean {
  return (
    attribute.parent_type !== undefined ||
    attribute.is_abstract !== undefined ||
    attribute.is_independent !== undefined ||
    attribute.regex !== undefined ||
    attribute.allowed_values !== undefined ||
    attribute.range !== undefined
  );
}

function copyAttributeSchemaEntry(entry: AttributeSchemaEntry): AttributeSchemaEntry {
  const copy: AttributeSchemaEntry = { ...entry };
  if (entry.allowed_values !== undefined) {
    copy.allowed_values = entry.allowed_values === null ? null : [...entry.allowed_values];
  }
  if (entry.range !== undefined) {
    copy.range = entry.range === null ? null : [entry.range[0], entry.range[1]];
  }
  return copy;
}

export class Marshalling {
  readonly #native: NativeMarshalling;

  constructor(nativeMarshalling?: NativeMarshalling | null) {
    this.#native = nativeMarshalling ?? loadNative();
  }

  attributeValue(value: AttributeValue): unknown {
    return parseJson(this.#native.normalizeAttributeValueJson(JSON.stringify(value)));
  }

  entityAttributes(descriptor: EntityDescriptor, attributes: AttributeInput): unknown {
    return parseJson(
      this.#native.normalizeEntityAttributesJson(JSON.stringify(descriptor), JSON.stringify(attributes)),
    );
  }

  relationAttributes(descriptor: RelationDescriptor, attributes: AttributeInput): unknown {
    return parseJson(
      this.#native.normalizeRelationAttributesJson(JSON.stringify(descriptor), JSON.stringify(attributes)),
    );
  }

  filters(descriptor: EntityDescriptor, filters: Record<string, AttributeValue> | FilterInput[]): unknown {
    return parseJson(this.#native.normalizeFiltersJson(JSON.stringify(descriptor), JSON.stringify(filters)));
  }

  relationFilters(
    descriptor: RelationDescriptor,
    filters: Record<string, AttributeValue> | FilterInput[],
  ): unknown {
    return parseJson(this.#native.normalizeRelationFiltersJson(JSON.stringify(descriptor), JSON.stringify(filters)));
  }

  aggregates(descriptor: EntityDescriptor, aggregates: AggregateInput[]): unknown {
    return parseJson(this.#native.normalizeAggregatesJson(JSON.stringify(descriptor), JSON.stringify(aggregates)));
  }

  rolePlayers(descriptor: RelationDescriptor, rolePlayers: RolePlayerInput[]): unknown {
    return parseJson(
      this.#native.normalizeRolePlayersJson(JSON.stringify(descriptor), JSON.stringify(rolePlayers)),
    );
  }

  relationWriteBatch(descriptor: RelationDescriptor, batch: RelationWriteInput[]): unknown {
    return parseJson(this.#native.normalizeRelationWriteBatchJson(JSON.stringify(descriptor), JSON.stringify(batch)));
  }
}

export class RustDatabase {
  readonly #native: NativeRustDatabase;

  private constructor(native: NativeRustDatabase) {
    this.#native = native;
  }

  static connect(address: string, database: string, options?: RustDatabaseConnectOptions): RustDatabase;
  static connect(
    native: NativeRuntime,
    address: string,
    database: string,
    options?: RustDatabaseConnectOptions,
  ): RustDatabase;
  static connect(
    nativeOrAddress: NativeRuntime | string,
    addressOrDatabase: string,
    databaseOrOptions: string | RustDatabaseConnectOptions = {},
    maybeOptions: RustDatabaseConnectOptions = {},
  ): RustDatabase {
    const parsed = parseConnectArguments(nativeOrAddress, addressOrDatabase, databaseOrOptions, maybeOptions);
    return new RustDatabase(
      parsed.native.connectRustDatabase(
        parsed.address,
        parsed.database,
        parsed.options.username ?? null,
        parsed.options.password ?? null,
        parsed.options.httpPort ?? null,
        parsed.options.serverVersion ?? null,
      ),
    );
  }

  isConnected(): boolean {
    return this.#native.isConnected();
  }

  databaseName(): string {
    return this.#native.databaseName();
  }

  databaseExists(): boolean {
    return this.#native.databaseExists();
  }

  createDatabase(): void {
    this.#native.createDatabase();
  }

  deleteDatabase(): void {
    this.#native.deleteDatabase();
  }

  resetDatabase(): void {
    this.#native.resetDatabase();
  }

  transaction(transactionType: TransactionType = "read"): RustTransactionContext {
    return new RustTransactionContext(this.#native.transaction(transactionType));
  }

  entityManager(descriptor: EntityDescriptor): RustDynamicEntityManager {
    return new RustDynamicEntityManager(this.#native.entityManagerJson(JSON.stringify(descriptor)));
  }

  relationManager(descriptor: RelationDescriptor): RustDynamicRelationManager {
    return new RustDynamicRelationManager(this.#native.relationManagerJson(JSON.stringify(descriptor)));
  }
}

export class RustTransactionContext {
  readonly #native: NativeRustTransactionContext;

  constructor(native: NativeRustTransactionContext) {
    this.#native = native;
  }

  query(query: string): unknown[] {
    return parseJson(this.#native.queryJson(query));
  }

  commit(): void {
    this.#native.commit();
  }

  rollback(): void {
    this.#native.rollback();
  }

  close(): void {
    this.#native.close();
  }

  transactionType(): TransactionType {
    return this.#native.transactionType();
  }

  entityManager(descriptor: EntityDescriptor): RustDynamicEntityManager {
    return new RustDynamicEntityManager(this.#native.entityManagerJson(JSON.stringify(descriptor)));
  }

  relationManager(descriptor: RelationDescriptor): RustDynamicRelationManager {
    return new RustDynamicRelationManager(this.#native.relationManagerJson(JSON.stringify(descriptor)));
  }
}

export class RustDynamicEntityManager {
  readonly #native: NativeDynamicEntityManager;

  constructor(native: NativeDynamicEntityManager) {
    this.#native = native;
  }

  insert(attributes: AttributeInput): string {
    return this.#native.insertJson(JSON.stringify(attributes));
  }

  insertMany(batch: AttributeInput[]): string[] {
    return parseJson(this.#native.insertManyJson(JSON.stringify(batch)));
  }

  put(attributes: AttributeInput): string {
    return this.#native.putJson(JSON.stringify(attributes));
  }

  putMany(batch: AttributeInput[]): string[] {
    return parseJson(this.#native.putManyJson(JSON.stringify(batch)));
  }

  update(attributes: AttributeInput, iid?: string | null): void {
    this.#native.updateJson(JSON.stringify(attributes), iid ?? null);
  }

  get(filters?: Record<string, AttributeValue> | FilterInput[] | null): DynamicEntityRow[] {
    return parseJson(this.#native.getJson(optionalJson(filters)));
  }

  getByIid(iid: string): DynamicEntityRow | null {
    return parseJson(this.#native.getByIidJson(iid));
  }

  all(): DynamicEntityRow[] {
    return parseJson(this.#native.allJson());
  }

  count(filters?: Record<string, AttributeValue> | FilterInput[] | null): bigint {
    return BigInt(this.#native.countJson(optionalJson(filters)));
  }

  aggregate(aggregates: AggregateInput[], filters?: Record<string, AttributeValue> | FilterInput[] | null): unknown[] {
    return parseJson(this.#native.aggregateJson(JSON.stringify(aggregates), optionalJson(filters)));
  }

  groupByAggregate(
    groupFields: string[],
    aggregates: AggregateInput[],
    filters?: Record<string, AttributeValue> | FilterInput[] | null,
  ): unknown[] {
    return parseJson(
      this.#native.groupByAggregateJson(JSON.stringify(groupFields), JSON.stringify(aggregates), optionalJson(filters)),
    );
  }

  query(spec: DynamicQuerySpec): DynamicEntityRow[] {
    return parseJson(this.#native.queryJson(JSON.stringify(spec)));
  }

  queryCount(spec: DynamicQuerySpec): bigint {
    return BigInt(this.#native.queryCountJson(JSON.stringify(spec)));
  }

  queryAggregate(spec: DynamicQuerySpec, aggregates: AggregateInput[]): unknown[] {
    return parseJson(this.#native.queryAggregateJson(JSON.stringify(spec), JSON.stringify(aggregates)));
  }

  queryGroupByAggregate(
    spec: DynamicQuerySpec,
    groupFields: string[],
    aggregates: AggregateInput[],
  ): unknown[] {
    return parseJson(
      this.#native.queryGroupByAggregateJson(
        JSON.stringify(spec),
        JSON.stringify(groupFields),
        JSON.stringify(aggregates),
      ),
    );
  }

  deleteByIid(iid: string): void {
    this.#native.deleteByIid(iid);
  }
}

export class RustDynamicRelationManager {
  readonly #native: NativeDynamicRelationManager;

  constructor(native: NativeDynamicRelationManager) {
    this.#native = native;
  }

  insert(attributes: AttributeInput, rolePlayers: RolePlayerInput[]): string {
    return this.#native.insertJson(JSON.stringify(attributes), JSON.stringify(rolePlayers));
  }

  insertMany(batch: RelationWriteInput[]): string[] {
    return parseJson(this.#native.insertManyJson(JSON.stringify(batch)));
  }

  put(attributes: AttributeInput, rolePlayers: RolePlayerInput[]): string {
    return this.#native.putJson(JSON.stringify(attributes), JSON.stringify(rolePlayers));
  }

  putMany(batch: RelationWriteInput[]): string[] {
    return parseJson(this.#native.putManyJson(JSON.stringify(batch)));
  }

  update(attributes: AttributeInput, rolePlayers: RolePlayerInput[], iid?: string | null): void {
    this.#native.updateJson(JSON.stringify(attributes), JSON.stringify(rolePlayers), iid ?? null);
  }

  get(filters?: Record<string, AttributeValue> | FilterInput[] | null): DynamicRelationRow[] {
    return parseJson(this.#native.getJson(optionalJson(filters)));
  }

  getWithRolePlayers(
    filters?: Record<string, AttributeValue> | FilterInput[] | null,
    rolePlayers?: RolePlayerInput[] | null,
  ): DynamicRelationRow[] {
    return parseJson(this.#native.getWithRolePlayersJson(optionalJson(filters), optionalJson(rolePlayers)));
  }

  getByIid(iid: string): DynamicRelationRow[] {
    return parseJson(this.#native.getByIidJson(iid));
  }

  all(): DynamicRelationRow[] {
    return parseJson(this.#native.allJson());
  }

  count(filters?: Record<string, AttributeValue> | FilterInput[] | null): bigint {
    return BigInt(this.#native.countJson(optionalJson(filters)));
  }

  aggregate(aggregates: AggregateInput[], filters?: Record<string, AttributeValue> | FilterInput[] | null): unknown[] {
    return parseJson(this.#native.aggregateJson(JSON.stringify(aggregates), optionalJson(filters)));
  }

  groupByAggregate(
    groupFields: string[],
    aggregates: AggregateInput[],
    filters?: Record<string, AttributeValue> | FilterInput[] | null,
  ): unknown[] {
    return parseJson(
      this.#native.groupByAggregateJson(JSON.stringify(groupFields), JSON.stringify(aggregates), optionalJson(filters)),
    );
  }

  query(spec: DynamicQuerySpec): DynamicRelationRow[] {
    return parseJson(this.#native.queryJson(JSON.stringify(spec)));
  }

  queryCount(spec: DynamicQuerySpec): bigint {
    return BigInt(this.#native.queryCountJson(JSON.stringify(spec)));
  }

  queryAggregate(spec: DynamicQuerySpec, aggregates: AggregateInput[]): unknown[] {
    return parseJson(this.#native.queryAggregateJson(JSON.stringify(spec), JSON.stringify(aggregates)));
  }

  queryGroupByAggregate(
    spec: DynamicQuerySpec,
    groupFields: string[],
    aggregates: AggregateInput[],
  ): unknown[] {
    return parseJson(
      this.#native.queryGroupByAggregateJson(
        JSON.stringify(spec),
        JSON.stringify(groupFields),
        JSON.stringify(aggregates),
      ),
    );
  }

  deleteByIid(iid: string): void {
    this.#native.deleteByIid(iid);
  }
}

function parseJson<T>(value: string): T {
  return JSON.parse(value) as T;
}

function optionalJson(value: unknown | null | undefined): string | null {
  return value == null ? null : JSON.stringify(value);
}

function parseConnectArguments(
  nativeOrAddress: NativeRuntime | string,
  addressOrDatabase: string,
  databaseOrOptions: string | RustDatabaseConnectOptions,
  maybeOptions: RustDatabaseConnectOptions,
): {
  native: NativeRuntime;
  address: string;
  database: string;
  options: RustDatabaseConnectOptions;
} {
  if (typeof nativeOrAddress === "string") {
    return {
      native: loadNative(),
      address: nativeOrAddress,
      database: addressOrDatabase,
      options: (databaseOrOptions as RustDatabaseConnectOptions) ?? {},
    };
  }

  if (typeof databaseOrOptions !== "string") {
    throw new TypeError("RustDatabase.connect(native, address, database, options?) requires a database string");
  }

  return {
    native: nativeOrAddress,
    address: addressOrDatabase,
    database: databaseOrOptions,
    options: maybeOptions ?? {},
  };
}

// ---------------------------------------------------------------------------
// Generator — additive re-export
// ---------------------------------------------------------------------------
export {
  generateModels,
  type GenerateModelsOptions,
  type NamingOptions,
} from "./generator/index.js";
