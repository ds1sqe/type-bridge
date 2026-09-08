import assert from "node:assert/strict";
import crypto from "node:crypto";
import { cpSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";


const STAGE = dirname(fileURLToPath(import.meta.url));
const SOURCE = resolve(STAGE, "generated_v2");
const FOREIGN = resolve(STAGE, "generated_foreign");
const MAX_SCHEMA_AUTHORITY_BYTES = 16 * 1024 * 1024;
const PREFIX = "export const SCHEMA_AUTHORITY_JSON = ";

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

function envelope(packageDirectory) {
  const line = readFileSync(resolve(packageDirectory, "dist/authority.js"), "utf8")
    .split("\n")
    .find((candidate) => candidate.startsWith(PREFIX));
  assert(line);
  return JSON.parse(line.slice(PREFIX.length, -1));
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

function mutated(change, resignAfter) {
  const value = JSON.parse(envelope(SOURCE));
  change(value);
  if (resignAfter) {
    resign(value);
  }
  return canonical(value);
}

function reject(name, authority, expected) {
  const packageName = `generated_rejected_${name}`;
  const packageDirectory = resolve(STAGE, packageName);
  rmSync(packageDirectory, { recursive: true, force: true });
  cpSync(SOURCE, packageDirectory, { recursive: true });
  writeFileSync(
    resolve(packageDirectory, "dist/authority.js"),
    `// Tampered generated-authority acceptance fixture.\n\n${PREFIX}${JSON.stringify(authority)};\n`,
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

reject("malformed", "{", "malformed_canonical_json");
reject(
  "foreign",
  envelope(FOREIGN),
  "generated_schema_authority_semantic_mismatch",
);
reject(
  "stale",
  mutated((value) => {
    value.content.declared_identity.digest = "0".repeat(64);
  }, false),
  "generated_schema_authority_integrity_mismatch",
);
reject(
  "missing_fingerprint",
  mutated((value) => {
    delete value.authority_fingerprint;
  }, false),
  "invalid_canonical_value",
);
reject(
  "managed_state",
  mutated((value) => {
    value.content.managed_state.managed_semantic_schema.digest = "0".repeat(64);
  }, true),
  "generated_schema_authority_integrity_mismatch",
);
reject(
  "capability",
  mutated((value) => {
    value.content.required_capabilities.push("query.future-feature");
    value.content.required_capabilities.sort();
  }, true),
  "unsupported_required_capability",
);
reject(
  "version",
  mutated((value) => {
    value.content.authority_version = "typebridge.schema-authority/v2";
  }, true),
  "generated_schema_authority_unsupported_version",
);
reject(
  "oversize",
  " ".repeat(MAX_SCHEMA_AUTHORITY_BYTES + 1),
  "canonical_json_too_large",
);

console.log("generated TypeScript authority rejection acceptance passed");
