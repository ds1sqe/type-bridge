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

export type Annotation = "Key" | "Unique" | { Card: [number, number | null] };

export interface OwnedAttributeDescriptor {
  field_name: string;
  attr_name: string;
  value_type: ValueType;
  annotations: Annotation[];
  is_optional: boolean;
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

export function string(value: string): AttributeValue {
  return { value_type: "string", value };
}

export function long(value: bigint): AttributeValue {
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
}

export interface NativeRuntime {
  ensureRustDatabase(
    address: string,
    database: string,
    username?: string | null,
    password?: string | null,
  ): void;
  connectRustDatabase(
    address: string,
    database: string,
    username?: string | null,
    password?: string | null,
  ): NativeRustDatabase;
}

export interface NativeModule extends NativeRuntime, NativeMarshalling {
  NodeDescriptorRegistry: new () => NativeDescriptorRegistry;
}

export interface RustDatabaseConnectOptions {
  username?: string | null;
  password?: string | null;
}

export interface EnsureDatabaseOptions {
  username?: string | null;
  password?: string | null;
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
  );
}

declare const nativeModule: NativeModule;

export function loadNative(): NativeModule {
  return nativeModule;
}

export class DescriptorRegistry {
  readonly #native: NativeDescriptorRegistry;

  constructor(nativeRegistry?: NativeDescriptorRegistry | null) {
    this.#native = nativeRegistry ?? new (loadNative().NodeDescriptorRegistry)();
  }

  registerEntity(descriptor: EntityDescriptor): EntityDescriptor {
    return parseJson(this.#native.registerEntityJson(JSON.stringify(descriptor)));
  }

  registerRelation(descriptor: RelationDescriptor): RelationDescriptor {
    return parseJson(this.#native.registerRelationJson(JSON.stringify(descriptor)));
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
      ),
    );
  }

  isConnected(): boolean {
    return this.#native.isConnected();
  }

  databaseName(): string {
    return this.#native.databaseName();
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
