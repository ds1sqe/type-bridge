/**
 * Naming convention utilities for TypeScript code generation.
 *
 * Mirrors `type_bridge/generator/naming.py` exactly so that the TS generator
 * and the Python generator apply identical naming policy to the same schema and
 * produce cross-language-parity descriptors.
 *
 * Build-time code generation; no runtime ORM logic. The RUNTIME facade is a
 * separate marshalling-only layer; this module does not touch it.
 */

/**
 * Convert a TypeDB type name to PascalCase for TypeScript class names.
 *
 * Mirrors Python `to_class_name`:
 *   `"".join(part.capitalize() for part in label.replace("_","-").split("-"))`
 *
 * `capitalize()` upper-cases the FIRST character and LOWER-CASES the rest —
 * equivalent to `p[0].toUpperCase() + p.slice(1).toLowerCase()` in JS.
 *
 * Examples:
 *   "person"          -> "Person"
 *   "isbn-13"         -> "Isbn13"
 *   "order-line"      -> "OrderLine"
 *   "user_story"      -> "UserStory"
 *   "login_at"        -> "LoginAt"
 *   "parity-tag"      -> "ParityTag"
 *   "URL"             -> "Url"  (proves lower-casing of the rest)
 */
export function toClassName(label: string): string {
  return label
    .replace(/_/g, "-")
    .split("-")
    .map((part) => (part.length === 0 ? "" : part[0]!.toUpperCase() + part.slice(1).toLowerCase()))
    .join("");
}

/**
 * Convert a TypeDB attribute name to a snake_case field key.
 *
 * Mirrors Python `to_python_name`:
 *   `label.replace("-", "_")`
 *
 * Kebab-to-snake only. Prefix is KEPT; no pluralization; no stripping.
 *
 * Examples:
 *   "parity-id"          -> "parity_id"
 *   "parity-tag"         -> "parity_tag"   (singular — no pluralization)
 *   "parity-birth-date"  -> "parity_birth_date"
 *   "isbn-13"            -> "isbn_13"
 *   "order-line"         -> "order_line"
 */
export function toFieldName(label: string): string {
  return label.replace(/-/g, "_");
}

/**
 * Options controlling key-inference policy, mirroring
 * `generate_models(... implicit_key_attributes=...)`.
 *
 * An attribute whose name is listed in `implicitKeyAttributes` is emitted with
 * `field(Attr, Key)` unless the schema already marks it `@key` (schema key always
 * wins — implicit key never downgrades a schema key).
 */
export interface NamingOptions {
  /** Attribute names to treat as keys even when the schema has no `@key`. */
  implicitKeyAttributes?: string[];
}

/**
 * Return true when `attrName` should be emitted as a Key field.
 *
 * Schema `is_key` always wins. `implicitKeyAttributes` adds key-ness only for
 * attributes NOT already marked by the schema — it never downgrades a schema key.
 */
export function isKeyAttribute(
  attrName: string,
  isSchemaKey: boolean,
  options?: NamingOptions,
): boolean {
  if (isSchemaKey) return true;
  return (options?.implicitKeyAttributes ?? []).includes(attrName);
}
