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
type ModelToken = string | ModelClassLike;
const ATTRIBUTE_SCHEMA_METADATA = Symbol.for("@type-bridge/node.attributeSchemaMetadata");

/**
 * A model class that also exposes its schema, used as a parent reference.
 * The schema constraint lets `Entity()` / `Relation()` merge parent fields into
 * the child at both the type level and the descriptor-emission level.
 */
export type ParentModelClass<ParentSchema extends Record<string, SchemaSpec> = Record<string, SchemaSpec>> =
  ModelClassLike & {
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
    return new ListFieldSpec(this.attrType, card) as ListFieldSpec<
      Attr,
      Min extends 0 ? true : false
    >;
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
  /** Explicit cardinality `[min, max | null]`. Always present for list fields. */
  readonly card: [number, number | null];
  readonly isOptional: Optional;

  constructor(
    readonly attrType: Attr,
    cardSpec: CardSpec,
  ) {
    this.card = [cardSpec.min, cardSpec.max];
    // A list field is optional when the minimum cardinality is 0, mirroring the
    // Python `_is_optional` rule: `flags.card_min == 0`.
    this.isOptional = (cardSpec.min === 0) as Optional;
  }
}

/**
 * A declared relation role: its permitted player model(s) and optional
 * cardinality. Produced by `role()`; consumed by the model factory to emit
 * `roles[*]`. Multi-player roles list more than one player token.
 */
export class RoleSpec<Players extends readonly ModelToken[]> {
  readonly kind = "role";
  readonly cardinality: [number, number | null] | null;

  constructor(readonly players: Players, cardinality?: CardSpec | null) {
    this.cardinality = cardinality == null ? null : [cardinality.min, cardinality.max];
  }
}

export type SchemaSpec =
  | FieldSpec<AttributeClass, boolean>
  | ListFieldSpec<AttributeClass, boolean>
  | RoleSpec<readonly ModelToken[]>;
export type EntitySchema = Record<string, FieldSpec<AttributeClass, boolean> | ListFieldSpec<AttributeClass, boolean>>;
export type RelationSchema = Record<string, SchemaSpec>;

export type FieldValue<Spec> = Spec extends ListFieldSpec<infer Attr, boolean>
  ? InstanceType<Attr>[]
  : Spec extends FieldSpec<infer Attr, boolean>
    ? InstanceType<Attr>
    : Spec extends RoleSpec<infer Players>
      ? RoleValue<Players>
      : never;

export type InstanceFields<Schema extends Record<string, SchemaSpec>> = {
  readonly [Key in keyof Schema]: Schema[Key] extends
    | FieldSpec<AttributeClass, true>
    | ListFieldSpec<AttributeClass, true>
    ? FieldValue<Schema[Key]> | undefined
    : FieldValue<Schema[Key]>;
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
  ? never
  : Token extends new (values: never) => infer Instance
    ? Instance
    : never;

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

/** Declare an owned-attribute field on a model schema, with optional flags. */
export function field<Attr extends AttributeClass>(
  attrType: Attr,
  ...flags: FlagInput[]
): FieldSpec<Attr, false> {
  return new FieldSpec(attrType, flags);
}

/**
 * Declare a relation role. Pass zero or more player model tokens, optionally
 * followed by `{ cardinality }`. Player tokens may be model classes or raw type
 * name strings (the latter for players whose typed class is not yet declared).
 */
export function role(): RoleSpec<readonly []>;
export function role(options: RoleOptions): RoleSpec<readonly []>;
export function role<const Players extends readonly [ModelToken, ...ModelToken[]]>(
  ...playersAndOptions: RoleArguments<Players>
): RoleSpec<Players>;
export function role<const Players extends readonly ModelToken[]>(
  ...playersAndOptions: RoleArguments<Players>
): RoleSpec<Players> {
  const { players, cardinality } = splitRoleArguments(playersAndOptions);
  return new RoleSpec(players as Players, cardinality);
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
export function Entity<const Schema extends EntitySchema>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
): ModelClass<Schema, EntityDescriptor>;
export function Entity<
  const ParentSchema extends EntitySchema,
  const Schema extends EntitySchema,
>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
  options: ParentOption<ParentSchema>,
): ModelClass<MergedSchema<ParentSchema, Schema>, EntityDescriptor>;
export function Entity<
  const ParentSchema extends EntitySchema,
  const Schema extends EntitySchema,
>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
  options?: ParentOption<ParentSchema>,
): ModelClass<Schema | MergedSchema<ParentSchema, Schema>, EntityDescriptor> {
  return createModelClass(typeNameOrFlags, schema, "entity", options?.parent ?? null) as never;
}

/**
 * Build a hard-typed relation base class. Like `Entity`, but the schema may also
 * contain `role(...)` specs, which are emitted as `roles[*]` in the descriptor.
 *
 * Pass a third `{ parent: ParentClass }` argument to declare a parent relation
 * type. The child's descriptor emits `parent_type` and the full flattened
 * `owned_attributes`; inherited roles are also re-listed.
 */
export function Relation<const Schema extends RelationSchema>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
): ModelClass<Schema, RelationDescriptor>;
export function Relation<
  const ParentSchema extends RelationSchema,
  const Schema extends RelationSchema,
>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
  options: ParentOption<ParentSchema>,
): ModelClass<MergedSchema<ParentSchema, Schema>, RelationDescriptor>;
export function Relation<
  const ParentSchema extends RelationSchema,
  const Schema extends RelationSchema,
>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
  options?: ParentOption<ParentSchema>,
): ModelClass<Schema | MergedSchema<ParentSchema, Schema>, RelationDescriptor> {
  return createModelClass(typeNameOrFlags, schema, "relation", options?.parent ?? null) as never;
}

type RoleOptions = { readonly cardinality?: CardSpec | null };
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
      attachAttributeSchemaMetadata(descriptor, parentSchema, schema);

      if (kind === "entity") {
        return descriptor;
      }

      const relationDescriptor: RelationDescriptor = {
        ...descriptor,
        roles: roleDescriptors(mergedSchema),
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

  return TypedModel;
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
    const cardAnnotation: Annotation = { Card: [spec.card[0], spec.card[1]] };
    return {
      field_name: fieldName,
      attr_name: spec.attrType.attrName,
      value_type: spec.attrType.valueType,
      annotations: [cardAnnotation],
      is_optional: spec.isOptional,
    };
  }
  if (spec instanceof FieldSpec) {
    return {
      field_name: fieldName,
      attr_name: spec.attrType.attrName,
      value_type: spec.attrType.valueType,
      annotations: spec.flags.annotations.map(copyAnnotation),
      is_optional: spec.isOptional,
    };
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

function roleDescriptors(schema: Record<string, SchemaSpec>): RoleDescriptor[] {
  const descriptors: RoleDescriptor[] = [];
  for (const [roleName, spec] of Object.entries(schema)) {
    if (!(spec instanceof RoleSpec)) {
      continue;
    }
    descriptors.push({
      role_name: roleName,
      player_type_names: spec.players.map(typeNameFor),
      cardinality: spec.cardinality,
    });
  }
  return descriptors;
}

function typeNameFor(token: ModelToken): string {
  return typeof token === "string" ? token : token.typeName;
}

function copyAnnotation(annotation: Annotation): Annotation {
  if (typeof annotation === "string") {
    return annotation;
  }
  return { Card: [annotation.Card[0], annotation.Card[1]] };
}

function splitRoleArguments(args: readonly unknown[]): {
  players: readonly ModelToken[];
  cardinality: CardSpec | null;
} {
  if (args.length === 0) {
    return { players: [], cardinality: null };
  }

  const maybeOptions = args[args.length - 1];
  if (isRoleOptions(maybeOptions)) {
    return {
      players: args.slice(0, -1) as ModelToken[],
      cardinality: maybeOptions.cardinality ?? null,
    };
  }

  return {
    players: args as ModelToken[],
    cardinality: null,
  };
}

// Distinguishes a trailing `{ cardinality }` options object from a player token.
// Safe while player tokens are only strings or model classes (neither carries a
// `cardinality` property); if `role()` gains richer player tokens in a later
// plan, this discriminator must be revisited.
function isRoleOptions(value: unknown): value is RoleOptions {
  return (
    typeof value === "object" &&
    value !== null &&
    "cardinality" in value &&
    !(value instanceof FieldSpec) &&
    !(value instanceof RoleSpec)
  );
}
export type { IidBearing };
