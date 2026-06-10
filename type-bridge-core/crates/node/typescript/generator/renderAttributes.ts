/**
 * Render the `attributes.ts` module text from a parsed TypeSchema.
 *
 * Mirrors `type_bridge/generator/render/attributes.py` → `render_attributes()`.
 * Emits one branded `export class <ClassName> extends attr.<Kind>("<name>", ...) {}`
 * per attribute, in deterministic parent-before-child order matching the
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
 * Return attribute names in deterministic topological order: parents before
 * children, with alphabetical order among unrelated attributes.
 */
function sortedAttrNames(schema: TypeSchema): string[] {
  const sorted: string[] = [];
  const visited = new Set<string>();
  const names = Object.keys(schema.attributes).sort();

  function visit(name: string): void {
    if (visited.has(name)) return;
    visited.add(name);
    const parent = schema.attributes[name]?.parent;
    if (parent && schema.attributes[parent]) {
      visit(parent);
    }
    sorted.push(name);
  }

  for (const name of names) {
    visit(name);
  }
  return sorted;
}

function attributeOptionsLiteral(
  attrName: string,
  schema: TypeSchema,
  attrClassMap: Map<string, string>,
): string {
  const attribute = schema.attributes[attrName];
  if (!attribute) return "";

  const fields: string[] = [];
  if (attribute.parent) {
    const parentClass = attrClassMap.get(attribute.parent);
    fields.push(
      parentClass
        ? `parent: ${parentClass}`
        : `parent: ${JSON.stringify(attribute.parent)}`,
    );
  }
  if (attribute.is_abstract) {
    fields.push(`abstract: true`);
  }
  if (attribute.is_independent) {
    fields.push(`independent: true`);
  }
  if (attribute.regex !== null) {
    fields.push(`regex: ${JSON.stringify(attribute.regex)}`);
  }
  if (attribute.allowed_values !== null) {
    fields.push(`values: ${JSON.stringify(attribute.allowed_values)}`);
  }
  if (attribute.range_min !== null || attribute.range_max !== null) {
    fields.push(
      `range: [${optionalStringLiteral(attribute.range_min)}, ${optionalStringLiteral(attribute.range_max)}]`,
    );
  }

  return fields.length === 0 ? "" : `, { ${fields.join(", ")} }`;
}

function optionalStringLiteral(value: string | null): string {
  return value === null ? "null" : JSON.stringify(value);
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
  const attrClassMap = new Map<string, string>();
  for (const name of Object.keys(schema.attributes)) {
    attrClassMap.set(name, toClassName(name));
  }

  const lines: string[] = [
    `import { attr } from "@type-bridge/node";`,
    ``,
  ];

  for (const name of names) {
    const className = attrClassMap.get(name) ?? toClassName(name);
    const kind = resolveAttrKind(name, schema);
    const options = attributeOptionsLiteral(name, schema, attrClassMap);
    lines.push(`export class ${className} extends attr.${kind}("${name}"${options}) {}`);
  }

  // Trailing newline
  lines.push(``);

  return lines.join("\n");
}
