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
import type { EntitySchema, FieldSpec, RelationSchema, SchemaSpec } from "./model.js";

type AttributeClass = (new (value: never) => Attribute<unknown, string>) & {
  readonly attrName: string;
  readonly valueType: ValueType;
};

type FieldLike = {
  readonly kind: "field";
  readonly attrType: AttributeClass;
  readonly flags: { readonly annotations: readonly unknown[] };
};

export class TypedCodecError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TypedCodecError";
  }
}

export function lowerAttributes(
  instance: object,
  schema: EntitySchema | RelationSchema,
): AttributeInput {
  const lowered: AttributeInput = {};
  const source = instance as Record<string, unknown>;
  for (const [fieldName, spec] of Object.entries(schema)) {
    if (!isFieldSpec(spec)) {
      continue;
    }
    const value = source[fieldName];
    if (value === undefined) {
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
): Record<string, Attribute<unknown, string>> {
  return hydrateAttributeEntries(row.attributes, schema);
}

export function hydrateAttributeEntries(
  entries: readonly (readonly [string, RuntimeAttributeValue])[],
  schema: EntitySchema | RelationSchema,
): Record<string, Attribute<unknown, string>> {
  const fieldsByAttrName = fieldSpecsByAttrName(schema);
  const hydrated: Record<string, Attribute<unknown, string>> = {};
  for (const [attrName, value] of entries) {
    const field = fieldsByAttrName.get(attrName);
    if (field === undefined) {
      continue;
    }
    hydrated[field.fieldName] = hydrateAttributeValue(field.spec.attrType, value);
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
  spec: FieldLike;
}> {
  const fields = new Map<string, { fieldName: string; spec: FieldLike }>();
  for (const [fieldName, spec] of Object.entries(schema)) {
    if (isFieldSpec(spec)) {
      fields.set(spec.attrType.attrName, { fieldName, spec });
    }
  }
  return fields;
}

function isFieldSpec(spec: SchemaSpec | undefined): spec is FieldSpec<AttributeClass, boolean> {
  return spec !== undefined && spec.kind === "field";
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
