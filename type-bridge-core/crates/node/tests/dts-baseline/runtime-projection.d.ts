import type { NativeRustDatabase, NativeRustTransactionContext, RustDatabase, RustTransactionContext } from "./index.js";
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
export declare class InstalledRuntimeProjection {
    #private;
    constructor(native: NativeProjectionHandle);
    /** @internal Bind one generated token without exposing its native handle. */
    manager(typeKey: string, connection: RuntimeProjectionConnection): NativeProjectedManager;
}
/** Verify and install one generated package's exact projection evidence. */
export declare function installRuntimeProjection(input: RuntimeProjectionInstall): InstalledRuntimeProjection;
export {};
