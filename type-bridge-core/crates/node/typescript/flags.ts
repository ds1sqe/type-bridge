import type { Annotation } from "./index.js";

/**
 * How a model class name is converted to its TypeDB type name when no explicit
 * name is given. Mirrors the Python `TypeNameCase`: `CLASS_NAME` keeps the class
 * name as-is, `LOWERCASE` lowercases it, `SNAKE_CASE` converts to snake_case.
 */
export enum TypeNameCase {
  LOWERCASE = "lowercase",
  CLASS_NAME = "classname",
  SNAKE_CASE = "snake_case",
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
  readonly isOrdered: boolean;
  readonly isDistinct: boolean;
}

/** Marks a field as the type's key attribute (implies cardinality `[1, 1]`). */
export const Key = "Key";
/** Marks a field's attribute value as unique across the type. */
export const Unique = "Unique";
/** Declares this owns clause as a list attribute (`owns attr[]`). Schema-only. */
export const Ordered = "Ordered";
/** Emits `@distinct` on the owns clause. Requires `Ordered`. Schema-only. */
export const Distinct = "Distinct";

export type FlagInput = typeof Key | typeof Unique | typeof Ordered | typeof Distinct | CardSpec | FlagSpec;

/** Type-level config for an `Entity`/`Relation` (explicit name, abstract, base, case). */
export function TypeFlags(options: TypeFlagsOptions = {}): ResolvedTypeFlags {
  return {
    name: options.name ?? null,
    abstract: options.abstract ?? false,
    base: options.base ?? false,
    case: options.case ?? TypeNameCase.CLASS_NAME,
  };
}

/** Attribute-level config: an explicit attribute name and/or case override. */
export function AttributeFlags(options: AttributeFlagsOptions = {}): ResolvedAttributeFlags {
  return {
    name: options.name ?? null,
    case: options.case ?? null,
  };
}

/** A cardinality bound `[min, max]`. Omitting `max` means unbounded (`[min, null]`). */
export function Card<const Min extends number, const Max extends number | null = null>(
  min: Min,
  max?: Max,
): CardSpec<Min, Max> {
  return {
    kind: "card",
    min,
    max: (max ?? null) as Max,
  };
}

/**
 * Combine flags (`Key`, `Unique`, `Card(...)`) for one field into a resolved set
 * of descriptor annotations plus a derived cardinality.
 */
export function Flag(...flags: FlagInput[]): FlagSpec {
  return resolveFlags(flags);
}

/** Lower a flag list to its `{ annotations, cardinality, isOrdered, isDistinct }` descriptor form. */
export function resolveFlags(flags: readonly FlagInput[]): FlagSpec {
  const annotations: Annotation[] = [];
  let cardinality: [number, number | null] | null = null;
  let isOrdered = false;
  let isDistinct = false;

  for (const flag of flags) {
    if (isFlagSpec(flag)) {
      annotations.push(...flag.annotations);
      cardinality = flag.cardinality ?? cardinality;
      isOrdered = isOrdered || flag.isOrdered;
      isDistinct = isDistinct || flag.isDistinct;
      continue;
    }
    if (flag === Key) {
      annotations.push("Key");
      cardinality = [1, 1];
      continue;
    }
    if (flag === Unique) {
      annotations.push("Unique");
      continue;
    }
    if (flag === Ordered) {
      isOrdered = true;
      continue;
    }
    if (flag === Distinct) {
      isDistinct = true;
      continue;
    }
    if (isCardSpec(flag)) {
      const card: [number, number | null] = [flag.min, flag.max];
      annotations.push({ Card: card });
      cardinality = card;
    }
  }

  if (isDistinct && !isOrdered) {
    throw new TypeError(
      "Flag(Distinct) requires Flag(Ordered): @distinct is only valid on a list attribute (owns attr[]).",
    );
  }

  return { kind: "flag", annotations, cardinality, isOrdered, isDistinct };
}

/** Convert a model class name to its TypeDB type name under the given case. */
export function formatTypeName(className: string, typeCase: TypeNameCase): string {
  if (typeCase === TypeNameCase.CLASS_NAME) {
    return className;
  }
  if (typeCase === TypeNameCase.SNAKE_CASE) {
    return toSnakeCase(className);
  }
  return className.toLowerCase();
}

function toSnakeCase(name: string): string {
  return name
    .replace(/(.)([A-Z][a-z]+)/g, "$1_$2")
    .replace(/([a-z0-9])([A-Z])/g, "$1_$2")
    .toLowerCase();
}

function isCardSpec(flag: FlagInput): flag is CardSpec {
  return typeof flag === "object" && flag !== null && flag.kind === "card";
}

function isFlagSpec(flag: FlagInput): flag is FlagSpec {
  return typeof flag === "object" && flag !== null && flag.kind === "flag";
}
