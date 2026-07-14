import type { AttributeSchemaEntry, ValueType } from "./index.js";
import { AggregateSpec, ComparisonExpr, SortExpr } from "./query.js";
declare const attributeBrand: unique symbol;
declare const attributeCategory: unique symbol;
/**
 * Base class for a branded attribute value. `Value` is the wrapped runtime
 * value; `Brand` is the schema attribute name carried as a phantom type so that
 * distinct attribute classes are nominally distinct at compile time. Instances
 * are immutable; equality is by `(constructor, value)`.
 */
export declare abstract class Attribute<Value, Brand extends string, Category extends ValueType = ValueType> {
    readonly value: Value;
    readonly [attributeBrand]: Brand;
    readonly [attributeCategory]: Category;
    constructor(value: Value);
    equals(other: unknown): boolean;
    toString(): string;
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
export type AttributeTypeParent = {
    readonly attrName: string;
    readonly attributeSchema?: AttributeSchemaEntry;
    readonly attributeSchemaEntries?: readonly AttributeSchemaEntry[];
} | string | null;
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
export type AttributeBase<Value, Brand extends string, Category extends ValueType = ValueType> = (abstract new (value: Value) => Attribute<Value, Brand, Category>) & {
    readonly attrName: Brand;
    readonly valueType: Category;
    readonly attributeSchema: AttributeSchemaEntry;
    readonly attributeSchemaEntries: readonly AttributeSchemaEntry[];
};
/** Attribute base with comparison + sort helpers (every value type). */
export type ComparableAttributeBase<Value, Brand extends string, Category extends ValueType = ValueType> = AttributeBase<Value, Brand, Category> & ComparisonStatics<Value, Brand> & OrderStatics;
/** Attribute base for string values: comparison + sort + string matching. */
export type StringAttributeBase<Brand extends string> = ComparableAttributeBase<string, Brand, "string"> & StringStatics;
/** Attribute base for numeric values: comparison + sort + reduce helpers. */
export type NumericAttributeBase<Value, Brand extends string, Category extends "long" | "double" | "decimal" = "long" | "double" | "decimal"> = ComparableAttributeBase<Value, Brand, Category> & AggregateStatics;
type ComparableFactory<Value, Category extends ValueType> = <const Name extends string>(name: Name, options?: AttributeTypeOptions) => ComparableAttributeBase<Value, Name, Category>;
type StringFactory = <const Name extends string>(name: Name, options?: AttributeTypeOptions) => StringAttributeBase<Name>;
type NumericFactory<Value, Category extends "long" | "double" | "decimal"> = <const Name extends string>(name: Name, options?: AttributeTypeOptions) => NumericAttributeBase<Value, Name, Category>;
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
export declare const attr: {
    readonly String: StringFactory;
    readonly Integer: NumericFactory<bigint, "long">;
    readonly Double: NumericFactory<number, "double">;
    readonly Boolean: ComparableFactory<boolean, "boolean">;
    readonly Date: ComparableFactory<string, "date">;
    readonly DateTime: ComparableFactory<string, "datetime">;
    readonly DateTimeTZ: ComparableFactory<string, "datetime-tz">;
    readonly Decimal: NumericFactory<string, "decimal">;
    readonly Duration: ComparableFactory<string, "duration">;
};
export {};
