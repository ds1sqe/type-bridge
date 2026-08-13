import assert from "node:assert/strict";
import crypto from "node:crypto";
import { cpSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";


const STAGE = dirname(fileURLToPath(import.meta.url));
const SOURCE = resolve(STAGE, "generated_v2");
const ORDERED = resolve(STAGE, "generated_ordered");
const FOREIGN = resolve(STAGE, "generated_foreign");
const MAX_SCHEMA_AUTHORITY_BYTES = 16 * 1024 * 1024;
const AUTHORITY_PREFIX = "export const SCHEMA_AUTHORITY_JSON = ";
const PROJECTION_PREFIX = "export const RUNTIME_PROJECTION_JSON = ";
const SEMANTIC_FINGERPRINT_PREFIX =
  "export const SEMANTIC_SCHEMA_FINGERPRINT_JSON = ";
const PROJECTION_FINGERPRINT_PREFIX =
  "export const PROJECTION_FINGERPRINT_JSON = ";

function sorted(value) {
  if (Array.isArray(value)) {
    return value.map(sorted);
  }
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, child]) => [key, sorted(child)]),
    );
  }
  return value;
}

function canonical(value) {
  return JSON.stringify(sorted(value));
}

function exported(packageDirectory, relativePath, prefix) {
  const line = readFileSync(resolve(packageDirectory, relativePath), "utf8")
    .split("\n")
    .find((candidate) => candidate.startsWith(prefix));
  assert(line);
  return JSON.parse(line.slice(prefix.length, -1));
}

function replaceExport(packageDirectory, relativePath, prefix, value) {
  const path = resolve(packageDirectory, relativePath);
  const lines = readFileSync(path, "utf8").split("\n");
  const index = lines.findIndex((candidate) => candidate.startsWith(prefix));
  assert.notEqual(index, -1);
  lines[index] = `${prefix}${JSON.stringify(value)};`;
  writeFileSync(path, lines.join("\n"));
}

function envelope(packageDirectory) {
  return exported(packageDirectory, "dist/authority.js", AUTHORITY_PREFIX);
}

function projection(packageDirectory) {
  return JSON.parse(
    exported(packageDirectory, "dist/schema.js", PROJECTION_PREFIX),
  );
}

function replaceProjection(packageDirectory, value) {
  replaceExport(
    packageDirectory,
    "dist/schema.js",
    PROJECTION_PREFIX,
    typeof value === "string" ? value : canonical(value),
  );
}

function field(digest, value) {
  const encoded = Buffer.from(value);
  const length = Buffer.alloc(8);
  length.writeBigUInt64BE(BigInt(encoded.length));
  digest.update(length);
  digest.update(encoded);
}

function fingerprint(content) {
  const digest = crypto.createHash("sha256");
  digest.update(Buffer.from("typebridge.fingerprint/v1\0"));
  field(digest, "typebridge.schema.authority");
  field(digest, "typebridge.schema-authority/v1");
  digest.update(Buffer.from([0]));
  field(digest, canonical(content));
  return digest.digest("hex");
}

function resign(value) {
  value.authority_fingerprint.digest = fingerprint(value.content);
}

function mutated(change, resignAfter, packageDirectory = SOURCE) {
  const value = JSON.parse(envelope(packageDirectory));
  change(value);
  if (resignAfter) {
    resign(value);
  }
  return canonical(value);
}

function rejectLegacy(name, authority, expected) {
  const packageName = `generated_rejected_${name}`;
  const packageDirectory = resolve(STAGE, packageName);
  rmSync(packageDirectory, { recursive: true, force: true });
  cpSync(SOURCE, packageDirectory, { recursive: true });
  writeFileSync(
    resolve(packageDirectory, "dist/authority.js"),
    `// Tampered generated-authority acceptance fixture.\n\n${AUTHORITY_PREFIX}${JSON.stringify(authority)};\n`,
  );
  const completed = spawnSync(
    process.execPath,
    ["--input-type=module", "-e", `import('./${packageName}/dist/index.js')`],
    { cwd: STAGE, encoding: "utf8" },
  );
  const output = `${completed.stdout}${completed.stderr}`;
  assert.notEqual(completed.status, 0, `${name} authority unexpectedly installed`);
  assert.match(output, new RegExp(expected), `${name} authority emitted:\n${output}`);
}

const PROJECTION_EVIDENCE_MISMATCH = {
  category: "integrity",
  sdkCategory: "integrity",
  queryCategory: null,
  code: "projection_evidence_mismatch",
  message: "Generated projection evidence does not match the verified schema package",
  path: [{ kind: "argument", value: "projection_evidence" }],
  details: {},
};

function rejectSuccessor(name, tamper) {
  const packageName = `generated_projection_rejected_${name}`;
  const packageDirectory = resolve(STAGE, packageName);
  rmSync(packageDirectory, { recursive: true, force: true });
  cpSync(ORDERED, packageDirectory, { recursive: true });
  tamper(packageDirectory);
  const completed = spawnSync(
    process.execPath,
    [
      "--input-type=module",
      "-e",
      `import('./${packageName}/dist/index.js').then(() => process.exit(0), (error) => { process.stdout.write(error instanceof Error ? error.message : String(error)); process.exit(1); })`,
    ],
    { cwd: STAGE, encoding: "utf8" },
  );
  assert.notEqual(completed.status, 0, `${name} evidence unexpectedly installed`);
  assert.equal(completed.stderr, "", `${name} emitted stderr:\n${completed.stderr}`);
  assert.deepEqual(
    JSON.parse(completed.stdout),
    PROJECTION_EVIDENCE_MISMATCH,
    `${name} evidence emitted:\n${completed.stdout}`,
  );
}

rejectLegacy("malformed", "{", "malformed_canonical_json");
rejectLegacy(
  "foreign",
  envelope(FOREIGN),
  "generated_schema_authority_semantic_mismatch",
);
rejectLegacy(
  "stale",
  mutated((value) => {
    value.content.declared_identity.digest = "0".repeat(64);
  }, false),
  "generated_schema_authority_integrity_mismatch",
);
rejectLegacy(
  "missing_fingerprint",
  mutated((value) => {
    delete value.authority_fingerprint;
  }, false),
  "invalid_canonical_value",
);
rejectLegacy(
  "managed_state",
  mutated((value) => {
    value.content.managed_state.managed_semantic_schema.digest = "0".repeat(64);
  }, true),
  "generated_schema_authority_integrity_mismatch",
);
rejectLegacy(
  "capability",
  mutated((value) => {
    value.content.required_capabilities.push("query.future-feature");
    value.content.required_capabilities.sort();
  }, true),
  "unsupported_required_capability",
);
rejectLegacy(
  "version",
  mutated((value) => {
    value.content.authority_version = "typebridge.schema-authority/v2";
  }, true),
  "generated_schema_authority_unsupported_version",
);
rejectLegacy(
  "oversize",
  " ".repeat(MAX_SCHEMA_AUTHORITY_BYTES + 1),
  "canonical_json_too_large",
);

rejectSuccessor("missing_authority", (directory) => {
  const path = resolve(directory, "dist/index.js");
  const source = readFileSync(path, "utf8");
  const changed = source.replace(
    ", SCHEMA_AUTHORITY_JSON);",
    ", undefined);",
  );
  assert.notEqual(changed, source);
  writeFileSync(path, changed);
});
rejectSuccessor("malformed_authority", (directory) => {
  replaceExport(directory, "dist/authority.js", AUTHORITY_PREFIX, "{");
});
rejectSuccessor("foreign_authority", (directory) => {
  replaceExport(
    directory,
    "dist/authority.js",
    AUTHORITY_PREFIX,
    envelope(FOREIGN),
  );
});
rejectSuccessor("stale_authority", (directory) => {
  replaceExport(
    directory,
    "dist/authority.js",
    AUTHORITY_PREFIX,
    mutated((value) => {
      value.content.declared_identity.digest = "0".repeat(64);
    }, false, ORDERED),
  );
});
rejectSuccessor("unsupported_authority_version", (directory) => {
  replaceExport(
    directory,
    "dist/authority.js",
    AUTHORITY_PREFIX,
    mutated((value) => {
      value.content.authority_version = "typebridge.schema-authority/v2";
    }, true, ORDERED),
  );
});
rejectSuccessor("malformed_projection", (directory) => {
  replaceProjection(directory, "{");
});
rejectSuccessor("missing_resource", (directory) => {
  const value = projection(directory);
  value.code_resources.pop();
  replaceProjection(directory, value);
});
rejectSuccessor("extra_resource", (directory) => {
  const value = projection(directory);
  const extra = structuredClone(value.code_resources.at(-1));
  extra.id = "typebridge.generator.typescript.zzz-extra-resource";
  value.code_resources.push(extra);
  replaceProjection(directory, value);
});
rejectSuccessor("duplicate_resource", (directory) => {
  const value = projection(directory);
  value.code_resources.push(structuredClone(value.code_resources[0]));
  replaceProjection(directory, value);
});
rejectSuccessor("reordered_resource", (directory) => {
  const value = projection(directory);
  value.code_resources.reverse();
  replaceProjection(directory, value);
});
rejectSuccessor("forged_resource", (directory) => {
  const value = projection(directory);
  value.code_resources[0].content_fingerprint.digest = "0".repeat(64);
  replaceProjection(directory, value);
});
rejectSuccessor("stale_handler", (directory) => {
  const value = projection(directory);
  value.generator_handlers[0].version = 1;
  replaceProjection(directory, value);
});
rejectSuccessor("foreign_target", (directory) => {
  const value = projection(directory);
  value.target = "python";
  replaceProjection(directory, value);
});
rejectSuccessor("malformed_semantic_fingerprint", (directory) => {
  replaceExport(
    directory,
    "dist/schema.js",
    SEMANTIC_FINGERPRINT_PREFIX,
    "{}",
  );
});
rejectSuccessor("malformed_projection_fingerprint", (directory) => {
  replaceExport(
    directory,
    "dist/schema.js",
    PROJECTION_FINGERPRINT_PREFIX,
    "{}",
  );
});

console.log("generated TypeScript authority and projection rejection acceptance passed");
