#!/usr/bin/env node
/** Produce Node's TypeDB-3.12.3 Manager-filter report. */

import crypto from "node:crypto";
import {
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  openSync,
  readSync,
} from "node:fs";
import { createRequire } from "node:module";
import { basename, isAbsolute, resolve } from "node:path";
import { pathToFileURL } from "node:url";

import {
  ProducerError,
  canonicalString,
  publishReport,
  realDirectory,
  requireCondition,
  validateOutputTarget,
} from "./live_support.mjs";
export {
  ProducerError,
  MAX_REPORT_BYTES,
  canonicalBytes,
  publishReport,
} from "./live_support.mjs";

export const REPORT_FORMAT = "typebridge.manager-filter-live-report/v1";
export const SEMANTIC_PROFILE = "typedb-3.12.1/v1";
export const SEMANTIC_SCHEMA_FINGERPRINT = Object.freeze({
  algorithm: "sha256",
  canonicalization: "typebridge.schema-canonical-json/v1",
  digest: "3c8d072b60c575b4c0381c1ea44088a9c18e01ed52e90730311aadccb72cd0c8",
  domain: "typebridge.schema.semantic",
  semantic_profile: SEMANTIC_PROFILE,
});

export const OUTPUT_ENV = "TYPE_BRIDGE_MANAGER_LIVE_REPORT";
export const ADDRESS_ENV = "TYPE_BRIDGE_MANAGER_LIVE_ADDRESS";
export const DATABASE_ENV = "TYPE_BRIDGE_MANAGER_LIVE_DATABASE";
export const HTTP_PORT_ENV = "TYPE_BRIDGE_MANAGER_LIVE_HTTP_PORT";
export const TLS_ROOT_CA_ENV = "TYPEDB_TLS_ROOT_CA";
export const PACKAGE_ROOT_ENV = "TYPE_BRIDGE_MANAGER_NODE_PACKAGE_ROOT";
export const REPOSITORY_ROOT_ENV = "TYPE_BRIDGE_MANAGER_REPOSITORY_ROOT";
export const LOCAL_PACKAGE = "generated_manager";
export const FOREIGN_PACKAGE = "generated_manager_foreign";
export const MAX_AUTHORITY_BYTES = 1024 * 1024;
export const SOURCE_PATHS = Object.freeze({
  schema: "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml",
  journey: "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json",
  provider:
    "tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql",
});
export const SOURCE_SHA256 = Object.freeze({
  schema: "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
  journey: "c5679b428c22e2bff7989f6d674cde3780b752a40f6bbce6ccb3aa451797b1c0",
  provider: "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
});

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
    throw new ProducerError("invalid_authority", `${label} cannot be read`, {
      cause: error,
    });
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
      `Manager ${label}`,
    );
    const digest = crypto.createHash("sha256").update(bytes).digest("hex");
    if (digest !== SOURCE_SHA256[label]) {
      throw new ProducerError(
        "authority_hash_mismatch",
        `Manager ${label} is not the frozen exact-3.12.1 authority`,
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
      `${name} is not bounded UTF-8 text`,
    );
  }
  return value;
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

export function outputPath(environment = process.env) {
  const value = requiredEnvironment(OUTPUT_ENV, environment);
  if (!isAbsolute(value) || ["", ".", ".."].includes(basename(value))) {
    throw new ProducerError(
      "invalid_output_path",
      "report path must be absolute",
    );
  }
  validateOutputTarget(value);
  return value;
}

function regularPackagePath(path, label, directory) {
  let metadata;
  try {
    metadata = lstatSync(path);
  } catch (error) {
    throw new ProducerError("generated_package_import", `${label} is absent`, {
      cause: error,
    });
  }
  const correctKind = directory ? metadata.isDirectory() : metadata.isFile();
  if (metadata.isSymbolicLink() || !correctKind) {
    throw new ProducerError(
      "generated_package_import",
      `${label} must be a real filesystem object`,
    );
  }
}

async function importGenerated(packageRoot, name) {
  const generatedRoot = resolve(packageRoot, name);
  const manifest = resolve(generatedRoot, "package.json");
  const entry = resolve(generatedRoot, "dist/index.js");
  regularPackagePath(generatedRoot, `${name} package`, true);
  regularPackagePath(manifest, `${name} manifest`, false);
  regularPackagePath(entry, `${name} entry`, false);
  try {
    return {
      package_: await import(pathToFileURL(entry).href),
      manifest,
    };
  } catch (error) {
    throw new ProducerError(
      "generated_package_import",
      `${name} cannot be imported`,
      { cause: error },
    );
  }
}

export async function loadGeneratedPackages(packageRoot) {
  const local = await importGenerated(packageRoot, LOCAL_PACKAGE);
  const foreign = await importGenerated(packageRoot, FOREIGN_PACKAGE);
  let runtime;
  try {
    runtime = createRequire(local.manifest)("@type-bridge/node");
  } catch (error) {
    throw new ProducerError(
      "generated_package_import",
      "@type-bridge/node cannot be imported",
      { cause: error },
    );
  }
  let semantic;
  let foreignSemantic;
  try {
    semantic = JSON.parse(local.package_.SEMANTIC_SCHEMA_FINGERPRINT_JSON);
    foreignSemantic = JSON.parse(
      foreign.package_.SEMANTIC_SCHEMA_FINGERPRINT_JSON,
    );
  } catch (error) {
    throw new ProducerError(
      "generated_package_identity",
      "generated semantic fingerprints are malformed",
      { cause: error },
    );
  }
  requireCondition(
    canonicalString(semantic) === canonicalString(SEMANTIC_SCHEMA_FINGERPRINT),
    "generated_package_identity",
    "local package is not the frozen Sdk V3 projection",
  );
  requireCondition(
    canonicalString(foreignSemantic) !== canonicalString(semantic),
    "generated_package_identity",
    "foreign package is not a genuinely different projection",
  );
  for (const package_ of [local.package_, foreign.package_]) {
    for (const field of ["fooBar", "identifier", "score", "valBool"]) {
      requireCondition(
        package_.Person?.fields?.[field] !== undefined,
        "generated_package_identity",
        `generated Person field ${field} is absent`,
      );
    }
  }
  requireCondition(
    typeof runtime?.RustDatabase?.connect === "function",
    "generated_package_import",
    "@type-bridge/node has no RustDatabase facade",
  );
  return { local: local.package_, foreign: foreign.package_, runtime };
}

function versionEndpoint(address, port, tls = false) {
  let parsed;
  try {
    parsed = new URL(address.includes("://") ? address : `http://${address}`);
  } catch (error) {
    throw new ProducerError(
      "invalid_address",
      `${ADDRESS_ENV} is not a bare host and port`,
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
      `${ADDRESS_ENV} is not a bare host and port`,
    );
  }
  const host = parsed.hostname.includes(":")
    ? `[${parsed.hostname.replace(/^\[|\]$/g, "")}]`
    : parsed.hostname;
  return `${tls ? "https" : "http"}://${host}:${port}/v1/version`;
}

export async function detectServerVersion(address, port, tls = false) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await fetch(versionEndpoint(address, port, tls), {
      redirect: "error",
      signal: controller.signal,
    });
    if (response.status !== 200) {
      throw new ProducerError(
        "server_version_probe_failed",
        "TypeDB version probe returned a non-success status",
      );
    }
    const bytes = Buffer.from(await response.arrayBuffer());
    if (bytes.length === 0 || bytes.length > 4096) {
      throw new ProducerError(
        "server_version_probe_failed",
        "TypeDB version probe returned an invalid response size",
      );
    }
    const document = JSON.parse(
      new TextDecoder("utf-8", { fatal: true }).decode(bytes),
    );
    if (
      document === null ||
      typeof document !== "object" ||
      typeof document.version !== "string" ||
      !/^\d+\.\d+\.\d+$/.test(document.version)
    ) {
      throw new ProducerError(
        "server_version_probe_failed",
        "TypeDB version probe returned an invalid document",
      );
    }
    return document.version;
  } catch (error) {
    if (error instanceof ProducerError) throw error;
    throw new ProducerError(
      "server_version_probe_failed",
      "TypeDB version probe failed",
      { cause: error },
    );
  } finally {
    clearTimeout(timeout);
  }
}

function jsonObject(bytes, label) {
  let value;
  try {
    value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch (error) {
    throw new ProducerError(
      "invalid_journey",
      `${label} is not valid UTF-8 JSON`,
      { cause: error },
    );
  }
  requireCondition(
    value !== null && typeof value === "object" && !Array.isArray(value),
    "invalid_journey",
    `${label} must be an object`,
  );
  return value;
}

function personRecords(journey) {
  requireCondition(
    journey.format === "typebridge.sdk-journey/v3" &&
      journey.fixture_id === "sdk-v3" &&
      journey.version === 3 &&
      journey.semantic_profile === SEMANTIC_PROFILE,
    "invalid_journey",
    "Sdk V3 journey identity drifted",
  );
  const people = journey.records?.people;
  requireCondition(
    Array.isArray(people),
    "invalid_journey",
    "person records are malformed",
  );
  const indexed = new Map(people.map((record) => [record.ref, record]));
  requireCondition(
    indexed.size === 2 && indexed.has("data-ada") && indexed.has("data-dana"),
    "invalid_journey",
    "manager person records drifted",
  );
  return [indexed.get("data-ada"), indexed.get("data-dana")];
}

function field(fields, name, kind) {
  const value = fields?.[name];
  requireCondition(
    value !== null &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      value.kind === kind,
    "invalid_journey",
    `person field ${name} drifted`,
  );
  return value;
}

function primitive(authored) {
  if (authored.kind === "double") {
    requireCondition(
      typeof authored.bits === "string" && /^[0-9a-f]{16}$/.test(authored.bits),
      "invalid_journey",
      "double bits are malformed",
    );
    return Buffer.from(authored.bits, "hex").readDoubleBE(0);
  }
  if (authored.kind === "long") {
    requireCondition(
      typeof authored.value === "string" && /^-?[0-9]+$/.test(authored.value),
      "invalid_journey",
      "long value is malformed",
    );
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
    if (authored.kind === "date")
      return new Date(`${authored.value}T00:00:00Z`);
    if (authored.kind === "datetime") return new Date(`${authored.value}Z`);
    if (authored.kind === "datetime_tz") return new Date(authored.value);
    return authored.value;
  }
  throw new ProducerError(
    "invalid_journey",
    `unsupported scalar domain ${String(authored.kind)}`,
  );
}

function attribute(package_, name, authored) {
  const token = package_[name];
  requireCondition(
    typeof token?.create === "function",
    "generated_package_identity",
    `generated ${name} attribute is absent`,
  );
  return token.create(primitive(authored));
}

function personValue(package_, record) {
  const fields = record.fields;
  const input = {
    fooBar: attribute(package_, "FooBar", field(fields, "foo__bar", "long")),
    identifier: attribute(
      package_,
      "Identifier",
      field(fields, "identifier", "string"),
    ),
    score: attribute(package_, "Score", field(fields, "score", "long")),
    scoreGte: attribute(
      package_,
      "ScoreGte",
      field(fields, "score__gte", "long"),
    ),
    valBool: attribute(
      package_,
      "ValBool",
      field(fields, "val_bool", "boolean"),
    ),
    valConstrained: attribute(
      package_,
      "ValConstrained",
      field(fields, "val_constrained", "long"),
    ),
    valDate: attribute(package_, "ValDate", field(fields, "val_date", "date")),
    valDatetime: attribute(
      package_,
      "ValDatetime",
      field(fields, "val_datetime", "datetime"),
    ),
    valDatetimeTz: attribute(
      package_,
      "ValDatetimeTz",
      field(fields, "val_datetime_tz", "datetime_tz"),
    ),
    valDecimal: attribute(
      package_,
      "ValDecimal",
      field(fields, "val_decimal", "decimal"),
    ),
    valDouble: attribute(
      package_,
      "ValDouble",
      field(fields, "val_double", "double"),
    ),
    valDuration: attribute(
      package_,
      "ValDuration",
      field(fields, "val_duration", "duration"),
    ),
  };
  if (Object.hasOwn(fields, "nickname")) {
    input.nickname = attribute(
      package_,
      "Nickname",
      field(fields, "nickname", "string"),
    );
  }
  return package_.Person.create(input);
}

function diagnostic(operation, kind, category, code) {
  try {
    operation();
  } catch (error) {
    if (error?.sdkCategory !== category || error?.code !== code) {
      throw new ProducerError(
        "diagnostic_mismatch",
        `${kind} returned ${String(error?.sdkCategory)}/${String(error?.code)}`,
        { cause: error },
      );
    }
    return {
      kind,
      category: error.sdkCategory,
      code: error.code,
      rejected_before_provider_io: true,
    };
  }
  throw new ProducerError(
    "missing_rejection",
    `${kind} unexpectedly succeeded`,
  );
}

export function providerFreeManagerSurfaceChecks(package_, database) {
  const manager = package_.Person.manager(database);
  const getterOrder = [];
  const authored = {};
  Object.defineProperties(authored, {
    foo__bar: {
      enumerable: true,
      get() {
        getterOrder.push("foo__bar");
        return package_.FooBar.create(7n);
      },
    },
    score: {
      enumerable: true,
      get() {
        getterOrder.push("score");
        return package_.Score.create(40n);
      },
    },
  });
  const snapshotted = manager.filter(authored);
  requireCondition(
    canonicalString(getterOrder) === canonicalString(["foo__bar", "score"]),
    "manager_filter_snapshot",
    "enumerable manager-filter getters were not observed exactly once in authored order",
  );
  diagnostic(
    () =>
      manager.where(
        package_.Robot.fields.robotId,
        "eq",
        package_.RobotId.create(7n),
      ),
    "structured_manager_diagnostic",
    "integrity",
    "field_owner_mismatch",
  );
  return {
    ambiguous: manager.filter({ score__gte: package_.Score.create(40n) }),
    raw: manager.filter({ foo__bar: package_.FooBar.create(7n) }),
    snapshotted,
  };
}

function exerciseProviderFreeManagerSurfaceChecks(package_, filters) {
  requireCondition(
    canonicalString(keys(package_, filters.snapshotted.all())) ===
      canonicalString([]),
    "manager_filter_snapshot",
    "legacy and common manager filters did not share the authored snapshot",
  );
  requireCondition(
    filters.raw.count() === 1n,
    "manager_filter_raw_label",
    "raw foo__bar did not route to the common manager filter",
  );
  requireCondition(
    canonicalString(keys(package_, filters.ambiguous.all())) ===
      canonicalString(["data-dana"]) && filters.ambiguous.exists() === true,
    "manager_filter_precedence",
    "ambiguous score__gte did not use native prefix/suffix precedence",
  );
}

function preIoRejections(local, foreign, database) {
  const manager = local.Person.manager(database);
  const rejections = [
    diagnostic(
      () =>
        manager.where(
          local.Robot.fields.robotId,
          "eq",
          local.RobotId.create(7n),
        ),
      "wrong_field_owner",
      "integrity",
      "field_owner_mismatch",
    ),
    diagnostic(
      () =>
        manager.where(
          local.Person.fields.fooBar,
          "eq",
          local.Identifier.create("wrong-domain"),
        ),
      "wrong_scalar_domain",
      "invalid_input",
      "wrong_scalar_domain",
    ),
    diagnostic(
      () =>
        manager.where(
          foreign.Person.fields.fooBar,
          "eq",
          foreign.FooBar.create(7n),
        ),
      "wrong_package",
      "integrity",
      "generated_token_package_mismatch",
    ),
    diagnostic(
      () =>
        manager.where(
          local.Person.fields.valBool,
          "gt",
          local.ValBool.create(true),
        ),
      "boolean_ordering",
      "invalid_input",
      "invalid_operator_for_type",
    ),
  ];
  const first = diagnostic(
    () =>
      manager
        .where(local.Person.fields.fooBar, "gte", local.FooBar.create(7n))
        .first(),
    "nonsingular_first",
    "invalid_input",
    "manager_first_requires_identity",
  );
  return {
    rejections,
    nonsingular: {
      category: first.category,
      code: first.code,
      rejected_before_provider_io: first.rejected_before_provider_io,
    },
  };
}

function compareText(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function keys(package_, values) {
  requireCondition(
    Array.isArray(values),
    "invalid_terminal_result",
    "manager all did not return an array",
  );
  const result = values.map((value) => {
    requireCondition(
      value?.__typebridgeModel === package_.Person.typeKey &&
        typeof value.identifier?.value === "string",
      "invalid_terminal_result",
      "manager all returned an invalid person",
    );
    return value.identifier.value;
  });
  requireCondition(
    new Set(result).size === result.length,
    "invalid_terminal_result",
    "manager all returned duplicate identities",
  );
  return result.sort(compareText);
}

function fieldTokenObservation(package_) {
  const token = package_.Person.fields.fooBar;
  requireCondition(
    token.owner === package_.Person.typeKey &&
      token.attribute === JSON.stringify("foo__bar") &&
      token.name === "fooBar",
    "generated_token_identity",
    "foo__bar token identity drifted",
  );
  const owner = JSON.parse(token.owner);
  const attribute = JSON.parse(token.attribute);
  return {
    owner: `${owner.kind}:${owner.label}`,
    attribute: `attribute:${attribute}`,
    binding_name: "foo__bar",
  };
}

function runManagerJourney(package_, database) {
  const transaction = database.transaction("read");
  try {
    const manager = package_.Person.manager(transaction);
    const root = manager.where();
    const operatorOutcomes = ["eq", "ne", "gt", "gte", "lt", "lte"].map(
      (operator) => ({
        operator,
        normalized_keys: keys(
          package_,
          manager
            .where(
              package_.Person.fields.fooBar,
              operator,
              package_.FooBar.create(7n),
            )
            .all(),
        ),
      }),
    );
    const authoredOrder = ["foo__bar:gte:7", "score:gt:40"];
    const conjunction = manager
      .where(package_.Person.fields.fooBar, "gte", package_.FooBar.create(7n))
      .where(package_.Person.fields.score, "gt", package_.Score.create(40n));
    const conjunctionKeys = keys(package_, conjunction.all());
    const allKeys = keys(package_, root.all());
    const count = root.count();
    const exists = root.exists();
    requireCondition(
      canonicalString(allKeys) === canonicalString(["data-ada", "data-dana"]) &&
        count === 2n &&
        exists === true,
      "terminal_mismatch",
      "empty manager terminals drifted",
    );
    const identity = manager.where(
      package_.Person.fields.identifier,
      "eq",
      package_.Identifier.create("data-ada"),
    );
    const first = identity.first();
    requireCondition(
      first?.__typebridgeModel === package_.Person.typeKey &&
        first.identifier?.value === "data-ada",
      "terminal_mismatch",
      "strict first returned the wrong person",
    );

    const session = new package_.QuerySession(transaction);
    try {
      const person = session.exact(package_.Person);
      const sibling_query = session
        .query(person)
        .where(
          person
            .field(package_.Person.fields.identifier)
            .eq(package_.Identifier.create("data-ada")),
        );
      requireCondition(
        sibling_query.countBy(person) === 1n,
        "borrowed_read_reuse",
        "sibling query sibling query did not remain usable",
      );
    } finally {
      session.close();
    }
    const sibling = manager.where(
      package_.Person.fields.fooBar,
      "eq",
      package_.FooBar.create(9n),
    );
    requireCondition(
      canonicalString(keys(package_, sibling.all())) ===
        canonicalString(["data-dana"]),
      "borrowed_read_reuse",
      "sibling manager filter was not reusable",
    );

    return {
      model: "person",
      field_token: fieldTokenObservation(package_),
      operator_literal: { kind: "long", value: "7" },
      operator_outcomes: operatorOutcomes,
      conjunction: {
        authored_order: authoredOrder,
        normalized_keys: conjunctionKeys,
      },
      terminals: {
        all_normalization: "reference_key_ascending",
        count: Number(count),
        exists,
      },
      first: {
        identity_predicate: "identifier:eq:data-ada",
        strict_singular: true,
        result: first.identifier.value,
      },
      borrowed_read: {
        reusable_after_each_terminal: true,
        sibling_filter_usable: true,
        final_state: "active",
      },
    };
  } finally {
    transaction.close();
  }
}

function installSchema(database, providerSchema) {
  const transaction = database.transaction("schema");
  try {
    transaction.query(providerSchema);
    transaction.commit();
  } catch (error) {
    try {
      transaction.close();
    } catch {
      // Database deletion is the authoritative schema-failure cleanup.
    }
    throw error;
  }
}

async function runOwnedDatabase({
  local,
  foreign,
  runtime,
  address,
  databaseName,
  port,
  providerSchema,
  records,
  tlsRootCa,
}) {
  const detected = await detectServerVersion(address, port, tlsRootCa !== undefined);
  if (detected !== "3.12.3") {
    throw new ProducerError(
      "server_version_mismatch",
      `exact TypeDB 3.12.3 required, detected ${JSON.stringify(detected)}`,
    );
  }
  const bootstrap = runtime.RustDatabase.connect(address, databaseName, {
    httpPort: port,
    ...(tlsRootCa === undefined ? {} : { tlsEnabled: true, tlsRootCa }),
  });
  let database;
  let ownsDatabase = false;
  let failure;
  let observation;
  try {
    requireCondition(
      !bootstrap.databaseExists(),
      "database_preexisting",
      "isolated database must be absent",
    );
    bootstrap.createDatabase();
    ownsDatabase = true;
    installSchema(bootstrap, providerSchema);
    bootstrap.close();
    if (tlsRootCa !== undefined) {
      let untrusted;
      try {
        untrusted = local.connect(
          new local.DirectConnectionPolicy(address, databaseName, {
            httpPort: port,
            tls: "native_roots",
          }),
        );
      } catch {
        // The isolated fixture root is deliberately absent from native roots.
      }
      if (untrusted !== undefined) {
        untrusted.close();
        throw new ProducerError(
          "wrong_trust_accepted",
          "TLS fixture connected without its configured custom root",
        );
      }
    }
    database = local.connect(
      new local.DirectConnectionPolicy(address, databaseName, {
        httpPort: port,
        ...(tlsRootCa === undefined
          ? { tls: "disabled" }
          : { tls: "custom_root", tlsRootCa }),
      }),
    );
    const providerFreeFilters = providerFreeManagerSurfaceChecks(local, database);
    const hostile = preIoRejections(local, foreign, database);

    const manager = local.Person.manager(database);
    const inserted = records.map((record) =>
      manager.insert(personValue(local, record)),
    );
    requireCondition(
      canonicalString(keys(local, inserted)) ===
        canonicalString(["data-ada", "data-dana"]),
      "seed_mismatch",
      "manager fixture insertion drifted",
    );
    exerciseProviderFreeManagerSurfaceChecks(local, providerFreeFilters);
    observation = runManagerJourney(local, database);
    observation.rejections = hostile.rejections;
    observation.first.nonsingular_rejection = hostile.nonsingular;
    for (const value of inserted.reverse()) manager.delete(value);
    requireCondition(
      manager.count() === 0n,
      "cleanup_failed",
      "person rows remained after cleanup",
    );
  } catch (error) {
    failure = error;
  }
  if (ownsDatabase) {
    try {
      const owner = database ?? bootstrap;
      const outcome = owner.planDatabaseDelete().execute();
      requireCondition(
        outcome === "deleted_standalone_managed" ||
          outcome === "deleted_owned_pair",
        "database_teardown_failed",
        `unexpected managed deletion outcome: ${outcome}`,
      );
      requireCondition(
        !owner.databaseExists(),
        "database_teardown_failed",
        "isolated database remained",
      );
    } catch (error) {
      failure ??= error;
    }
  }
  try {
    database?.close();
    bootstrap.close();
  } catch (error) {
    failure ??= error;
  }
  if (failure !== undefined) throw failure;
  requireCondition(
    observation !== undefined,
    "missing_observation",
    "manager journey produced no observation",
  );
  return observation;
}

export async function main(environment = process.env) {
  try {
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
    const tlsRootCa = environment[TLS_ROOT_CA_ENV];
    if (tlsRootCa === "") {
      throw new ProducerError(
        "invalid_tls_root_ca",
        `${TLS_ROOT_CA_ENV} must not be empty`,
      );
    }
    const { authority, sources } = sourceAuthority(root);
    const { local, foreign, runtime } =
      await loadGeneratedPackages(packageRoot);
    const records = personRecords(
      jsonObject(sources.journey, "Manager journey"),
    );
    const providerSchema = new TextDecoder("utf-8", { fatal: true }).decode(
      sources.provider,
    );
    const observation = await runOwnedDatabase({
      local,
      foreign,
      runtime,
      address,
      databaseName,
      port,
      providerSchema,
      records,
      tlsRootCa,
    });
    publishReport(output, {
      authority,
      binding: "node",
      format: REPORT_FORMAT,
      observation,
      semantic_profile: SEMANTIC_PROFILE,
    });
    return 0;
  } catch (error) {
    console.error(
      `Node Manager-filter producer rejected: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
    return 1;
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  process.exitCode = await main();
}
