/** Provider JSON in native JavaScript types, preserving large integer tokens. */
export type RawValue = null | boolean | string | number | bigint | RawValue[] | { [key: string]: RawValue };

interface RawErrorFields {
  code: string;
  message: string;
  status: number;
  providerCode: string | null;
  commitOutcome: "definitely_aborted" | "unknown" | null;
}

/** Provider error or raw lifecycle refusal, including commit certainty. */
export class RawQueryError extends Error {
  readonly code: string;
  readonly status: number;
  readonly providerCode: string | null;
  readonly commitOutcome: "definitely_aborted" | "unknown" | null;
  /** @internal */
  constructor(fields: RawErrorFields) {
    super(fields.message);
    this.name = "RawQueryError";
    this.code = fields.code;
    this.status = fields.status;
    this.providerCode = fields.providerCode;
    this.commitOutcome = fields.commitOutcome;
  }
}

function rawError(error: unknown): unknown {
  if (!(error instanceof Error) || !error.message.startsWith("typebridge.raw:")) return error;
  try {
    const fields = JSON.parse(error.message.slice("typebridge.raw:".length)) as RawErrorFields;
    return new RawQueryError(fields);
  } catch {
    return error;
  }
}

/** @internal */
export function rawNativeCall<T>(operation: () => T): T {
  try { return operation(); } catch (error) { throw rawError(error); }
}
/** @internal */
export function rawNativePromise<T>(promise: Promise<T>): Promise<T> {
  return promise.catch((error: unknown) => { throw rawError(error); });
}

/** @internal */
export interface NativeRawTransaction {
  executeQuery(query: string): Promise<RawValue>;
  commit(): Promise<void>;
  rollback(): Promise<void>;
  close(): Promise<void>;
}

/** An eager direct transaction or single-query proxy raw context. */
export class RawTransaction {
  readonly #native: NativeRawTransaction;
  /** @internal */
  constructor(native: NativeRawTransaction) { this.#native = native; }

  /**
   * Raw, unchecked TypeQL: the escape hatch. The TypeQL text is sent
   * to TypeDB as written, and TypeDB's output comes back as it
   * answered, in this SDK's native data types. TypeBridge adds no
   * validation, rewriting, hydration, hooks, limits or extra
   * queries. It runs in read, write and schema transactions and
   * accepts any TypeQL, schema-breaking changes included. The caller
   * owns its correctness; TypeBridge makes no promise about its
   * effects. On a proxy connection it posts to the server's opt-in
   * `/v3/raw` endpoint, which is off by default.
   *
   * Integer tokens outside the JavaScript safe range become bigint; safe integers and doubles stay number.
   * On a proxy connection a raw transaction holds one query, because `/v3/raw` runs each request as its own transaction.
   */
  executeQuery(query: string): Promise<RawValue> {
    return rawNativePromise(rawNativeCall(() => this.#native.executeQuery(query)));
  }
  /** Commit direct writes; after a successful proxy write this is a no-op. */
  commit(): Promise<void> { return rawNativePromise(rawNativeCall(() => this.#native.commit())); }
  /** Roll back direct writes; a committed proxy write raises raw_transaction_already_committed. */
  rollback(): Promise<void> { return rawNativePromise(rawNativeCall(() => this.#native.rollback())); }
  /** Release the context without committing pending direct writes. */
  close(): Promise<void> { return rawNativePromise(rawNativeCall(() => this.#native.close())); }
}
