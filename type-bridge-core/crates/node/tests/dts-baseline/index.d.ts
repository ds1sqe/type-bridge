import { type NativeQueryV2Authority, type NativeQueryV2BuilderRuntime } from "./query-v2-internals.js";
export { QueryV2Error } from "./query-v2-internals.js";
export type { QueryV2ErrorCategory, QueryV2ErrorDetail, QueryV2ErrorPathSegment, } from "./query-v2-internals.js";
export type ValueType = "string" | "long" | "double" | "boolean" | "date" | "datetime" | "datetime-tz" | "decimal" | "duration";
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
export { AggregateSpec, BooleanExpr, ComparisonExpr, NotExpr, QueryExpr, SortExpr, TypedGroupByQuery, TypedQuery, TypedQueryError, agg, type QueryGroupField, } from "./query.js";
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
export type DynamicComparisonOp = "eq" | "neq" | "gt" | "gte" | "lt" | "lte" | "contains" | "like" | "starts_with" | "ends_with";
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
export type DynamicSortDir = "Asc" | "Desc";
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
    queryV2Authority(declaredSchema: Uint8Array, scope: string, profile: string): NativeQueryV2Authority;
    queryV2QueryOnlyAuthority(database: NativeRustDatabase, declaredSchema: Uint8Array, scope: string, profile: string): NativeQueryV2Authority;
    queryV2ExecuteLocal(database: NativeRustDatabase, authority: NativeQueryV2Authority, plan: Uint8Array, invocationJson: string, deadlineMs?: bigint | null): Promise<string>;
    queryV2RemoteCapabilities(advertisement: Uint8Array): string[];
    queryV2PrepareRemote(authority: NativeQueryV2Authority, plan: Uint8Array, invocationJson: string, advertisement: Uint8Array, maxItems: bigint, maxBytes: bigint, maxCollectionMembers: bigint, deadlineMs?: bigint | null): NativePendingQueryV2Remote;
}
export interface NativeRuntime {
    ensureRustDatabase(address: string, database: string, username?: string | null, password?: string | null, httpPort?: number | null, serverVersion?: string | null, tlsEnabled?: boolean | null, tlsRootCa?: string | null): void;
    connectRustDatabase(address: string, database: string, username?: string | null, password?: string | null, httpPort?: number | null, serverVersion?: string | null, tlsEnabled?: boolean | null, tlsRootCa?: string | null): NativeRustDatabase;
}
/** Package-private native contract used by retained runtime entry points. */
export interface NativeModule extends NativeRuntime, NativeQueryV2Runtime, NativeQueryV2BuilderRuntime {
}
export interface RustDatabaseConnectOptions {
    username?: string | null;
    password?: string | null;
    httpPort?: number;
    serverVersion?: string | null;
    tlsEnabled?: boolean;
    tlsRootCa?: string;
}
export interface EnsureDatabaseOptions extends RustDatabaseConnectOptions {
}
export declare function ensureDatabase(address: string, database: string, options?: EnsureDatabaseOptions): void;
/** Opaque declared-schema authority for prepared V2 plan execution. */
export declare class QueryV2Authority {
    #private;
    constructor(declaredSchema: Uint8Array, scope: string, profile: string);
    static queryOnly(database: RustDatabase, declaredSchema: Uint8Array, scope: string, profile: string): QueryV2Authority;
}
export interface QueryV2RemoteLimits {
    readonly maxItems: bigint;
    readonly maxBytes: bigint;
    readonly maxCollectionMembers: bigint;
    readonly deadlineMs?: bigint | null;
}
export declare function queryV2ExecuteLocal(database: RustDatabase, authority: QueryV2Authority, plan: Uint8Array, invocationJson: string, deadlineMs?: bigint | null): Promise<string>;
export declare function queryV2RemoteCapabilities(advertisement: Uint8Array): readonly string[];
export declare function queryV2PrepareRemote(authority: QueryV2Authority, plan: Uint8Array, invocationJson: string, advertisement: Uint8Array, limits: QueryV2RemoteLimits): PendingQueryV2Remote;
export declare class RustDatabase {
    #private;
    private constructor();
    static connect(address: string, databaseName: string, options?: RustDatabaseConnectOptions): RustDatabase;
    isConnected(): boolean;
    close(): void;
    databaseName(): string;
    databaseExists(): boolean;
    createDatabase(): void;
    deleteDatabase(): void;
    resetDatabase(): void;
    transaction(transactionType?: TransactionType): RustTransactionContext;
}
export declare class RustTransactionContext {
    #private;
    private constructor();
    query(query: string): unknown[];
    commit(): void;
    rollback(): void;
    close(): void;
    transactionType(): TransactionType;
}
