#!/usr/bin/env node
/** Produce Node's exact-TypeDB-3.12.1 Phase-2 live-subset report. */

import crypto from "node:crypto";
import {
  closeSync,
  constants,
  fstatSync,
  fsyncSync,
  linkSync,
  lstatSync,
  openSync,
  readSync,
  rmSync,
  writeSync,
} from "node:fs";
import { createRequire } from "node:module";
import { basename, dirname, isAbsolute, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const REPORT_FORMAT = "typebridge.phase2-projected-live-report/v1";
export const SEMANTIC_PROFILE = "typedb-3.12.1/v1";
export const SEMANTIC_SCHEMA_FINGERPRINT = Object.freeze({
  algorithm: "sha256",
  canonicalization: "typebridge.schema-canonical-json/v1",
  digest: "3c8d072b60c575b4c0381c1ea44088a9c18e01ed52e90730311aadccb72cd0c8",
  domain: "typebridge.schema.semantic",
  semantic_profile: SEMANTIC_PROFILE,
});
export const PROJECTION_FINGERPRINT = Object.freeze({
  algorithm: "sha256",
  canonicalization: "typebridge.binding-projection/v1",
  digest: "d2aa868a17f1c8f2ac8f276421b78966f9dbfced608f6565f2647d3a8e584b59",
  domain: "typebridge.binding.projection",
  semantic_profile: SEMANTIC_PROFILE,
});

export const OUTPUT_ENV = "TYPE_BRIDGE_PHASE2_LIVE_REPORT";
export const ADDRESS_ENV = "TYPE_BRIDGE_PHASE2_LIVE_ADDRESS";
export const DATABASE_ENV = "TYPE_BRIDGE_PHASE2_LIVE_DATABASE";
export const HTTP_PORT_ENV = "TYPE_BRIDGE_PHASE2_LIVE_HTTP_PORT";
export const PACKAGE_ROOT_ENV = "TYPE_BRIDGE_PHASE2_NODE_PACKAGE_ROOT";
export const REPOSITORY_ROOT_ENV = "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT";
export const MAX_REPORT_BYTES = 256 * 1024;
export const MAX_AUTHORITY_BYTES = 1024 * 1024;

export const SOURCE_PATHS = Object.freeze({
  schema: "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml",
  journey: "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json",
  provider:
    "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql",
});
export const SOURCE_SHA256 = Object.freeze({
  schema: "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
  journey: "912130753fe7938a38c054cff16e202b312551a6aa65265148628bfbd2abbbef",
  provider: "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
});
export const OBSERVATION_REFS = Object.freeze([
  "canonical_scalar_values",
  "cleanup",
  "inherited_plain_activity_role_lifecycle",
  "integer_key_polymorphic_optional_role",
  "relation_as_player",
]);

const LIVE_REFS = new Set([
  "container-event",
  "data-ada",
  "data-dana",
  "event-ada",
  "interaction-absent",
  "interaction-person",
  "interaction-robot",
  "plain-activity-ada",
  "robot-7",
  "robot-negative-7",
]);
const SCALAR_FIELDS = Object.freeze({
  boolean: "valBool",
  date: "valDate",
  datetime: "valDatetime",
  datetime_tz: "valDatetimeTz",
  decimal: "valDecimal",
  double: "valDouble",
  duration: "valDuration",
  long: "score",
  string: "nickname",
});
const EXPECTED_MODEL_TYPES = Object.freeze({
  Container: '{"kind":"relation","label":"container"}',
  Event: '{"kind":"relation","label":"event"}',
  Interaction: '{"kind":"relation","label":"interaction"}',
  Person: '{"kind":"entity","label":"person"}',
  PlainActivity: '{"kind":"relation","label":"plain-activity"}',
  Robot: '{"kind":"entity","label":"robot"}',
});

export class ProducerError extends Error {
  constructor(code, message, options = undefined) {
    super(`${code}: ${message}`, options);
    this.name = "ProducerError";
    this.code = code;
  }
}

function requireCondition(condition, code, message) {
  if (!condition) throw new ProducerError(code, message);
}

function canonicalValue(value, ancestors = new Set()) {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new ProducerError(
        "invalid_report_value",
        "report contains a non-finite number",
      );
    }
    return value;
  }
  if (typeof value !== "object") {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a non-JSON value",
    );
  }
  if (ancestors.has(value)) {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a recursive value",
    );
  }
  const nextAncestors = new Set(ancestors);
  nextAncestors.add(value);
  if (Array.isArray(value)) {
    return value.map((member) => canonicalValue(member, nextAncestors));
  }
  if (Object.getPrototypeOf(value) !== Object.prototype) {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a non-plain object",
    );
  }
  return Object.fromEntries(
    Object.entries(value)
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
      .map(([key, member]) => [key, canonicalValue(member, nextAncestors)]),
  );
}

function canonicalString(value) {
  let encoded;
  try {
    encoded = JSON.stringify(canonicalValue(value));
  } catch (error) {
    if (error instanceof ProducerError) throw error;
    throw new ProducerError(
      "invalid_report_value",
      "report cannot be canonicalized",
      { cause: error },
    );
  }
  if (encoded === undefined) {
    throw new ProducerError(
      "invalid_report_value",
      "report cannot be canonicalized",
    );
  }
  return encoded;
}

export function canonicalBytes(value) {
  return Buffer.from(`${canonicalString(value)}\n`);
}

function boundedRegularBytes(path, label) {
  let metadata;
  try {
    metadata = lstatSync(path);
  } catch (error) {
    throw new ProducerError(
      "invalid_authority",
      `${label} cannot be inspected`,
      { cause: error },
    );
  }
  if (metadata.isSymbolicLink() || !metadata.isFile()) {
    throw new ProducerError(
      "invalid_authority",
      `${label} must be a regular non-symlink file`,
    );
  }
  if (metadata.size > MAX_AUTHORITY_BYTES) {
    throw new ProducerError("authority_size_limit", `${label} is too large`);
  }
  let descriptor;
  try {
    descriptor = openSync(path, constants.O_RDONLY);
    const opened = fstatSync(descriptor);
    if (!opened.isFile() || opened.size > MAX_AUTHORITY_BYTES) {
      throw new ProducerError(
        opened.size > MAX_AUTHORITY_BYTES
          ? "authority_size_limit"
          : "invalid_authority",
        opened.size > MAX_AUTHORITY_BYTES
          ? `${label} is too large`
          : `${label} must be a regular file`,
      );
    }
    const bytes = Buffer.alloc(opened.size);
    let offset = 0;
    while (offset < bytes.length) {
      const count = readSync(
        descriptor,
        bytes,
        offset,
        bytes.length - offset,
        offset,
      );
      if (count === 0) {
        throw new ProducerError(
          "invalid_authority",
          `${label} was truncated while reading`,
        );
      }
      offset += count;
    }
    return bytes;
  } catch (error) {
    if (error instanceof ProducerError) throw error;
    throw new ProducerError(
      "invalid_authority",
      `${label} cannot be read`,
      { cause: error },
    );
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
  }
}

export function sourceAuthority(root) {
  const authority = {};
  const sources = {};
  for (const label of ["schema", "journey", "provider"]) {
    const relative = SOURCE_PATHS[label];
    const bytes = boundedRegularBytes(
      resolve(root, relative),
      `Phase-2 ${label}`,
    );
    const digest = crypto.createHash("sha256").update(bytes).digest("hex");
    if (digest !== SOURCE_SHA256[label]) {
      throw new ProducerError(
        "authority_hash_mismatch",
        `Phase-2 ${label} is not the frozen exact-3.12.1 authority`,
      );
    }
    authority[label] = { path: relative, sha256: digest };
    sources[label] = bytes;
  }
  return { authority, sources };
}

function requiredEnvironment(name, environment) {
  const value = environment[name];
  if (typeof value !== "string" || value.length === 0) {
    throw new ProducerError(
      "missing_environment",
      `${name} must be set and non-empty`,
    );
  }
  const encoded = Buffer.from(value, "utf8");
  if (
    encoded.toString("utf8") !== value ||
    encoded.length > 4096 ||
    [...value].some((character) => character.codePointAt(0) < 32)
  ) {
    throw new ProducerError(
      "invalid_environment",
      `${name} is not a bounded UTF-8 text value`,
    );
  }
  return value;
}

function realDirectory(value, label) {
  if (!isAbsolute(value)) {
    throw new ProducerError("invalid_directory", `${label} must be absolute`);
  }
  let metadata;
  try {
    metadata = lstatSync(value);
  } catch (error) {
    throw new ProducerError(
      "invalid_directory",
      `${label} cannot be inspected`,
      { cause: error },
    );
  }
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    throw new ProducerError(
      "invalid_directory",
      `${label} must be a real directory`,
    );
  }
  return resolve(value);
}

export function httpPort(environment = process.env) {
  const value = requiredEnvironment(HTTP_PORT_ENV, environment);
  if (!/^[0-9]+$/.test(value)) {
    throw new ProducerError(
      "invalid_http_port",
      `${HTTP_PORT_ENV} must be an ASCII integer`,
    );
  }
  const port = Number(value);
  if (!Number.isSafeInteger(port) || port < 1 || port > 65_535) {
    throw new ProducerError(
      "invalid_http_port",
      `${HTTP_PORT_ENV} must be in 1..65535`,
    );
  }
  return port;
}

function validateOutputTarget(path) {
  let parent;
  try {
    parent = lstatSync(dirname(path));
  } catch (error) {
    throw new ProducerError(
      "invalid_output_parent",
      "report parent cannot be inspected",
      { cause: error },
    );
  }
  if (parent.isSymbolicLink() || !parent.isDirectory()) {
    throw new ProducerError(
      "invalid_output_parent",
      "report parent must be a real directory",
    );
  }
  try {
    lstatSync(path);
  } catch (error) {
    if (error?.code === "ENOENT") return;
    throw new ProducerError(
      "invalid_output_path",
      "report path cannot be inspected",
      { cause: error },
    );
  }
  throw new ProducerError(
    "output_exists",
    "report destination already exists",
  );
}

export function outputPath(environment = process.env) {
  const value = requiredEnvironment(OUTPUT_ENV, environment);
  if (!isAbsolute(value) || ["", ".", ".."].includes(basename(value))) {
    throw new ProducerError(
      "invalid_output_path",
      "report path must be an absolute file path",
    );
  }
  validateOutputTarget(value);
  return value;
}

export function publishReport(path, report) {
  const payload = canonicalBytes(report);
  if (payload.length > MAX_REPORT_BYTES) {
    throw new ProducerError(
      "report_size_limit",
      "canonical report is too large",
    );
  }
  validateOutputTarget(path);
  const temporary = resolve(
    dirname(path),
    `.${basename(path)}.${process.pid}.${crypto.randomUUID()}.tmp`,
  );
  let descriptor;
  let published = false;
  try {
    descriptor = openSync(
      temporary,
      constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL,
      0o600,
    );
    let offset = 0;
    while (offset < payload.length) {
      const count = writeSync(
        descriptor,
        payload,
        offset,
        payload.length - offset,
        offset,
      );
      if (count === 0) {
        throw new ProducerError(
          "output_write_failed",
          "report could not be written completely",
        );
      }
      offset += count;
    }
    fsyncSync(descriptor);
    closeSync(descriptor);
    descriptor = undefined;
    linkSync(temporary, path);
    published = true;
  } catch (error) {
    if (error?.code === "EEXIST") {
      throw new ProducerError(
        "output_exists",
        "report destination already exists",
        { cause: error },
      );
    }
    if (error instanceof ProducerError) throw error;
    throw new ProducerError(
      "output_write_failed",
      "report could not be written",
      { cause: error },
    );
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
    try {
      rmSync(temporary, { force: true });
    } catch (error) {
      if (!published) {
        throw new ProducerError(
          "output_write_failed",
          "temporary report could not be removed",
          { cause: error },
        );
      }
    }
  }
}

function packageObject(value, label) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new ProducerError(
      "generated_package_identity",
      `${label} is malformed`,
    );
  }
  return value;
}

export function validateGeneratedPackage(package_) {
  let semantic;
  let runtimeProjection;
  try {
    semantic = JSON.parse(package_.SEMANTIC_SCHEMA_FINGERPRINT_JSON);
    runtimeProjection = JSON.parse(package_.RUNTIME_PROJECTION_JSON);
  } catch (error) {
    throw new ProducerError(
      "generated_package_identity",
      "generated package has no parseable projection identity",
      { cause: error },
    );
  }
  packageObject(semantic, "generated semantic fingerprint");
  packageObject(runtimeProjection, "generated runtime projection");
  let identityMatches = false;
  try {
    identityMatches =
      canonicalString(semantic) ===
        canonicalString(SEMANTIC_SCHEMA_FINGERPRINT) &&
      canonicalString(runtimeProjection.semantic_fingerprint) ===
        canonicalString(SEMANTIC_SCHEMA_FINGERPRINT) &&
      canonicalString(JSON.parse(package_.PROJECTION_FINGERPRINT_JSON)) ===
        canonicalString(PROJECTION_FINGERPRINT) &&
      canonicalString(runtimeProjection.projection_fingerprint) ===
        canonicalString(PROJECTION_FINGERPRINT) &&
      runtimeProjection.target === "typescript";
  } catch {
    identityMatches = false;
  }
  if (!identityMatches) {
    throw new ProducerError(
      "generated_package_identity",
      "generated package is not the frozen Workforce V3 schema projection",
    );
  }
  for (const [name, typeKey] of Object.entries(EXPECTED_MODEL_TYPES)) {
    const model = package_[name];
    if (
      model === null ||
      typeof model !== "object" ||
      model.typeKey !== typeKey ||
      typeof model.create !== "function" ||
      typeof model.manager !== "function"
    ) {
      throw new ProducerError(
        "generated_package_identity",
        `generated ${name} token does not match Workforce V3`,
      );
    }
  }
}

function regularPackagePath(path, label, directory) {
  let metadata;
  try {
    metadata = lstatSync(path);
  } catch (error) {
    throw new ProducerError(
      "generated_package_import",
      `${label} is absent`,
      { cause: error },
    );
  }
  const correctKind = directory ? metadata.isDirectory() : metadata.isFile();
  if (metadata.isSymbolicLink() || !correctKind) {
    throw new ProducerError(
      "generated_package_import",
      `${label} must be a real filesystem object`,
    );
  }
}

export async function loadGeneratedPackage(packageRoot) {
  const generatedRoot = resolve(packageRoot, "generated_phase2");
  const manifest = resolve(generatedRoot, "package.json");
  const entry = resolve(generatedRoot, "dist/index.js");
  regularPackagePath(generatedRoot, "generated package", true);
  regularPackagePath(manifest, "generated package manifest", false);
  regularPackagePath(entry, "generated package entry", false);
  let package_;
  let runtime;
  try {
    package_ = await import(pathToFileURL(entry).href);
    const require = createRequire(manifest);
    runtime = require("@type-bridge/node");
  } catch (error) {
    throw new ProducerError(
      "generated_package_import",
      "generated Node package or its public runtime cannot be imported",
      { cause: error },
    );
  }
  validateGeneratedPackage(package_);
  if (
    runtime === null ||
    typeof runtime !== "object" ||
    typeof runtime.RustDatabase?.connect !== "function"
  ) {
    throw new ProducerError(
      "generated_package_import",
      "@type-bridge/node has no public RustDatabase connection facade",
    );
  }
  return { package_, runtime };
}

function versionEndpoint(address, port) {
  let parsed;
  try {
    parsed = new URL(address.includes("://") ? address : `http://${address}`);
  } catch (error) {
    throw new ProducerError(
      "invalid_address",
      `${ADDRESS_ENV} cannot be used for the HTTP version probe`,
      { cause: error },
    );
  }
  if (
    parsed.hostname.length === 0 ||
    parsed.username.length !== 0 ||
    parsed.password.length !== 0 ||
    (parsed.pathname !== "" && parsed.pathname !== "/") ||
    parsed.search.length !== 0 ||
    parsed.hash.length !== 0
  ) {
    throw new ProducerError(
      "invalid_address",
      `${ADDRESS_ENV} is not a bare TypeDB host and port`,
    );
  }
  const host = parsed.hostname.includes(":")
    ? `[${parsed.hostname.replace(/^\[|\]$/g, "")}]`
    : parsed.hostname;
  return `http://${host}:${port}/v1/version`;
}

export async function detectServerVersion(
  address,
  port,
  fetchImplementation = fetch,
) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await fetchImplementation(versionEndpoint(address, port), {
      redirect: "error",
      signal: controller.signal,
    });
    if (response.status !== 200 || response.body === null) {
      throw new ProducerError(
        "server_version_probe_failed",
        "TypeDB HTTP version probe returned an invalid response",
      );
    }
    const reader = response.body.getReader();
    const chunks = [];
    let length = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > 4096) {
        await reader.cancel();
        throw new ProducerError(
          "server_version_probe_failed",
          "TypeDB HTTP version probe exceeded its response limit",
        );
      }
      chunks.push(Buffer.from(value));
    }
    if (length === 0) {
      throw new ProducerError(
        "server_version_probe_failed",
        "TypeDB HTTP version probe returned an empty response",
      );
    }
    let document;
    try {
      const text = new TextDecoder("utf-8", { fatal: true }).decode(
        Buffer.concat(chunks, length),
      );
      document = JSON.parse(text);
    } catch (error) {
      throw new ProducerError(
        "server_version_probe_failed",
        "TypeDB HTTP version probe returned invalid UTF-8 JSON",
        { cause: error },
      );
    }
    if (
      document === null ||
      typeof document !== "object" ||
      Array.isArray(document) ||
      typeof document.version !== "string" ||
      !/^\d+\.\d+\.\d+$/.test(document.version)
    ) {
      throw new ProducerError(
        "server_version_probe_failed",
        "TypeDB HTTP version probe returned an invalid version",
      );
    }
    return document.version;
  } finally {
    clearTimeout(timeout);
  }
}

function parseJourney(raw) {
  let journey;
  try {
    journey = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(raw));
  } catch (error) {
    throw new ProducerError(
      "invalid_journey",
      "workforce-v3 journey is not valid UTF-8 JSON",
      { cause: error },
    );
  }
  if (
    journey === null ||
    typeof journey !== "object" ||
    Array.isArray(journey) ||
    journey.format !== "typebridge.workforce-journey/v3" ||
    journey.fixture_id !== "workforce-v3" ||
    journey.version !== 3 ||
    journey.semantic_profile !== SEMANTIC_PROFILE
  ) {
    throw new ProducerError(
      "invalid_journey",
      "workforce-v3 journey identity drifted",
    );
  }
  const groups = journey.records;
  if (groups === null || typeof groups !== "object" || Array.isArray(groups)) {
    throw new ProducerError("invalid_journey", "workforce-v3 records are malformed");
  }
  const records = new Map();
  const index = (record, group) => {
    if (
      record === null ||
      typeof record !== "object" ||
      Array.isArray(record) ||
      typeof record.ref !== "string" ||
      record.ref.length === 0 ||
      typeof record.model !== "string" ||
      record.model.length === 0 ||
      records.has(record.ref)
    ) {
      throw new ProducerError(
        "invalid_journey",
        `journey record in ${group} is malformed or duplicated`,
      );
    }
    records.set(record.ref, record);
  };
  for (const group of [
    "people",
    "robots",
    "counters",
    "memberships",
    "network_links",
    "interactions",
  ]) {
    if (!Array.isArray(groups[group])) {
      throw new ProducerError(
        "invalid_journey",
        `journey group ${group} is malformed`,
      );
    }
    for (const record of groups[group]) index(record, group);
  }
  for (const group of ["plain_activity", "event", "container"]) {
    index(groups[group], group);
  }
  const createOrder = journey.create_order;
  const cleanupOrder = journey.cleanup_order;
  if (
    !Array.isArray(createOrder) ||
    !Array.isArray(cleanupOrder) ||
    [...createOrder, ...cleanupOrder].some((ref) => typeof ref !== "string") ||
    new Set(createOrder).size !== createOrder.length ||
    createOrder.length !== records.size ||
    createOrder.some((ref) => !records.has(ref)) ||
    canonicalString(cleanupOrder) !== canonicalString([...createOrder].reverse()) ||
    [...LIVE_REFS].some((ref) => !records.has(ref))
  ) {
    throw new ProducerError(
      "invalid_journey",
      "journey operation order is not exact",
    );
  }
  return { records, createOrder, cleanupOrder };
}

function record(records, ref, model) {
  const value = records.get(ref);
  if (value === undefined || value.model !== model) {
    throw new ProducerError(
      "invalid_journey",
      `journey record ${ref} is absent or changed`,
    );
  }
  return value;
}

function fields(recordValue, label) {
  const value = recordValue.fields;
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new ProducerError("invalid_journey", `${label} fields are malformed`);
  }
  return value;
}

function field(fieldValues, name, kind) {
  const value = fieldValues[name];
  const keys = kind === "double" ? ["bits", "kind"] : ["kind", "value"];
  if (
    value === null ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.keys(value).sort().join("\0") !== keys.join("\0") ||
    value.kind !== kind
  ) {
    throw new ProducerError(
      "invalid_journey",
      `field ${name} is not an exact ${kind} value`,
    );
  }
  return value;
}

function primitive(authored) {
  if (authored.kind === "double") {
    if (
      typeof authored.bits !== "string" ||
      !/^[0-9a-f]{16}$/.test(authored.bits)
    ) {
      throw new ProducerError("invalid_journey", "double bits are malformed");
    }
    return Buffer.from(authored.bits, "hex").readDoubleBE(0);
  }
  if (authored.kind === "long") {
    if (typeof authored.value !== "string" || !/^-?[0-9]+$/.test(authored.value)) {
      throw new ProducerError("invalid_journey", "long value is malformed");
    }
    return BigInt(authored.value);
  }
  if (authored.kind === "boolean" && typeof authored.value === "boolean") {
    return authored.value;
  }
  if (
    [
      "date",
      "datetime",
      "datetime_tz",
      "decimal",
      "duration",
      "string",
    ].includes(authored.kind) &&
    typeof authored.value === "string"
  ) {
    if (authored.kind === "date") return new Date(`${authored.value}T00:00:00Z`);
    if (authored.kind === "datetime") return new Date(`${authored.value}Z`);
    if (authored.kind === "datetime_tz") return new Date(authored.value);
    return authored.value;
  }
  throw new ProducerError(
    "invalid_journey",
    `unsupported authored scalar ${String(authored.kind)}`,
  );
}

function attribute(package_, className, authored) {
  const token = package_[className];
  if (typeof token?.create !== "function") {
    throw new ProducerError(
      "generated_package_identity",
      `generated ${className} attribute token is absent`,
    );
  }
  return token.create(primitive(authored));
}

function personValue(package_, recordValue) {
  const value = fields(recordValue, "person");
  if (!Array.isArray(value.aliases)) {
    throw new ProducerError("invalid_journey", "person list field is malformed");
  }
  const input = {
    identifier: attribute(package_, "Identifier", field(value, "identifier", "string")),
    score: attribute(package_, "Score", field(value, "score", "long")),
    valBool: attribute(package_, "ValBool", field(value, "val_bool", "boolean")),
    valConstrained: attribute(
      package_,
      "ValConstrained",
      field(value, "val_constrained", "long"),
    ),
    valDate: attribute(package_, "ValDate", field(value, "val_date", "date")),
    valDatetime: attribute(
      package_,
      "ValDatetime",
      field(value, "val_datetime", "datetime"),
    ),
    valDatetimeTz: attribute(
      package_,
      "ValDatetimeTz",
      field(value, "val_datetime_tz", "datetime_tz"),
    ),
    valDecimal: attribute(
      package_,
      "ValDecimal",
      field(value, "val_decimal", "decimal"),
    ),
    valDouble: attribute(
      package_,
      "ValDouble",
      field(value, "val_double", "double"),
    ),
    valDuration: attribute(
      package_,
      "ValDuration",
      field(value, "val_duration", "duration"),
    ),
  };
  for (const [source, target, className, kind] of [
    ["nickname", "nickname", "Nickname", "string"],
    ["foo__bar", "fooBar", "FooBar", "long"],
    ["score__gte", "scoreGte", "ScoreGte", "long"],
  ]) {
    if (Object.hasOwn(value, source)) {
      input[target] = attribute(package_, className, field(value, source, kind));
    }
  }
  return package_.Person.create(input);
}

function robotValue(package_, recordValue) {
  const value = fields(recordValue, "robot");
  return package_.Robot.create({
    robotId: attribute(package_, "RobotId", field(value, "robot_id", "long")),
    valConstrained: attribute(
      package_,
      "ValConstrained",
      field(value, "val_constrained", "long"),
    ),
  });
}

function modelLabel(value) {
  let identity;
  try {
    identity = JSON.parse(value.__typebridgeModel);
  } catch (error) {
    throw new ProducerError(
      "invalid_hydration",
      "hydrated value has no model identity",
      { cause: error },
    );
  }
  if (
    identity === null ||
    typeof identity !== "object" ||
    typeof identity.label !== "string"
  ) {
    throw new ProducerError(
      "invalid_hydration",
      "hydrated value has no model label",
    );
  }
  return identity.label;
}

function reportScalar(kind, value) {
  if (kind === "double" && typeof value === "number" && Number.isFinite(value)) {
    const bits = Buffer.alloc(8);
    bits.writeDoubleBE(value);
    return { bits: bits.toString("hex"), kind };
  }
  if (kind === "long" && typeof value === "bigint") {
    return { kind, value: value.toString() };
  }
  if (kind === "boolean" && typeof value === "boolean") {
    return { kind, value };
  }
  if (kind === "string" && typeof value === "string") {
    return { kind, value };
  }
  if (
    ["decimal", "duration"].includes(kind) &&
    typeof value === "string"
  ) {
    return { kind, value };
  }
  if (["date", "datetime", "datetime_tz"].includes(kind) && value instanceof Date) {
    const encoded = value.toISOString();
    if (kind === "date" && encoded.endsWith("T00:00:00.000Z")) {
      return { kind, value: encoded.slice(0, 10) };
    }
    if (kind === "datetime") {
      return { kind, value: encoded.replace(/\.000Z$/, "") };
    }
    if (kind === "datetime_tz") {
      return { kind, value: encoded.replace(/\.000Z$/, "Z") };
    }
  }
  throw new ProducerError(
    "invalid_hydration",
    `hydrated ${kind} scalar has the wrong type`,
  );
}

function canonicalScalars(package_, person) {
  if (person.__typebridgeModel !== package_.Person.typeKey) {
    throw new ProducerError("invalid_hydration", "Ada did not hydrate as Person");
  }
  const hydrated = Object.fromEntries(
    Object.entries(SCALAR_FIELDS).map(([kind, property]) => [
      kind,
      reportScalar(kind, person[property].value),
    ]),
  );
  return { hydrated, model: modelLabel(person), ref: "data-ada" };
}

function roleLabel(token) {
  let identity;
  try {
    identity = JSON.parse(token.role);
  } catch (error) {
    throw new ProducerError(
      "generated_package_identity",
      "generated role token identity is malformed",
      { cause: error },
    );
  }
  if (
    identity === null ||
    typeof identity !== "object" ||
    typeof identity.label !== "string"
  ) {
    throw new ProducerError(
      "generated_package_identity",
      "generated role token has no label",
    );
  }
  return identity;
}

function actorIdentity(package_, actor) {
  if (actor === null) return null;
  const model = modelLabel(actor);
  if (actor.__typebridgeModel === package_.Person.typeKey) {
    return { key: actor.identifier.value, model };
  }
  if (actor.__typebridgeModel === package_.Robot.typeKey) {
    return { key: actor.robotId.value.toString(), model };
  }
  throw new ProducerError(
    "invalid_hydration",
    "Interaction actor has an unknown model",
  );
}

function insertOne(ref, manager, value, created, actualOrder) {
  const inserted = manager.insert(value);
  if (
    inserted === null ||
    typeof inserted !== "object" ||
    typeof inserted.iid !== "string" ||
    inserted.iid.length === 0
  ) {
    throw new ProducerError(
      "invalid_crud_result",
      `insert did not return an attached generated value for ${ref}`,
    );
  }
  created.set(ref, { manager, value: inserted, iid: inserted.iid });
  actualOrder.push(ref);
  const hydrated = manager.getByIid(inserted.iid);
  if (hydrated === null) {
    throw new ProducerError(
      "invalid_crud_result",
      `inserted value ${ref} did not hydrate`,
    );
  }
  return hydrated;
}

function countNumber(value, label) {
  if (typeof value !== "bigint" || value < 0n || value > BigInt(Number.MAX_SAFE_INTEGER)) {
    throw new ProducerError("invalid_crud_result", `${label} returned an invalid count`);
  }
  return Number(value);
}

function cleanupCreated(created, cleanupOrder, managers) {
  const actualOrder = [];
  const countsAtDelete = new Map();
  const readsAfterDelete = new Map();
  const failures = [];
  for (const ref of cleanupOrder) {
    const entry = created.get(ref);
    if (entry === undefined) continue;
    try {
      entry.manager.delete(entry.value);
      const remained = entry.manager.getByIid(entry.iid) !== null;
      readsAfterDelete.set(ref, remained);
      if (remained) {
        throw new ProducerError(
          "cleanup_failed",
          `deleted value ${ref} remained readable`,
        );
      }
      actualOrder.push(ref);
      countsAtDelete.set(
        ref,
        countNumber(entry.manager.count(), `${ref} manager`),
      );
    } catch (error) {
      failures.push(error);
    }
  }
  const zeroChecks = [];
  for (const model of [...managers.keys()].sort()) {
    try {
      const count = countNumber(managers.get(model).count(), `${model} manager`);
      zeroChecks.push({ count_after_cleanup: count, model });
      if (count !== 0) {
        throw new ProducerError(
          "cleanup_failed",
          `model ${model} retained ${count} live values`,
        );
      }
    } catch (error) {
      failures.push(error);
    }
  }
  if (failures.length !== 0) {
    throw new ProducerError(
      "cleanup_failed",
      `${failures.length} generated CRUD cleanup operation(s) failed`,
      { cause: failures[0] },
    );
  }
  return { actualOrder, countsAtDelete, readsAfterDelete, zeroChecks };
}

export function runJourney(
  package_,
  database,
  records,
  createOrder,
  cleanupOrder,
) {
  const managers = new Map([
    ["container", package_.Container.manager(database)],
    ["event", package_.Event.manager(database)],
    ["interaction", package_.Interaction.manager(database)],
    ["person", package_.Person.manager(database)],
    ["plain-activity", package_.PlainActivity.manager(database)],
    ["robot", package_.Robot.manager(database)],
  ]);
  for (const [model, manager] of managers) {
    if (manager.count() !== 0n) {
      throw new ProducerError(
        "database_not_empty",
        `new database contains ${model} values`,
      );
    }
  }

  const created = new Map();
  const actualCreateOrder = [];
  const observations = {};
  let plainReadAfterCreate = false;
  let plainRolePreserved = false;
  let plainParticipant = null;
  let cleanup;
  try {
    const ada = personValue(package_, record(records, "data-ada", "person"));
    const dana = personValue(package_, record(records, "data-dana", "person"));
    const hydratedAda = insertOne(
      "data-ada",
      managers.get("person"),
      ada,
      created,
      actualCreateOrder,
    );
    const hydratedDana = insertOne(
      "data-dana",
      managers.get("person"),
      dana,
      created,
      actualCreateOrder,
    );
    observations.canonical_scalar_values = canonicalScalars(
      package_,
      hydratedAda,
    );

    const hydratedRobot7 = insertOne(
      "robot-7",
      managers.get("robot"),
      robotValue(package_, record(records, "robot-7", "robot")),
      created,
      actualCreateOrder,
    );
    const hydratedRobotNegative7 = insertOne(
      "robot-negative-7",
      managers.get("robot"),
      robotValue(package_, record(records, "robot-negative-7", "robot")),
      created,
      actualCreateOrder,
    );

    const interactionRecords = Object.fromEntries(
      ["interaction-robot", "interaction-absent", "interaction-person"].map(
        (ref) => [ref, record(records, ref, "interaction")],
      ),
    );
    const interactionIdentifier = (ref) =>
      attribute(
        package_,
        "Identifier",
        field(fields(interactionRecords[ref], "Interaction"), "identifier", "string"),
      );
    const interactions = {
      "interaction-robot": package_.Interaction.create({
        identifier: interactionIdentifier("interaction-robot"),
        actor: hydratedRobot7,
        target: hydratedAda,
      }),
      "interaction-absent": package_.Interaction.create({
        identifier: interactionIdentifier("interaction-absent"),
        target: hydratedDana,
      }),
      "interaction-person": package_.Interaction.create({
        identifier: interactionIdentifier("interaction-person"),
        actor: hydratedAda,
        target: hydratedDana,
      }),
    };
    const hydratedInteractions = {};
    for (const ref of [
      "interaction-robot",
      "interaction-absent",
      "interaction-person",
    ]) {
      hydratedInteractions[ref] = insertOne(
        ref,
        managers.get("interaction"),
        interactions[ref],
        created,
        actualCreateOrder,
      );
    }

    const hydratedPlain = insertOne(
      "plain-activity-ada",
      managers.get("plain-activity"),
      package_.PlainActivity.create({ participant: hydratedAda }),
      created,
      actualCreateOrder,
    );
    const inheritedRole = roleLabel(package_.PlainActivity.participant);
    plainParticipant = actorIdentity(package_, hydratedPlain.participant);
    plainReadAfterCreate =
      hydratedPlain.__typebridgeModel === package_.PlainActivity.typeKey &&
      hydratedPlain.participant.__typebridgeModel === package_.Person.typeKey &&
      plainParticipant?.key === "data-ada";
    plainRolePreserved =
      inheritedRole.declaring_relation === "base-activity" &&
      inheritedRole.label === "participant";
    if (!plainReadAfterCreate || !plainRolePreserved) {
      throw new ProducerError(
        "invalid_hydration",
        "inherited PlainActivity role drifted",
      );
    }

    const hydratedEvent = insertOne(
      "event-ada",
      managers.get("event"),
      package_.Event.create({ subject: hydratedAda }),
      created,
      actualCreateOrder,
    );
    const hydratedContainer = insertOne(
      "container-event",
      managers.get("container"),
      package_.Container.create({
        item: [package_.Event.reference(hydratedEvent.iid, {})],
      }),
      created,
      actualCreateOrder,
    );
    const relationPlayer = hydratedContainer.item[0];
    const relationPlayerPreserved =
      hydratedContainer.__typebridgeModel === package_.Container.typeKey &&
      hydratedContainer.item.length === 1 &&
      relationPlayer?.__typebridgeModel === package_.Event.typeKey &&
      relationPlayer.__typebridgeForm === "reference" &&
      relationPlayer.iid === hydratedEvent.iid;
    if (!relationPlayerPreserved) {
      throw new ProducerError(
        "invalid_hydration",
        "Event-as-Container player drifted",
      );
    }
    observations.relation_as_player = {
      owner: { model: modelLabel(hydratedContainer), ref: "container-event" },
      player: { model: modelLabel(relationPlayer), ref: "event-ada" },
      preserved: relationPlayerPreserved,
      role: roleLabel(package_.Container.item).label,
    };

    const expectedLiveCreate = createOrder.filter((ref) => LIVE_REFS.has(ref));
    if (canonicalString(actualCreateOrder) !== canonicalString(expectedLiveCreate)) {
      throw new ProducerError(
        "invalid_operation_order",
        "live creation order drifted",
      );
    }

    const robotValues = [
      ["robot-7", hydratedRobot7],
      ["robot-negative-7", hydratedRobotNegative7],
    ].sort((left, right) =>
      left[1].robotId.value < right[1].robotId.value
        ? -1
        : left[1].robotId.value > right[1].robotId.value
          ? 1
          : 0,
    );
    observations.integer_key_polymorphic_optional_role = {
      integer_keys: robotValues.map(([ref, robot]) => ({
        model: modelLabel(robot),
        ref,
        value: robot.robotId.value.toString(),
      })),
      optional_role: roleLabel(package_.Interaction.actor).label,
      relation: modelLabel(hydratedInteractions["interaction-person"]),
      states: [
        "interaction-absent",
        "interaction-person",
        "interaction-robot",
      ].map((ref) => ({
        actor: actorIdentity(package_, hydratedInteractions[ref].actor),
        relation_ref: ref,
      })),
    };
  } finally {
    cleanup = cleanupCreated(
      created,
      cleanupOrder.filter((ref) => LIVE_REFS.has(ref)),
      managers,
    );
  }

  const expectedLiveCleanup = cleanupOrder.filter((ref) => LIVE_REFS.has(ref));
  if (
    canonicalString(cleanup.actualOrder) !== canonicalString(expectedLiveCleanup)
  ) {
    throw new ProducerError(
      "invalid_operation_order",
      "live cleanup order drifted",
    );
  }
  observations.inherited_plain_activity_role_lifecycle = {
    count_after_delete: cleanup.countsAtDelete.get("plain-activity-ada"),
    created: created.has("plain-activity-ada"),
    deleted: cleanup.actualOrder.includes("plain-activity-ada"),
    inherited_relation: "base-activity",
    model: "plain-activity",
    participant: plainParticipant,
    read_after_create: plainReadAfterCreate,
    read_after_delete: cleanup.readsAfterDelete.get("plain-activity-ada"),
    ref: "plain-activity-ada",
    role: "participant",
    role_identity_preserved: plainRolePreserved,
  };
  observations.cleanup = {
    order: cleanup.actualOrder,
    zero_checks: cleanup.zeroChecks,
  };
  if (
    canonicalString(Object.keys(observations).sort()) !==
    canonicalString([...OBSERVATION_REFS].sort())
  ) {
    throw new ProducerError(
      "invalid_observation_ledger",
      "live observation ledger is incomplete",
    );
  }
  return observations;
}

function teardownError(code, message, error, prior) {
  let cause = error;
  if (prior !== undefined) {
    cause = new Error(
      error instanceof Error ? error.message : String(error),
      { cause: prior },
    );
  }
  return new ProducerError(code, message, { cause });
}

export async function runOwnedDatabase({
  package_,
  runtime,
  address,
  databaseName,
  port,
  providerSchema,
  records,
  createOrder,
  cleanupOrder,
  versionDetector = detectServerVersion,
  journeyRunner = runJourney,
}) {
  const database = runtime.RustDatabase.connect(address, databaseName, {
    httpPort: port,
  });
  let ownsDatabase = false;
  let observations;
  let failure;
  try {
    const detected = await versionDetector(address, port);
    if (detected !== "3.12.1") {
      throw new ProducerError(
        "server_version_mismatch",
        "live evidence requires the actually detected TypeDB server version 3.12.1; " +
          `detected ${JSON.stringify(detected)}`,
      );
    }
    if (database.databaseExists()) {
      throw new ProducerError(
        "database_preexisting",
        `${DATABASE_ENV} must name an absent isolated database`,
      );
    }
    database.createDatabase();
    ownsDatabase = true;
    if (!database.databaseExists()) {
      throw new ProducerError(
        "database_create_failed",
        "isolated database was not created",
      );
    }
    const transaction = database.transaction("schema");
    try {
      transaction.query(providerSchema);
      transaction.commit();
    } catch (error) {
      try {
        transaction.close();
      } catch {
        // Database deletion below is the authoritative schema-failure teardown.
      }
      throw error;
    }
    observations = journeyRunner(
      package_,
      database,
      records,
      createOrder,
      cleanupOrder,
    );
  } catch (error) {
    failure = error;
  }

  if (ownsDatabase) {
    try {
      database.deleteDatabase();
      if (database.databaseExists()) {
        throw new ProducerError(
          "database_teardown_failed",
          "isolated database remained after deletion",
        );
      }
    } catch (error) {
      failure = teardownError(
        "database_teardown_failed",
        "isolated database could not be deleted",
        error,
        failure,
      );
    }
  }
  try {
    database.close();
  } catch (error) {
    failure = teardownError(
      "database_close_failed",
      "TypeDB connection could not be closed",
      error,
      failure,
    );
  }
  if (failure !== undefined) throw failure;
  return observations;
}

export function buildReport(authority, observations) {
  return {
    authority,
    binding: "node",
    format: REPORT_FORMAT,
    observations,
    semantic_profile: SEMANTIC_PROFILE,
  };
}

export async function produce(environment = process.env) {
  const output = outputPath(environment);
  const root = realDirectory(
    requiredEnvironment(REPOSITORY_ROOT_ENV, environment),
    REPOSITORY_ROOT_ENV,
  );
  const packageRoot = realDirectory(
    requiredEnvironment(PACKAGE_ROOT_ENV, environment),
    PACKAGE_ROOT_ENV,
  );
  const address = requiredEnvironment(ADDRESS_ENV, environment);
  const databaseName = requiredEnvironment(DATABASE_ENV, environment);
  const port = httpPort(environment);
  const { authority, sources } = sourceAuthority(root);
  const { package_, runtime } = await loadGeneratedPackage(packageRoot);
  const { records, createOrder, cleanupOrder } = parseJourney(sources.journey);
  let providerSchema;
  try {
    providerSchema = new TextDecoder("utf-8", { fatal: true }).decode(
      sources.provider,
    );
  } catch (error) {
    throw new ProducerError(
      "invalid_authority",
      "provider schema is not UTF-8",
      { cause: error },
    );
  }
  const observations = await runOwnedDatabase({
    package_,
    runtime,
    address,
    databaseName,
    port,
    providerSchema,
    records,
    createOrder,
    cleanupOrder,
  });
  publishReport(output, buildReport(authority, observations));
}

export async function main(environment = process.env) {
  try {
    await produce(environment);
    return 0;
  } catch (error) {
    process.stderr.write(
      `Node Phase-2 live producer rejected: ${
        error instanceof Error ? error.message : String(error)
      }\n`,
    );
    return 1;
  }
}

if (resolve(process.argv[1] ?? "") === fileURLToPath(import.meta.url)) {
  process.exitCode = await main();
}
