import { loadNative } from "../native.js";
import type { QueryV2Authority } from "../index.js";
import {
  queryV2AuthorityHandle,
  queryV2NativeCall,
} from "../query-v2-internals.js";
import {
  type NamedQueryRow,
  type NamedSelectionInput,
  type Query,
  type QuerySelections,
  type SelectedOutputs,
} from "./query.js";
import type {
  QueryModelClass,
  Selection,
} from "./references.js";
import type { RemoteQueryLimits } from "./remote-limits.js";
import {
  createRemoteQuery,
  type RemoteQuery,
  type RemoteQueryExchange,
  type RemoteQueryRuntime,
} from "./remote-query.js";
import {
  diagnosticQuerySession,
  type QuerySession,
} from "./session.js";

/**
 * Transport-neutral remote typed-query composition over the released grammar.
 *
 * Authority, capability advertisement, and all seven limits are snapshotted
 * once. Each terminal performs one caller exchange and no retry.
 */
export class RemoteQuerySession {
  readonly #direct: QuerySession;
  readonly #runtime: RemoteQueryRuntime;

  /** Exact released binding constructor, delegated to one diagnostic session. */
  readonly var: QuerySession["var"];

  /** Exact released reachable predicate constructor. */
  readonly reachable: QuerySession["reachable"];

  constructor(
    authority: QueryV2Authority,
    advertisement: Uint8Array,
    exchange: RemoteQueryExchange,
    limits: RemoteQueryLimits,
  ) {
    const nativeAuthority = queryV2AuthorityHandle(authority);
    if (nativeAuthority === undefined) {
      throw new TypeError(
        "RemoteQuerySession requires a type-bridge QueryV2Authority",
      );
    }
    if (!(advertisement instanceof Uint8Array)) {
      throw new TypeError(
        "RemoteQuerySession advertisement must be a Uint8Array",
      );
    }
    if (typeof exchange !== "function") {
      throw new TypeError(
        "RemoteQuerySession exchange must be callable",
      );
    }
    if (typeof limits !== "object" || limits === null) {
      throw new TypeError(
        "RemoteQuerySession limits must be an object",
      );
    }

    const context = queryV2NativeCall(() =>
      loadNative().queryV2RemoteModelContext(
        nativeAuthority,
        advertisement,
        limits.maxItems,
        limits.maxBytes,
        limits.maxCollectionMembers,
        limits.maxGraphNodes,
        limits.maxAttributeValues,
        limits.maxRolePlayers,
        limits.deadlineMs,
      ),
    );
    this.#direct = diagnosticQuerySession();
    this.#runtime = Object.freeze({ context, exchange });
    this.var = this.#direct.var.bind(this.#direct) as QuerySession["var"];
    this.reachable = this.#direct.reachable.bind(
      this.#direct,
    ) as QuerySession["reachable"];
    Object.freeze(this);
  }

  registerModels<
    const Models extends readonly [
      QueryModelClass,
      ...QueryModelClass[],
    ],
  >(...models: Models): this {
    this.#direct.registerModels(...models);
    return this;
  }

  query<const Selections extends QuerySelections>(
    ...selections: Selections
  ): RemoteQuery<SelectedOutputs<Selections>> {
    const query = this.#direct.query.bind(this.#direct) as unknown as (
      ...values: readonly [
        Selection<unknown>,
        ...Selection<unknown>[],
      ]
    ) => Query<QuerySelections>;
    const direct = query(
      ...(selections as readonly [
        Selection<unknown>,
        ...Selection<unknown>[],
      ]),
    ) as unknown as Query<SelectedOutputs<Selections>>;
    return createRemoteQuery(
      direct,
      this.#runtime,
    );
  }

  queryNamed<const Shape extends object>(
    selections: NamedSelectionInput<Shape>,
  ): RemoteQuery<readonly [NamedQueryRow<Shape>]> {
    return createRemoteQuery(
      this.#direct.queryNamed(selections),
      this.#runtime,
    );
  }
}
