import type { EntityDescriptor, RelationDescriptor } from "../index.js";
import {
  modelConstructorDependencies,
  type ModelOwnerToken,
} from "../model.js";
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
  TypedReferenceError,
  createBoundVar,
  createPredicate,
  nativeBindingHandle,
  nativeCall,
  nativeSelectionHandle,
  queryRoleReference,
  type BoundVar,
  type Predicate,
  type QueryModelClass,
  type RoleRef,
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

/** Inclusive minimum and maximum hop counts for bounded reachability. */
export interface ReachabilityBounds {
  readonly minDepth: number;
  readonly maxDepth: number;
}

type ReachabilityRelationClass<Model extends object = object> =
  QueryModelClass<Model> & {
    descriptor(): RelationDescriptor;
  };

type CompatibleOwner<Declared extends object, Actual extends object> = [
  ModelOwnerToken<Declared>,
] extends [never]
  ? false
  : ModelOwnerToken<Declared> extends ModelOwnerToken<Actual>
    ? true
    : false;

type CompatibleEndpoint<
  Allowed extends object,
  Actual extends object,
> = Allowed extends object
  ? CompatibleOwner<Allowed, Actual> extends true
    ? Actual
    : never
  : never;

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

  /**
   * Require one finite directed walk from `source` to `target`.
   *
   * Each hop uses the exact relation type in `roleFrom -> roleTo` order.
   * Bounds are inclusive. Depth zero means exact concept identity; positive
   * paths may revisit vertices or relation instances. Proof paths are
   * existential and never alter the selected `Query` output type or ordering;
   * stable ordering remains controlled by explicit query order terms.
   */
  reachable<
    Source extends object,
    Target extends object,
    Relation extends ReachabilityRelationClass,
    FromOwner extends object,
    FromPlayer extends object,
    ToOwner extends object,
    ToPlayer extends object,
  >(
    source: BoundVar<Source> &
      ([CompatibleEndpoint<FromPlayer, Source>] extends [never]
        ? never
        : object),
    target: BoundVar<Target> &
      ([CompatibleEndpoint<ToPlayer, Target>] extends [never] ? never : object),
    relation: Relation &
      (CompatibleOwner<FromOwner, InstanceType<Relation>> extends true
        ? object
        : never) &
      (CompatibleOwner<ToOwner, InstanceType<Relation>> extends true
        ? object
        : never),
    roleFrom: RoleRef<FromOwner, FromPlayer>,
    roleTo: RoleRef<ToOwner, ToPlayer>,
    bounds: ReachabilityBounds,
  ): Predicate {
    const { minDepth, maxDepth } = requireReachabilityBounds(bounds);
    const sourceHandle = nativeBindingHandle(source);
    const targetHandle = nativeBindingHandle(target);
    const relationTypeName = requireReachabilityRelation(relation);
    const from = queryRoleReference(roleFrom);
    const to = queryRoleReference(roleTo);

    requireReachabilityRoleOwner(relation, from, "roleFrom");
    requireReachabilityRoleOwner(relation, to, "roleTo");

    this.#register(relation);
    const handle = nativeCall(() =>
      querySessionState(this).session.reachable(
        relationTypeName,
        from.name,
        to.name,
        sourceHandle,
        targetHandle,
        minDepth,
        maxDepth,
      ),
    );
    return createPredicate(handle);
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

function requireReachabilityBounds(
  bounds: ReachabilityBounds,
): ReachabilityBounds {
  if (typeof bounds !== "object" || bounds === null || Array.isArray(bounds)) {
    throw new TypeError(
      "reachable bounds must be an object with minDepth and maxDepth",
    );
  }
  return Object.freeze({
    minDepth: requireReachabilityDepth(bounds.minDepth, "minDepth"),
    maxDepth: requireReachabilityDepth(bounds.maxDepth, "maxDepth"),
  });
}

function requireReachabilityDepth(value: unknown, name: string): number {
  if (typeof value !== "number" || !Number.isInteger(value)) {
    throw new TypeError(`${name} must be an integer`);
  }
  if (value < 0 || value > 255) {
    throw new RangeError(`${name} must be between 0 and 255`);
  }
  return value;
}

function requireReachabilityRelation(
  relation: ReachabilityRelationClass,
): string {
  if (
    typeof relation !== "function" ||
    typeof relation.typeName !== "string" ||
    relation.typeName.length === 0 ||
    typeof relation.descriptor !== "function"
  ) {
    throw new TypeError(
      "reachable relation must be a declared Relation model class",
    );
  }
  const descriptor = relation.descriptor();
  if (
    !isRelationDescriptor(descriptor) ||
    descriptor.type_name !== relation.typeName
  ) {
    throw new TypeError(
      "reachable relation must be a declared Relation model class",
    );
  }
  return relation.typeName;
}

function relationModelIncludesOwner(
  relation: ReachabilityRelationClass,
  owner: QueryModelClass,
): boolean {
  const visited = new Set<object>();
  let current: QueryModelClass | undefined = relation;
  while (current !== undefined && !visited.has(current)) {
    if (
      current === owner ||
      Object.prototype.isPrototypeOf.call(owner, current)
    ) {
      return true;
    }
    visited.add(current);
    const descriptor = current.descriptor();
    if (!isRelationDescriptor(descriptor) || descriptor.parent_type === null) {
      return false;
    }
    current = modelConstructorDependencies(current)
      .map((candidate) => candidate as QueryModelClass)
      .find((candidate) => {
        if (candidate.typeName !== descriptor.parent_type) {
          return false;
        }
        return isRelationDescriptor(candidate.descriptor());
      });
  }
  return false;
}

function requireReachabilityRoleOwner(
  relation: ReachabilityRelationClass,
  reference: Readonly<{
    owner: QueryModelClass;
    ownerTypeName: string;
  }>,
  name: "roleFrom" | "roleTo",
): void {
  if (!relationModelIncludesOwner(relation, reference.owner)) {
    throw new TypedReferenceError(
      `${name} must belong to the relation model or one of its declared ancestors; received owner '${reference.ownerTypeName}'`,
    );
  }
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
