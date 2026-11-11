import type {
  NativeDatabase,
  NativeTransactionContext,
  Database,
  TransactionContext,
  MigrationTransaction,
} from "./index.js";

const databaseHandles = new WeakMap<object, NativeDatabase>();
const transactionHandles = new WeakMap<object, NativeTransactionContext>();

/** @internal Reject structurally forged execution connections. */
export function isRegisteredConnection(
  connection: unknown,
): connection is Database | TransactionContext | MigrationTransaction {
  if (
    connection === null ||
    (typeof connection !== "object" && typeof connection !== "function")
  ) {
    return false;
  }
  return databaseHandles.has(connection) || transactionHandles.has(connection);
}

/** @internal Retain an opaque native database handle. */
export function registerDatabaseHandle(
  database: Database,
  native: NativeDatabase,
): void {
  databaseHandles.set(database, native);
}

/** @internal Retain an opaque borrowed transaction handle. */
export function registerTransactionHandle(
  transaction: TransactionContext | MigrationTransaction,
  native: NativeTransactionContext,
): void {
  transactionHandles.set(transaction, native);
}

/** @internal Resolve database ownership for generated execution. */
export function rustDatabaseHandle(
  connection: Database | TransactionContext | MigrationTransaction,
): NativeDatabase | undefined {
  return databaseHandles.get(connection);
}

/** @internal Resolve borrowed transaction ownership for generated execution. */
export function rustTransactionHandle(
  connection: Database | TransactionContext | MigrationTransaction,
): NativeTransactionContext | undefined {
  return transactionHandles.get(connection);
}
