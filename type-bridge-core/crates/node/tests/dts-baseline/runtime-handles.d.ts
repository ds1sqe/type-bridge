import type { NativeRustDatabase, NativeRustTransactionContext, RustDatabase, RustTransactionContext } from "./index.js";
/** @internal Reject structurally forged execution connections. */
export declare function isRegisteredRustConnection(connection: unknown): connection is RustDatabase | RustTransactionContext;
/** @internal Retain an opaque native database handle. */
export declare function registerRustDatabaseHandle(database: RustDatabase, native: NativeRustDatabase): void;
/** @internal Retain an opaque borrowed transaction handle. */
export declare function registerRustTransactionHandle(transaction: RustTransactionContext, native: NativeRustTransactionContext): void;
/** @internal Resolve database ownership for generated execution. */
export declare function rustDatabaseHandle(connection: RustDatabase | RustTransactionContext): NativeRustDatabase | undefined;
/** @internal Resolve borrowed transaction ownership for generated execution. */
export declare function rustTransactionHandle(connection: RustDatabase | RustTransactionContext): NativeRustTransactionContext | undefined;
