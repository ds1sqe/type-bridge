import type { Attribute } from "./attribute.js";
import type { ValueType } from "./index.js";
import type {
  Annotation,
  AttributeSchemaEntry,
  EntityDescriptor,
  OwnedAttributeDescriptor,
  RelationDescriptor,
  RoleDescriptor,
} from "./index.js";
import {
  formatTypeName,
  resolveFlags,
  TypeFlags,
  type CardSpec,
  type FlagInput,
  type ResolvedTypeFlags,
} from "./flags.js";
import { defineIidSlot, type IidBearing } from "./iid.js";
import {
  entityManagerFor,
  relationManagerFor,
  type ManagerConnection,
  type TypedEntityManager,
  type TypedRelationManager,
} from "./manager.js";
import {
  TypedCodecError,
  attributeToPlain,
  plainToAttribute,
} from "./codec.js";

export type AttributeClass = (new (value: never) => Attribute<unknown, string>) & {
  readonly attrName: string;
  readonly valueType: ValueType;
  readonly attributeSchema: AttributeSchemaEntry;
  readonly attributeSchemaEntries: readonly AttributeSchemaEntry[];
};
type ModelClassLike = (new (values: never) => object) & {
  readonly typeName: string;
};
declare const modelClassBrand: unique symbol;
declare const modelOwnerBrand: unique symbol;
type ModelToken = string | ModelClassLike;
const ATTRIBUTE_SCHEMA_METADATA = Symbol.for("@type-bridge/node.attributeSchemaMetadata");
const modelParents = new WeakMap<object, ParentModelClass | null>();
const modelChildren = new WeakMap<object, Set<ModelDependencyClass>>();

type ModelDependencyClass = ModelClassLike & {
  readonly schema: Record<string, SchemaSpec>;
};

/**
 * A model class that also exposes its schema, used as a parent reference.
 * The schema constraint lets `Entity()` / `Relation()` merge parent fields into
 * the child at both the type level and the descriptor-emission level.
 */
export type ParentModelClass<
  ParentSchema extends Record<string, SchemaSpec> = Record<string, SchemaSpec>,
> =
  ModelClassLike & {
    readonly schema: ParentSchema;
  };

/**
 * Options accepted as the third argument to `Entity()` / `Relation()` for
 * declaring a parent (supertype) model.
 */
export interface ParentOption<
  ParentSchema extends Record<string, SchemaSpec>,
> {
  readonly parent: ParentModelClass<ParentSchema>;
}

type OwnerBrandedParentOption<
  ParentSchema extends Record<string, SchemaSpec>,
  ParentOwners extends string,
> = Readonly<{
  parent: ParentModelClass<ParentSchema> & {
    readonly [modelClassBrand]: ParentOwners;
  };
}>;

/**
 * Merge two schema records: parent fields followed by child-local fields.
 * Child keys shadow parent keys when both declare the same field name.
 */
export type MergedSchema<
  ParentSchema extends Record<string, SchemaSpec>,
  ChildSchema extends Record<string, SchemaSpec>,
> = Omit<ParentSchema, keyof ChildSchema> & ChildSchema;

/**
 * A declared owned-attribute field: the attribute class, its resolved flags, and
 * whether it is optional. Produced by `field()`; consumed by the model factory to
 * type construction and to emit `owned_attributes`.
 */
export class FieldSpec<Attr extends AttributeClass, Optional extends boolean = false> {
  readonly kind = "field";
  readonly flags;

  constructor(
    readonly attrType: Attr,
    flags: readonly FlagInput[] = [],
    readonly isOptional: Optional = false as Optional,
  ) {
    this.flags = resolveFlags(flags);
  }

  optional(): FieldSpec<Attr, true> {
    return new FieldSpec(this.attrType, [this.flags], true);
  }

  /**
   * Declare this field as a multi-value (list) attribute. Pass a `Card(min, max)`
   * cardinality bound; an unbounded list uses `Card(0)` (max = null).
   *
   * The returned `ListFieldSpec` emits `{"Card":[min,max|null]}` in `annotations`
   * and sets `is_optional` when `card_min == 0` (mirroring `_is_optional` in
   * `type_bridge/_rust_runtime.py`). The typed value is `Attr[] | undefined`
   * when optional, `Attr[]` when required.
   */
  list<const Min extends number, const Max extends number | null>(
    card: CardSpec<Min, Max>,
  ): ListFieldSpec<Attr, Min extends 0 ? true : false> {
    return new ListFieldSpec(
      this.attrType,
      card,
      this.flags.isOrdered,
      this.flags.isDistinct,
      this.flags.doc,
      this.flags.meta,
      this.flags.annotations,
    ) as ListFieldSpec<Attr, Min extends 0 ? true : false>;
  }

  /** Declare an ordered TypeDB list (`owns attr[]`), optionally with a card. */
  ordered(): ListFieldSpec<Attr, Optional>;
  ordered<const Min extends number, const Max extends number | null>(
    card: CardSpec<Min, Max>,
  ): ListFieldSpec<Attr, Min extends 0 ? true : false>;
  ordered(card: CardSpec | null = null): ListFieldSpec<Attr, boolean> {
    return new ListFieldSpec(
      this.attrType,
      card,
      true,
      this.flags.isDistinct,
      this.flags.doc,
      this.flags.meta,
      this.flags.annotations,
      card === null ? this.isOptional : null,
    ) as ListFieldSpec<Attr, boolean>;
  }
}

/**
 * A declared multi-value (list) owned-attribute field. Produced by
 * `field(Attr).list(Card(min, max))`; emits a `{"Card":[min,max|null]}`
 * annotation. Analogous to the Python ORM's `card_min`/`card_max` flags.
 *
 * `is_optional` is derived from `card_min == 0`, matching `_is_optional` in
 * `type_bridge/_rust_runtime.py`.
 */
export class ListFieldSpec<Attr extends AttributeClass, Optional extends boolean = false> {
  readonly kind = "list-field";
  /** Explicit cardinality `[min, max | null]`, or null for bare ordered lists. */
  readonly card: [number, number | null] | null;
  readonly isOptional: Optional;
  /** True when the parent FieldSpec carried the `Ordered` flag. A multi-value
   * `Card` field is a set, not a TypeDB list: only the `Ordered` flag makes
   * the descriptor emit `is_ordered: true` (and `[]` in the define block). */
  readonly isOrdered: boolean;
  /** True when the parent FieldSpec carried the `Distinct` flag. */
  readonly isDistinct: boolean;
  /** TypeDB 3.12+ `@doc("...")` from the parent FieldSpec's flags. */
  readonly doc: string | null;
  /** TypeDB 3.12+ `@meta` annotations from the parent FieldSpec's flags. */
  readonly meta: Record<string, string>;
  /** Non-cardinality annotations retained from the parent field flags. */
  readonly annotations: readonly Annotation[];

  constructor(
    readonly attrType: Attr,
    cardSpec: CardSpec | null,
    isOrdered = false,
    isDistinct = false,
    doc: string | null = null,
    meta: Record<string, string> = {},
    annotations: readonly Annotation[] = [],
    optional: Optional | null = null,
  ) {
    this.card = cardSpec === null ? null : [cardSpec.min, cardSpec.max];
    // A list field is optional when the minimum cardinality is 0, mirroring the
    // Python `_is_optional` rule: `flags.card_min == 0`.
    this.isOptional = (
      optional ?? (cardSpec === null || cardSpec.min === 0)
    ) as Optional;
    this.isOrdered = isOrdered;
    this.isDistinct = isDistinct;
    this.doc = doc;
    this.meta = { ...meta };
    this.annotations = Object.freeze(
      annotations
        .filter((annotation) => typeof annotation === "string")
        .map(copyAnnotation),
    );
  }


  /** Add `@distinct` to an ordered list while preserving its value shape. */
  distinct(): ListFieldSpec<Attr, Optional> {
    if (!this.isOrdered) {
      throw new TypeError("distinct() requires an ordered list field");
    }
    const cardSpec = this.card === null
      ? null
      : { kind: "card" as const, min: this.card[0], max: this.card[1] };
    return new ListFieldSpec(
      this.attrType,
      cardSpec,
      this.isOrdered,
      true,
      this.doc,
      this.meta,
      this.annotations,
      this.isOptional,
    );
  }
}

/**
 * A declared relation role: its permitted player model(s), optional
 * relates-side cardinality, and optional plays-side cardinality. Produced by
 * `role()`; consumed by the model factory to emit `roles[*]` plus metadata
 * used by `DescriptorRegistry.schemaInfo()`. Multi-player roles list more than
 * one player token.
 */
export class RoleSpec<Players extends readonly ModelToken[]> {
  readonly kind = "role";
  readonly cardinality: [number, number | null] | null;
  readonly playsCardinality: [number, number | null] | null;
  /** Parent role name this role specializes via TypeDB's `relates child as parent` syntax.
   * Used only for descriptor computation (effective-set role exclusion); specialization
   * semantics are resolved at schema-define time. */
  readonly overrides: string | undefined;
  /** When ``true``, this role is abstract at the TypeDB schema level (``@abstract`` on
   * the ``relates`` clause). The engine rejects direct players at the declaring
   * relation's own scope; subtypes that plain-inherit or override the role are
   * unaffected. */
  readonly isAbstract: boolean;
  /** When ``true``, declares this role as a list role (``relates name[]`` in TypeQL).
   * Schema-only; instance-level list writes are not yet supported by the engine. */
  readonly ordered: boolean;
  /** When ``true``, emits ``@distinct`` on the relates clause. Requires ``ordered``. */
  readonly distinct: boolean;
  /** TypeDB 3.12+ `@doc("...")` documentation for the relates clause. */
  readonly doc: string | null;
  /** TypeDB 3.12+ `@meta("key", "value")` annotations for the relates clause. */
  readonly meta: Record<string, string>;

  constructor(
    readonly players: Players,
    cardinality?: CardSpec | null,
    playsCardinality?: CardSpec | null,
    overrides?: string,
    isAbstract?: boolean,
    ordered?: boolean,
    distinct?: boolean,
    doc?: string | null,
    meta?: Record<string, string>,
  ) {
    if (playsCardinality != null && players.length === 0) {
      throw new TypeError("playsCardinality requires at least one role player");
    }
    if (distinct && !ordered) {
      throw new TypeError(
        "RoleSpec: distinct requires ordered — @distinct is only valid on a list role (relates name[]).",
      );
    }
    this.cardinality = cardinality == null ? null : [cardinality.min, cardinality.max];
    this.playsCardinality =
      playsCardinality == null ? null : [playsCardinality.min, playsCardinality.max];
    this.overrides = overrides;
    this.isAbstract = isAbstract ?? false;
    this.ordered = ordered ?? false;
    this.distinct = distinct ?? false;
    this.doc = doc ?? null;
    this.meta = { ...(meta ?? {}) };
  }
}

export type SchemaSpec =
  | FieldSpec<AttributeClass, boolean>
  | ListFieldSpec<AttributeClass, boolean>
  | RoleSpec<readonly ModelToken[]>;
export type EntitySchema = Record<string, FieldSpec<AttributeClass, boolean> | ListFieldSpec<AttributeClass, boolean>>;
export type RelationSchema = Record<string, SchemaSpec>;

/** Value accepted for one model-constructor field. */
export type FieldValue<Spec> = Spec extends ListFieldSpec<infer Attr, boolean>
  ? InstanceType<Attr>[]
  : Spec extends FieldSpec<infer Attr, boolean>
    ? InstanceType<Attr>
    : Spec extends RoleSpec<infer Players>
      ? RoleValue<Players>
      : never;

type MaterializedFieldValue<Spec> = Spec extends RoleSpec<infer Players>
  ? MaterializedRoleValue<Players>
  : FieldValue<Spec>;

/**
 * Fields exposed by a materialized model. Relation-valued role players are
 * shallow: their IID and attributes are present, while their own role fields
 * are explicitly `undefined` to prevent recursive graph hydration.
 */
export type InstanceFields<Schema extends Record<string, SchemaSpec>> = {
  readonly [Key in keyof Schema]: Schema[Key] extends
    | FieldSpec<AttributeClass, true>
    | ListFieldSpec<AttributeClass, true>
    ? MaterializedFieldValue<Schema[Key]> | undefined
    : MaterializedFieldValue<Schema[Key]>;
};

/**
 * The plain primitive value corresponding to a schema field spec, used as the
 * value type in the `toDict()` / `fromDict()` canonical dict shape.
 *
 * - `FieldSpec<Attr, false>` → the raw `.value` type of the attribute instance
 * - `FieldSpec<Attr, true>` → raw `.value` type or `undefined` (optional field)
 * - `ListFieldSpec<Attr, *>` → array of raw `.value` type (optional list is
 *   included only when the list is non-empty; absent optional list is omitted)
 *
 * `RoleSpec` fields are deliberately excluded — `toDict`/`fromDict` are for
 * attribute serialization only.
 */
export type PlainFieldValue<Spec> = Spec extends ListFieldSpec<infer Attr, boolean>
  ? InstanceType<Attr>["value"][]
  : Spec extends FieldSpec<infer Attr, boolean>
    ? InstanceType<Attr>["value"]
    : never;

/**
 * The schema-derived canonical plain dict type produced by `toDict()` and
 * consumed by `fromDict()`. Each field key maps to its plain primitive (or
 * plain-primitive array for list fields). Optional fields are marked `?` and
 * are omitted from `toDict()` output when absent (mirroring Python's
 * `model_dump(exclude_unset=...)` behaviour).
 *
 * This type is intentionally NOT `Record<string, unknown>` — it is fully
 * derived from the schema so that indexing an unknown key is a type error.
 *
 * Required non-role attribute fields → required key with plain primitive type.
 * Optional attribute fields → optional key (`?`) with plain primitive type.
 * Role fields → excluded entirely (key maps to `never` via the `as` clause).
 */
export type InstanceDict<Schema extends Record<string, SchemaSpec>> =
  // Required attribute field entries (non-optional, non-role):
  {
    readonly [Key in keyof Schema as Schema[Key] extends
      | RoleSpec<readonly ModelToken[]>
      | FieldSpec<AttributeClass, true>
      | ListFieldSpec<AttributeClass, true>
      ? never
      : Key]: PlainFieldValue<Schema[Key]>;
  } &
  // Optional attribute field entries (optional flag):
  {
    readonly [Key in keyof Schema as Schema[Key] extends
      | FieldSpec<AttributeClass, true>
      | ListFieldSpec<AttributeClass, true>
      ? Key
      : never]?: PlainFieldValue<Schema[Key]>;
  };

type ConstructorInput<Schema extends Record<string, SchemaSpec>> = {
  readonly [Key in RequiredKeys<Schema>]: FieldValue<Schema[Key]>;
} & {
  readonly [Key in OptionalKeys<Schema>]?: FieldValue<Schema[Key]>;
};

type OptionalKeys<Schema extends Record<string, SchemaSpec>> = {
  [Key in keyof Schema]: Schema[Key] extends
    | FieldSpec<AttributeClass, true>
    | ListFieldSpec<AttributeClass, true>
    | RoleSpec<readonly []>
    ? Key
    : never;
}[keyof Schema];

type RequiredKeys<Schema extends Record<string, SchemaSpec>> = Exclude<
  keyof Schema,
  OptionalKeys<Schema>
>;

type RoleValue<Players extends readonly ModelToken[]> = Players extends readonly []
  ? undefined
  : RolePlayerInstance<Players[number]> | readonly RolePlayerInstance<Players[number]>[];

type RolePlayerInstance<Token> = Token extends string
  ? object
  : Token extends new (values: never) => infer Instance
    ? Instance
    : never;

type MaterializedRoleValue<Players extends readonly ModelToken[]> =
  Players extends readonly []
    ? undefined
    : | MaterializedRolePlayerInstance<Players[number]>
      | readonly MaterializedRolePlayerInstance<Players[number]>[];

type MaterializedRolePlayerInstance<Token> = Token extends string
  ? object
  : Token extends (new (values: never) => infer Instance) & {
        readonly schema: infer Schema extends Record<string, SchemaSpec>;
        descriptor(): infer Descriptor;
      }
    ? Descriptor extends RelationDescriptor
      ? ShallowRelationInstance<Instance, Schema>
      : Instance
    : never;

type RelationRoleKeys<Schema extends Record<string, SchemaSpec>> = {
  [Key in keyof Schema]: Schema[Key] extends RoleSpec<readonly ModelToken[]>
    ? Key
    : never;
}[keyof Schema];

type ShallowRelationInstance<
  Instance,
  Schema extends Record<string, SchemaSpec>,
> = Omit<Instance, RelationRoleKeys<Schema>> & {
  readonly [Key in RelationRoleKeys<Schema>]: undefined;
};

/**
 * The canonical hydrated-instance type for a model schema. This is the single
 * source of truth for what `class X extends Entity(...) {}` produces AND what a
 * manager's `get`/`all`/`first`/`insert`/hydrate paths return. Both surfaces
 * reference this type so a manager-fetched instance is assignable to the user's
 * model class — manager-hydrated instances are real `new modelClass(...)`
 * instances, so they genuinely carry `toDict` at runtime.
 *
 * Shape: schema-derived fields, the `_iid` slot (`IidBearing`), and the
 * precise `toDict(): InstanceDict<Schema>` serializer.
 */
export type ModelInstance<Schema extends Record<string, SchemaSpec>> =
  InstanceFields<Schema> &
    IidBearing & {
      /**
       * Serialize this typed instance to a canonical plain dict, byte-shape
       * identical to Python `to_dict()` (`type_bridge/models/entity.py:291-335`).
       *
       * Each branded `Attribute` field is unwrapped to its plain primitive; list
       * fields serialize to a plain array of primitives. Absent optional fields
       * are omitted from the result (not included as `undefined`) — this mirrors
       * Python `model_dump(exclude_unset=...)` + `_unwrap_value`.
       *
       * Return type: `InstanceDict<Schema>` — a schema-derived mapped type keyed
       * by field name with plain primitive values. Indexing a non-schema key is a
       * compile-time type error; the return is never `Record<string, any>`.
       */
      toDict(): InstanceDict<Schema>;
    };

export type ModelClass<
  Schema extends Record<string, SchemaSpec>,
  Descriptor extends EntityDescriptor | RelationDescriptor,
> = (new (values: ConstructorInput<Schema>) => ModelInstance<Schema>) & {
  readonly typeName: string;
  readonly schema: Schema;
  readonly flags: ResolvedTypeFlags;
  descriptor(): Descriptor;
  /**
   * Construct a typed instance from a canonical plain dict, mirroring Python
   * `from_dict()` (`type_bridge/models/entity.py:337-378`).
   *
   * Each plain primitive (or array) is re-branded into a `Attribute` /
   * `Attribute[]` using the field's attribute class constructor (`new
   * attrType(value)`). The resulting values are passed to the model
   * constructor. Runtime guards:
   * - Unknown keys: throws `TypedCodecError` (strict mode).
   * - Missing required fields: the constructor throws `TypeError`.
   */
  fromDict(data: InstanceDict<Schema>): ModelInstance<Schema>;
  manager(
    db: ManagerConnection,
  ): Descriptor extends EntityDescriptor
    ? TypedEntityManager<ModelInstance<Schema>>
    : TypedRelationManager<ModelInstance<Schema>>;
};

type OwnerBrandedModelInstance<
  Schema extends Record<string, SchemaSpec>,
  Owners extends string,
> = ModelInstance<Schema> & {
  readonly [modelOwnerBrand]: Owners;
};

type OwnerBrandedModelClass<
  Schema extends Record<string, SchemaSpec>,
  Descriptor extends EntityDescriptor | RelationDescriptor,
  TypeName extends string = string,
  Owners extends string = TypeName,
> = (new (
  values: ConstructorInput<Schema>,
) => OwnerBrandedModelInstance<Schema, Owners>) & {
  readonly typeName: TypeName;
  readonly [modelClassBrand]: Owners;
  readonly schema: Schema;
  readonly flags: ResolvedTypeFlags;
  descriptor(): Descriptor;
  fromDict(data: InstanceDict<Schema>): OwnerBrandedModelInstance<Schema, Owners>;
  manager(
    db: ManagerConnection,
  ): Descriptor extends EntityDescriptor
    ? TypedEntityManager<OwnerBrandedModelInstance<Schema, Owners>>
    : TypedRelationManager<OwnerBrandedModelInstance<Schema, Owners>>;
};

/** Return the nominal declaring-type lineage carried by a typed model instance. */
export type ModelOwnerToken<Model> = Model extends {
  readonly [modelOwnerBrand]: infer Owners extends string;
}
  ? Owners
  : never;

/** Declare an owned-attribute field on a model schema, with optional flags. */
export function field<Attr extends AttributeClass>(
  attrType: Attr,
  ...flags: FlagInput[]
): FieldSpec<Attr, false> {
  return new FieldSpec(attrType, flags);
}

/**
 * Declare a relation role. Pass zero or more player model tokens, optionally
 * followed by `{ cardinality, playsCardinality }`. Player tokens may be model
 * classes or raw type name strings (the latter for players whose typed class is
 * not yet declared). `cardinality` is relates-side; `playsCardinality` is
 * player-side and therefore requires at least one player token.
 */
export function role(): RoleSpec<readonly []>;
export function role(options: RelatesOnlyRoleOptions): RoleSpec<readonly []>;
export function role<const Players extends readonly [ModelToken, ...ModelToken[]]>(
  ...players: Players
): RoleSpec<Players>;
export function role<const Players extends readonly [ModelToken, ...ModelToken[]]>(
  ...playersAndOptions: [...Players, RoleOptions]
): RoleSpec<Players>;
export function role<const Players extends readonly ModelToken[]>(
  ...playersAndOptions: RoleArguments<Players>
): RoleSpec<Players> {
  const { players, cardinality, playsCardinality, overrides, isAbstract, ordered, distinct, doc, meta } = splitRoleArguments(playersAndOptions);
  return new RoleSpec(players as Players, cardinality, playsCardinality, overrides, isAbstract, ordered, distinct, doc, meta);
}

/**
 * Build a hard-typed entity base class from a name (or `TypeFlags`) and a field
 * schema. Extend the result to declare the model:
 * `class Person extends Entity("person", { name: field(Name, Key) }) {}`.
 * Construction, field reads, and `descriptor()` are all typed from the schema.
 *
 * Pass a third `{ parent: ParentClass }` argument to declare an inheritance
 * relationship. The child's descriptor emits `parent_type` and the full flattened
 * `owned_attributes` (parent attrs re-listed, then child-local attrs). Inherited
 * fields are accessible on the child instance with the parent's attribute brand.
 */
export function Entity<const TypeName extends string, const Schema extends EntitySchema>(
  typeNameOrFlags: TypeName,
  schema: Schema,
): OwnerBrandedModelClass<Schema, EntityDescriptor, TypeName>;
export function Entity<
  const TypeName extends string,
  const Schema extends EntitySchema,
>(
  typeNameOrFlags: ResolvedTypeFlags<TypeName>,
  schema: Schema,
): OwnerBrandedModelClass<Schema, EntityDescriptor, TypeName>;
export function Entity<const Schema extends EntitySchema>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
): OwnerBrandedModelClass<Schema, EntityDescriptor>;
export function Entity<
  const TypeName extends string,
  const ParentSchema extends EntitySchema,
  const ParentOwners extends string,
  const Schema extends EntitySchema,
>(
  typeNameOrFlags: TypeName,
  schema: Schema,
  options: OwnerBrandedParentOption<ParentSchema, ParentOwners>,
): OwnerBrandedModelClass<
  MergedSchema<ParentSchema, Schema>,
  EntityDescriptor,
  TypeName,
  TypeName | ParentOwners
>;
export function Entity<
  const TypeName extends string,
  const ParentSchema extends EntitySchema,
  const ParentOwners extends string,
  const Schema extends EntitySchema,
>(
  typeNameOrFlags: ResolvedTypeFlags<TypeName>,
  schema: Schema,
  options: OwnerBrandedParentOption<ParentSchema, ParentOwners>,
): OwnerBrandedModelClass<
  MergedSchema<ParentSchema, Schema>,
  EntityDescriptor,
  TypeName,
  TypeName | ParentOwners
>;
export function Entity<
  const ParentSchema extends EntitySchema,
  const Schema extends EntitySchema,
>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
  options: ParentOption<ParentSchema>,
): OwnerBrandedModelClass<MergedSchema<ParentSchema, Schema>, EntityDescriptor>;
export function Entity<
  const ParentSchema extends EntitySchema,
  const Schema extends EntitySchema,
>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
  options?: ParentOption<ParentSchema>,
): OwnerBrandedModelClass<
  Schema | MergedSchema<ParentSchema, Schema>,
  EntityDescriptor
> {
  return createModelClass(typeNameOrFlags, schema, "entity", options?.parent ?? null) as never;
}

/**
 * Build a hard-typed relation base class. Like `Entity`, but the schema may also
 * contain `role(...)` specs, which are emitted as `roles[*]` in the descriptor.
 *
 * Pass a third `{ parent: ParentClass }` argument to declare a parent relation
 * type. The child's descriptor emits `parent_type`, the full flattened
 * `owned_attributes`, and the **effective role set**: plain-inherited parent roles
 * first (in the parent's own effective order), then child-local roles — excluding
 * any parent role whose name appears as the `overrides` target of a child role.
 * This mirrors the Python descriptor contract (see `internals.md`, "Descriptor Contract").
 */
export function Relation<const TypeName extends string, const Schema extends RelationSchema>(
  typeNameOrFlags: TypeName,
  schema: Schema,
): OwnerBrandedModelClass<Schema, RelationDescriptor, TypeName>;
export function Relation<
  const TypeName extends string,
  const Schema extends RelationSchema,
>(
  typeNameOrFlags: ResolvedTypeFlags<TypeName>,
  schema: Schema,
): OwnerBrandedModelClass<Schema, RelationDescriptor, TypeName>;
export function Relation<const Schema extends RelationSchema>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
): OwnerBrandedModelClass<Schema, RelationDescriptor>;
export function Relation<
  const TypeName extends string,
  const ParentSchema extends RelationSchema,
  const ParentOwners extends string,
  const Schema extends RelationSchema,
>(
  typeNameOrFlags: TypeName,
  schema: Schema,
  options: OwnerBrandedParentOption<ParentSchema, ParentOwners>,
): OwnerBrandedModelClass<
  MergedSchema<ParentSchema, Schema>,
  RelationDescriptor,
  TypeName,
  TypeName | ParentOwners
>;
export function Relation<
  const TypeName extends string,
  const ParentSchema extends RelationSchema,
  const ParentOwners extends string,
  const Schema extends RelationSchema,
>(
  typeNameOrFlags: ResolvedTypeFlags<TypeName>,
  schema: Schema,
  options: OwnerBrandedParentOption<ParentSchema, ParentOwners>,
): OwnerBrandedModelClass<
  MergedSchema<ParentSchema, Schema>,
  RelationDescriptor,
  TypeName,
  TypeName | ParentOwners
>;
export function Relation<
  const ParentSchema extends RelationSchema,
  const Schema extends RelationSchema,
>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
  options: ParentOption<ParentSchema>,
): OwnerBrandedModelClass<MergedSchema<ParentSchema, Schema>, RelationDescriptor>;
export function Relation<
  const ParentSchema extends RelationSchema,
  const Schema extends RelationSchema,
>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
  options?: ParentOption<ParentSchema>,
): OwnerBrandedModelClass<
  Schema | MergedSchema<ParentSchema, Schema>,
  RelationDescriptor
> {
  return createModelClass(typeNameOrFlags, schema, "relation", options?.parent ?? null) as never;
}

type RelatesOnlyRoleOptions = {
  readonly cardinality?: CardSpec | null;
  /** TypeDB 3.12+ `@doc("...")` documentation for the relates clause. */
  readonly doc?: string | null;
  /** TypeDB 3.12+ `@meta("key", "value")` annotations for the relates clause. */
  readonly meta?: Record<string, string>;
  readonly playsCardinality?: never;
  readonly overrides?: string;
  readonly abstract?: boolean;
  readonly ordered?: boolean;
  readonly distinct?: boolean;
};
type RoleOptions = {
  readonly cardinality?: CardSpec | null;
  /** TypeDB 3.12+ `@doc("...")` documentation for the relates clause. */
  readonly doc?: string | null;
  /** TypeDB 3.12+ `@meta("key", "value")` annotations for the relates clause. */
  readonly meta?: Record<string, string>;
  readonly playsCardinality?: CardSpec | null;
  readonly overrides?: string;
  readonly abstract?: boolean;
  readonly ordered?: boolean;
  readonly distinct?: boolean;
};
type RoleArguments<Players extends readonly ModelToken[]> =
  | [...Players]
  | [...Players, RoleOptions];

// Shared factory behind `Entity()` and `Relation()`. Returns an abstract base
// class carrying the schema, resolved flags, a typed frozen-field constructor,
// and `descriptor()`. The two public wrappers only differ in their return type
// and whether `descriptor()` includes `roles`.
//
// When `parent` is supplied, the merged schema (parent + child) is used for
// constructor field population and instance field access. The descriptor emits
// `parent_type` and the full flattened `owned_attributes`.
function createModelClass(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Record<string, SchemaSpec>,
  kind: "entity" | "relation",
  parent: ParentModelClass | null = null,
) {
  const flags =
    typeof typeNameOrFlags === "string" ? TypeFlags({ name: typeNameOrFlags }) : typeNameOrFlags;

  // The merged schema is what the constructor and instance fields use.
  // Parent fields come first; child-local fields follow (and shadow parent keys).
  const parentSchema: Record<string, SchemaSpec> = parent?.schema ?? {};
  const mergedSchema: Record<string, SchemaSpec> = { ...parentSchema, ...schema };

  abstract class TypedModel {
    // Expose the child-local schema as the declared schema (matches the class
    // declaration surface), but use mergedSchema at runtime for construction.
    static readonly schema = mergedSchema;
    static readonly flags = flags;

    static get typeName(): string {
      return flags.name ?? formatTypeName(this.name, flags.case);
    }

    constructor(values: Record<string, unknown>) {
      defineIidSlot(this);
      for (const key of Object.keys(mergedSchema)) {
        const spec = mergedSchema[key];
        const present = key in values;
        const optional =
          spec instanceof RoleSpec ||
          (spec instanceof FieldSpec && spec.isOptional) ||
          (spec instanceof ListFieldSpec && spec.isOptional);
        // The constructor type already forbids omitting a required field, but
        // guard at runtime too: callers crossing an `any`/JS boundary bypass the
        // compile-time check, and a silent `undefined` would corrupt the
        // descriptor round-trip rather than fail loudly.
        //
        // RoleSpec fields are always treated as optional here: role players are
        // a relation-level concern that fromDict() and attribute-only hydration
        // paths legitimately leave unset. The manager's hydrateRelationGroup
        // always populates them via hydrateRoleFields().
        if (!present && !optional) {
          throw new TypeError(`${flags.name ?? this.constructor.name}: missing required field "${key}"`);
        }
        Object.defineProperty(this, key, {
          value: present ? values[key] : undefined,
          enumerable: true,
          writable: false,
        });
      }
    }

    /**
     * Serialize this typed instance to the canonical plain dict.
     * See `ModelClass.toDict()` for the full contract.
     *
     * The concrete body returns the structural `Record<string, unknown>` because
     * the runtime walks `mergedSchema` dynamically; the precise
     * `InstanceDict<Schema>` is recovered at the public boundary by
     * `ModelInstance<Schema>` / `ModelClass`, which every caller sees. The
     * widening is contained to this base class — callers never observe it.
     */
    toDict(): Record<string, unknown> {
      const result: Record<string, unknown> = {};
      const self = this as Record<string, unknown>;
      for (const [key, spec] of Object.entries(mergedSchema)) {
        if (spec instanceof RoleSpec) {
          // Role fields are not serialized in the attribute dict.
          continue;
        }
        const value = self[key];
        if (value === undefined) {
          // Absent optional field: omit entirely (mirrors Python exclude_unset).
          continue;
        }
        if (spec instanceof ListFieldSpec) {
          if (!Array.isArray(value)) {
            throw new TypedCodecError(`List field "${key}" is not an array`);
          }
          result[key] = attributeToPlain(value as Attribute<unknown, string>[]);
        } else {
          result[key] = attributeToPlain(value as Attribute<unknown, string>);
        }
      }
      return result;
    }

    static fromDict(data: Record<string, unknown>): TypedModel {
      // Check for unknown keys (strict mode, mirrors Python from_dict strict=True).
      for (const key of Object.keys(data)) {
        if (!(key in mergedSchema) || mergedSchema[key] instanceof RoleSpec) {
          throw new TypedCodecError(
            `fromDict: unknown field "${key}" for ${this.typeName}`,
          );
        }
      }

      // Re-brand each plain value into a typed Attribute / Attribute[].
      const values: Record<string, unknown> = {};
      for (const [key, spec] of Object.entries(mergedSchema)) {
        if (spec instanceof RoleSpec) {
          continue;
        }
        const rawValue = (data as Record<string, unknown>)[key];
        if (rawValue === undefined || rawValue === null) {
          // Let the constructor handle missing required field detection.
          continue;
        }
        const isList = spec instanceof ListFieldSpec;
        values[key] = plainToAttribute(spec.attrType, rawValue, key, isList);
      }

      // Constructor throws TypeError on missing required fields (existing guard).
      return new (this as unknown as new (values: Record<string, unknown>) => TypedModel)(values);
    }

    static descriptor(): EntityDescriptor | RelationDescriptor {
      const parentTypeName = parent != null ? parent.typeName : null;
      const descriptor: EntityDescriptor = {
        type_name: this.typeName,
        is_abstract: flags.abstract,
        parent_type: parentTypeName,
        // Flattened owned_attributes: parent attrs first, then child-local attrs.
        // The schema generator only emits `owns` for non-inherited attrs, but the
        // Python descriptor contract re-lists inherited attrs in the child —
        // descriptors.json `parity-person` is the reference.
        owned_attributes: ownedAttributes(parentSchema, schema),
      };
      if (flags.doc != null) {
        descriptor.doc = flags.doc;
      }
      if (Object.keys(flags.meta).length > 0) {
        descriptor.meta = { ...flags.meta };
      }
      attachAttributeSchemaMetadata(descriptor, parentSchema, schema);

      if (kind === "entity") {
        return descriptor;
      }

      // For the effective role set we need the parent's own effective roles
      // (recursively resolved), not its raw schema.  Calling parent.descriptor()
      // here is safe: the parent class is already fully defined when descriptor()
      // is invoked on the child.
      const parentEffectiveRoles: RoleDescriptor[] = (() => {
        if (parent == null) return [];
        const pd = (parent as unknown as { descriptor(): EntityDescriptor | RelationDescriptor }).descriptor();
        return (pd as RelationDescriptor).roles ?? [];
      })();
      const relationDescriptor: RelationDescriptor = {
        ...descriptor,
        roles: roleDescriptors(parentEffectiveRoles, schema),
      };
      attachAttributeSchemaMetadata(relationDescriptor, parentSchema, schema);
      return relationDescriptor;
    }

    static manager(db: ManagerConnection) {
      if (kind === "entity") {
        return entityManagerFor(this as never, db);
      }
      return relationManagerFor(this as never, db);
    }
  }

  modelParents.set(TypedModel, parent);
  return TypedModel;
}

/**
 * @internal Return constructor dependencies needed before registering a model
 * in an owner-aware query session. The constructors themselves stay private to
 * the session; raw string-only role targets cannot provide hydration metadata.
 */
export function modelConstructorDependencies(
  model: ModelDependencyClass,
  includeOwnDescendants = false,
  includeDescendant: (model: ModelDependencyClass) => boolean = () => true,
): readonly ModelDependencyClass[] {
  observeModelConstructor(model);
  const dependencies: ModelDependencyClass[] = [];
  const seen = new Set<object>();
  const append = (dependency: ModelDependencyClass): void => {
    if (dependency !== model && !seen.has(dependency)) {
      seen.add(dependency);
      dependencies.push(dependency);
    }
  };
  const appendDescendants = (owner: ModelDependencyClass): void => {
    const pending = [...(modelChildren.get(owner) ?? [])];
    const visited = new Set<object>();
    while (pending.length > 0) {
      const descendant = pending.shift()!;
      if (visited.has(descendant) || descendant === model) {
        continue;
      }
      visited.add(descendant);
      if (includeDescendant(descendant)) {
        append(descendant);
      }
      pending.push(...(modelChildren.get(descendant) ?? []));
    }
  };

  if (includeOwnDescendants) {
    appendDescendants(model);
  }
  const parent = modelParent(model);
  if (parent !== undefined && parent !== null) {
    append(parent);
  }
  for (const spec of Object.values(model.schema)) {
    if (!(spec instanceof RoleSpec)) {
      continue;
    }
    for (const player of spec.players) {
      if (typeof player !== "string") {
        const constructor = player as ModelDependencyClass;
        observeModelConstructor(constructor);
        append(constructor);
        // A declared base player can legally hydrate as any already-loaded
        // concrete subtype. Register those exact constructors up front rather
        // than falling back to the declared base at materialization time.
        appendDescendants(constructor);
      }
    }
  }
  return Object.freeze(dependencies);
}

function observeModelConstructor(model: ModelDependencyClass): void {
  const factory = modelFactory(model);
  if (factory === null || factory === model) {
    return;
  }
  const parent = modelParents.get(factory);
  if (parent === undefined || parent === null) {
    return;
  }
  const children = modelChildren.get(parent) ?? new Set<ModelDependencyClass>();
  children.add(model);
  modelChildren.set(parent, children);
}

function modelParent(model: ModelDependencyClass): ParentModelClass | null | undefined {
  const factory = modelFactory(model);
  return factory === null ? undefined : modelParents.get(factory);
}

function modelFactory(model: ModelDependencyClass): ModelDependencyClass | null {
  let candidate: object | null = model;
  while (candidate !== null) {
    if (modelParents.has(candidate)) {
      return candidate as ModelDependencyClass;
    }
    candidate = Object.getPrototypeOf(candidate) as object | null;
  }
  return null;
}

/**
 * Emit the full flattened `owned_attributes` list for a descriptor.
 *
 * When a model has a parent, the Python facade contract re-lists the parent's
 * owned attributes in the child's descriptor before the child-local attrs.
 * `parentSchema` is the parent's schema (empty `{}` when there is no parent);
 * `childSchema` is the locally-declared schema. The normalizer sorts by
 * `field_name`, so insertion order is irrelevant for parity, but we follow
 * the Python facade ordering (parent-first) for readability.
 */
function ownedAttributes(
  parentSchema: Record<string, SchemaSpec>,
  childSchema: Record<string, SchemaSpec>,
): OwnedAttributeDescriptor[] {
  const descriptors: OwnedAttributeDescriptor[] = [];
  // Parent attrs first (inherited, re-listed).
  for (const [fieldName, spec] of Object.entries(parentSchema)) {
    const entry = ownedAttributeEntry(fieldName, spec);
    if (entry != null) {
      descriptors.push(entry);
    }
  }
  // Child-local attrs (not already covered by the parent re-listing).
  for (const [fieldName, spec] of Object.entries(childSchema)) {
    const entry = ownedAttributeEntry(fieldName, spec);
    if (entry != null) {
      descriptors.push(entry);
    }
  }
  return descriptors;
}

/**
 * Convert a single schema spec entry to an `OwnedAttributeDescriptor`.
 *
 * Handles both scalar `FieldSpec` and multi-value `ListFieldSpec`:
 *
 * - `FieldSpec`: emits `annotations` from `resolveFlags()`, `is_optional` from
 *   `spec.isOptional`. The normalizer strips scalar `Card(0,1)/(1,1)` — the
 *   emitter does NOT strip them (invariant: emitter is unconditional).
 * - `ListFieldSpec`: emits a single `{"Card":[min,max|null]}` annotation from
 *   the explicit cardinality. `is_optional` is derived from `card_min == 0`,
 *   mirroring `_is_optional` in `type_bridge/_rust_runtime.py`.
 *
 * Returns `null` for `RoleSpec` entries (not attribute descriptors).
 */
function ownedAttributeEntry(
  fieldName: string,
  spec: SchemaSpec,
): OwnedAttributeDescriptor | null {
  if (spec instanceof ListFieldSpec) {
    // Multi-value list field: emit the explicit Card annotation unconditionally.
    // is_optional mirrors Python's `_is_optional`: card_min == 0.
    const annotations: Annotation[] = spec.annotations.map(copyAnnotation);
    if (spec.card !== null) {
      annotations.push({ Card: [spec.card[0], spec.card[1]] });
    }
    if (spec.isDistinct) {
      annotations.push("Distinct");
    }
    const entry: OwnedAttributeDescriptor = {
      field_name: fieldName,
      attr_name: spec.attrType.attrName,
      value_type: spec.attrType.valueType,
      annotations,
      is_optional: spec.isOptional,
      is_ordered: spec.isOrdered,
    };
    if (spec.doc != null) {
      entry.doc = spec.doc;
    }
    if (Object.keys(spec.meta).length > 0) {
      entry.meta = { ...spec.meta };
    }
    return entry;
  }
  if (spec instanceof FieldSpec) {
    const entry: OwnedAttributeDescriptor = {
      field_name: fieldName,
      attr_name: spec.attrType.attrName,
      value_type: spec.attrType.valueType,
      annotations: spec.flags.annotations.map(copyAnnotation),
      is_optional: spec.isOptional,
      is_ordered: spec.flags.isOrdered,
    };
    if (spec.flags.doc != null) {
      entry.doc = spec.flags.doc;
    }
    if (Object.keys(spec.flags.meta).length > 0) {
      entry.meta = { ...spec.flags.meta };
    }
    return entry;
  }
  // RoleSpec — not an attribute descriptor.
  return null;
}

function attachAttributeSchemaMetadata(
  descriptor: EntityDescriptor | RelationDescriptor,
  parentSchema: Record<string, SchemaSpec>,
  childSchema: Record<string, SchemaSpec>,
): void {
  const metadata = attributeSchemaMetadata(parentSchema, childSchema);
  if (Object.keys(metadata).length === 0) {
    return;
  }
  Object.defineProperty(descriptor, ATTRIBUTE_SCHEMA_METADATA, {
    value: metadata,
    enumerable: false,
    configurable: false,
    writable: false,
  });
}

function attributeSchemaMetadata(
  parentSchema: Record<string, SchemaSpec>,
  childSchema: Record<string, SchemaSpec>,
): Record<string, AttributeSchemaEntry> {
  const metadata: Record<string, AttributeSchemaEntry> = {};
  for (const spec of [...Object.values(parentSchema), ...Object.values(childSchema)]) {
    if (spec instanceof FieldSpec || spec instanceof ListFieldSpec) {
      for (const entry of spec.attrType.attributeSchemaEntries) {
        metadata[entry.attr_name] = copyAttributeSchemaEntry(entry);
      }
    }
  }
  return metadata;
}

function copyAttributeSchemaEntry(entry: AttributeSchemaEntry): AttributeSchemaEntry {
  const copy: AttributeSchemaEntry = { ...entry };
  if (entry.allowed_values !== undefined) {
    copy.allowed_values = entry.allowed_values === null ? null : [...entry.allowed_values];
  }
  if (entry.range !== undefined) {
    copy.range = entry.range === null ? null : [entry.range[0], entry.range[1]];
  }
  return copy;
}

/**
 * Emit the canonical effective role set for a relation descriptor.
 *
 * `parentEffectiveRoles` is the parent's already-resolved effective role list
 * (empty for root relations).  `childSchema` is the child-local schema.
 *
 * Result: parent effective roles first (excluding those overridden by a child
 * specialization), then child-local roles in declaration order.  This mirrors
 * the Python `_effective_roles` algorithm and the contract in `descriptor.rs`.
 */
function roleDescriptors(
  parentEffectiveRoles: RoleDescriptor[],
  childSchema: Record<string, SchemaSpec>,
): RoleDescriptor[] {
  // Collect parent role names overridden by child specializations.
  const overriddenRoleNames = new Set<string>();
  for (const spec of Object.values(childSchema)) {
    if (spec instanceof RoleSpec && spec.overrides != null) {
      overriddenRoleNames.add(spec.overrides);
    }
  }

  const descriptors: RoleDescriptor[] = [];

  // Parent effective roles first — skip those overridden at this level.
  // Inherited-role entries keep all parent markers (overrides, is_abstract,
  // ordered, distinct) because they were set at declaration time and travel
  // with the descriptor.
  for (const parentRole of parentEffectiveRoles) {
    if (overriddenRoleNames.has(parentRole.role_name)) continue;
    descriptors.push({ ...parentRole });
  }

  // Child-local roles follow in declaration order.
  for (const [roleName, spec] of Object.entries(childSchema)) {
    if (!(spec instanceof RoleSpec)) continue;
    const roleDescriptor: RoleDescriptor = {
      role_name: roleName,
      player_type_names: spec.players.map(typeNameFor),
      cardinality: spec.cardinality,
      plays_cardinality: spec.playsCardinality,
      overrides: spec.overrides ?? null,
      is_abstract: spec.isAbstract,
      ordered: spec.ordered,
      distinct: spec.distinct,
    };
    if (spec.doc != null) {
      roleDescriptor.doc = spec.doc;
    }
    if (Object.keys(spec.meta).length > 0) {
      roleDescriptor.meta = { ...spec.meta };
    }
    descriptors.push(roleDescriptor);
  }

  return descriptors;
}

function typeNameFor(token: ModelToken): string {
  return typeof token === "string" ? token : token.typeName;
}

function copyAnnotation(annotation: Annotation): Annotation {
  if (typeof annotation === "string") {
    // Covers "Key", "Unique", "Distinct" — all string-tag variants.
    return annotation;
  }
  return { Card: [annotation.Card[0], annotation.Card[1]] };
}

function splitRoleArguments(args: readonly unknown[]): {
  players: readonly ModelToken[];
  cardinality: CardSpec | null;
  playsCardinality: CardSpec | null;
  overrides: string | undefined;
  isAbstract: boolean;
  ordered: boolean;
  distinct: boolean;
  doc: string | null;
  meta: Record<string, string>;
} {
  if (args.length === 0) {
    return { players: [], cardinality: null, playsCardinality: null, overrides: undefined, isAbstract: false, ordered: false, distinct: false, doc: null, meta: {} };
  }

  const maybeOptions = args[args.length - 1];
  if (isRoleOptions(maybeOptions)) {
    return {
      players: args.slice(0, -1) as ModelToken[],
      cardinality: maybeOptions.cardinality ?? null,
      playsCardinality: maybeOptions.playsCardinality ?? null,
      overrides: maybeOptions.overrides,
      isAbstract: maybeOptions.abstract ?? false,
      ordered: maybeOptions.ordered ?? false,
      distinct: maybeOptions.distinct ?? false,
      doc: maybeOptions.doc ?? null,
      meta: { ...(maybeOptions.meta ?? {}) },
    };
  }

  return {
    players: args as ModelToken[],
    cardinality: null,
    playsCardinality: null,
    overrides: undefined,
    isAbstract: false,
    ordered: false,
    distinct: false,
    doc: null,
    meta: {},
  };
}

// Distinguishes a trailing role-options object from a player token.
// Safe while player tokens are only strings or model classes (neither carries a
// cardinality option property); if `role()` gains richer player tokens in a later
// plan, this discriminator must be revisited.
function isRoleOptions(value: unknown): value is RoleOptions {
  return (
    typeof value === "object" &&
    value !== null &&
    ("cardinality" in value || "playsCardinality" in value || "overrides" in value || "abstract" in value || "ordered" in value || "distinct" in value || "doc" in value || "meta" in value) &&
    !(value instanceof FieldSpec) &&
    !(value instanceof RoleSpec)
  );
}
export type { IidBearing };
