import {
  boolean,
  date,
  datetime,
  datetimetz,
  decimal,
  double,
  duration,
  long,
  string,
  type AttributeInput,
  type AttributeValue,
  type DynamicEntityRow,
  type OwnedAttributeDescriptor,
  type RuntimeAttributeValue,
  type ValueType,
} from "./index.js";
import { type Attribute } from "./attribute.js";
import type { EntitySchema, FieldSpec, ListFieldSpec, RelationSchema, SchemaSpec } from "./model.js";

type AttributeClass = (new (value: never) => Attribute<unknown, string>) & {
  readonly attrName: string;
  readonly valueType: ValueType;
};

type FieldLike = {
  readonly kind: "field";
  readonly attrType: AttributeClass;
  readonly flags: { readonly annotations: readonly unknown[] };
};

type ListFieldLike = {
  readonly kind: "list-field";
  readonly attrType: AttributeClass;
};

export class TypedCodecError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TypedCodecError";
  }
}

/**
 * Unwrap a branded `Attribute` instance to its plain primitive value. For a
 * list field, unwrap each element of the array element-wise. Mirrors Python
 * `Entity._unwrap_value` (`type_bridge/models/entity.py:329-335`).
 *
 * This is the serialization-only path (no query language, no DB crossing). The
 * canonical plain-dict value encodings are:
 * - `long` / i64  → `bigint`
 * - `string`, `double`, `boolean` → JS native equivalents
 * - `decimal`, `date`, `datetime`, `datetime-tz`, `duration` → string (as
 *   stored in the attribute's `.value` field, matching `expected-canonical.json`)
 */
export function attributeToPlain(
  value: Attribute<unknown, string>,
): unknown;
export function attributeToPlain(
  value: Attribute<unknown, string>[],
): unknown[];
export function attributeToPlain(
  value: Attribute<unknown, string> | Attribute<unknown, string>[],
): unknown | unknown[] {
  if (Array.isArray(value)) {
    return value.map((element) => element.value);
  }
  return value.value;
}

/**
 * Wrap a plain primitive (or array of plain primitives) into a branded
 * `Attribute` (or `Attribute[]`) using the given attribute class constructor.
 * Mirrors the `new attrType(value)` brand-construction pattern in
 * `hydrateAttributeValue` (`codec.ts:192-224`).
 *
 * Used by `fromDict` to re-brand plain dict values back into typed instances.
 */
export function plainToAttribute(
  attrType: AttributeClass,
  value: unknown,
  fieldName: string,
  isList: boolean,
): Attribute<unknown, string> | Attribute<unknown, string>[] {
  if (isList) {
    if (!Array.isArray(value)) {
      throw new TypedCodecError(
        `List field "${fieldName}" must be an array in fromDict input`,
      );
    }
    return (value as unknown[]).map((element) => {
      if (element === null || element === undefined) {
        throw new TypedCodecError(
          `List field "${fieldName}" contains null/undefined element`,
        );
      }
      return new attrType(element as never);
    });
  }
  return new attrType(value as never);
}

export function lowerAttributes(
  instance: object,
  schema: EntitySchema | RelationSchema,
): AttributeInput {
  const lowered: AttributeInput = {};
  const source = instance as Record<string, unknown>;
  for (const [fieldName, spec] of Object.entries(schema)) {
    const value = source[fieldName];
    if (value === undefined) {
      continue;
    }
    if (isListFieldSpec(spec)) {
      // A list field value is Attr[] — lower each element to its plain wire value
      // and pass the resulting array. The NAPI insert parser already accepts a JS
      // array per field and lowers it to repeated (attr_name, value) tuples
      // (crates/node/src/lib.rs, the `value.as_array()` branch).
      if (!Array.isArray(value)) {
        throw new TypedCodecError(`List field "${fieldName}" must be an array`);
      }
      lowered[fieldName] = (value as unknown[]).map((element) => {
        if (!isAttributeInstance(element)) {
          throw new TypedCodecError(`List field "${fieldName}" elements must be Attribute instances`);
        }
        return lowerAttributeValue(spec.attrType.valueType, element.value);
      });
      continue;
    }
    if (!isFieldSpec(spec)) {
      continue;
    }
    if (!isAttributeInstance(value)) {
      throw new TypedCodecError(`Field "${fieldName}" must be an Attribute instance`);
    }
    lowered[fieldName] = lowerAttributeValue(spec.attrType.valueType, value.value);
  }
  return lowered;
}

export function lowerFilters(
  filters: Record<string, Attribute<unknown, string>> | null | undefined,
  schema: EntitySchema | RelationSchema,
): Record<string, AttributeValue> | null {
  if (filters == null) {
    return null;
  }
  const lowered: Record<string, AttributeValue> = {};
  for (const [fieldName, value] of Object.entries(filters)) {
    const spec = schema[fieldName];
    if (!isFieldSpec(spec)) {
      throw new TypedCodecError(`Unknown attribute filter field "${fieldName}"`);
    }
    lowered[fieldName] = lowerAttributeValue(spec.attrType.valueType, value.value);
  }
  return lowered;
}

export function hydrateAttributes(
  row: Pick<DynamicEntityRow, "attributes">,
  schema: EntitySchema | RelationSchema,
): Record<string, Attribute<unknown, string> | Attribute<unknown, string>[]> {
  return hydrateAttributeEntries(row.attributes, schema);
}

export function hydrateAttributeEntries(
  entries: readonly (readonly [string, RuntimeAttributeValue])[],
  schema: EntitySchema | RelationSchema,
): Record<string, Attribute<unknown, string> | Attribute<unknown, string>[]> {
  const fieldsByAttrName = fieldSpecsByAttrName(schema);
  const hydrated: Record<string, Attribute<unknown, string> | Attribute<unknown, string>[]> = {};
  for (const [attrName, value] of entries) {
    const field = fieldsByAttrName.get(attrName);
    if (field === undefined) {
      continue;
    }
    const hydrated_value = hydrateAttributeValue(field.spec.attrType, value);
    if (field.isList) {
      // Collect repeated-name tuples into a typed Attr[] for list fields.
      // TypeDB returns multi-value attributes as repeated [attr_name, value] pairs
      // with the same attr_name. Each pair is one element of the list.
      const existing = hydrated[field.fieldName];
      if (Array.isArray(existing)) {
        existing.push(hydrated_value);
      } else {
        hydrated[field.fieldName] = [hydrated_value];
      }
    } else {
      // Scalar field: single value, same behavior as before (no change).
      hydrated[field.fieldName] = hydrated_value;
    }
  }
  return hydrated;
}

export function runtimeAttributeValueFromUnknown(
  value: unknown,
  valueType?: ValueType,
): RuntimeAttributeValue {
  if (!isRuntimeAttributeValue(value)) {
    if (valueType !== undefined) {
      return runtimeAttributeValueFromPrimitive(value, valueType);
    }
    throw new TypedCodecError("Role-player attribute value has an unknown runtime tag");
  }
  return value;
}

export function keyAttributeDescriptor(schema: EntitySchema): OwnedAttributeDescriptor | null {
  for (const [fieldName, spec] of Object.entries(schema)) {
    if (!isFieldSpec(spec)) {
      continue;
    }
    const annotations = spec.flags.annotations;
    if (annotations.includes("Key")) {
      return {
        field_name: fieldName,
        attr_name: spec.attrType.attrName,
        value_type: spec.attrType.valueType,
        annotations: ["Key"],
        is_optional: false,
      };
    }
  }
  return null;
}

export function lowerAttributeValue(valueType: ValueType, value: unknown): AttributeValue {
  switch (valueType) {
    case "string":
      return string(requireType(value, "string", valueType));
    case "long":
      return long(requireType(value, "bigint", valueType));
    case "double":
      return double(requireType(value, "number", valueType));
    case "boolean":
      return boolean(requireType(value, "boolean", valueType));
    case "date":
      return date(requireType(value, "string", valueType));
    case "datetime":
      return datetime(requireType(value, "string", valueType));
    case "datetime-tz":
      return datetimetz(requireType(value, "string", valueType));
    case "decimal":
      return decimal(requireType(value, "string", valueType));
    case "duration":
      return duration(requireType(value, "string", valueType));
  }
}

function hydrateAttributeValue(
  attrType: AttributeClass,
  value: RuntimeAttributeValue,
): Attribute<unknown, string> {
  if ("String" in value) {
    return new attrType(value.String as never);
  }
  if ("Long" in value) {
    return new attrType(BigInt(value.Long) as never);
  }
  if ("Double" in value) {
    return new attrType(value.Double as never);
  }
  if ("Boolean" in value) {
    return new attrType(value.Boolean as never);
  }
  if ("Date" in value) {
    return new attrType(value.Date as never);
  }
  if ("DateTime" in value) {
    return new attrType(value.DateTime as never);
  }
  if ("DateTimeTZ" in value) {
    return new attrType(value.DateTimeTZ as never);
  }
  if ("Decimal" in value) {
    return new attrType(value.Decimal as never);
  }
  if ("Duration" in value) {
    return new attrType(value.Duration as never);
  }
  throw new TypedCodecError("Unknown runtime attribute value tag");
}

function fieldSpecsByAttrName(schema: EntitySchema | RelationSchema): Map<string, {
  fieldName: string;
  spec: FieldLike | ListFieldLike;
  isList: boolean;
}> {
  const fields = new Map<string, { fieldName: string; spec: FieldLike | ListFieldLike; isList: boolean }>();
  for (const [fieldName, spec] of Object.entries(schema)) {
    if (isListFieldSpec(spec)) {
      fields.set(spec.attrType.attrName, { fieldName, spec, isList: true });
    } else if (isFieldSpec(spec)) {
      fields.set(spec.attrType.attrName, { fieldName, spec, isList: false });
    }
  }
  return fields;
}

function isFieldSpec(spec: SchemaSpec | undefined): spec is FieldSpec<AttributeClass, boolean> {
  return spec !== undefined && spec.kind === "field";
}

function isListFieldSpec(spec: SchemaSpec | undefined): spec is ListFieldSpec<AttributeClass, boolean> {
  return spec !== undefined && spec.kind === "list-field";
}

function isAttributeInstance(value: unknown): value is Attribute<unknown, string> {
  return typeof value === "object" && value !== null && "value" in value;
}

function isRuntimeAttributeValue(value: unknown): value is RuntimeAttributeValue {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    typeof record.String === "string" ||
    typeof record.Long === "string" ||
    typeof record.Double === "number" ||
    typeof record.Boolean === "boolean" ||
    typeof record.Date === "string" ||
    typeof record.DateTime === "string" ||
    typeof record.DateTimeTZ === "string" ||
    typeof record.Decimal === "string" ||
    typeof record.Duration === "string"
  );
}

function runtimeAttributeValueFromPrimitive(value: unknown, valueType: ValueType): RuntimeAttributeValue {
  switch (valueType) {
    case "string":
      return { String: requireType(value, "string", valueType) };
    case "long":
      if (typeof value === "number" && Number.isInteger(value)) {
        return { Long: value.toString() };
      }
      return { Long: requireType(value, "string", valueType) };
    case "double":
      return { Double: requireType(value, "number", valueType) };
    case "boolean":
      return { Boolean: requireType(value, "boolean", valueType) };
    case "date":
      return { Date: requireType(value, "string", valueType) };
    case "datetime":
      return { DateTime: requireType(value, "string", valueType) };
    case "datetime-tz":
      return { DateTimeTZ: requireType(value, "string", valueType) };
    case "decimal":
      return { Decimal: requireType(value, "string", valueType) };
    case "duration":
      return { Duration: requireType(value, "string", valueType) };
  }
}

function requireType<T extends "string" | "bigint" | "number" | "boolean">(
  value: unknown,
  expected: T,
  valueType: ValueType,
): T extends "string"
  ? string
  : T extends "bigint"
    ? bigint
    : T extends "number"
      ? number
      : boolean {
  if (typeof value !== expected) {
    throw new TypedCodecError(`${valueType} attribute expected ${expected} value`);
  }
  return value as T extends "string"
    ? string
    : T extends "bigint"
      ? bigint
      : T extends "number"
        ? number
        : boolean;
}
