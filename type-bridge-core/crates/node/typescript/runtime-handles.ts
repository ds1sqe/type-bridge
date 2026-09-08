import type {
  NativeRustDatabase,
  NativeRustTransactionContext,
  RustDatabase,
  RustTransactionContext,
} from "./index.js";

const databaseHandles = new WeakMap<object, NativeRustDatabase>();
const transactionHandles = new WeakMap<object, NativeRustTransactionContext>();

/** @internal Reject structurally forged execution connections. */
export function isRegisteredRustConnection(
  connection: unknown,
): connection is RustDatabase | RustTransactionContext {
  if (
    connection === null ||
    (typeof connection !== "object" && typeof connection !== "function")
  ) {
    return false;
  }
  return databaseHandles.has(connection) || transactionHandles.has(connection);
}

/** @internal Retain an opaque native database handle. */
export function registerRustDatabaseHandle(
  database: RustDatabase,
  native: NativeRustDatabase,
): void {
  databaseHandles.set(database, native);
}

/** @internal Retain an opaque borrowed transaction handle. */
export function registerRustTransactionHandle(
  transaction: RustTransactionContext,
  native: NativeRustTransactionContext,
): void {
  transactionHandles.set(transaction, native);
}

/** @internal Resolve database ownership for generated execution. */
export function rustDatabaseHandle(
  connection: RustDatabase | RustTransactionContext,
): NativeRustDatabase | undefined {
  return databaseHandles.get(connection);
}

/** @internal Resolve borrowed transaction ownership for generated execution. */
export function rustTransactionHandle(
  connection: RustDatabase | RustTransactionContext,
): NativeRustTransactionContext | undefined {
  return transactionHandles.get(connection);
}
