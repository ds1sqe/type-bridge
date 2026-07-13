import type { AttributeSchemaEntry, DynamicComparisonOp, ValueType } from "./index.js";
import { lowerAttributeValue } from "./codec.js";
import { AggregateSpec, ComparisonExpr, SortExpr } from "./query.js";

// `attributeBrand` is a module-private unique symbol that exists only in the
// type system. Two attribute classes built from different `attr.*(name)` calls
// carry different `Brand` literals at this key, which is what makes `Name` and
// `Email` non-interchangeable under TypeScript's structural typing. There is no
// runtime field — `declare` erases it.
declare const attributeBrand: unique symbol;

/**
 * Base class for a branded attribute value. `Value` is the wrapped runtime
 * value; `Brand` is the schema attribute name carried as a phantom type so that
 * distinct attribute classes are nominally distinct at compile time. Instances
 * are immutable; equality is by `(constructor, value)`.
 */
export abstract class Attribute<Value, Brand extends string> {
  declare readonly [attributeBrand]: Brand;

  constructor(public readonly value: Value) {}

  equals(other: unknown): boolean {
    return (
      other instanceof Attribute &&
      other.constructor === this.constructor &&
      other.value === this.value
    );
  }

  toString(): string {
    const rendered =
      typeof this.value === "string" ? JSON.stringify(this.value) : String(this.value);
    return `${this.constructor.name}(${rendered})`;
  }
}

/** Comparison helpers available on every attribute class. */
type ComparisonStatics<Value, Brand extends string> = {
  eq(value: Attribute<Value, Brand>): ComparisonExpr;
  ne(value: Attribute<Value, Brand>): ComparisonExpr;
  gt(value: Attribute<Value, Brand>): ComparisonExpr;
  gte(value: Attribute<Value, Brand>): ComparisonExpr;
  lt(value: Attribute<Value, Brand>): ComparisonExpr;
  lte(value: Attribute<Value, Brand>): ComparisonExpr;
};

/** Sort-key helpers available on every attribute class. */
type OrderStatics = {
  asc(): SortExpr;
  desc(): SortExpr;
};

/** String-matching helpers, available only on string-valued attribute classes. */
type StringStatics = {
  contains(value: string): ComparisonExpr;
  like(value: string): ComparisonExpr;
  startsWith(value: string): ComparisonExpr;
  endsWith(value: string): ComparisonExpr;
};

/** Reduce helpers, available only on numeric attribute classes (long/double/decimal). */
type AggregateStatics = {
  sum(): AggregateSpec;
  avg(): AggregateSpec;
  min(): AggregateSpec;
  max(): AggregateSpec;
  median(): AggregateSpec;
  std(): AggregateSpec;
};

export type AttributeTypeParent =
  | {
      readonly attrName: string;
      readonly attributeSchema?: AttributeSchemaEntry;
      readonly attributeSchemaEntries?: readonly AttributeSchemaEntry[];
    }
  | string
  | null;

export interface AttributeTypeOptions {
  readonly parent?: AttributeTypeParent;
  readonly abstract?: boolean;
  readonly independent?: boolean;
  readonly regex?: string | null;
  readonly values?: readonly string[] | null;
  readonly range?: readonly [string | null, string | null] | null;
  /** TypeDB 3.12+ `@doc("...")` documentation for the attribute type. */
  readonly doc?: string;
  /** TypeDB 3.12+ `@meta("key", "value")` annotations, one value per key. */
  readonly meta?: Record<string, string>;
}

/**
 * The abstract base class returned by an `attr.*(name)` factory: an abstract
 * `Attribute` constructor branded by `Name`, plus the static `attrName` and
 * wire `valueType`. Users extend it (`class Name extends attr.String("name") {}`);
 * they never name this type directly.
 */
export type AttributeBase<Value, Brand extends string> = (abstract new (
  value: Value,
) => Attribute<Value, Brand>) & {
  readonly attrName: Brand;
  readonly valueType: ValueType;
  readonly attributeSchema: AttributeSchemaEntry;
  readonly attributeSchemaEntries: readonly AttributeSchemaEntry[];
};

/** Attribute base with comparison + sort helpers (every value type). */
export type ComparableAttributeBase<Value, Brand extends string> = AttributeBase<Value, Brand> &
  ComparisonStatics<Value, Brand> &
  OrderStatics;

/** Attribute base for string values: comparison + sort + string matching. */
export type StringAttributeBase<Brand extends string> = ComparableAttributeBase<string, Brand> &
  StringStatics;

/** Attribute base for numeric values: comparison + sort + reduce helpers. */
export type NumericAttributeBase<Value, Brand extends string> = ComparableAttributeBase<
  Value,
  Brand
> &
  AggregateStatics;

function comparisonStatics<Value, Name extends string>(
  name: Name,
  valueType: ValueType,
): ComparisonStatics<Value, Name> {
  const build =
    (operator: DynamicComparisonOp) =>
    (value: Attribute<Value, Name>): ComparisonExpr =>
      new ComparisonExpr(name, operator, lowerAttributeValue(valueType, value.value));
  return {
    eq: build("eq"),
    ne: build("neq"),
    gt: build("gt"),
    gte: build("gte"),
    lt: build("lt"),
    lte: build("lte"),
  };
}

function orderStatics(name: string): OrderStatics {
  return {
    asc: () => new SortExpr(name, "Asc"),
    desc: () => new SortExpr(name, "Desc"),
  };
}

function stringStatics(name: string): StringStatics {
  const build =
    (operator: DynamicComparisonOp) =>
    (value: string): ComparisonExpr =>
      new ComparisonExpr(name, operator, { value_type: "string", value });
  return {
    contains: build("contains"),
    like: build("like"),
    startsWith: build("starts_with"),
    endsWith: build("ends_with"),
  };
}

function aggregateStatics(name: string): AggregateStatics {
  // The reduce variable must be a valid identifier, so attribute names with `-`
  // (kebab-case TypeDB type names) are sanitized to `_` for the wire `result_key`.
  // The user-facing result key keeps the original attribute name. `avg` lowers to
  // TypeDB's `mean`.
  const wireName = name.replace(/[^A-Za-z0-9_]/g, "_");
  const spec = (resultPrefix: string, fn: string) => (): AggregateSpec =>
    new AggregateSpec(
      { result_key: `${resultPrefix}_${wireName}`, function: fn, attr_name: name },
      `${resultPrefix}_${name}`,
    );
  return {
    sum: spec("sum", "sum"),
    avg: spec("avg", "mean"),
    min: spec("min", "min"),
    max: spec("max", "max"),
    median: spec("median", "median"),
    std: spec("std", "std"),
  };
}

// Build the branded base class and attach the statics whose category was chosen
// by the factory. The statics objects are type-checked individually; the final
// assertion only states that `Object.assign` placed them on the constructor.
function namedAttribute<Value, Name extends string>(
  name: Name,
  valueType: ValueType,
  options?: AttributeTypeOptions,
): { new (value: Value): Attribute<Value, Name> } & {
  readonly attrName: Name;
  readonly valueType: ValueType;
  readonly attributeSchema: AttributeSchemaEntry;
  readonly attributeSchemaEntries: readonly AttributeSchemaEntry[];
} {
  const attributeSchema = buildAttributeSchema(name, valueType, options);
  const attributeSchemaEntries = buildAttributeSchemaEntries(attributeSchema, options);
  abstract class NamedAttribute extends Attribute<Value, Name> {
    static readonly attrName = name;
    static readonly valueType = valueType;
    static readonly attributeSchema = attributeSchema;
    static readonly attributeSchemaEntries = attributeSchemaEntries;
  }
  return NamedAttribute as never;
}

function buildAttributeSchema(
  attrName: string,
  valueType: ValueType,
  options?: AttributeTypeOptions,
): AttributeSchemaEntry {
  const entry: AttributeSchemaEntry = { attr_name: attrName, value_type: valueType };
  if (options === undefined) return entry;

  if (options.parent !== undefined) {
    entry.parent_type = parentTypeName(options.parent);
  }
  if (options.abstract !== undefined) {
    entry.is_abstract = options.abstract;
  }
  if (options.independent !== undefined) {
    entry.is_independent = options.independent;
  }
  if (options.regex !== undefined) {
    entry.regex = options.regex;
  }
  if (options.values !== undefined) {
    entry.allowed_values = options.values == null ? null : [...options.values];
  }
  if (options.range !== undefined) {
    entry.range = options.range == null ? null : [options.range[0], options.range[1]];
  }
  if (options.doc !== undefined) {
    entry.doc = options.doc;
  }
  if (options.meta !== undefined && Object.keys(options.meta).length > 0) {
    entry.meta = { ...options.meta };
  }
  return entry;
}

function buildAttributeSchemaEntries(
  entry: AttributeSchemaEntry,
  options?: AttributeTypeOptions,
): readonly AttributeSchemaEntry[] {
  const parent = options?.parent;
  const entries: AttributeSchemaEntry[] = [];
  if (parent !== undefined && parent !== null && typeof parent !== "string") {
    const parentEntries =
      parent.attributeSchemaEntries ??
      (parent.attributeSchema === undefined ? [] : [parent.attributeSchema]);
    entries.push(...parentEntries.map(copyAttributeSchemaEntry));
  }
  entries.push(copyAttributeSchemaEntry(entry));
  return entries;
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

function parentTypeName(parent: AttributeTypeParent): string | null {
  if (parent == null || typeof parent === "string") {
    return parent;
  }
  return parent.attrName;
}

type ComparableFactory<Value> = <const Name extends string>(
  name: Name,
  options?: AttributeTypeOptions,
) => ComparableAttributeBase<Value, Name>;
type StringFactory = <const Name extends string>(
  name: Name,
  options?: AttributeTypeOptions,
) => StringAttributeBase<Name>;
type NumericFactory<Value> = <const Name extends string>(
  name: Name,
  options?: AttributeTypeOptions,
) => NumericAttributeBase<Value, Name>;

function makeComparableFactory<Value>(valueType: ValueType): ComparableFactory<Value> {
  return <const Name extends string>(
    name: Name,
    options?: AttributeTypeOptions,
  ): ComparableAttributeBase<Value, Name> => {
    const cls = namedAttribute<Value, Name>(name, valueType, options);
    Object.assign(cls, comparisonStatics<Value, Name>(name, valueType), orderStatics(name));
    return cls as unknown as ComparableAttributeBase<Value, Name>;
  };
}

function makeStringFactory(): StringFactory {
  return <const Name extends string>(
    name: Name,
    options?: AttributeTypeOptions,
  ): StringAttributeBase<Name> => {
    const cls = namedAttribute<string, Name>(name, "string", options);
    Object.assign(
      cls,
      comparisonStatics<string, Name>(name, "string"),
      orderStatics(name),
      stringStatics(name),
    );
    return cls as unknown as StringAttributeBase<Name>;
  };
}

function makeNumericFactory<Value>(valueType: ValueType): NumericFactory<Value> {
  return <const Name extends string>(
    name: Name,
    options?: AttributeTypeOptions,
  ): NumericAttributeBase<Value, Name> => {
    const cls = namedAttribute<Value, Name>(name, valueType, options);
    Object.assign(
      cls,
      comparisonStatics<Value, Name>(name, valueType),
      orderStatics(name),
      aggregateStatics(name),
    );
    return cls as unknown as NumericAttributeBase<Value, Name>;
  };
}

/**
 * Attribute base-class factories, one per TypeDB value type. Each call returns a
 * branded base to extend: the mandatory `name` is both the schema `attr_name`
 * and the compile-time brand, so distinct names produce non-interchangeable
 * types. The `attr.*` namespace avoids shadowing the `String`/`Boolean`/`Date`
 * JS globals. `Integer` wraps `bigint` and maps to the `long` wire type. String
 * bases gain `contains`/`like`/`startsWith`/`endsWith`; numeric bases gain
 * `sum`/`avg`/`min`/`max`/`median`/`std`; every base gains the comparison and
 * sort helpers used to build typed queries.
 */
export const attr = {
  String: makeStringFactory(),
  Integer: makeNumericFactory<bigint>("long"),
  Double: makeNumericFactory<number>("double"),
  Boolean: makeComparableFactory<boolean>("boolean"),
  Date: makeComparableFactory<string>("date"),
  DateTime: makeComparableFactory<string>("datetime"),
  DateTimeTZ: makeComparableFactory<string>("datetime-tz"),
  Decimal: makeNumericFactory<string>("decimal"),
  Duration: makeComparableFactory<string>("duration"),
} as const;
