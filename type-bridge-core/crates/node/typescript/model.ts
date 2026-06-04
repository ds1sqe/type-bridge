import type { Attribute } from "./attribute.js";
import type { ValueType } from "./index.js";
import type {
  Annotation,
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

type AnyAttributeClass = (new (value: never) => Attribute<unknown, string>) & {
  readonly attrName: string;
  readonly valueType: ValueType;
};
type ModelClassLike = (new (values: never) => object) & {
  readonly typeName: string;
};
type ModelToken = string | ModelClassLike;

/**
 * A declared owned-attribute field: the attribute class, its resolved flags, and
 * whether it is optional. Produced by `field()`; consumed by the model factory to
 * type construction and to emit `owned_attributes`.
 */
export class FieldSpec<Attr extends AnyAttributeClass, Optional extends boolean = false> {
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

export type SchemaSpec = FieldSpec<AnyAttributeClass, boolean> | RoleSpec<readonly ModelToken[]>;
export type EntitySchema = Record<string, FieldSpec<AnyAttributeClass, boolean>>;
export type RelationSchema = Record<string, SchemaSpec>;

export type FieldValue<Spec> = Spec extends FieldSpec<infer Attr, boolean>
  ? InstanceType<Attr>
  : Spec extends RoleSpec<infer Players>
    ? RoleValue<Players>
    : never;

export type InstanceFields<Schema extends Record<string, SchemaSpec>> = {
  readonly [Key in keyof Schema]: Schema[Key] extends FieldSpec<AnyAttributeClass, true>
    ? FieldValue<Schema[Key]> | undefined
    : FieldValue<Schema[Key]>;
};

type ConstructorInput<Schema extends Record<string, SchemaSpec>> = {
  readonly [Key in RequiredKeys<Schema>]: FieldValue<Schema[Key]>;
} & {
  readonly [Key in OptionalKeys<Schema>]?: FieldValue<Schema[Key]>;
};

type OptionalKeys<Schema extends Record<string, SchemaSpec>> = {
  [Key in keyof Schema]: Schema[Key] extends FieldSpec<AnyAttributeClass, true> ? Key : never;
}[keyof Schema];

type RequiredKeys<Schema extends Record<string, SchemaSpec>> = Exclude<
  keyof Schema,
  OptionalKeys<Schema>
>;

type RoleValue<Players extends readonly ModelToken[]> =
  | ModelInstance<Players[number]>
  | readonly ModelInstance<Players[number]>[];

type ModelInstance<Token> = Token extends string
  ? never
  : Token extends new (values: never) => infer Instance
    ? Instance
    : never;

export type ModelClass<
  Schema extends Record<string, SchemaSpec>,
  Descriptor extends EntityDescriptor | RelationDescriptor,
> = (new (values: ConstructorInput<Schema>) => InstanceFields<Schema>) & {
  readonly typeName: string;
  readonly schema: Schema;
  readonly flags: ResolvedTypeFlags;
  descriptor(): Descriptor;
};

/** Declare an owned-attribute field on a model schema, with optional flags. */
export function field<Attr extends AnyAttributeClass>(
  attrType: Attr,
  ...flags: FlagInput[]
): FieldSpec<Attr, false> {
  return new FieldSpec(attrType, flags);
}

/**
 * Declare a relation role. Pass one or more player model tokens, optionally
 * followed by `{ cardinality }`. Player tokens may be model classes or raw type
 * name strings (the latter for players whose typed class is not yet declared).
 */
export function role<const Players extends readonly [ModelToken, ...ModelToken[]]>(
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
 */
export function Entity<const Schema extends EntitySchema>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
): ModelClass<Schema, EntityDescriptor> {
  return createModelClass(typeNameOrFlags, schema, "entity") as ModelClass<Schema, EntityDescriptor>;
}

/**
 * Build a hard-typed relation base class. Like `Entity`, but the schema may also
 * contain `role(...)` specs, which are emitted as `roles[*]` in the descriptor.
 */
export function Relation<const Schema extends RelationSchema>(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Schema,
): ModelClass<Schema, RelationDescriptor> {
  return createModelClass(typeNameOrFlags, schema, "relation") as ModelClass<Schema, RelationDescriptor>;
}

type RoleOptions = { readonly cardinality?: CardSpec | null };
type RoleArguments<Players extends readonly [ModelToken, ...ModelToken[]]> =
  | [...Players]
  | [...Players, RoleOptions];

// Shared factory behind `Entity()` and `Relation()`. Returns an abstract base
// class carrying the schema, resolved flags, a typed frozen-field constructor,
// and `descriptor()`. The two public wrappers only differ in their return type
// and whether `descriptor()` includes `roles`.
function createModelClass(
  typeNameOrFlags: string | ResolvedTypeFlags,
  schema: Record<string, SchemaSpec>,
  kind: "entity" | "relation",
) {
  const flags =
    typeof typeNameOrFlags === "string" ? TypeFlags({ name: typeNameOrFlags }) : typeNameOrFlags;

  abstract class TypedModel {
    static readonly schema = schema;
    static readonly flags = flags;

    static get typeName(): string {
      return flags.name ?? formatTypeName(this.name, flags.case);
    }

    constructor(values: Record<string, unknown>) {
      for (const key of Object.keys(schema)) {
        const spec = schema[key];
        const present = key in values;
        const optional = spec instanceof FieldSpec && spec.isOptional;
        // The constructor type already forbids omitting a required field, but
        // guard at runtime too: callers crossing an `any`/JS boundary bypass the
        // compile-time check, and a silent `undefined` would corrupt the
        // descriptor round-trip rather than fail loudly.
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

    static descriptor(): EntityDescriptor | RelationDescriptor {
      const descriptor: EntityDescriptor = {
        type_name: this.typeName,
        is_abstract: flags.abstract,
        parent_type: null,
        owned_attributes: ownedAttributes(schema),
      };

      if (kind === "entity") {
        return descriptor;
      }

      return {
        ...descriptor,
        roles: roleDescriptors(schema),
      };
    }
  }

  return TypedModel;
}

function ownedAttributes(schema: Record<string, SchemaSpec>): OwnedAttributeDescriptor[] {
  const descriptors: OwnedAttributeDescriptor[] = [];
  for (const [fieldName, spec] of Object.entries(schema)) {
    if (!(spec instanceof FieldSpec)) {
      continue;
    }
    descriptors.push({
      field_name: fieldName,
      attr_name: spec.attrType.attrName,
      value_type: spec.attrType.valueType,
      annotations: spec.flags.annotations.map(copyAnnotation),
      is_optional: spec.isOptional,
    });
  }
  return descriptors;
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
    throw new TypeError("role() requires at least one player type");
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
