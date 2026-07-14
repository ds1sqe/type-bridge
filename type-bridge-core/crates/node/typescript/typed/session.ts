import type { EntityDescriptor, RelationDescriptor } from "../index.js";
import { modelConstructorDependencies } from "../model.js";
import { loadNative } from "../native.js";
import { isRegisteredRustConnection } from "./runtime-handles.js";
import {
  createQuery,
  type NamedQueryRow,
  type NamedSelectionInput,
  type Query,
  type QueryConnection,
  type QuerySelections,
  type SelectedOutputs,
} from "./query.js";
import {
  TypedMatchError,
  createBoundVar,
  nativeCall,
  nativeSelectionHandle,
  type BoundVar,
  type QueryModelClass,
  type Selection,
} from "./references.js";

type NativeModule = ReturnType<typeof loadNative>;
type NativeRegistry = InstanceType<NativeModule["NodeDescriptorRegistry"]>;
type NativeSession = InstanceType<NativeModule["NodeMatchSessionHandle"]>;

interface QuerySessionState {
  readonly registry: NativeRegistry;
  readonly session: NativeSession;
  readonly connection: QueryConnection | undefined;
  readonly models: Map<string, QueryModelClass>;
  readonly knownModels: Set<QueryModelClass>;
}

const querySessionStates = new WeakMap<QuerySession, QuerySessionState>();
const diagnosticQuerySessionToken = Symbol(
  "type-bridge.diagnostic-query-session",
);

export type MatchMode = "exact" | "subtypes";

/**
 * Owner of one native reference-construction session.
 *
 * Models are registered lazily from their canonical descriptors. Repeated
 * calls for the same model deliberately return distinct native variables.
 */
export class QuerySession {
  readonly #registered = new WeakSet<object>();
  readonly #registering = new WeakSet<object>();

  constructor(connection: QueryConnection);
  constructor(
    connection: QueryConnection | typeof diagnosticQuerySessionToken,
  ) {
    if (
      connection !== diagnosticQuerySessionToken &&
      !isRegisteredRustConnection(connection)
    ) {
      throw new TypeError(
        "QuerySession requires a RustDatabase or RustTransactionContext",
      );
    }
    const native = loadNative();
    const registry = new native.NodeDescriptorRegistry();
    querySessionStates.set(this, {
      registry,
      session: new native.NodeMatchSessionHandle(registry),
      connection:
        connection === diagnosticQuerySessionToken ? undefined : connection,
      models: new Map(),
      knownModels: new Set(),
    });
  }

  /**
   * Register concrete constructors that JavaScript cannot discover through
   * reverse class reflection. This is session-local metadata only; it does not
   * create a query binding. Register subtype and relation-player descendants
   * here before selecting a declared base with `"subtypes"`.
   */
  registerModels<
    const Models extends readonly [QueryModelClass, ...QueryModelClass[]],
  >(...models: Models): this {
    const state = querySessionState(this);
    for (const model of models) {
      this.#requireConstructorAvailable(model);
    }
    for (const model of models) {
      state.knownModels.add(model);
    }
    for (const model of models) {
      this.#register(model);
    }
    return this;
  }

  var<Model extends QueryModelClass>(
    model: Model,
    matchMode: MatchMode = "exact",
  ): BoundVar<InstanceType<Model>> {
    querySessionState(this).knownModels.add(model);
    this.#register(model);
    if (matchMode === "subtypes") {
      const state = querySessionState(this);
      for (const descendant of modelConstructorDependencies(
        model,
        true,
        (candidate) => state.knownModels.has(candidate as QueryModelClass),
      )) {
        this.#register(descendant as QueryModelClass);
      }
    }
    const session = querySessionState(this).session;
    const modelTypeName = model.typeName;
    const handle = nativeCall(() =>
      matchMode === "exact"
        ? session.exact(modelTypeName)
        : session.subtypes(modelTypeName),
    );
    return createBoundVar(model, modelTypeName, handle);
  }

  query<const Selections extends QuerySelections>(
    ...selections: Selections
  ): Query<SelectedOutputs<Selections>> {
    requireSelectionArity(selections.length);
    const state = querySessionState(this);
    const shape = nativeCall(() =>
      state.session.positional(selections.map(nativeSelectionHandle)),
    );
    const handle = nativeCall(() => state.session.query(shape));
    return createQuery(handle, {
      registry: state.registry,
      connection: state.connection,
      models: state.models,
    });
  }

  queryNamed<const Shape extends object>(
    selections: NamedSelectionInput<Shape>,
  ): Query<readonly [NamedQueryRow<Shape>]> {
    const entries = Object.entries(selections) as [
      string,
      Selection<unknown>,
    ][];
    requireSelectionArity(entries.length);
    const state = querySessionState(this);
    const shape = nativeCall(() =>
      state.session.named(
        entries.map(([name]) => name),
        entries.map(([, selection]) => nativeSelectionHandle(selection)),
      ),
    );
    const handle = nativeCall(() => state.session.query(shape));
    return createQuery(handle, {
      registry: state.registry,
      connection: state.connection,
      models: state.models,
    });
  }

  #register(model: QueryModelClass): void {
    this.#requireConstructorAvailable(model);
    if (this.#registered.has(model)) {
      return;
    }
    if (this.#registering.has(model)) {
      return;
    }
    this.#registering.add(model);
    try {
      const state = querySessionState(this);
      state.knownModels.add(model);
      for (const dependency of modelConstructorDependencies(
        model,
        false,
        (candidate) => state.knownModels.has(candidate as QueryModelClass),
      )) {
        this.#register(dependency as QueryModelClass);
      }
      const descriptor = model.descriptor();
      const encoded = JSON.stringify(descriptor);
      const registry = querySessionState(this).registry;
      nativeCall(() => {
        if (isRelationDescriptor(descriptor)) {
          registry.registerRelationJson(encoded);
        } else {
          registry.registerEntityJson(encoded);
        }
      });
      querySessionState(this).models.set(model.typeName, model);
      this.#registered.add(model);
    } finally {
      this.#registering.delete(model);
    }
  }

  #requireConstructorAvailable(model: QueryModelClass): void {
    const existing = querySessionState(this).models.get(model.typeName);
    if (existing !== undefined && existing !== model) {
      throw new TypedMatchError(
        "invalid_plan",
        "model_constructor_conflict",
        `type '${model.typeName}' already has a different exact model constructor in this query session`,
        Object.freeze([]),
        Object.freeze({}),
      );
    }
  }
}

/** @internal Construct a source-test session that can exercise planning without execution. */
export function diagnosticQuerySession(): QuerySession {
  const DiagnosticQuerySession = QuerySession as unknown as new (
    connection: typeof diagnosticQuerySessionToken,
  ) => QuerySession;
  return new DiagnosticQuerySession(diagnosticQuerySessionToken);
}

function querySessionState(session: QuerySession): QuerySessionState {
  const state = querySessionStates.get(session);
  if (state === undefined) {
    throw new TypeError("QuerySession was not constructed by this package");
  }
  return state;
}

/** @internal Extract the opaque native session for the future Query facade. */
export function nativeSessionHandle(session: QuerySession): NativeSession {
  return querySessionState(session).session;
}

/** @internal Inspect exact constructor registration in focused binding tests. */
export function registeredModelConstructors(
  session: QuerySession,
): ReadonlyMap<string, QueryModelClass> {
  return querySessionState(session).models;
}

function isRelationDescriptor(
  descriptor: EntityDescriptor | RelationDescriptor,
): descriptor is RelationDescriptor {
  return "roles" in descriptor;
}

function requireSelectionArity(actual: number): void {
  if (actual === 0) {
    throw new TypedMatchError(
      "invalid_plan",
      "empty_output",
      "typed queries must select at least one output binding",
      Object.freeze([{ kind: "output" }]),
      Object.freeze({}),
    );
  }
  if (actual > 16) {
    throw new TypedMatchError(
      "invalid_plan",
      "selection_cap_exceeded",
      "selected output exceeds the canonical sixteen-slot ceiling",
      Object.freeze([{ kind: "output" }]),
      Object.freeze({
        limit: Object.freeze({ kind: "text", value: "selected_slots" }),
        actual: Object.freeze({ kind: "unsigned", value: actual }),
        maximum: Object.freeze({ kind: "unsigned", value: 16 }),
      }),
    );
  }
}
