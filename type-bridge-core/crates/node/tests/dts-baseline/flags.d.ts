import type { Annotation } from "./index.js";
/**
 * How a model class name is converted to its TypeDB type name when no explicit
 * name is given. Mirrors the Python `TypeNameCase`: `CLASS_NAME` keeps the class
 * name as-is, `LOWERCASE` lowercases it, `SNAKE_CASE` converts to snake_case.
 */
export declare enum TypeNameCase {
    LOWERCASE = "lowercase",
    CLASS_NAME = "classname",
    SNAKE_CASE = "snake_case"
}
export interface TypeFlagsOptions {
    name?: string | null;
    abstract?: boolean;
    base?: boolean;
    case?: TypeNameCase;
}
export interface ResolvedTypeFlags {
    readonly name: string | null;
    readonly abstract: boolean;
    readonly base: boolean;
    readonly case: TypeNameCase;
}
export interface AttributeFlagsOptions {
    name?: string | null;
    case?: TypeNameCase | null;
}
export interface ResolvedAttributeFlags {
    readonly name: string | null;
    readonly case: TypeNameCase | null;
}
export interface CardSpec<Min extends number = number, Max extends number | null = number | null> {
    readonly kind: "card";
    readonly min: Min;
    readonly max: Max;
}
export interface FlagSpec {
    readonly kind: "flag";
    readonly annotations: Annotation[];
    readonly cardinality: [number, number | null] | null;
}
/** Marks a field as the type's key attribute (implies cardinality `[1, 1]`). */
export declare const Key = "Key";
/** Marks a field's attribute value as unique across the type. */
export declare const Unique = "Unique";
export type FlagInput = typeof Key | typeof Unique | CardSpec | FlagSpec;
/** Type-level config for an `Entity`/`Relation` (explicit name, abstract, base, case). */
export declare function TypeFlags(options?: TypeFlagsOptions): ResolvedTypeFlags;
/** Attribute-level config: an explicit attribute name and/or case override. */
export declare function AttributeFlags(options?: AttributeFlagsOptions): ResolvedAttributeFlags;
/** A cardinality bound `[min, max]`. Omitting `max` means unbounded (`[min, null]`). */
export declare function Card<const Min extends number, const Max extends number | null = null>(min: Min, max?: Max): CardSpec<Min, Max>;
/**
 * Combine flags (`Key`, `Unique`, `Card(...)`) for one field into a resolved set
 * of descriptor annotations plus a derived cardinality.
 */
export declare function Flag(...flags: FlagInput[]): FlagSpec;
/** Lower a flag list to its `{ annotations, cardinality }` descriptor form. */
export declare function resolveFlags(flags: readonly FlagInput[]): FlagSpec;
/** Convert a model class name to its TypeDB type name under the given case. */
export declare function formatTypeName(className: string, typeCase: TypeNameCase): string;
