import * as fs from "fs";
import * as path from "path";
import type { NativeModule } from "./index.js";

// This module compiles to dist/native.js (CommonJS). __dirname is therefore
// the dist/ directory. The .node artifacts are placed at the package root
// (one level up, i.e. dist/..) by build-native.js, which writes
// type_bridge_node.<triple>.node beside package.json. The candidates list
// probes the package root first (primary), then dist/ itself as a robustness
// fallback for atypical build layouts.

let _cached: NativeModule | null = null;

/**
 * Returns the platform triple used in the built .node filename, or null when
 * the current platform has no recognised triple.
 */
function platformTriple(): string | null {
  const arch = process.arch;
  switch (process.platform) {
    case "darwin":
      return arch === "arm64" ? "darwin-arm64" : "darwin-x64";
    case "linux":
      if (arch === "arm64") {
        return "linux-arm64-gnu";
      }
      return arch === "x64" ? "linux-x64-gnu" : null;
    case "win32":
      return arch === "arm64" ? "win32-arm64-msvc" : "win32-x64-msvc";
    default:
      return null;
  }
}

/**
 * Returns the ordered list of absolute candidate paths to probe for the .node
 * artifact. Platform-triple-specific names are tried first; generic fallbacks
 * follow. Both the package root (dist/..) and dist/ itself are probed so that
 * the loader works whether the artifact sits beside package.json or beside the
 * compiled output.
 */
function nativeCandidates(): string[] {
  const triple = platformTriple();
  const names: string[] = [];

  if (triple) {
    names.push(
      `type_bridge_node.${triple}.node`,
      `type-bridge-node.${triple}.node`,
    );
  }

  names.push(
    "type_bridge_node.node",
    "type-bridge-node.node",
    "index.node",
  );

  // Primary: package root (dist/..) — where build-native.js places the artifact.
  // Secondary: dist/ itself — robustness fallback for atypical build layouts.
  const packageRoot = path.join(__dirname, "..");
  const candidates: string[] = [];
  for (const name of names) {
    candidates.push(path.join(packageRoot, name));
  }
  for (const name of names) {
    candidates.push(path.join(__dirname, name));
  }
  return candidates;
}

/**
 * Loads and returns the native .node module. The result is cached after the
 * first successful load; subsequent calls return the same object.
 *
 * Resolution order:
 *   1. TYPE_BRIDGE_NODE_NATIVE_PATH env var (explicit override).
 *   2. Platform-triple candidates at the package root (dist/..).
 *   3. Generic-name candidates at the package root.
 *   4. Same set probed inside dist/ as a robustness fallback.
 *
 * Throws an actionable error listing all tried paths when no candidate exists.
 */
export function loadNative(): NativeModule {
  if (_cached !== null) {
    return _cached;
  }

  const explicitPath = process.env["TYPE_BRIDGE_NODE_NATIVE_PATH"];
  const candidates: string[] = explicitPath ? [explicitPath] : [];
  candidates.push(...nativeCandidates());

  const tried: string[] = [];
  for (const candidate of candidates) {
    tried.push(candidate);
    if (fs.existsSync(candidate)) {
      // eslint-disable-next-line @typescript-eslint/no-require-imports
      _cached = require(candidate) as NativeModule;
      return _cached;
    }
  }

  throw new Error(
    [
      "Unable to load the type-bridge native Node module.",
      "Run `npm run build:native`, or set TYPE_BRIDGE_NODE_NATIVE_PATH to the built .node artifact.",
      `Tried: ${tried.join(", ")}`,
    ].join(" "),
  );
}
