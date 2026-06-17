import type { Attribute } from "./attribute.js";
import type { ValueType } from "./index.js";
import type { AttributeSchemaEntry, EntityDescriptor, RelationDescriptor } from "./index.js";
import { type CardSpec, type FlagInput, type ResolvedTypeFlags } from "./flags.js";
import { type IidBearing } from "./iid.js";
import { type ManagerConnection, type TypedEntityManager, type TypedRelationManager } from "./manager.js";
export type AttributeClass = (new (value: never) => Attribute<unknown, string>) & {
    readonly attrName: string;
    readonly valueType: ValueType;
    readonly attributeSchema: AttributeSchemaEntry;
    readonly attributeSchemaEntries: readonly AttributeSchemaEntry[];
};
type ModelClassLike = (new (values: never) => object) & {
    readonly typeName: string;
};
type ModelToken = string | ModelClassLike;
/**
 * A model class that also exposes its schema, used as a parent reference.
 * The schema constraint lets `Entity()` / `Relation()` merge parent fields into
 * the child at both the type level and the descriptor-emission level.
 */
export type ParentModelClass<ParentSchema extends Record<string, SchemaSpec> = Record<string, SchemaSpec>> = ModelClassLike & {
    readonly schema: ParentSchema;
};
/**
 * Options accepted as the third argument to `Entity()` / `Relation()` for
 * declaring a parent (supertype) model.
 */
export interface ParentOption<ParentSchema extends Record<string, SchemaSpec>> {
    readonly parent: ParentModelClass<ParentSchema>;
}
/**
 * Merge two schema records: parent fields followed by child-local fields.
 * Child keys shadow parent keys when both declare the same field name.
 */
export type MergedSchema<ParentSchema extends Record<string, SchemaSpec>, ChildSchema extends Record<string, SchemaSpec>> = Omit<ParentSchema, keyof ChildSchema> & ChildSchema;
/**
 * A declared owned-attribute field: the attribute class, its resolved flags, and
 * whether it is optional. Produced by `field()`; consumed by the model factory to
 * type construction and to emit `owned_attributes`.
 */
export declare class FieldSpec<Attr extends AttributeClass, Optional extends boolean = false> {
    readonly attrType: Attr;
    readonly isOptional: Optional;
    readonly kind = "field";
    readonly flags: import("./flags.js").FlagSpec;
    constructor(attrType: Attr, flags?: readonly FlagInput[], isOptional?: Optional);
    optional(): FieldSpec<Attr, true>;
    /**
     * Declare this field as a multi-value (list) attribute. Pass a `Card(min, max)`
     * cardinality bound; an unbounded list uses `Card(0)` (max = null).
     *
     * The returned `ListFieldSpec` emits `{"Card":[min,max|null]}` in `annotations`
     * and sets `is_optional` when `card_min == 0` (mirroring `_is_optional` in
     * `type_bridge/_rust_runtime.py`). The typed value is `Attr[] | undefined`
     * when optional, `Attr[]` when required.
     */
    list<const Min extends number, const Max extends number | null>(card: CardSpec<Min, Max>): ListFieldSpec<Attr, Min extends 0 ? true : false>;
}
/**
 * A declared multi-value (list) owned-attribute field. Produced by
 * `field(Attr).list(Card(min, max))`; emits a `{"Card":[min,max|null]}`
 * annotation. Analogous to the Python ORM's `card_min`/`card_max` flags.
 *
 * `is_optional` is derived from `card_min == 0`, matching `_is_optional` in
 * `type_bridge/_rust_runtime.py`.
 */
export declare class ListFieldSpec<Attr extends AttributeClass, Optional extends boolean = false> {
    readonly attrType: Attr;
    readonly kind = "list-field";
    /** Explicit cardinality `[min, max | null]`. Always present for list fields. */
    readonly card: [number, number | null];
    readonly isOptional: Optional;
    /** True when the parent FieldSpec carried the `Ordered` flag. A multi-value
     * `Card` field is a set, not a TypeDB list: only the `Ordered` flag makes
     * the descriptor emit `is_ordered: true` (and `[]` in the define block). */
    readonly isOrdered: boolean;
    /** True when the parent FieldSpec carried the `Distinct` flag. */
    readonly isDistinct: boolean;
    constructor(attrType: Attr, cardSpec: CardSpec, isOrdered?: boolean, isDistinct?: boolean);
}
/**
 * A declared relation role: its permitted player model(s), optional
 * relates-side cardinality, and optional plays-side cardinality. Produced by
 * `role()`; consumed by the model factory to emit `roles[*]` plus metadata
 * used by `DescriptorRegistry.schemaInfo()`. Multi-player roles list more than
 * one player token.
 */
export declare class RoleSpec<Players extends readonly ModelToken[]> {
    readonly players: Players;
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
    constructor(players: Players, cardinality?: CardSpec | null, playsCardinality?: CardSpec | null, overrides?: string, isAbstract?: boolean, ordered?: boolean, distinct?: boolean);
}
export type SchemaSpec = FieldSpec<AttributeClass, boolean> | ListFieldSpec<AttributeClass, boolean> | RoleSpec<readonly ModelToken[]>;
export type EntitySchema = Record<string, FieldSpec<AttributeClass, boolean> | ListFieldSpec<AttributeClass, boolean>>;
export type RelationSchema = Record<string, SchemaSpec>;
export type FieldValue<Spec> = Spec extends ListFieldSpec<infer Attr, boolean> ? InstanceType<Attr>[] : Spec extends FieldSpec<infer Attr, boolean> ? InstanceType<Attr> : Spec extends RoleSpec<infer Players> ? RoleValue<Players> : never;
export type InstanceFields<Schema extends Record<string, SchemaSpec>> = {
    readonly [Key in keyof Schema]: Schema[Key] extends FieldSpec<AttributeClass, true> | ListFieldSpec<AttributeClass, true> ? FieldValue<Schema[Key]> | undefined : FieldValue<Schema[Key]>;
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
export type PlainFieldValue<Spec> = Spec extends ListFieldSpec<infer Attr, boolean> ? InstanceType<Attr>["value"][] : Spec extends FieldSpec<infer Attr, boolean> ? InstanceType<Attr>["value"] : never;
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
export type InstanceDict<Schema extends Record<string, SchemaSpec>> = {
    readonly [Key in keyof Schema as Schema[Key] extends RoleSpec<readonly ModelToken[]> | FieldSpec<AttributeClass, true> | ListFieldSpec<AttributeClass, true> ? never : Key]: PlainFieldValue<Schema[Key]>;
} & {
    readonly [Key in keyof Schema as Schema[Key] extends FieldSpec<AttributeClass, true> | ListFieldSpec<AttributeClass, true> ? Key : never]?: PlainFieldValue<Schema[Key]>;
};
type ConstructorInput<Schema extends Record<string, SchemaSpec>> = {
    readonly [Key in RequiredKeys<Schema>]: FieldValue<Schema[Key]>;
} & {
    readonly [Key in OptionalKeys<Schema>]?: FieldValue<Schema[Key]>;
};
type OptionalKeys<Schema extends Record<string, SchemaSpec>> = {
    [Key in keyof Schema]: Schema[Key] extends FieldSpec<AttributeClass, true> | ListFieldSpec<AttributeClass, true> | RoleSpec<readonly []> ? Key : never;
}[keyof Schema];
type RequiredKeys<Schema extends Record<string, SchemaSpec>> = Exclude<keyof Schema, OptionalKeys<Schema>>;
type RoleValue<Players extends readonly ModelToken[]> = Players extends readonly [] ? undefined : RolePlayerInstance<Players[number]> | readonly RolePlayerInstance<Players[number]>[];
type RolePlayerInstance<Token> = Token extends string ? object : Token extends new (values: never) => infer Instance ? Instance : never;
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
export type ModelInstance<Schema extends Record<string, SchemaSpec>> = InstanceFields<Schema> & IidBearing & {
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
export type ModelClass<Schema extends Record<string, SchemaSpec>, Descriptor extends EntityDescriptor | RelationDescriptor> = (new (values: ConstructorInput<Schema>) => ModelInstance<Schema>) & {
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
    manager(db: ManagerConnection): Descriptor extends EntityDescriptor ? TypedEntityManager<ModelInstance<Schema>> : TypedRelationManager<ModelInstance<Schema>>;
};
/** Declare an owned-attribute field on a model schema, with optional flags. */
export declare function field<Attr extends AttributeClass>(attrType: Attr, ...flags: FlagInput[]): FieldSpec<Attr, false>;
/**
 * Declare a relation role. Pass zero or more player model tokens, optionally
 * followed by `{ cardinality, playsCardinality }`. Player tokens may be model
 * classes or raw type name strings (the latter for players whose typed class is
 * not yet declared). `cardinality` is relates-side; `playsCardinality` is
 * player-side and therefore requires at least one player token.
 */
export declare function role(): RoleSpec<readonly []>;
export declare function role(options: RelatesOnlyRoleOptions): RoleSpec<readonly []>;
export declare function role<const Players extends readonly [ModelToken, ...ModelToken[]]>(...players: Players): RoleSpec<Players>;
export declare function role<const Players extends readonly [ModelToken, ...ModelToken[]]>(...playersAndOptions: [...Players, RoleOptions]): RoleSpec<Players>;
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
export declare function Entity<const Schema extends EntitySchema>(typeNameOrFlags: string | ResolvedTypeFlags, schema: Schema): ModelClass<Schema, EntityDescriptor>;
export declare function Entity<const ParentSchema extends EntitySchema, const Schema extends EntitySchema>(typeNameOrFlags: string | ResolvedTypeFlags, schema: Schema, options: ParentOption<ParentSchema>): ModelClass<MergedSchema<ParentSchema, Schema>, EntityDescriptor>;
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
export declare function Relation<const Schema extends RelationSchema>(typeNameOrFlags: string | ResolvedTypeFlags, schema: Schema): ModelClass<Schema, RelationDescriptor>;
export declare function Relation<const ParentSchema extends RelationSchema, const Schema extends RelationSchema>(typeNameOrFlags: string | ResolvedTypeFlags, schema: Schema, options: ParentOption<ParentSchema>): ModelClass<MergedSchema<ParentSchema, Schema>, RelationDescriptor>;
type RelatesOnlyRoleOptions = {
    readonly cardinality?: CardSpec | null;
    readonly playsCardinality?: never;
    readonly overrides?: string;
    readonly abstract?: boolean;
    readonly ordered?: boolean;
    readonly distinct?: boolean;
};
type RoleOptions = {
    readonly cardinality?: CardSpec | null;
    readonly playsCardinality?: CardSpec | null;
    readonly overrides?: string;
    readonly abstract?: boolean;
    readonly ordered?: boolean;
    readonly distinct?: boolean;
};
export type { IidBearing };
