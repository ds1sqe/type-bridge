import type {
  NativeRustDatabase,
  NativeRustTransactionContext,
  Database,
  TransactionContext,
} from "./index.js";

const databaseHandles = new WeakMap<object, NativeRustDatabase>();
const transactionHandles = new WeakMap<object, NativeRustTransactionContext>();

/** @internal Reject structurally forged execution connections. */
export function isRegisteredRustConnection(
  connection: unknown,
): connection is Database | TransactionContext {
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
  database: Database,
  native: NativeRustDatabase,
): void {
  databaseHandles.set(database, native);
}

/** @internal Retain an opaque borrowed transaction handle. */
export function registerRustTransactionHandle(
  transaction: TransactionContext,
  native: NativeRustTransactionContext,
): void {
  transactionHandles.set(transaction, native);
}

/** @internal Resolve database ownership for generated execution. */
export function rustDatabaseHandle(
  connection: Database | TransactionContext,
): NativeRustDatabase | undefined {
  return databaseHandles.get(connection);
}

/** @internal Resolve borrowed transaction ownership for generated execution. */
export function rustTransactionHandle(
  connection: Database | TransactionContext,
): NativeRustTransactionContext | undefined {
  return transactionHandles.get(connection);
}
