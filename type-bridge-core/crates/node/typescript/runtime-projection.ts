import type {
  NativeRustDatabase,
  NativeRustTransactionContext,
  RustDatabase,
  RustTransactionContext,
} from "./index.js";
import { loadNative } from "./native.js";
import {
  isRegisteredRustConnection,
  rustDatabaseHandle,
  rustTransactionHandle,
} from "./typed/runtime-handles.js";

export type RuntimeProjectionConnection = RustDatabase | RustTransactionContext;

export interface RuntimeProjectionBinding {
  readonly typeKey: string;
  readonly targetName: string;
  readonly create: boolean;
  readonly reference: boolean;
}

export interface RuntimeProjectionInstall {
  readonly projectionJson: string;
  readonly semanticFingerprintJson: string;
  readonly projectionFingerprintJson: string;
  readonly bindings: readonly RuntimeProjectionBinding[];
}

export interface NativeProjectedManager {
  insertJson(instanceJson: string): string;
  getByIidJson(iid: string): string;
  allJson(): string;
}

interface NativeProjectionHandle {
  managerForDatabase(typeKey: string, database: NativeRustDatabase): NativeProjectedManager;
  managerForTransaction(typeKey: string, transaction: NativeRustTransactionContext): NativeProjectedManager;
}

/** A verified native projection scoped to one generated package instance. */
export class InstalledRuntimeProjection {
  readonly #native: NativeProjectionHandle;

  constructor(native: NativeProjectionHandle) {
    this.#native = native;
    Object.freeze(this);
  }

  /** @internal Bind one generated token without exposing its native handle. */
  manager(typeKey: string, connection: RuntimeProjectionConnection): NativeProjectedManager {
    if (!isRegisteredRustConnection(connection)) {
      throw new TypeError("projected manager requires a registered RustDatabase or RustTransactionContext");
    }
    const database = rustDatabaseHandle(connection);
    if (database !== undefined) {
      return this.#native.managerForDatabase(typeKey, database);
    }
    const transaction = rustTransactionHandle(connection);
    if (transaction !== undefined) {
      return this.#native.managerForTransaction(typeKey, transaction);
    }
    throw new TypeError("projected manager has no registered native execution handle");
  }
}

/** Verify and install one generated package's exact projection evidence. */
export function installRuntimeProjection(input: RuntimeProjectionInstall): InstalledRuntimeProjection {
  const native = loadNative();
  return new InstalledRuntimeProjection(new native.NodeRuntimeProjection(
    input.projectionJson,
    input.semanticFingerprintJson,
    input.projectionFingerprintJson,
    JSON.stringify(input.bindings),
  ));
}
