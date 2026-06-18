/**
 * TypeScript model generator facade.
 *
 * Rendering is owned by the Rust bindgen engine. This module only injects the
 * native module, requests a target package, and writes returned files.
 */

import fs from "node:fs";
import path from "node:path";

import type { NamingOptions } from "./naming.js";

/** Output targets supported by the Rust bindgen engine. */
export type BindgenTarget = "python" | "typescript" | "rust";

/** One generated source file returned by the native bindgen engine. */
export interface GeneratedFile {
  /** Relative output path. */
  path: string;
  /** Complete source text. */
  contents: string;
}

/** Generated package payload returned by the native bindgen engine. */
export interface GeneratedPackage {
  /** Target language rendered by Rust. */
  target: BindgenTarget;
  /** Files to write. */
  files: GeneratedFile[];
}

/** Native module slice required by the generator facade. */
export interface BindgenNative {
  /** Render model files as a JSON {@link GeneratedPackage}. */
  renderModelsJson(input: string, target: string, optionsJson?: string | null): string;
}

/** Shared Rust bindgen render options exposed through the TypeScript facade. */
export interface BindgenRenderOptions extends NamingOptions {
  /** Schema version rendered into generated Python package metadata. */
  schemaVersion?: string;
  /** Bundled schema filename for generated Python `schema_text()`, or `null` to omit it. */
  schemaFilename?: string | null;
  /** Source schema text used by generated Python registry metadata. */
  schemaText?: string | null;
}

/** Options for `generateModels`. */
export interface GenerateModelsOptions extends BindgenRenderOptions {
  /**
   * The native module slice providing `renderModelsJson`. Callers obtain this
   * from the package root's `loadNative()` return value.
   */
  native: BindgenNative;
}

/** Options for cross-target model generation. */
export interface GenerateTargetModelsOptions extends GenerateModelsOptions {
  /** Target language to render. */
  target: BindgenTarget;
}

function requestGeneratedPackage(
  tql: string,
  options: GenerateModelsOptions,
  target: BindgenTarget,
): GeneratedPackage {
  const payload: Record<string, unknown> = {
    implicit_key_attributes: options.implicitKeyAttributes ?? [],
  };
  if (options.schemaVersion !== undefined) {
    payload.schema_version = options.schemaVersion;
  }
  if (options.schemaFilename !== undefined) {
    payload.schema_filename = options.schemaFilename;
  }
  if (options.schemaText !== undefined) {
    payload.schema_text = options.schemaText;
  }
  return JSON.parse(
    options.native.renderModelsJson(tql, target, JSON.stringify(payload)),
  ) as GeneratedPackage;
}

function writePackage(outputDir: string, generated: GeneratedPackage): void {
  fs.mkdirSync(outputDir, { recursive: true });
  for (const file of generated.files) {
    const filePath = path.join(outputDir, file.path);
    fs.mkdirSync(path.dirname(filePath), { recursive: true });
    fs.writeFileSync(filePath, file.contents, "utf-8");
  }
}

/**
 * Generate the typed TypeScript model surface from a TQL `define` block.
 *
 * Historical entrypoint: this always targets TypeScript. Use
 * `generateModelsForTarget` for cross-target generation.
 */
export function generateModels(
  tql: string,
  outputDir: string,
  options: GenerateModelsOptions,
): void {
  writePackage(outputDir, requestGeneratedPackage(tql, options, "typescript"));
}

/** Generate model files for any Rust-bindgen target. */
export function generateModelsForTarget(
  tql: string,
  outputDir: string,
  options: GenerateTargetModelsOptions,
): void {
  writePackage(outputDir, requestGeneratedPackage(tql, options, options.target));
}

// Re-export the naming utilities and options type for callers that need them.
export { toClassName, toFieldName, type NamingOptions } from "./naming.js";
