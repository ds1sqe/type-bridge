import { Attribute, type AttributeBase } from "../attribute.js";
import { lowerAttributeValue } from "../codec.js";
import type {
  EntityDescriptor,
  RelationDescriptor,
  ValueType,
} from "../index.js";
import {
  FieldSpec,
  ListFieldSpec,
  RoleSpec,
  type AttributeClass,
  type ModelOwnerToken,
  type SchemaSpec,
} from "../model.js";
import { loadNative } from "../native.js";

type NativeModule = ReturnType<typeof loadNative>;
type NativeSessionHandle = InstanceType<NativeModule["NodeMatchSessionHandle"]>;
export type NativeBindingHandle = ReturnType<NativeSessionHandle["exact"]>;
type NativeFieldHandle = ReturnType<NativeBindingHandle["field"]>;
type NativeRoleHandle = ReturnType<NativeBindingHandle["role"]>;
type NativePredicateHandle = ReturnType<NativeFieldHandle["compareValueJson"]>;
type NativeOrderHandle = ReturnType<NativeFieldHandle["order"]>;
type NativeSelectionHandle = ReturnType<NativeBindingHandle["one"]>;

/** A model constructor accepted by the owner-aware query-reference layer. */
export type QueryModelClass<
  Model extends object = object,
  Schema extends Record<string, SchemaSpec> = Record<string, SchemaSpec>,
> = (new (values: never) => Model) & {
  readonly typeName: string;
  readonly schema: Schema;
  descriptor(): EntityDescriptor | RelationDescriptor;
};

declare const selectionBrand: unique symbol;
declare const boundVarBrand: unique symbol;
declare const collectedBrand: unique symbol;
declare const fieldRefBrand: unique symbol;
declare const roleRefBrand: unique symbol;
declare const boundFieldBrand: unique symbol;
declare const boundRoleBrand: unique symbol;
declare const predicateBrand: unique symbol;
declare const queryOrderBrand: unique symbol;

/** Covariant output-only view of one native selected binding. */
export interface Selection<out Output> {
  readonly [selectionBrand]: () => Output;
}

/** Owner-aware generated field reference; it has no public string constructor. */
export interface FieldRef<
  in out Owner extends object,
  out Attr extends AttributeClass,
> {
  readonly [fieldRefBrand]: {
    readonly owner: (value: Owner) => Owner;
    readonly attribute: Attr;
  };
}

/** Owner/player-aware generated relation-role reference. */
export interface RoleRef<
  in out RelationOwner extends object,
  in out Player extends object,
> {
  readonly [roleRefBrand]: {
    readonly owner: (value: RelationOwner) => RelationOwner;
    readonly player: (value: Player) => Player;
  };
}

/** One immutable native predicate handle. */
export interface Predicate {
  readonly [predicateBrand]: true;
  and(other: Predicate): Predicate;
  or(other: Predicate): Predicate;
  not(): Predicate;
}

/** One immutable native ordering handle. */
export interface QueryOrder {
  readonly [queryOrderBrand]: true;
}

type AttributeCategory<Attr extends AttributeClass> =
  InstanceType<Attr> extends Attribute<infer _Value, string, infer Category>
    ? Category
    : never;

type SameCategory<Left extends AttributeClass, Right extends AttributeClass> = [
  AttributeCategory<Left>,
] extends [AttributeCategory<Right>]
  ? [AttributeCategory<Right>] extends [AttributeCategory<Left>]
    ? true
    : false
  : false;

type CompatibleField<
  Left extends AttributeClass,
  Right extends AttributeClass,
> = SameCategory<Left, Right> extends true ? BoundField<Right> : never;

interface EqualityOperations<Attr extends AttributeClass> {
  eq(value: InstanceType<Attr>): Predicate;
  eq<Other extends AttributeClass>(
    field: CompatibleField<Attr, Other>,
  ): Predicate;
  eqField<Other extends AttributeClass>(
    field: CompatibleField<Attr, Other>,
  ): Predicate;
  ne(value: InstanceType<Attr>): Predicate;
  ne<Other extends AttributeClass>(
    field: CompatibleField<Attr, Other>,
  ): Predicate;
}

interface OrderedOperations<Attr extends AttributeClass> {
  gt(value: InstanceType<Attr>): Predicate;
  gt<Other extends AttributeClass>(
    field: CompatibleField<Attr, Other>,
  ): Predicate;
  gte(value: InstanceType<Attr>): Predicate;
  gte<Other extends AttributeClass>(
    field: CompatibleField<Attr, Other>,
  ): Predicate;
  lt(value: InstanceType<Attr>): Predicate;
  lt<Other extends AttributeClass>(
    field: CompatibleField<Attr, Other>,
  ): Predicate;
  lte(value: InstanceType<Attr>): Predicate;
  lte<Other extends AttributeClass>(
    field: CompatibleField<Attr, Other>,
  ): Predicate;
  asc(missing?: "reject" | "first" | "last"): QueryOrder;
  desc(missing?: "reject" | "first" | "last"): QueryOrder;
}

interface StringOperations {
  contains(value: string): Predicate;
  startsWith(value: string): Predicate;
  endsWith(value: string): Predicate;
  regex(value: string): Predicate;
}

interface BoundFieldIdentity<Attr extends AttributeClass> {
  readonly [boundFieldBrand]: (attribute: Attr) => Attr;
}

/**
 * A field bound to one native variable. Boolean fields expose equality only;
 * ordered categories add range/order methods and strings add match methods.
 */
export type BoundField<Attr extends AttributeClass> = BoundFieldIdentity<Attr> &
  EqualityOperations<Attr> &
  (AttributeCategory<Attr> extends "boolean"
    ? object
    : OrderedOperations<Attr>) &
  (AttributeCategory<Attr> extends "string" ? StringOperations : object);

type CompatibleOwner<Declared extends object, Actual extends object> = [
  ModelOwnerToken<Declared>,
] extends [never]
  ? false
  : ModelOwnerToken<Declared> extends ModelOwnerToken<Actual>
    ? true
    : false;

type CompatiblePlayer<
  Allowed extends object,
  Actual extends object,
> = Allowed extends object
  ? CompatibleOwner<Allowed, Actual> extends true
    ? Actual
    : never
  : never;

/** A role bound to one native relation variable. */
export interface BoundRole<in out Player extends object> {
  readonly [boundRoleBrand]: (player: Player) => Player;
  connects<Actual extends object>(
    player: BoundVar<Actual> &
      ([CompatiblePlayer<Player, Actual>] extends [never] ? never : object),
  ): Predicate;
  is<Actual extends object>(
    player: BoundVar<Actual> &
      ([CompatiblePlayer<Player, Actual>] extends [never] ? never : object),
  ): Predicate;
}

/** A session-owned variable and singular model selection. */
export interface BoundVar<
  in out Model extends object,
> extends Selection<Model> {
  readonly [boundVarBrand]: (model: Model) => Model;
  field<Owner extends object, Attr extends AttributeClass>(
    reference: FieldRef<Owner, Attr> &
      (CompatibleOwner<Owner, Model> extends true ? object : never),
  ): BoundField<Attr>;
  role<Owner extends object, Player extends object>(
    reference: RoleRef<Owner, Player> &
      (CompatibleOwner<Owner, Model> extends true ? object : never),
  ): BoundRole<Player>;
  collect(): Collected<Model>;
}

/** A persistent collection selection over one native binding. */
export interface Collected<in out Model extends object> extends Selection<
  readonly Model[]
> {
  readonly [collectedBrand]: (model: Model) => Model;
  distinct(distinct?: boolean): Collected<Model>;
  orderBy(order: QueryOrder): Collected<Model>;
}

type FieldAttribute<Spec> =
  Spec extends FieldSpec<infer Attr, boolean>
    ? Attr
    : Spec extends ListFieldSpec<infer Attr, boolean>
      ? Attr
      : never;

type RolePlayer<Token> = Token extends new (values: never) => infer Player
  ? Player extends object
    ? Player
    : never
  : never;

type RolePlayers<Spec> =
  Spec extends RoleSpec<infer Players> ? RolePlayer<Players[number]> : never;

/** Exact owner-aware references derived from one model's typed schema. */
export type ModelReferences<Model extends QueryModelClass> = Readonly<{
  fields: Readonly<{
    [Key in keyof Model["schema"] as Model["schema"][Key] extends
      | FieldSpec<AttributeClass, boolean>
      | ListFieldSpec<AttributeClass, boolean>
      ? Key
      : never]: FieldRef<
      InstanceType<Model>,
      FieldAttribute<Model["schema"][Key]>
    >;
  }>;
  roles: Readonly<{
    [Key in keyof Model["schema"] as Model["schema"][Key] extends RoleSpec<
      infer _Players
    >
      ? Key
      : never]: RoleRef<InstanceType<Model>, RolePlayers<Model["schema"][Key]>>;
  }>;
}>;

type FieldSpecValue =
  | FieldSpec<AttributeClass, boolean>
  | ListFieldSpec<AttributeClass, boolean>;

interface FieldReferenceState {
  readonly owner: QueryModelClass;
  readonly ownerTypeName: string;
  readonly name: string;
  readonly spec: FieldSpecValue;
  readonly attrType: AttributeClass;
}

interface RoleReferenceState {
  readonly owner: QueryModelClass;
  readonly ownerTypeName: string;
  readonly name: string;
  readonly spec: SchemaSpec;
}

const fieldReferenceStates = new WeakMap<object, FieldReferenceState>();
const roleReferenceStates = new WeakMap<object, RoleReferenceState>();

class FieldReferenceImpl {
  constructor(state: FieldReferenceState) {
    fieldReferenceStates.set(this, state);
    Object.freeze(this);
  }
}

class RoleReferenceImpl {
  constructor(state: RoleReferenceState) {
    roleReferenceStates.set(this, state);
    Object.freeze(this);
  }
}

/**
 * Derive field/role references from a model's typed schema without accepting a
 * public field or role name. Generated packages can re-export these exact
 * values as model-specific constants.
 */
export function references<Model extends QueryModelClass>(
  model: Model,
): ModelReferences<Model> {
  const fields: Record<string, FieldReferenceImpl> = Object.create(
    null,
  ) as Record<string, FieldReferenceImpl>;
  const roles: Record<string, RoleReferenceImpl> = Object.create(
    null,
  ) as Record<string, RoleReferenceImpl>;
  for (const [name, spec] of Object.entries(model.schema)) {
    if (spec instanceof FieldSpec || spec instanceof ListFieldSpec) {
      fields[name] = new FieldReferenceImpl({
        owner: model,
        ownerTypeName: model.typeName,
        name,
        spec,
        attrType: spec.attrType,
      });
    } else if (spec instanceof RoleSpec) {
      roles[name] = new RoleReferenceImpl({
        owner: model,
        ownerTypeName: model.typeName,
        name,
        spec,
      });
    }
  }
  return Object.freeze({
    fields: Object.freeze(fields),
    roles: Object.freeze(roles),
  }) as ModelReferences<Model>;
}

/** Stable canonical match-error categories surfaced by Rust. */
export type TypedMatchErrorCategory =
  | "invalid_plan"
  | "cardinality"
  | "unsupported_capability"
  | "stale_schema"
  | "resource_limit"
  | "provider"
  | "result_decode";

export type TypedMatchErrorPathSegment =
  | Readonly<{
      kind:
        | "request"
        | "plan"
        | "operation"
        | "predicate"
        | "output"
        | "provider_evidence"
        | "result";
    }>
  | Readonly<{
      kind: "binding" | "role_edge" | "output_slot" | "index";
      value: number;
    }>
  | Readonly<{ kind: "output_name"; value: string }>
  | Readonly<{
      kind: "field" | "role";
      value: Readonly<{ owner: string; name: string }>;
    }>;

export type TypedMatchErrorDetail =
  | Readonly<{ kind: "text"; value: string }>
  | Readonly<{ kind: "unsigned" | "signed"; value: number }>
  | Readonly<{ kind: "boolean"; value: boolean }>
  | Readonly<{ kind: "text_list"; value: readonly string[] }>;

export class TypedMatchError extends Error {
  readonly name = "TypedMatchError";

  constructor(
    readonly category: TypedMatchErrorCategory,
    readonly code: string,
    message: string,
    readonly path: readonly TypedMatchErrorPathSegment[],
    readonly details: Readonly<Record<string, TypedMatchErrorDetail>>,
  ) {
    super(message);
  }
}

interface NativeMatchErrorPayload {
  readonly category: TypedMatchErrorCategory;
  readonly code: string;
  readonly message: string;
  readonly path: readonly TypedMatchErrorPathSegment[];
  readonly details: Readonly<Record<string, TypedMatchErrorDetail>>;
}

function isNativeMatchPayload(value: object): value is NativeMatchErrorPayload {
  return (
    "category" in value &&
    typeof value.category === "string" &&
    "code" in value &&
    typeof value.code === "string" &&
    "message" in value &&
    typeof value.message === "string" &&
    "path" in value &&
    Array.isArray(value.path) &&
    "details" in value &&
    typeof value.details === "object" &&
    value.details !== null
  );
}

/** @internal Invoke one native transition and preserve structured match errors. */
export function nativeCall<Result>(operation: () => Result): Result {
  try {
    return operation();
  } catch (error) {
    if (error instanceof Error) {
      try {
        const payload: object = JSON.parse(error.message) as object;
        if (isNativeMatchPayload(payload)) {
          throw new TypedMatchError(
            payload.category,
            payload.code,
            payload.message,
            payload.path,
            payload.details,
          );
        }
      } catch (payloadError) {
        if (payloadError instanceof TypedMatchError) {
          throw payloadError;
        }
      }
    }
    throw error;
  }
}

export class TypedReferenceError extends TypeError {
  readonly name = "TypedReferenceError";
}

interface BoundVariableState {
  readonly model: QueryModelClass;
  readonly modelTypeName: string;
  readonly handle: NativeBindingHandle;
}

const boundVariableStates = new WeakMap<object, BoundVariableState>();
const collectedStates = new WeakMap<object, NativeSelectionHandle>();
const boundFieldStates = new WeakMap<
  object,
  Readonly<{ handle: NativeFieldHandle; attrType: AttributeClass }>
>();
const boundRoleStates = new WeakMap<object, NativeRoleHandle>();
const predicateStates = new WeakMap<object, NativePredicateHandle>();
const orderStates = new WeakMap<object, NativeOrderHandle>();

class PredicateImpl implements Predicate {
  declare readonly [predicateBrand]: true;

  constructor(handle: NativePredicateHandle) {
    predicateStates.set(this, handle);
    Object.freeze(this);
  }

  and(other: Predicate): Predicate {
    return new PredicateImpl(
      nativeCall(() => predicateHandle(this).and(predicateHandle(other))),
    );
  }

  or(other: Predicate): Predicate {
    return new PredicateImpl(
      nativeCall(() => predicateHandle(this).or(predicateHandle(other))),
    );
  }

  not(): Predicate {
    return new PredicateImpl(nativeCall(() => predicateHandle(this).not()));
  }
}

class QueryOrderImpl implements QueryOrder {
  declare readonly [queryOrderBrand]: true;

  constructor(handle: NativeOrderHandle) {
    orderStates.set(this, handle);
    Object.freeze(this);
  }
}

class BoundFieldImpl<Attr extends AttributeClass> {
  declare readonly [boundFieldBrand]: (attribute: Attr) => Attr;

  constructor(handle: NativeFieldHandle, attrType: Attr) {
    boundFieldStates.set(this, { handle, attrType });
    Object.freeze(this);
  }

  eq(value: InstanceType<Attr> | BoundField<AttributeClass>): Predicate {
    return this.compare("equal", value);
  }

  eqField(value: BoundField<AttributeClass>): Predicate {
    return this.compare("equal", value);
  }

  ne(value: InstanceType<Attr> | BoundField<AttributeClass>): Predicate {
    return this.compare("not_equal", value);
  }

  gt(value: InstanceType<Attr> | BoundField<AttributeClass>): Predicate {
    return this.compare("greater_than", value);
  }

  gte(value: InstanceType<Attr> | BoundField<AttributeClass>): Predicate {
    return this.compare("greater_than_or_equal", value);
  }

  lt(value: InstanceType<Attr> | BoundField<AttributeClass>): Predicate {
    return this.compare("less_than", value);
  }

  lte(value: InstanceType<Attr> | BoundField<AttributeClass>): Predicate {
    return this.compare("less_than_or_equal", value);
  }

  contains(value: string): Predicate {
    return this.compareString("contains", value);
  }

  startsWith(value: string): Predicate {
    return this.compareString("starts_with", value);
  }

  endsWith(value: string): Predicate {
    return this.compareString("ends_with", value);
  }

  regex(value: string): Predicate {
    return this.compareString("regex", value);
  }

  asc(missing: "reject" | "first" | "last" = "reject"): QueryOrder {
    return this.order("ascending", missing);
  }

  desc(missing: "reject" | "first" | "last" = "reject"): QueryOrder {
    return this.order("descending", missing);
  }

  private compare(
    comparison:
      | "equal"
      | "not_equal"
      | "greater_than"
      | "greater_than_or_equal"
      | "less_than"
      | "less_than_or_equal",
    value: InstanceType<Attr> | BoundField<AttributeClass>,
  ): Predicate {
    const own = boundFieldState(this);
    const other = boundFieldStates.get(value as object);
    if (other !== undefined) {
      return new PredicateImpl(
        nativeCall(() => own.handle.compareField(comparison, other.handle)),
      );
    }
    if (!(value instanceof own.attrType)) {
      throw new TypedReferenceError(
        `comparison value must be an instance of ${own.attrType.attrName}`,
      );
    }
    const literal = value as {
      readonly value: string | number | bigint | boolean;
    };
    const lowered = lowerAttributeValue(own.attrType.valueType, literal.value);
    return new PredicateImpl(
      nativeCall(() =>
        own.handle.compareValueJson(comparison, JSON.stringify(lowered)),
      ),
    );
  }

  private compareString(
    comparison: "contains" | "starts_with" | "ends_with" | "regex",
    value: string,
  ): Predicate {
    const own = boundFieldState(this);
    return new PredicateImpl(
      nativeCall(() =>
        own.handle.compareValueJson(
          comparison,
          JSON.stringify({ value_type: "string", value }),
        ),
      ),
    );
  }

  private order(
    direction: "ascending" | "descending",
    missing: "reject" | "first" | "last",
  ): QueryOrder {
    const own = boundFieldState(this);
    return new QueryOrderImpl(
      nativeCall(() => own.handle.order(direction, missing)),
    );
  }
}

class BoundRoleImpl<Player extends object> implements BoundRole<Player> {
  declare readonly [boundRoleBrand]: (player: Player) => Player;

  constructor(handle: NativeRoleHandle) {
    boundRoleStates.set(this, handle);
    Object.freeze(this);
  }

  connects<Actual extends object>(
    player: BoundVar<Actual> &
      ([CompatiblePlayer<Player, Actual>] extends [never] ? never : object),
  ): Predicate {
    return new PredicateImpl(
      nativeCall(() =>
        boundRoleHandle(this).connects(boundVariableState(player).handle),
      ),
    );
  }

  is<Actual extends object>(
    player: BoundVar<Actual> &
      ([CompatiblePlayer<Player, Actual>] extends [never] ? never : object),
  ): Predicate {
    return this.connects(player);
  }
}

class BoundVarImpl<Model extends object> implements BoundVar<Model> {
  declare readonly [selectionBrand]: () => Model;
  declare readonly [boundVarBrand]: (model: Model) => Model;

  constructor(
    model: QueryModelClass<Model>,
    modelTypeName: string,
    handle: NativeBindingHandle,
  ) {
    boundVariableStates.set(this, { model, modelTypeName, handle });
    Object.freeze(this);
  }

  field<Owner extends object, Attr extends AttributeClass>(
    reference: FieldRef<Owner, Attr> &
      (CompatibleOwner<Owner, Model> extends true ? object : never),
  ): BoundField<Attr> {
    const state = fieldReferenceStates.get(reference as object);
    if (state === undefined) {
      throw new TypedReferenceError(
        "field reference was not created by references(model)",
      );
    }
    const variable = boundVariableState(this);
    const modelTypeName = stableBoundVariableTypeName(variable);
    if (!Object.values(variable.model.schema).includes(state.spec)) {
      throw new TypedReferenceError(
        `field reference '${state.name}' does not belong to ${modelTypeName}`,
      );
    }
    const ownerTypeName = stableReferenceOwnerTypeName(state);
    if (ownerTypeName === modelTypeName && state.owner !== variable.model) {
      throw new TypedReferenceError(
        "field reference owner does not match the bound model",
      );
    }
    const handle = nativeCall(() =>
      variable.handle.fieldOwnedBy(ownerTypeName, state.name),
    );
    return new BoundFieldImpl(
      handle,
      state.attrType as Attr,
    ) as BoundField<Attr>;
  }

  role<Owner extends object, Player extends object>(
    reference: RoleRef<Owner, Player> &
      (CompatibleOwner<Owner, Model> extends true ? object : never),
  ): BoundRole<Player> {
    const state = roleReferenceStates.get(reference as object);
    if (state === undefined) {
      throw new TypedReferenceError(
        "role reference was not created by references(model)",
      );
    }
    const variable = boundVariableState(this);
    const modelTypeName = stableBoundVariableTypeName(variable);
    if (!Object.values(variable.model.schema).includes(state.spec)) {
      throw new TypedReferenceError(
        `role reference '${state.name}' does not belong to ${modelTypeName}`,
      );
    }
    const ownerTypeName = stableReferenceOwnerTypeName(state);
    if (ownerTypeName === modelTypeName && state.owner !== variable.model) {
      throw new TypedReferenceError(
        "role reference owner does not match the bound model",
      );
    }
    return new BoundRoleImpl(
      nativeCall(() => variable.handle.roleOwnedBy(ownerTypeName, state.name)),
    ) as BoundRole<Player>;
  }

  collect(): Collected<Model> {
    return new CollectedImpl<Model>(
      nativeCall(() => boundVariableState(this).handle.collect()),
    );
  }
}

class CollectedImpl<Model extends object> implements Collected<Model> {
  declare readonly [selectionBrand]: () => readonly Model[];
  declare readonly [collectedBrand]: (model: Model) => Model;

  constructor(handle: NativeSelectionHandle) {
    collectedStates.set(this, handle);
    Object.freeze(this);
  }

  distinct(distinct = true): Collected<Model> {
    return new CollectedImpl(
      nativeCall(() => collectedHandle(this).distinct(distinct)),
    );
  }

  orderBy(order: QueryOrder): Collected<Model> {
    return new CollectedImpl(
      nativeCall(() => collectedHandle(this).orderBy(orderHandle(order))),
    );
  }
}

function stableReferenceOwnerTypeName(
  state: FieldReferenceState | RoleReferenceState,
): string {
  let currentTypeName: string;
  try {
    currentTypeName = state.owner.typeName;
  } catch {
    throw new TypedReferenceError(
      "reference owner type name changed after references(model)",
    );
  }
  if (currentTypeName !== state.ownerTypeName) {
    throw new TypedReferenceError(
      "reference owner type name changed after references(model)",
    );
  }
  return state.ownerTypeName;
}

function stableBoundVariableTypeName(state: BoundVariableState): string {
  let currentTypeName: string;
  try {
    currentTypeName = state.model.typeName;
  } catch {
    throw new TypedReferenceError(
      "bound variable model type name changed after QuerySession.var",
    );
  }
  if (currentTypeName !== state.modelTypeName) {
    throw new TypedReferenceError(
      "bound variable model type name changed after QuerySession.var",
    );
  }
  return state.modelTypeName;
}

function boundVariableState(variable: object): BoundVariableState {
  const state = boundVariableStates.get(variable);
  if (state === undefined) {
    throw new TypedReferenceError(
      "bound variable was not created by QuerySession.var",
    );
  }
  return state;
}

function boundFieldState(field: object): Readonly<{
  handle: NativeFieldHandle;
  attrType: AttributeClass;
}> {
  const state = boundFieldStates.get(field);
  if (state === undefined) {
    throw new TypedReferenceError(
      "bound field was not created from a BoundVar",
    );
  }
  return state;
}

function boundRoleHandle(role: object): NativeRoleHandle {
  const handle = boundRoleStates.get(role);
  if (handle === undefined) {
    throw new TypedReferenceError("bound role was not created from a BoundVar");
  }
  return handle;
}

function predicateHandle(predicate: object): NativePredicateHandle {
  const handle = predicateStates.get(predicate);
  if (handle === undefined) {
    throw new TypedReferenceError(
      "predicate was not created by a bound reference",
    );
  }
  return handle;
}

function orderHandle(order: object): NativeOrderHandle {
  const handle = orderStates.get(order);
  if (handle === undefined) {
    throw new TypedReferenceError("order was not created by a bound field");
  }
  return handle;
}

function collectedHandle(selection: object): NativeSelectionHandle {
  const handle = collectedStates.get(selection);
  if (handle === undefined) {
    throw new TypedReferenceError("collection was not created by a BoundVar");
  }
  return handle;
}

/** @internal Construct a public variable around one opaque native binding. */
export function createBoundVar<Model extends QueryModelClass>(
  model: Model,
  modelTypeName: string,
  handle: NativeBindingHandle,
): BoundVar<InstanceType<Model>> {
  return new BoundVarImpl<InstanceType<Model>>(
    model as never,
    modelTypeName,
    handle,
  );
}

/** @internal Extract a fresh/native selection for the future Query facade. */
export function nativeSelectionHandle(
  selection: Selection<unknown>,
): NativeSelectionHandle {
  const variable = boundVariableStates.get(selection as object);
  if (variable !== undefined) {
    return nativeCall(() => variable.handle.one());
  }
  return collectedHandle(selection as object);
}

/** @internal Extract the binding behind a session-owned variable. */
export function nativeBindingHandle<Model extends object>(
  variable: BoundVar<Model>,
): NativeBindingHandle {
  return boundVariableState(variable).handle;
}

/** @internal Extract the native field retained by one owner-aware bound field. */
export function nativeBoundFieldHandle<Attr extends AttributeClass>(
  field: BoundField<Attr>,
): NativeFieldHandle {
  return boundFieldState(field).handle;
}

/** @internal Inspect the canonical value category retained by a bound field. */
export function boundFieldValueCategory<Attr extends AttributeClass>(
  field: BoundField<Attr>,
): AttributeValueCategory<Attr> {
  return boundFieldState(field).attrType.valueType as AttributeValueCategory<Attr>;
}

/** @internal Inspect one owner-aware role reference during predicate construction. */
export function queryRoleReference(
  reference: object,
): Readonly<{
  owner: QueryModelClass;
  ownerTypeName: string;
  name: string;
}> {
  const state = roleReferenceStates.get(reference as object);
  if (state === undefined) {
    throw new TypedReferenceError(
      "role reference was not created by references(model)",
    );
  }
  return Object.freeze({
    owner: state.owner,
    ownerTypeName: stableReferenceOwnerTypeName(state),
    name: state.name,
  });
}

/** @internal Construct a public predicate around one opaque native handle. */
export function createPredicate(handle: NativePredicateHandle): Predicate {
  return new PredicateImpl(handle);
}

/** @internal Extract a native predicate for the future Query facade. */
export function nativePredicateHandle(
  predicate: Predicate,
): NativePredicateHandle {
  return predicateHandle(predicate);
}

/** @internal Extract a native order for the future Query facade. */
export function nativeQueryOrderHandle(order: QueryOrder): NativeOrderHandle {
  return orderHandle(order);
}

/** Wire category retained by a branded attribute constructor. */
export type AttributeValueCategory<Attr extends AttributeClass> =
  AttributeCategory<Attr> & ValueType;

/** Convenience constraint for generated attributes. */
export type TypedAttributeBase<
  Value,
  Brand extends string,
  Category extends ValueType,
> = AttributeBase<Value, Brand, Category>;
