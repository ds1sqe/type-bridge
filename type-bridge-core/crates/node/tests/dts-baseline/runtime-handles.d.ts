import type { NativeRustDatabase, NativeRustTransactionContext, Database, TransactionContext } from "./index.js";
/** @internal Reject structurally forged execution connections. */
export declare function isRegisteredRustConnection(connection: unknown): connection is Database | TransactionContext;
/** @internal Retain an opaque native database handle. */
export declare function registerRustDatabaseHandle(database: Database, native: NativeRustDatabase): void;
/** @internal Retain an opaque borrowed transaction handle. */
export declare function registerRustTransactionHandle(transaction: TransactionContext, native: NativeRustTransactionContext): void;
/** @internal Resolve database ownership for generated execution. */
export declare function rustDatabaseHandle(connection: Database | TransactionContext): NativeRustDatabase | undefined;
/** @internal Resolve borrowed transaction ownership for generated execution. */
export declare function rustTransactionHandle(connection: Database | TransactionContext): NativeRustTransactionContext | undefined;
