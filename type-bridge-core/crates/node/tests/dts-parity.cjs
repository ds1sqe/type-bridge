"use strict";

/**
 * .d.ts export parity gate for the generated-only Node package boundary.
 *
 * DECLARATION-ONLY BOUNDARY
 * -------------------------
 * This gate asserts ONLY the content of the emitted TypeScript declaration files
 * (`dist/*.d.ts`, produced by `tsc -p tsconfig.json`, the `build:types` step). It
 * deliberately asserts nothing about `package.json` exports, JavaScript files,
 * the native loader, or `*.node` artifacts. The package smoke gate owns those
 * surfaces. If a declaration changes intentionally, regenerate the baseline or
 * record its exact content hash in the allowed-diffs file.
 *
 * HOW IT WORKS
 * ------------
 * 1. Emit fresh declarations hermetically into a temp dir (never clobbers the
 *    committed baseline; does not depend on a prior `dist/` build).
 * 2. Compare each emitted `*.d.ts` against the committed baseline under
 *    `tests/dts-baseline/`.
 * 3. A file whose emitted content differs from the baseline is a violation,
 *    UNLESS an allowed-diffs entry records the emitted content's sha256 with a
 *    rationale. Unexplained drift fails; recorded-with-rationale drift passes.
 *
 * The allowed-diffs entry is keyed by the emitted file's sha256, so an allowance
 * covers exactly one intended content — it cannot blanket-permit a file forever.
 *
 * USAGE
 *   node tests/dts-parity.cjs            # gate: exit 1 on unexplained drift
 *   node tests/dts-parity.cjs --update   # regenerate the committed baseline
 *
 * The `--update` path is also exposed as `npm run dts:baseline`; run it when the
 * public type surface intentionally changes, then record a rationale in
 * dts-allowed-diffs.json for the change (or rely on the regenerated baseline).
 */

const { spawnSync } = require("child_process");
const crypto = require("crypto");
const fs = require("fs");
const os = require("os");
const path = require("path");

const CRATE_ROOT = path.resolve(__dirname, "..");
const BASELINE_DIR = path.join(__dirname, "dts-baseline");
const ALLOWED_DIFFS = path.join(__dirname, "dts-allowed-diffs.json");
const TSCONFIG = path.join(CRATE_ROOT, "tsconfig.json");
const TSC_BIN = path.join(CRATE_ROOT, "node_modules", ".bin", "tsc");

function sha256(text) {
  return crypto.createHash("sha256").update(text, "utf8").digest("hex");
}

function declarationFiles(dir) {
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir).filter((name) => name.endsWith(".d.ts")).sort();
}

function readFiles(dir) {
  const out = new Map();
  for (const name of declarationFiles(dir)) {
    out.set(name, fs.readFileSync(path.join(dir, name), "utf8"));
  }
  return out;
}

function loadAllowedDiffs() {
  if (!fs.existsSync(ALLOWED_DIFFS)) return [];
  const parsed = JSON.parse(fs.readFileSync(ALLOWED_DIFFS, "utf8"));
  return Array.isArray(parsed.allowedDiffs) ? parsed.allowedDiffs : [];
}

/**
 * Compare emitted declarations against the baseline, honoring allowed-diffs.
 *
 * Pure function over already-read file maps, so the gate logic is unit-testable
 * without invoking tsc. Returns { ok, violations, allowed } where a violation is
 * { file, reason, sha } and an allowed entry is { file, rationale }.
 */
function compareDeclarations(emit, baseline, allowedDiffs) {
  const allowedByFile = new Map();
  for (const entry of allowedDiffs) {
    if (!allowedByFile.has(entry.file)) allowedByFile.set(entry.file, []);
    allowedByFile.get(entry.file).push(entry);
  }

  const violations = [];
  const allowed = [];
  const names = new Set([...emit.keys(), ...baseline.keys()]);

  for (const name of [...names].sort()) {
    const emitContent = emit.get(name) ?? null;
    const baseContent = baseline.get(name) ?? null;

    if (emitContent === baseContent) continue;

    if (emitContent === null) {
      violations.push({ file: name, reason: "removed (present in baseline, not emitted)", sha: null });
      continue;
    }
    const emitSha = sha256(emitContent);
    const match = (allowedByFile.get(name) ?? []).find((entry) => entry.sha256 === emitSha);
    if (match) {
      allowed.push({ file: name, rationale: match.rationale });
      continue;
    }
    const reason = baseContent === null ? "added (not in baseline)" : "content drift vs baseline";
    violations.push({ file: name, reason, sha: emitSha });
  }

  return { ok: violations.length === 0, violations, allowed };
}

function emitDeclarations() {
  const outDir = fs.mkdtempSync(path.join(os.tmpdir(), "dts-parity-"));
  const result = spawnSync(
    TSC_BIN,
    ["-p", TSCONFIG, "--outDir", outDir],
    { cwd: CRATE_ROOT, stdio: "inherit" },
  );
  if (result.status !== 0) {
    throw new Error("dts-parity: `tsc` declaration emit failed");
  }
  return outDir;
}

function printDiff(name, emitDir) {
  const basePath = path.join(BASELINE_DIR, name);
  const emitPath = path.join(emitDir, name);
  if (fs.existsSync(basePath) && fs.existsSync(emitPath)) {
    const diff = spawnSync("diff", ["-u", basePath, emitPath], { encoding: "utf8" });
    if (diff.stdout) process.stderr.write(diff.stdout);
  }
}

function main() {
  const update = process.argv.includes("--update");
  const emitDir = emitDeclarations();

  if (update) {
    fs.mkdirSync(BASELINE_DIR, { recursive: true });
    for (const name of declarationFiles(BASELINE_DIR)) {
      if (!fs.existsSync(path.join(emitDir, name))) fs.rmSync(path.join(BASELINE_DIR, name));
    }
    for (const name of declarationFiles(emitDir)) {
      fs.copyFileSync(path.join(emitDir, name), path.join(BASELINE_DIR, name));
    }
    console.log(`dts-parity: baseline regenerated (${declarationFiles(emitDir).length} files).`);
    console.log("Record a rationale in tests/dts-allowed-diffs.json for any intentional change.");
    return;
  }

  const result = compareDeclarations(readFiles(emitDir), readFiles(BASELINE_DIR), loadAllowedDiffs());

  for (const entry of result.allowed) {
    console.log(`dts-parity: allowed drift in ${entry.file} — ${entry.rationale}`);
  }

  if (!result.ok) {
    process.stderr.write("\ndts-parity: unexplained .d.ts declaration drift\n\n");
    for (const violation of result.violations) {
      process.stderr.write(`  ${violation.file}: ${violation.reason}\n`);
      printDiff(violation.file, emitDir);
      if (violation.sha) {
        process.stderr.write(
          `  → if intentional: run \`npm run dts:baseline\` to regenerate the baseline, ` +
            `or add an allowed-diffs entry {"file":"${violation.file}","sha256":"${violation.sha}","rationale":"..."}\n\n`,
        );
      }
    }
    process.exitCode = 1;
    return;
  }

  console.log(`dts-parity: ${readFiles(emitDir).size} declaration file(s) match the baseline.`);
}

module.exports = { compareDeclarations, sha256 };

if (require.main === module) {
  main();
}
