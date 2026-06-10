/**
 * Render the output package barrel (`index.ts`) text.
 *
 * Mirrors Python `render_package_init()` in `type_bridge/generator/__init__.py`.
 * The barrel re-exports everything from the three generated modules so that
 * callers can import from a single entry point.
 *
 * This file produces the content written to `<outputDir>/index.ts`.
 * It is intentionally named `renderPackage.ts` (not `index.ts`) to avoid
 * confusion with `generator/index.ts` (the generator entry point).
 *
 * Build-time code generation; no runtime ORM logic.
 */

import type { TypeSchema } from "../parser.js";

/**
 * Render the package barrel `index.ts` that re-exports all generated modules.
 *
 * The `schema` parameter is accepted for API symmetry with the other renderers
 * (and to allow future conditional re-exports), but the barrel is unconditional:
 * all three modules are always re-exported.
 */
// eslint-disable-next-line @typescript-eslint/no-unused-vars
export function renderPackage(_schema: TypeSchema): string {
  const lines: string[] = [
    `export * from "./attributes.js";`,
    `export * from "./entities.js";`,
    `export * from "./relations.js";`,
    ``,
  ];
  return lines.join("\n");
}
