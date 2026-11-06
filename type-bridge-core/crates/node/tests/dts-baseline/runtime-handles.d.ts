import type { NativeRustDatabase, NativeRustTransactionContext, Database, TransactionContext, MigrationTransaction } from "./index.js";
/** @internal Reject structurally forged execution connections. */
export declare function isRegisteredRustConnection(connection: unknown): connection is Database | TransactionContext | MigrationTransaction;
/** @internal Retain an opaque native database handle. */
export declare function registerRustDatabaseHandle(database: Database, native: NativeRustDatabase): void;
/** @internal Retain an opaque borrowed transaction handle. */
export declare function registerRustTransactionHandle(transaction: TransactionContext | MigrationTransaction, native: NativeRustTransactionContext): void;
/** @internal Resolve database ownership for generated execution. */
export declare function rustDatabaseHandle(connection: Database | TransactionContext | MigrationTransaction): NativeRustDatabase | undefined;
/** @internal Resolve borrowed transaction ownership for generated execution. */
export declare function rustTransactionHandle(connection: Database | TransactionContext | MigrationTransaction): NativeRustTransactionContext | undefined;
