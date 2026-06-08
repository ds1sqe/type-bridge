/**
 * Render the `attributes.ts` module text from a parsed TypeSchema.
 *
 * Mirrors `type_bridge/generator/render/attributes.py` → `render_attributes()`.
 * Emits one branded `export class <ClassName> extends attr.<Kind>("<name>") {}`
 * per attribute, in deterministic (alphabetical-by-name) order matching the
 * Python generator's topological/sorted emit.
 *
 * Build-time code generation; no runtime ORM logic.
 */

import type { TypeSchema } from "../parser.js";
import { toClassName } from "./naming.js";

/**
 * Map TypeDB `value_type` strings to the `attr.*` factory method name.
 *
 * Verified against `typescript/attribute.ts` `attr` export:
 *   attr.String       -> "string"
 *   attr.Integer      -> "long"    (bigint, maps to the `long` wire type)
 *   attr.Double       -> "double"
 *   attr.Boolean      -> "boolean"
 *   attr.Date         -> "date"
 *   attr.DateTime     -> "datetime"
 *   attr.DateTimeTZ   -> "datetime-tz"
 *   attr.Decimal      -> "decimal"
 *   attr.Duration     -> "duration"
 *
 * The parsed schema preserves the declared value_type: an integer attribute
 * arrives as "integer", not "long". Both "integer" and "long" map to attr.Integer
 * (whose descriptor records the "long" wire type), mirroring the Python generator.
 */
const VALUE_TYPE_TO_KIND: Record<string, string> = {
  string: "String",
  integer: "Integer",
  long: "Integer",
  double: "Double",
  boolean: "Boolean",
  date: "Date",
  datetime: "DateTime",
  "datetime-tz": "DateTimeTZ",
  decimal: "Decimal",
  duration: "Duration",
};

/**
 * Resolve the `attr.*` factory kind for an attribute, following value_type
 * inheritance. Mirrors `_resolve_value_type` in `render/attributes.py`.
 */
function resolveAttrKind(
  attrName: string,
  schema: TypeSchema,
  visited = new Set<string>(),
): string {
  if (visited.has(attrName)) return "String";
  visited.add(attrName);

  const attr = schema.attributes[attrName];
  if (!attr) return "String";

  if (attr.value_type) {
    const kind = VALUE_TYPE_TO_KIND[attr.value_type];
    if (kind === undefined) {
      // Fail loudly rather than emit a silently-wrong attr.String — an
      // unmapped value_type means a TypeDB type the generator does not handle.
      throw new Error(
        `Cannot generate attribute "${attrName}": unsupported value_type "${attr.value_type}".`,
      );
    }
    return kind;
  }

  if (attr.parent) {
    return resolveAttrKind(attr.parent, schema, visited);
  }

  return "String";
}

/**
 * Return attribute names sorted alphabetically — deterministic emit order.
 *
 * Python's `render_attributes` emits in topological order (parents before
 * children); here the generated TS has no inheritance between attribute classes
 * (each extends `attr.<Kind>(name)` directly), so alphabetical order is stable
 * and deterministic without a topo-sort.
 */
function sortedAttrNames(schema: TypeSchema): string[] {
  return Object.keys(schema.attributes).sort();
}

/**
 * Render the complete `attributes.ts` source text from the parsed schema.
 *
 * Each attribute emits:
 *   export class <ClassName> extends attr.<Kind>("<schema-name>") {}
 *
 * The import header always uses the PACKAGE ENTRYPOINT `@type-bridge/node`,
 * never a hardcoded relative path, so generated files stay valid across
 * the packaging layout change.
 */
export function renderAttributes(schema: TypeSchema): string {
  const names = sortedAttrNames(schema);

  const lines: string[] = [
    `import { attr } from "@type-bridge/node";`,
    ``,
  ];

  for (const name of names) {
    const className = toClassName(name);
    const kind = resolveAttrKind(name, schema);
    lines.push(`export class ${className} extends attr.${kind}("${name}") {}`);
  }

  // Trailing newline
  lines.push(``);

  return lines.join("\n");
}
