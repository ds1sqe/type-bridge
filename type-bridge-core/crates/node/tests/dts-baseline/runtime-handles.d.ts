import type { NativeDatabase, NativeTransactionContext, Database, TransactionContext, MigrationTransaction } from "./index.js";
/** @internal Reject structurally forged execution connections. */
export declare function isRegisteredConnection(connection: unknown): connection is Database | TransactionContext | MigrationTransaction;
/** @internal Retain an opaque native database handle. */
export declare function registerDatabaseHandle(database: Database, native: NativeDatabase): void;
/** @internal Retain an opaque borrowed transaction handle. */
export declare function registerTransactionHandle(transaction: TransactionContext | MigrationTransaction, native: NativeTransactionContext): void;
/** @internal Resolve database ownership for generated execution. */
export declare function rustDatabaseHandle(connection: Database | TransactionContext | MigrationTransaction): NativeDatabase | undefined;
/** @internal Resolve borrowed transaction ownership for generated execution. */
export declare function rustTransactionHandle(connection: Database | TransactionContext | MigrationTransaction): NativeTransactionContext | undefined;
