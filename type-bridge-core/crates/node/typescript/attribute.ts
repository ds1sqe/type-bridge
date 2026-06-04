import type { ValueType } from "./index.js";

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
};

type AttributeFactory<Value> = <const Name extends string>(
  name: Name,
) => AttributeBase<Value, Name>;

function makeAttributeFactory<Value>(valueType: ValueType): AttributeFactory<Value> {
  return <const Name extends string>(name: Name): AttributeBase<Value, Name> => {
    abstract class NamedAttribute extends Attribute<Value, Name> {
      static readonly attrName = name;
      static readonly valueType = valueType;
    }

    return NamedAttribute;
  };
}

/**
 * Attribute base-class factories, one per TypeDB value type. Each call returns a
 * branded base to extend: the mandatory `name` is both the schema `attr_name`
 * and the compile-time brand, so distinct names produce non-interchangeable
 * types. The `attr.*` namespace avoids shadowing the `String`/`Boolean`/`Date`
 * JS globals. `Integer` wraps `bigint` and maps to the `long` wire type.
 */
export const attr = {
  String: makeAttributeFactory<string>("string"),
  Integer: makeAttributeFactory<bigint>("long"),
  Double: makeAttributeFactory<number>("double"),
  Boolean: makeAttributeFactory<boolean>("boolean"),
  Date: makeAttributeFactory<string>("date"),
  DateTime: makeAttributeFactory<string>("datetime"),
  DateTimeTZ: makeAttributeFactory<string>("datetime-tz"),
  Decimal: makeAttributeFactory<string>("decimal"),
  Duration: makeAttributeFactory<string>("duration"),
} as const;
