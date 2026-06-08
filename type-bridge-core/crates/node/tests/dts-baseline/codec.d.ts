import { type AttributeInput, type AttributeValue, type DynamicEntityRow, type OwnedAttributeDescriptor, type RuntimeAttributeValue, type ValueType } from "./index.js";
import { type Attribute } from "./attribute.js";
import type { AttributeClass, EntitySchema, RelationSchema } from "./model.js";
export declare class TypedCodecError extends Error {
    constructor(message: string);
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
export declare function attributeToPlain(value: Attribute<unknown, string>): unknown;
export declare function attributeToPlain(value: Attribute<unknown, string>[]): unknown[];
/**
 * Wrap a plain primitive (or array of plain primitives) into a branded
 * `Attribute` (or `Attribute[]`) using the given attribute class constructor.
 * Mirrors the `new attrType(value)` brand-construction pattern in
 * `hydrateAttributeValue` (`codec.ts:192-224`).
 *
 * Used by `fromDict` to re-brand plain dict values back into typed instances.
 */
export declare function plainToAttribute(attrType: AttributeClass, value: unknown, fieldName: string, isList: boolean): Attribute<unknown, string> | Attribute<unknown, string>[];
export declare function lowerAttributes(instance: object, schema: EntitySchema | RelationSchema): AttributeInput;
export declare function lowerFilters(filters: Record<string, Attribute<unknown, string>> | null | undefined, schema: EntitySchema | RelationSchema): Record<string, AttributeValue> | null;
export declare function hydrateAttributes(row: Pick<DynamicEntityRow, "attributes">, schema: EntitySchema | RelationSchema): Record<string, Attribute<unknown, string> | Attribute<unknown, string>[]>;
export declare function hydrateAttributeEntries(entries: readonly (readonly [string, RuntimeAttributeValue])[], schema: EntitySchema | RelationSchema): Record<string, Attribute<unknown, string> | Attribute<unknown, string>[]>;
export declare function runtimeAttributeValueFromUnknown(value: unknown, valueType?: ValueType): RuntimeAttributeValue;
export declare function keyAttributeDescriptor(schema: EntitySchema): OwnedAttributeDescriptor | null;
export declare function lowerAttributeValue(valueType: ValueType, value: unknown): AttributeValue;
