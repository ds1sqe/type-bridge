import { loadNative } from "./native.js";
import {
  queryV2AuthorityHandle,
  queryV2NativeCall,
  queryV2NativePromise,
  registerQueryV2AuthorityHandle,
  type NativeQueryV2Authority,
  type NativeQueryV2BuilderRuntime,
} from "./query-v2-internals.js";
import {
  registerRustDatabaseHandle,
  registerRustTransactionHandle,
  rustDatabaseHandle,
} from "./runtime-handles.js";

export { QueryV2Error } from "./query-v2-internals.js";
export type {
  QueryV2ErrorCategory,
  QueryV2ErrorDetail,
  QueryV2ErrorPathSegment,
} from "./query-v2-internals.js";

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

export type AttributeInput = Record<
  string,
  AttributeValue | AttributeValue[] | null | undefined
>;

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
  type QueryGroupField,
} from "./query.js";

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

/** Wire shape accepted by the separately retained V1 query facade. */
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

export type DynamicExpr =
  | {
      kind: "compare";
      attr_name: string;
      operator: DynamicComparisonOp;
      value: AttributeValue;
    }
  | { kind: "iid"; iid: string }
  | { kind: "is_null"; attr_name: string; is_null: boolean }
  | { kind: "and"; exprs: DynamicExpr[] }
  | { kind: "or"; exprs: DynamicExpr[] }
  | { kind: "not"; expr: DynamicExpr }
  | { kind: "role_player"; role_name: string; expr: DynamicExpr };

export type DynamicSortDir = "Asc" | "Desc";

export type DynamicSort =
  | { kind: "attribute"; attr_name: string; direction: DynamicSortDir }
  | {
      kind: "role_player_attribute";
      role_name: string;
      attr_name: string;
      direction: DynamicSortDir;
    };

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

export interface NativeRustDatabase {
  isConnected(): boolean;
  close(): void;
  databaseName(): string;
  databaseExists(): boolean;
  createDatabase(): void;
  deleteDatabase(): void;
  resetDatabase(): void;
  transaction(transactionType?: TransactionType): NativeRustTransactionContext;
}

export interface NativeRustTransactionContext {
  queryJson(query: string): string;
  commit(): void;
  rollback(): void;
  close(): void;
  transactionType(): TransactionType;
}

interface NativePendingQueryV2Remote {
  requestBytes(): Uint8Array;
  decodeReply(response: Uint8Array): Promise<string>;
}

export interface PendingQueryV2Remote {
  requestBytes(): Uint8Array;
  decodeReply(response: Uint8Array): Promise<string>;
}

interface NativeQueryV2Runtime {
  queryV2Authority(
    declaredSchema: Uint8Array,
    scope: string,
    profile: string,
  ): NativeQueryV2Authority;
  queryV2AuthorityFromSchemaAuthority(
    schemaAuthorityJson: string,
    semanticFingerprintJson: string,
  ): NativeQueryV2Authority;
  queryV2QueryOnlyAuthority(
    database: NativeRustDatabase,
    declaredSchema: Uint8Array,
    scope: string,
    profile: string,
  ): NativeQueryV2Authority;
  queryV2ExecuteLocal(
    database: NativeRustDatabase,
    authority: NativeQueryV2Authority,
    plan: Uint8Array,
    invocationJson: string,
    deadlineMs?: bigint | null,
  ): Promise<string>;
  queryV2RemoteCapabilities(advertisement: Uint8Array): string[];
  queryV2PrepareRemote(
    authority: NativeQueryV2Authority,
    plan: Uint8Array,
    invocationJson: string,
    advertisement: Uint8Array,
    maxItems: bigint,
    maxBytes: bigint,
    maxCollectionMembers: bigint,
    deadlineMs?: bigint | null,
  ): NativePendingQueryV2Remote;
}

export interface NativeRuntime {
  ensureRustDatabase(
    address: string,
    database: string,
    username?: string | null,
    password?: string | null,
    httpPort?: number | null,
    serverVersion?: string | null,
    tlsEnabled?: boolean | null,
    tlsRootCa?: string | null,
  ): void;
  connectRustDatabase(
    address: string,
    database: string,
    username?: string | null,
    password?: string | null,
    httpPort?: number | null,
    serverVersion?: string | null,
    tlsEnabled?: boolean | null,
    tlsRootCa?: string | null,
  ): NativeRustDatabase;
}

/** Package-private native contract used by retained runtime entry points. */
export interface NativeModule
  extends NativeRuntime,
    NativeQueryV2Runtime,
    NativeQueryV2BuilderRuntime {}

export interface RustDatabaseConnectOptions {
  username?: string | null;
  password?: string | null;
  httpPort?: number;
  serverVersion?: string | null;
  tlsEnabled?: boolean;
  tlsRootCa?: string;
}

export interface EnsureDatabaseOptions extends RustDatabaseConnectOptions {}

export function ensureDatabase(
  address: string,
  database: string,
  options?: EnsureDatabaseOptions,
): void {
  validateConnectionTransport(options ?? {});
  loadNative().ensureRustDatabase(
    address,
    database,
    options?.username ?? null,
    options?.password ?? null,
    options?.httpPort ?? null,
    options?.serverVersion ?? null,
    options?.tlsEnabled ?? null,
    options?.tlsRootCa ?? null,
  );
}

/** Opaque declared-schema authority for prepared V2 plan execution. */
export class QueryV2Authority {
  readonly #brand = undefined;

  constructor(declaredSchema: Uint8Array, scope: string, profile: string) {
    registerQueryV2AuthorityHandle(
      this,
      queryV2NativeCall(() =>
        loadNative().queryV2Authority(declaredSchema, scope, profile),
      ),
    );
  }

  static queryOnly(
    database: RustDatabase,
    declaredSchema: Uint8Array,
    scope: string,
    profile: string,
  ): QueryV2Authority {
    const authority = Object.create(
      QueryV2Authority.prototype,
    ) as QueryV2Authority;
    registerQueryV2AuthorityHandle(
      authority,
      queryV2NativeCall(() =>
        loadNative().queryV2QueryOnlyAuthority(
          preparedV2DatabaseHandle(database),
          declaredSchema,
          scope,
          profile,
        ),
      ),
    );
    return authority;
  }
}

export interface QueryV2RemoteLimits {
  readonly maxItems: bigint;
  readonly maxBytes: bigint;
  readonly maxCollectionMembers: bigint;
  readonly deadlineMs?: bigint | null;
}

function preparedV2DatabaseHandle(database: RustDatabase): NativeRustDatabase {
  const native = rustDatabaseHandle(database);
  if (native === undefined) {
    throw new TypeError(
      "queryV2ExecuteLocal requires a type-bridge RustDatabase",
    );
  }
  return native;
}

function preparedV2AuthorityHandle(
  authority: QueryV2Authority,
): NativeQueryV2Authority {
  const native = queryV2AuthorityHandle(authority);
  if (native === undefined) {
    throw new TypeError(
      "prepared V2 execution requires a type-bridge QueryV2Authority",
    );
  }
  return native;
}

export function queryV2ExecuteLocal(
  database: RustDatabase,
  authority: QueryV2Authority,
  plan: Uint8Array,
  invocationJson: string,
  deadlineMs?: bigint | null,
): Promise<string> {
  return queryV2NativePromise(
    queryV2NativeCall(() =>
      loadNative().queryV2ExecuteLocal(
        preparedV2DatabaseHandle(database),
        preparedV2AuthorityHandle(authority),
        plan,
        invocationJson,
        deadlineMs,
      ),
    ),
  );
}

export function queryV2RemoteCapabilities(
  advertisement: Uint8Array,
): readonly string[] {
  return queryV2NativeCall(() =>
    loadNative().queryV2RemoteCapabilities(advertisement),
  );
}

export function queryV2PrepareRemote(
  authority: QueryV2Authority,
  plan: Uint8Array,
  invocationJson: string,
  advertisement: Uint8Array,
  limits: QueryV2RemoteLimits,
): PendingQueryV2Remote {
  const pending = queryV2NativeCall(() =>
    loadNative().queryV2PrepareRemote(
      preparedV2AuthorityHandle(authority),
      plan,
      invocationJson,
      advertisement,
      limits.maxItems,
      limits.maxBytes,
      limits.maxCollectionMembers,
      limits.deadlineMs,
    ),
  );
  return Object.freeze({
    requestBytes: (): Uint8Array =>
      queryV2NativeCall(() => new Uint8Array(pending.requestBytes())),
    decodeReply: (response: Uint8Array): Promise<string> =>
      queryV2NativePromise(
        queryV2NativeCall(() => pending.decodeReply(response)),
      ),
  });
}

export class RustDatabase {
  readonly #native: NativeRustDatabase;

  private constructor(native: NativeRustDatabase) {
    this.#native = native;
    registerRustDatabaseHandle(this, native);
  }

  static connect(
    address: string,
    databaseName: string,
    options: RustDatabaseConnectOptions = {},
  ): RustDatabase {
    validateConnectionTransport(options);
    const native = loadNative().connectRustDatabase(
      address,
      databaseName,
      options.username ?? null,
      options.password ?? null,
      options.httpPort ?? null,
      options.serverVersion ?? null,
      options.tlsEnabled ?? null,
      options.tlsRootCa ?? null,
    );
    return new RustDatabase(native);
  }

  isConnected(): boolean {
    return this.#native.isConnected();
  }

  close(): void {
    this.#native.close();
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

  transaction(
    transactionType: TransactionType = "read",
  ): RustTransactionContext {
    return createRustTransactionContext(
      this.#native.transaction(transactionType),
    );
  }
}

const TRANSACTION_CONSTRUCTOR = Symbol("RustTransactionContext");

export class RustTransactionContext {
  readonly #native: NativeRustTransactionContext;

  private constructor(
    native: NativeRustTransactionContext,
    token: typeof TRANSACTION_CONSTRUCTOR,
  ) {
    if (token !== TRANSACTION_CONSTRUCTOR) {
      throw new TypeError(
        "RustTransactionContext values are created by RustDatabase.transaction()",
      );
    }
    this.#native = native;
    registerRustTransactionHandle(this, native);
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
}

function createRustTransactionContext(
  native: NativeRustTransactionContext,
): RustTransactionContext {
  const TransactionContext = RustTransactionContext as unknown as new (
    native: NativeRustTransactionContext,
    token: typeof TRANSACTION_CONSTRUCTOR,
  ) => RustTransactionContext;
  return new TransactionContext(native, TRANSACTION_CONSTRUCTOR);
}

function parseJson<T>(value: string): T {
  return JSON.parse(value) as T;
}

function validateConnectionTransport(
  options: Pick<RustDatabaseConnectOptions, "tlsEnabled" | "tlsRootCa">,
): void {
  for (const key in options) {
    const normalized = key.replace(/[^a-zA-Z0-9]/g, "").toLowerCase();
    if (
      normalized.startsWith("tls") &&
      key !== "tlsEnabled" &&
      key !== "tlsRootCa"
    ) {
      throw new TypeError(`unknown TLS connection option ${JSON.stringify(key)}`);
    }
  }
  const tlsEnabled = options.tlsEnabled;
  const tlsRootCa = options.tlsRootCa;
  if (tlsEnabled !== undefined && typeof tlsEnabled !== "boolean") {
    throw new TypeError("tlsEnabled must be a boolean when provided");
  }
  if (tlsRootCa !== undefined && typeof tlsRootCa !== "string") {
    throw new TypeError("tlsRootCa must be a string path when provided");
  }
  if (tlsRootCa !== undefined) {
    if (tlsEnabled === undefined) {
      throw new TypeError("tlsRootCa requires explicit tlsEnabled=true");
    }
    if (!tlsEnabled) {
      throw new TypeError(
        "tlsRootCa contradicts explicit tlsEnabled=false",
      );
    }
    if (tlsRootCa.length === 0) {
      throw new TypeError("tlsRootCa must not be empty");
    }
  }
}
