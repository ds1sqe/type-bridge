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
export interface TypeFlagsOptions<Name extends string | null = string | null> {
    name?: Name;
    abstract?: boolean;
    base?: boolean;
    case?: TypeNameCase;
    /** TypeDB 3.12+ `@doc("...")` documentation for the type. */
    doc?: string | null;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, one value per key. */
    meta?: Record<string, string>;
}
export interface ResolvedTypeFlags<Name extends string | null = string | null> {
    readonly name: Name;
    readonly abstract: boolean;
    readonly base: boolean;
    readonly case: TypeNameCase;
    readonly doc: string | null;
    readonly meta: Record<string, string>;
}
export interface AttributeFlagsOptions {
    name?: string | null;
    case?: TypeNameCase | null;
    /** TypeDB 3.12+ `@doc("...")` documentation for the attribute type. */
    doc?: string | null;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations, one value per key. */
    meta?: Record<string, string>;
}
export interface ResolvedAttributeFlags {
    readonly name: string | null;
    readonly case: TypeNameCase | null;
    readonly doc: string | null;
    readonly meta: Record<string, string>;
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
    readonly isOrdered: boolean;
    readonly isDistinct: boolean;
    /** TypeDB 3.12+ `@doc("...")` documentation for the ownership. */
    readonly doc: string | null;
    /** TypeDB 3.12+ `@meta("key", "value")` annotations for the ownership. */
    readonly meta: Record<string, string>;
}
/** Marks a field as the type's key attribute (implies cardinality `[1, 1]`). */
export declare const Key = "Key";
/** Marks a field's attribute value as unique across the type. */
export declare const Unique = "Unique";
/** Declares this owns clause as a list attribute (`owns attr[]`). Schema-only. */
export declare const Ordered = "Ordered";
/** Emits `@distinct` on the owns clause. Requires `Ordered`. Schema-only. */
export declare const Distinct = "Distinct";
/** TypeDB 3.12+ `@doc("...")` marker for one ownership. */
export interface DocSpec {
    readonly kind: "doc";
    readonly text: string;
}
/** TypeDB 3.12+ `@meta("key", "value")` marker for one ownership. */
export interface MetaSpec {
    readonly kind: "meta";
    readonly key: string;
    readonly value: string;
}
/** Documentation marker for the TypeDB 3.12+ `@doc("...")` ownership annotation. */
export declare function Doc(text: string): DocSpec;
/** Metadata marker for the TypeDB 3.12+ `@meta("key", "value")` ownership annotation. */
export declare function Meta(key: string, value: string): MetaSpec;
export type FlagInput = typeof Key | typeof Unique | typeof Ordered | typeof Distinct | CardSpec | FlagSpec | DocSpec | MetaSpec;
type NamelessTypeFlagsOptions = Omit<TypeFlagsOptions, "name"> & {
    readonly name?: never;
};
/** Type-level config for an `Entity`/`Relation` (explicit name, abstract, base, case). */
export declare function TypeFlags(): ResolvedTypeFlags<null>;
export declare function TypeFlags(options: NamelessTypeFlagsOptions): ResolvedTypeFlags<null>;
export declare function TypeFlags<const Name extends string | null>(options: TypeFlagsOptions<Name> & {
    readonly name: Name;
}): ResolvedTypeFlags<Name>;
export declare function TypeFlags(options: TypeFlagsOptions): ResolvedTypeFlags;
/** Attribute-level config: an explicit attribute name and/or case override. */
export declare function AttributeFlags(options?: AttributeFlagsOptions): ResolvedAttributeFlags;
/** A cardinality bound `[min, max]`. Omitting `max` means unbounded (`[min, null]`). */
export declare function Card<const Min extends number, const Max extends number | null = null>(min: Min, max?: Max): CardSpec<Min, Max>;
/**
 * Combine flags (`Key`, `Unique`, `Card(...)`) for one field into a resolved set
 * of descriptor annotations plus a derived cardinality.
 */
export declare function Flag(...flags: FlagInput[]): FlagSpec;
/** Lower a flag list to its `{ annotations, cardinality, isOrdered, isDistinct }` descriptor form. */
export declare function resolveFlags(flags: readonly FlagInput[]): FlagSpec;
/** Convert a model class name to its TypeDB type name under the given case. */
export declare function formatTypeName(className: string, typeCase: TypeNameCase): string;
export {};
