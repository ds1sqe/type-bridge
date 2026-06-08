/**
 * TypeScript model generator — the `generate_models()` twin.
 *
 * Mirrors `type_bridge/generator/__init__.py` → `generate_models()`.
 * Consumes a parsed `TypeSchema` (via `parseSchema`) and writes the typed
 * `.ts` surface to a caller-supplied output directory.
 *
 * The native module is INJECTED (not self-resolved) — callers load the native
 * module via the package root's `loadNative()` and pass the `SchemaParserNative`
 * slice in `options.native`. This keeps the generator free of any native-resolution
 * path, so it survives the package layout change.
 *
 * Generated files import from `@type-bridge/node` (package entrypoint), never
 * from a hardcoded relative path, so they are valid across packaging layout changes.
 *
 * Build-time code generation; no runtime ORM logic.
 */

import fs from "node:fs";
import path from "node:path";

import { parseSchema, type SchemaParserNative } from "../parser.js";
import { renderAttributes } from "./renderAttributes.js";
import { renderEntities } from "./renderEntities.js";
import { renderRelations } from "./renderRelations.js";
import { renderPackage } from "./renderPackage.js";
import type { NamingOptions } from "./naming.js";

/** Options for `generateModels`. */
export interface GenerateModelsOptions extends NamingOptions {
  /**
   * The native module slice providing `parseSchemaJson`. Callers obtain this
   * from the package root's `loadNative()` return value. Required — the
   * generator does not self-load the native module.
   */
  native: SchemaParserNative;
}

/**
 * Generate the typed TypeScript model surface from a TQL `define` block.
 *
 * Mirrors Python `generate_models(tql, output_dir, implicit_key_attributes=...)`.
 *
 * Writes `attributes.ts`, `entities.ts`, `relations.ts`, and `index.ts`
 * (package barrel) to `outputDir`.
 *
 * @param tql       - TQL schema string (a `define` block).
 * @param outputDir - Directory to write the generated `.ts` files into.
 *                    Created if absent. Passed by the caller; the generator
 *                    does not enforce a specific location.
 * @param options   - `native` (required) + optional `implicitKeyAttributes`.
 */
export function generateModels(
  tql: string,
  outputDir: string,
  options: GenerateModelsOptions,
): void {
  // Parse: TQL string → fully-resolved TypeSchema (Rust-backed, via NAPI)
  const schema = parseSchema(tql, options.native);

  // Render
  const attrSource = renderAttributes(schema);
  const entitySource = renderEntities(schema, options);
  const relationSource = renderRelations(schema, options);
  const packageSource = renderPackage(schema);

  // Write
  fs.mkdirSync(outputDir, { recursive: true });
  fs.writeFileSync(path.join(outputDir, "attributes.ts"), attrSource, "utf-8");
  fs.writeFileSync(path.join(outputDir, "entities.ts"), entitySource, "utf-8");
  fs.writeFileSync(path.join(outputDir, "relations.ts"), relationSource, "utf-8");
  fs.writeFileSync(path.join(outputDir, "index.ts"), packageSource, "utf-8");
}

// Re-export the naming utilities and options type for callers that need them
export { toClassName, toFieldName, type NamingOptions } from "./naming.js";
