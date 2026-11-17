/** Millisecond timeout input; zero disables the deadline. */
export type TimeoutMilliseconds = number | bigint;
/** @internal Validate once before crossing the native boundary. */
export declare function normalizeTimeoutMilliseconds(value: TimeoutMilliseconds | undefined): bigint | undefined;
