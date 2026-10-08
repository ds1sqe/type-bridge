import assert from "node:assert/strict";
import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { link, lstat, mkdtemp, mkdir, open, readFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { delimiter, dirname, isAbsolute, join, resolve } from "node:path";
import net from "node:net";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { TextDecoder } from "node:util";

import type { QueryV2Error } from "../../typescript/index.js";

import {
  connectIntegration,
  defineSchema,
  INTG_DATABASE,
  TYPEDB_ADDRESS,
  TYPEDB_HTTP_PORT,
  TYPEDB_PASSWORD,
  TYPEDB_USERNAME,
} from "../integration/common/index.js";

const NODE_SOURCE_PACKAGE = process.cwd();
const NODE_RUNTIME_PACKAGE = process.env.TYPE_BRIDGE_NODE_PACKAGE_ROOT ?? NODE_SOURCE_PACKAGE;
const CORE = resolve(NODE_SOURCE_PACKAGE, "../..");
const ROOT = resolve(CORE, "..");
const ACCEPTANCE = resolve(CORE, "crates/schema-codegen/tests/acceptance");
const TYPEDB_VERSION = process.env.TYPEDB_VERSION ?? "3.12.1";
const IS_TYPEDB_3_11 = TYPEDB_VERSION.startsWith("3.11.");
const PROVIDER_SCHEMA = resolve(
  ACCEPTANCE,
  IS_TYPEDB_3_11 ? "provider-3.11.5.tql" : "provider-3.12.1.tql",
);
const REMOTE_PROFILE = IS_TYPEDB_3_11 ? "typedb-3.11.5/v1" : "typedb-3.12.1/v1";
const SDK_MANIFEST_RELATIVE = "tests/contracts/sdk_conformance/manifest-v1.json";
const SDK_CATALOG_RELATIVE =
  "tests/contracts/sdk_conformance/sdk-v1/catalog-v1.json";
const SDK_JOURNEY_RELATIVE =
  "tests/contracts/sdk_conformance/sdk-v1/journey-v1.json";
const SDK_V2_CATALOG_RELATIVE =
  "tests/contracts/sdk_conformance/sdk-v2/catalog-v2.json";
const SDK_V2_JOURNEY_RELATIVE =
  "tests/contracts/sdk_conformance/sdk-v2/journey-v2.json";
const SDK_V3_JOURNEY = resolve(
  ROOT,
  "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json",
);
const SDK_V3_PROVIDER_SCHEMA = resolve(
  ROOT,
  "tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql",
);
const SDK_MANIFEST = resolve(ROOT, SDK_MANIFEST_RELATIVE);
const SDK_CATALOG = resolve(ROOT, SDK_CATALOG_RELATIVE);
const SDK_JOURNEY = resolve(ROOT, SDK_JOURNEY_RELATIVE);
const SDK_V2_CATALOG = resolve(ROOT, SDK_V2_CATALOG_RELATIVE);
const SDK_V2_JOURNEY = resolve(ROOT, SDK_V2_JOURNEY_RELATIVE);
const SDK_V2_PROOF_VALIDATOR = resolve(
  ROOT,
  "scripts/ci/proof_fragments.py",
);

function sourceIdentity(path: string, raw: Uint8Array): Record<string, string> {
  return { path, sha256: createHash("sha256").update(raw).digest("hex") };
}

function observationKey(reference: string, proofKind: string): string {
  return `${reference}\u0000${proofKind}`;
}

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function sdkResults(
  catalog: any,
  journey: any,
  observed: ReadonlyMap<string, Record<string, unknown>>,
): Record<string, unknown>[] {
  const capabilityByCase = new Map<string, string>(
    catalog.cases.map((entry: any) => [entry.id, entry.capability_id]),
  );
  const results = catalog.selected_proofs.map((proof: any) => {
    const actual = observed.get(observationKey(proof.observation_ref, proof.proof_kind));
    assert.notEqual(actual, undefined);
    const committed = journey.expected_observations[proof.observation_ref];
    assert.deepEqual(actual, committed);
    const capabilityId = capabilityByCase.get(proof.case_id);
    assert.notEqual(capabilityId, undefined);
    return {
      case_id: proof.case_id,
      capability_id: capabilityId,
      proof_kind: proof.proof_kind,
      outcome: "passed",
      observation: actual,
    };
  });
  return results.sort((left: any, right: any) =>
    compareText(left.case_id, right.case_id) || compareText(left.proof_kind, right.proof_kind)
  );
}

function sdkV3SupplementObservations(): ReadonlyMap<string, Record<string, unknown>> {
  const duplicateTarget = {
    category: "invalid_input",
    code: "duplicate_batch_target",
    path: [
      { kind: "argument", value: "rows" },
      { kind: "index", value: 1 },
      { kind: "argument", value: "iid" },
    ],
    details: { first_conflicting_index: { kind: "count", value: "0" } },
    rejected_before_provider_io: true,
  };
  return new Map([
    [observationKey("entity_batch_insert_put", "direct_runtime"), {
      empty: { result_count: 0, transaction_opened: false, provider_calls: 0 },
      insert: { input_order: ["data-ada", "data-dana"], result_order: ["data-ada", "data-dana"], persisted_keys: ["data-ada", "data-dana"] },
      duplicate_key: { key: "data-ada", category: "invalid_input", code: "duplicate_batch_key", path: [{ kind: "argument", value: "rows" }, { kind: "index", value: 1 }, { kind: "field", value: "person:identifier" }], details: { first_conflicting_index: { kind: "count", value: "0" } }, rejected_before_provider_io: true },
      put: { input_order: ["data-ada", "data-dana"], result_order: ["data-ada", "data-dana"], replaced_keys: ["data-ada"], inserted_keys: ["data-dana"] },
      late_failure: { rollback_completed: true, committed_prefix: false, persisted_keys: [], published_results: 0 },
    }],
    [observationKey("entity_batch_update_delete_atomic", "direct_runtime"), {
      update: { identity_kind: "iid", input_order: ["counter-left", "counter-right"], result_order: ["counter-left", "counter-right"], identity_preserved: [true, true], replacement_complete: true },
      duplicate_target: duplicateTarget,
      delete_failure: { requested: ["counter-left", "counter-right"], outcome_published: false, all_targets_remain: true, rollback_completed: true, committed_prefix: false },
      delete_success: { requested: ["counter-left", "counter-right"], outcome: "unit", affected_count_exposed: false, all_targets_absent: true, missing_identity_noop: true },
    }],
    [observationKey("relation_batch_insert_put", "direct_runtime"), {
      empty: { result_count: 0, transaction_opened: false, provider_calls: 0 },
      insert: { input_order: ["link-forward", "link-return"], result_order: ["link-forward", "link-return"], persisted_keys: ["data-link-forward", "data-link-return"] },
      duplicate_key: { key: "data-link-forward", category: "invalid_input", code: "duplicate_batch_key", path: [{ kind: "argument", value: "rows" }, { kind: "index", value: 1 }, { kind: "field", value: "network-link:identifier" }], details: { first_conflicting_index: { kind: "count", value: "0" } }, rejected_before_provider_io: true },
      put: { input_order: ["link-forward", "link-return"], result_order: ["link-forward", "link-return"], replaced_keys: ["data-link-forward"], inserted_keys: ["data-link-return"], roles_preserved: true },
      late_failure: { rollback_completed: true, committed_prefix: false, persisted_keys: [], published_results: 0 },
    }],
    [observationKey("relation_batch_update_delete_atomic", "direct_runtime"), {
      update: { identity_kind: "iid", input_order: ["membership-ada", "membership-robot"], result_order: ["membership-ada", "membership-robot"], identity_preserved: [true, true], roles_preserved: true, replacement_complete: true },
      duplicate_target: duplicateTarget,
      delete_failure: { requested: ["membership-ada", "membership-robot"], outcome_published: false, all_targets_remain: true, rollback_completed: true, committed_prefix: false },
      delete_success: { requested: ["membership-ada", "membership-robot"], outcome: "unit", affected_count_exposed: false, all_targets_absent: true, missing_identity_noop: true },
    }],
    [observationKey("unkeyed_entity_iid_lifecycle", "direct_runtime"), {
      model: "counter", identity_kind: "iid", surface: { key_present: false, put_present: false },
      insert: { refs: ["counter-left", "counter-right"], count_after: 2, canonical_identity_retained: true },
      get_by_identity: { ref: "counter-left", found: true, value: "1" },
      update_by_identity: { ref: "counter-left", value_before: "1", value_after: "11", identity_preserved: true, count_after: 2 },
      delete_by_identity: { ref: "counter-left", deleted: true, read_after_delete: false, count_after: 1, missing_identity_noop: true },
      count_after_cleanup: 0,
    }],
    [observationKey("unkeyed_relation_iid_lifecycle", "direct_runtime"), {
      model: "membership", identity_kind: "iid", surface: { key_present: false, put_present: false },
      insert: { refs: ["membership-ada", "membership-robot"], count_after: 2, canonical_identity_retained: true },
      get_by_identity: { ref: "membership-ada", found: true, roles: { member: ["data-ada"] } },
      update_by_identity: { ref: "membership-ada", roles_before: { member: ["data-ada"] }, roles_after: { member: ["data-dana"] }, identity_preserved: true, count_after: 2 },
      delete_by_identity: { ref: "membership-ada", deleted: true, read_after_delete: false, count_after: 1, missing_identity_noop: true },
      count_after_cleanup: 0,
    }],
    [observationKey("data_resource_lifecycle", "lifecycle"), {
      resources: ["database", "read_transaction", "write_transaction", "cancellation", "batch_builder", "batch_input", "filter", "result", "projected_value", "projected_thing", "diagnostic"],
      close_contract: { idempotent: true, post_close_rejected: true, post_close_provider_calls: 0 },
      parent_child: { runtime_close_with_database: "in_use", database_close_with_transaction: "in_use", parent_handle_retained_on_rejection: true, child_remains_usable: true, parent_closes_after_children: true },
      session_rules: { borrowed_read_not_consumed: true, sibling_filter_usable: true, write_recovery_after_cancellation: true },
      result_survival: { result_survives_filter_close: true, result_survives_transaction_close: true, owned_thing_survives_result_close: true },
      cancellation_close_idempotent: true, projected_value_close_idempotent: true, projected_thing_close_idempotent: true,
    }],
    [observationKey("borrowed_transaction_lifecycle", "lifecycle"), {
      read: { reusable_after_success: true, sibling_filter_usable: true, terminal_sequence: ["all", "count", "exists", "first"], state_after_terminals: "active", close_idempotent: true },
      commit_visibility: { before_commit: { same_transaction_visible: true, outside_transaction_visible: false }, after_commit: { outside_transaction_visible: true, state: "committed" } },
      rollback_visibility: { before_rollback: { same_transaction_visible: true, outside_transaction_visible: false }, after_rollback: { outside_transaction_visible: false, state: "rolled_back" } },
      poison: { state: "rollback_only", first_cause: { category: "provider", code: "provider_operation_failed" }, later_cause: { category: "resource_limit", code: "batch_item_limit" }, retained_cause: { category: "provider", code: "provider_operation_failed" }, commit_rejection: { category: "transaction", code: "transaction_rollback_only", provider_commit_calls: 0 } },
      post_rollback: { state: "rolled_back", rollback_idempotent: true, commit_rejected: true, mutation_rejected: true, close_state: "closed", close_idempotent: true },
    }],
  ]);
}

function loadSdkV2ProofObservations(): ReadonlyMap<string, Record<string, unknown>> {
  const rawPaths = process.env.TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENTS;
  const runNonce = process.env.TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE;
  const python = process.env.TYPE_BRIDGE_SDK_V2_VALIDATOR_PYTHON;
  if (rawPaths === undefined || runNonce === undefined || python === undefined) {
    throw new Error(
      "sdk-v2 reports require proof fragment paths, the same-run nonce, "
      + "and an explicit validator Python executable",
    );
  }
  if (!isAbsolute(python)) {
    throw new TypeError(
      "TYPE_BRIDGE_SDK_V2_VALIDATOR_PYTHON must be an absolute path",
    );
  }
  const paths = rawPaths.split(delimiter);
  if (paths.length === 0 || paths.some((value) => value.length === 0)) {
    throw new Error("sdk-v2 proof fragment paths must be a nonempty path list");
  }
  const validation = spawnSync(
    python,
    [
      SDK_V2_PROOF_VALIDATOR,
      "--sdk",
      "2",
      "--binding",
      "node",
      "--run-nonce",
      runNonce,
      "--observations-json",
      ...paths,
    ],
    {
      cwd: ROOT,
      encoding: "utf8",
      maxBuffer: 128 * 1024,
      stdio: "pipe",
    },
  );
  if (validation.error !== undefined) throw validation.error;
  if (validation.status !== 0) {
    throw new Error(
      `sdk-v2 proof fragments were rejected (${validation.status ?? "unknown"}): `
      + validation.stderr,
    );
  }
  if (validation.stderr !== "") {
    throw new Error(`sdk-v2 proof validator wrote stderr: ${validation.stderr}`);
  }
  const rows: unknown = JSON.parse(validation.stdout);
  if (!Array.isArray(rows)) {
    throw new TypeError("sdk-v2 proof validator returned a non-array payload");
  }
  const observations = new Map<string, Record<string, unknown>>();
  for (const row of rows) {
    if (
      row === null
      || typeof row !== "object"
      || Array.isArray(row)
      || Object.keys(row).sort().join("\0") !== "observation\0observation_ref\0proof_kind"
    ) {
      throw new TypeError("sdk-v2 proof validator returned a malformed row");
    }
    const value = row as Record<string, unknown>;
    if (
      typeof value.observation_ref !== "string"
      || typeof value.proof_kind !== "string"
      || value.observation === null
      || typeof value.observation !== "object"
      || Array.isArray(value.observation)
    ) {
      throw new TypeError("sdk-v2 proof validator returned an invalid observation");
    }
    const key = observationKey(value.observation_ref, value.proof_kind);
    assert.equal(observations.has(key), false);
    observations.set(key, value.observation as Record<string, unknown>);
  }
  assert.equal(observations.size, 3);
  return observations;
}

async function sdkReport(
  catalogRaw: Buffer,
  catalog: any,
  journeyRaw: Buffer,
  semanticFingerprint: Record<string, unknown>,
  projectionFingerprint: Record<string, unknown>,
  results: readonly Record<string, unknown>[],
): Promise<Record<string, unknown>> {
  const fixture = catalog.fixture;
  const schemaRaw = await readFile(resolve(ROOT, fixture.schema_path));
  const providerSchemaRaw = await readFile(resolve(ROOT, fixture.provider_schema_path));
  const manifestRaw = await readFile(SDK_MANIFEST);
  assert.equal(catalog.journey_path, SDK_JOURNEY_RELATIVE);
  return {
    format: "typebridge.sdk-conformance-report/v1",
    binding: "node",
    manifest: sourceIdentity(SDK_MANIFEST_RELATIVE, manifestRaw),
    catalog: sourceIdentity(SDK_CATALOG_RELATIVE, catalogRaw),
    fixture: {
      id: fixture.id,
      version: fixture.version,
      semantic_profile: fixture.semantic_profile,
      schema: sourceIdentity(fixture.schema_path, schemaRaw),
      provider_schema: sourceIdentity(fixture.provider_schema_path, providerSchemaRaw),
      journey: sourceIdentity(catalog.journey_path, journeyRaw),
      semantic_fingerprint: semanticFingerprint,
      projection_target: catalog.projection_targets.node,
      projection_fingerprint: projectionFingerprint,
    },
    results,
  };
}

async function sdkV2Report(
  catalogRaw: Buffer,
  catalog: any,
  journeyRaw: Buffer,
  semanticFingerprint: Record<string, unknown>,
  projectionFingerprint: Record<string, unknown>,
  results: readonly Record<string, unknown>[],
): Promise<Record<string, unknown>> {
  const fixture = catalog.fixture;
  const schemaRaw = await readFile(resolve(ROOT, fixture.schema_path));
  const providerSchemaRaw = await readFile(resolve(ROOT, fixture.provider_schema_path));
  const manifestRaw = await readFile(SDK_MANIFEST);
  assert.equal(catalog.journey_path, SDK_V2_JOURNEY_RELATIVE);
  return {
    format: "typebridge.sdk-conformance-report/v2",
    binding: "node",
    manifest: sourceIdentity(SDK_MANIFEST_RELATIVE, manifestRaw),
    catalog: sourceIdentity(SDK_V2_CATALOG_RELATIVE, catalogRaw),
    fixture: {
      id: fixture.id,
      version: fixture.version,
      semantic_profile: fixture.semantic_profile,
      schema: sourceIdentity(fixture.schema_path, schemaRaw),
      provider_schema: sourceIdentity(fixture.provider_schema_path, providerSchemaRaw),
      journey: sourceIdentity(catalog.journey_path, journeyRaw),
      semantic_fingerprint: semanticFingerprint,
      projection_target: catalog.projection_targets.node,
      projection_fingerprint: projectionFingerprint,
    },
    results,
  };
}

function sortJson(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(sortJson);
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => compareText(left, right))
        .map(([key, member]) => [key, sortJson(member)]),
    );
  }
  return value;
}

async function metadataIfPresent(path: string): Promise<Awaited<ReturnType<typeof lstat>> | undefined> {
  try {
    return await lstat(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
}

async function validateSdkReportPath(rawPath: string): Promise<string> {
  const encodedPath = Buffer.from(rawPath, "utf8");
  if (encodedPath.toString("utf8") !== rawPath) {
    throw new TypeError("TYPE_BRIDGE_SDK_REPORT must be a UTF-8 path");
  }
  if (encodedPath.length === 0 || encodedPath.length > 4096) {
    throw new RangeError("TYPE_BRIDGE_SDK_REPORT must contain 1 to 4096 UTF-8 bytes");
  }
  if (!isAbsolute(rawPath)) {
    throw new TypeError("TYPE_BRIDGE_SDK_REPORT must be an absolute path");
  }
  const parent = dirname(rawPath);
  const parentMetadata = await metadataIfPresent(parent);
  if (
    parentMetadata === undefined
    || parentMetadata.isSymbolicLink()
    || !parentMetadata.isDirectory()
  ) {
    throw new TypeError(
      "TYPE_BRIDGE_SDK_REPORT parent must be an existing non-symlink directory",
    );
  }
  if (await metadataIfPresent(rawPath) !== undefined) {
    throw new Error("TYPE_BRIDGE_SDK_REPORT destination must not exist");
  }
  return parent;
}

function requireSdkServerVersion(detected: string | null): void {
  if (detected !== "3.12.3") {
    throw new Error(
      "sdk reports require the actual detected TypeDB server version 3.12.3; "
      + `detected ${JSON.stringify(detected)}`,
    );
  }
}

function typeDBVersionEndpoint(address: string, httpPort: number, tls: boolean): string {
  if (!Number.isInteger(httpPort) || httpPort < 1 || httpPort > 65_535) {
    throw new RangeError("TypeDB HTTP version probe port is invalid");
  }
  let parsed: URL;
  try {
    parsed = new URL(address.includes("://") ? address : `http://${address}`);
  } catch (error) {
    throw new TypeError("TypeDB address cannot be used for the HTTP version probe", {
      cause: error,
    });
  }
  if (parsed.hostname.length === 0) {
    throw new TypeError("TypeDB address has no host for the HTTP version probe");
  }
  const host = parsed.hostname.includes(":") && !parsed.hostname.startsWith("[")
    ? `[${parsed.hostname}]`
    : parsed.hostname;
  return `${tls ? "https" : "http"}://${host}:${httpPort}/v1/version`;
}

async function detectTypeDBServerVersion(address: string, httpPort: number): Promise<string> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 30_000);
  try {
    const response = await fetch(
      typeDBVersionEndpoint(
        address,
        httpPort,
        process.env.TYPEDB_TLS_ROOT_CA !== undefined,
      ),
      { redirect: "error", signal: controller.signal },
    );
    if (response.status !== 200) {
      throw new Error(`TypeDB HTTP version probe returned status ${response.status}`);
    }
    if (response.body === null) {
      throw new Error("TypeDB HTTP version probe returned an invalid bounded body");
    }
    const reader = response.body.getReader();
    const chunks: Buffer[] = [];
    let bodyLength = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      bodyLength += value.byteLength;
      if (bodyLength > 4096) {
        await reader.cancel();
        throw new Error("TypeDB HTTP version probe returned an invalid bounded body");
      }
      chunks.push(Buffer.from(value));
    }
    if (bodyLength === 0) {
      throw new Error("TypeDB HTTP version probe returned an invalid bounded body");
    }
    let bodyText: string;
    try {
      bodyText = new TextDecoder("utf-8", { fatal: true }).decode(
        Buffer.concat(chunks, bodyLength),
      );
    } catch (error) {
      throw new Error("TypeDB HTTP version probe returned invalid UTF-8", { cause: error });
    }
    let document: unknown;
    try {
      document = JSON.parse(bodyText);
    } catch (error) {
      throw new Error("TypeDB HTTP version probe returned malformed JSON", { cause: error });
    }
    if (
      document === null
      || typeof document !== "object"
      || !("version" in document)
      || typeof document.version !== "string"
      || !/^\d+\.\d+\.\d+$/.test(document.version)
    ) {
      throw new Error("TypeDB HTTP version probe returned an invalid version field");
    }
    return document.version;
  } finally {
    clearTimeout(timeout);
  }
}

async function publishSdkReport(
  rawPath: string,
  report: Record<string, unknown>,
): Promise<void> {
  const parent = await validateSdkReportPath(rawPath);

  const payload = `${JSON.stringify(sortJson(report))}\n`;
  const temporary = join(parent, `.typebridge-sdk-${randomUUID()}.tmp`);
  let published = false;
  try {
    const output = await open(temporary, "wx", 0o600);
    try {
      await output.writeFile(payload, "utf8");
      await output.sync();
    } finally {
      await output.close();
    }
    try {
      await link(temporary, rawPath);
      published = true;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "EEXIST") {
        throw new Error(
          "TYPE_BRIDGE_SDK_REPORT destination appeared during publication",
          { cause: error },
        );
      }
      throw error;
    }
  } finally {
    await rm(temporary, { force: true });
  }
  assert.equal(published, true);
}

async function runSdkJourney(
  generated: any,
  personManager: any,
  membershipManager: any,
  querySession: any,
  remoteSession: any,
  requests: Buffer[],
  catalog: any,
  journey: any,
): Promise<Record<string, unknown>[]> {
  const {
    Aliases,
    FooBar,
    Identifier,
    Membership,
    Nickname,
    Person,
    Score,
    ScoreGte,
    ValBool,
    ValConstrained,
    ValDate,
    ValDatetime,
    ValDatetimeTz,
    ValDecimal,
    ValDouble,
    ValDuration,
  } = generated;
  const personRecord = journey.records.person;
  const membershipRecord = journey.records.membership;
  const fields = personRecord.fields;
  const update = personRecord.update;
  const doubleBytes = Buffer.from(fields.val_double.bits, "hex");
  assert.equal(doubleBytes.length, 8);
  const doubleValue = doubleBytes.readDoubleBE(0);
  const person = (nicknameValue: string) => Person.create({
    identifier: Identifier.create(fields.identifier.value),
    nickname: Nickname.create(nicknameValue),
    aliases: fields.aliases.map((alias: any) => Aliases.create(alias.value)),
    score: Score.create(BigInt(fields.score.value)),
    fooBar: FooBar.create(BigInt(fields.foo__bar.value)),
    scoreGte: ScoreGte.create(BigInt(fields.score__gte.value)),
    valBool: ValBool.create(fields.val_bool.value),
    valConstrained: ValConstrained.create(BigInt(fields.val_constrained.value)),
    valDate: ValDate.create(new Date(`${fields.val_date.value}T00:00:00Z`)),
    valDatetime: ValDatetime.create(new Date(`${fields.val_datetime.value}Z`)),
    valDatetimeTz: ValDatetimeTz.create(new Date(fields.val_datetime_tz.value)),
    valDecimal: ValDecimal.create(fields.val_decimal.value),
    valDouble: ValDouble.create(doubleValue),
    valDuration: ValDuration.create(fields.val_duration.value),
  });
  const normalizePerson = (candidate: any, nicknameField: any): Record<string, unknown> => {
    assert.equal(candidate.__typebridgeModel, Person.typeKey);
    assert.equal(candidate.identifier.__typebridgeModel, Identifier.typeKey);
    assert.equal(candidate.identifier.value, fields.identifier.value);
    assert.equal(candidate.nickname.__typebridgeModel, Nickname.typeKey);
    assert.equal(candidate.nickname.value, nicknameField.value);
    const aliases = candidate.aliases.map((alias: any) => {
      assert.equal(alias.__typebridgeModel, Aliases.typeKey);
      return alias.value;
    }).sort(compareText);
    assert.deepEqual(
      aliases,
      fields.aliases.map((alias: any) => alias.value).sort(compareText),
    );
    assert.equal(candidate.score.__typebridgeModel, Score.typeKey);
    assert.equal(candidate.score.value, BigInt(fields.score.value));
    assert.equal(candidate.fooBar.__typebridgeModel, FooBar.typeKey);
    assert.equal(candidate.fooBar.value, BigInt(fields.foo__bar.value));
    assert.equal(candidate.scoreGte.__typebridgeModel, ScoreGte.typeKey);
    assert.equal(candidate.scoreGte.value, BigInt(fields.score__gte.value));
    assert.equal(candidate.valBool.__typebridgeModel, ValBool.typeKey);
    assert.equal(candidate.valBool.value, fields.val_bool.value);
    assert.equal(candidate.valConstrained.__typebridgeModel, ValConstrained.typeKey);
    assert.equal(candidate.valConstrained.value, BigInt(fields.val_constrained.value));
    assert.equal(candidate.valDate.__typebridgeModel, ValDate.typeKey);
    assert.equal(candidate.valDate.value.toISOString().slice(0, 10), fields.val_date.value);
    assert.equal(candidate.valDatetime.__typebridgeModel, ValDatetime.typeKey);
    assert.equal(
      candidate.valDatetime.value.toISOString().replace(".000Z", "Z").slice(0, -1),
      fields.val_datetime.value,
    );
    assert.equal(candidate.valDatetimeTz.__typebridgeModel, ValDatetimeTz.typeKey);
    assert.equal(
      candidate.valDatetimeTz.value.toISOString().replace(".000Z", "Z"),
      fields.val_datetime_tz.value,
    );
    assert.equal(candidate.valDecimal.__typebridgeModel, ValDecimal.typeKey);
    assert.equal(candidate.valDecimal.value, fields.val_decimal.value);
    assert.equal(candidate.valDouble.__typebridgeModel, ValDouble.typeKey);
    const candidateDouble = Buffer.alloc(8);
    candidateDouble.writeDoubleBE(candidate.valDouble.value);
    assert.equal(candidateDouble.toString("hex"), fields.val_double.bits);
    assert.equal(candidate.valDuration.__typebridgeModel, ValDuration.typeKey);
    assert.equal(candidate.valDuration.value, fields.val_duration.value);
    const scalarDomains = [...new Set<string>(
      Object.values(fields).flatMap((field: any) =>
        (Array.isArray(field) ? field : [field]).map((member: any) => member.kind)
      ),
    )].sort(compareText);
    return {
      aliases,
      key: fields.identifier.value,
      model: personRecord.model,
      nickname: nicknameField.value,
      scalar_domains: scalarDomains,
    };
  };
  const normalizeRole = (relation: any, player: any) => {
    assert.equal(relation.__typebridgeModel, Membership.typeKey);
    assert.equal(player.__typebridgeModel, Person.typeKey);
    assert.equal(relation.member.__typebridgeModel, Person.typeKey);
    assert.equal(relation.member.identifier.value, membershipRecord.player.key);
    assert.equal(player.identifier.value, membershipRecord.player.key);
    assertIid(player.iid);
    const reference = Person.reference(player.iid, { identifier: player.identifier });
    assert.equal(reference.__typebridgeModel, Person.typeKey);
    assert.equal(reference.__typebridgeForm, "reference");
    assert.equal(reference.iid, player.iid);
    assert.equal(reference.identifier.__typebridgeModel, Identifier.typeKey);
    assert.equal(reference.identifier.value, membershipRecord.player.key);
    const playerIdentity = {
      key: reference.identifier.value,
      model: membershipRecord.player.model,
    };
    return {
      hydrated: {
        relation: { model: membershipRecord.model },
        role: membershipRecord.role,
        player: playerIdentity,
      },
      traversal: {
        relation: membershipRecord.model,
        role: membershipRecord.role,
        player: playerIdentity,
      },
      reference: playerIdentity,
    };
  };

  const observed = new Map<string, Record<string, unknown>>();
  let insertedPerson: any;
  let insertedMembership: any;
  let personIid: string | undefined;
  let membershipIid: string | undefined;
  let personCreated = false;
  let membershipCreated = false;
  let personDeleted = false;
  let membershipDeleted = false;
  let readAfterUpdate = false;
  try {
    insertedPerson = personManager.insert(person(fields.nickname.value));
    assertIid(insertedPerson.iid);
    personIid = insertedPerson.iid;
    personCreated = true;
    const readPerson = personManager.getByIid(personIid);
    assert.notEqual(readPerson, null);
    normalizePerson(readPerson, fields.nickname);

    const updatedPerson = personManager.update(personIid, person(update.nickname.value));
    const storedUpdatedPerson = personManager.getByIid(personIid);
    assert.notEqual(storedUpdatedPerson, null);
    normalizePerson(storedUpdatedPerson, update.nickname);
    readAfterUpdate = true;

    insertedMembership = membershipManager.insert(Membership.create({ member: updatedPerson }));
    assertIid(insertedMembership.iid);
    membershipIid = insertedMembership.iid;
    membershipCreated = true;
    const storedMembership = membershipManager.getByIid(membershipIid);
    assert.notEqual(storedMembership, null);
    assert.equal(storedMembership.member.identifier.value, membershipRecord.player.key);

    // One role query, run on the direct and the exchange connection.
    const roleLane = async (session: any): Promise<number> => {
      const relationVariable = session.exact(Membership);
      const personVariable = session.exact(Person);
      const requestsBefore = requests.length;
      const [relation, player] = await session
        .query(relationVariable, personVariable)
        .where(
          relationVariable.role(Membership.member).connects(personVariable),
          personVariable
            .field(Person.identifier)
            .eq(Identifier.create(fields.identifier.value)),
        )
        .one();
      const exchangeCount = requests.length - requestsBefore;
      assert.ok(requests.slice(requestsBefore).every((request) => request.length > 0));
      const lane = session === querySession ? "direct_runtime" : "remote_runtime";
      const role = normalizeRole(relation, player);
      observed.set(
        observationKey("model_values_and_references", lane),
        { ...normalizePerson(player, update.nickname), reference: role.reference },
      );
      observed.set(observationKey("hydrated_role_result", lane), role.hydrated);
      observed.set(observationKey("role_traversal", lane), role.traversal);
      return exchangeCount;
    };
    assert.equal(await roleLane(querySession), 0);
    const exchangeCount = await roleLane(remoteSession);
    assert.equal(exchangeCount, 1);
    observed.set(
      observationKey("remote_one_exchange", "remote_runtime"),
      { exchange_count: exchangeCount, terminal: "one" },
    );
  } finally {
    const cleanupFailures: unknown[] = [];
    if (insertedMembership !== undefined && membershipIid !== undefined) {
      try {
        membershipManager.delete(insertedMembership);
        membershipDeleted = membershipManager.getByIid(membershipIid) === null;
      } catch (error) {
        cleanupFailures.push(error);
      }
    }
    if (insertedPerson !== undefined && personIid !== undefined) {
      try {
        personManager.delete(insertedPerson);
        personDeleted = personManager.getByIid(personIid) === null;
      } catch (error) {
        cleanupFailures.push(error);
      }
    }
    if (cleanupFailures.length > 0) throw cleanupFailures[0];
  }

  observed.set(
    observationKey("entity_lifecycle", "direct_runtime"),
    {
      created: personCreated,
      deleted: personDeleted,
      key: fields.identifier.value,
      model: personRecord.model,
      nickname_after_update: update.nickname.value,
      read_after_update: readAfterUpdate,
    },
  );
  observed.set(
    observationKey("relation_lifecycle", "direct_runtime"),
    {
      created: membershipCreated,
      deleted: membershipDeleted,
      model: membershipRecord.model,
      player_key: membershipRecord.player.key,
      role: membershipRecord.role,
    },
  );
  return sdkResults(catalog, journey, observed);
}

function sdkV2Key(value: any): string {
  assert.equal(typeof value.identifier?.value, "string");
  return value.identifier.value;
}

function sdkV2Model(generated: any, value: any): string {
  for (const [model, name] of [
    [generated.Person, "person"],
    [generated.Employee, "employee"],
    [generated.Manager, "manager"],
    [generated.Membership, "membership"],
    [generated.NetworkLink, "network-link"],
  ] as const) {
    if (value.__typebridgeModel === model.typeKey) return name;
  }
  throw new TypeError(`unexpected sdk-v2 projected model ${value.__typebridgeModel}`);
}

function sdkV2ModelKey(
  generated: any,
  value: any,
): Record<string, string> {
  return { model: sdkV2Model(generated, value), key: sdkV2Key(value) };
}

function sdkV2Keys(values: readonly any[]): string[] {
  return values.map(sdkV2Key);
}

function sdkV2PersonValues(
  generated: any,
  person: any,
  membership: any,
): Record<string, unknown> {
  const scalarFields: readonly [string, any][] = [
    ["boolean", person.valBool],
    ["date", person.valDate],
    ["datetime", person.valDatetime],
    ["datetime_tz", person.valDatetimeTz],
    ["decimal", person.valDecimal],
    ["double", person.valDouble],
    ["duration", person.valDuration],
    ["long", person.score],
    ["string", person.identifier],
  ];
  assert.ok(scalarFields.every(([, field]) => field.value !== undefined));
  return {
    aliases: person.aliases.map((alias: any) => alias.value),
    key: sdkV2Key(person),
    model: sdkV2Model(generated, person),
    nickname: person.nickname.value,
    reference: sdkV2ModelKey(generated, membership.member),
    scalar_domains: scalarFields.map(([domain]) => domain),
  };
}

function sdkV2RoleObservation(
  generated: any,
  membership: any,
  network: any,
): Record<string, unknown> {
  return {
    membership: {
      relation: sdkV2Model(generated, membership),
      role: "member",
      players: [sdkV2ModelKey(generated, membership.member)],
    },
    network_link: {
      relation: sdkV2Model(generated, network),
      origin: sdkV2Key(network.origin),
      destination: sdkV2Key(network.destination),
      participants: sdkV2Keys(network.participant).sort(compareText),
    },
  };
}

function sdkV2HydratedResult(
  generated: any,
  membership: any,
): Record<string, unknown> {
  return {
    rows: [{
      model: sdkV2Model(generated, membership),
      roles: { member: [sdkV2ModelKey(generated, membership.member)] },
    }],
  };
}

/** The core's one enum parser refuses `operation` with `unknown_enum_value`. */
function assertUnknownEnumValue(operation: () => unknown, kind: string): void {
  assert.throws(operation, (error: unknown) => {
    assert.ok(error instanceof Error);
    assert.equal(error.name, "QueryV2Error");
    const diagnostic = error as QueryV2Error;
    assert.equal(diagnostic.category, "invalid_input");
    assert.equal(diagnostic.code, "unknown_enum_value");
    assert.deepEqual(diagnostic.details.kind, { kind: "query_identity", value: kind });
    return true;
  });
}

function sdkV2CardinalityDiagnostic(error: any): Record<string, unknown> {
  assert.ok(error instanceof Error);
  assert.equal(error.name, "QueryV2Error");
  const diagnostic = error as QueryV2Error;
  assert.equal(typeof diagnostic.sdkCategory, "string");
  assert.equal(typeof diagnostic.queryCategory, "string");
  assert.equal(typeof diagnostic.code, "string");
  assert.equal(typeof diagnostic.diagnosticMessage, "string");
  assert.ok(Array.isArray(diagnostic.path));
  assert.equal(typeof diagnostic.details, "object");
  assert.equal(diagnostic.details.actual.kind, "count");
  assert.match(diagnostic.details.actual.value as string, /^(?:0|[1-9][0-9]*)$/);
  const serialized = JSON.stringify({
    message: diagnostic.diagnosticMessage,
    path: diagnostic.path,
    details: diagnostic.details,
  });
  return {
    category: diagnostic.sdkCategory,
    query_category: diagnostic.queryCategory,
    code: diagnostic.code,
    message: diagnostic.diagnosticMessage,
    path: diagnostic.path,
    details: { actual: diagnostic.details.actual },
    redacted: ["query-ada", "query-dana", "localhost", "password"].every(
      (secret) => !serialized.includes(secret),
    ),
  };
}

async function sdkV2ResourceLimits(
  generated: any,
  database: any,
  advertisement: Uint8Array,
  exchange: (request: Uint8Array, signal?: AbortSignal) => Promise<Uint8Array>,
  membershipIid: string,
): Promise<Record<string, unknown>> {
  const defaults = {
    timeout_milliseconds: 30_000,
    items: 65_536,
    bytes: 33_554_432,
    graph_nodes: 65_536,
    attribute_values: 65_536,
    collection_members: 65_536,
    role_players: 65_536,
    statements: 3,
  };
  // Every dimension but the statement count may be raised past its default
  // and is kept as given.
  const raised = (offset: number): Record<string, bigint> => ({
    timeoutMilliseconds: BigInt(defaults.timeout_milliseconds + offset),
    items: BigInt(defaults.items + offset),
    bytes: BigInt(defaults.bytes + offset),
    graphNodes: BigInt(defaults.graph_nodes + offset),
    attributeValues: BigInt(defaults.attribute_values + offset),
    collectionMembers: BigInt(defaults.collection_members + offset),
    rolePlayers: BigInt(defaults.role_players + offset),
    statements: BigInt(defaults.statements),
  });
  const plus = new generated.QueryExecutionResourceLimits(raised(1));
  // Past a supported maximum a value is refused by name, never clamped.
  const refusedByName = (options: Record<string, bigint>, name: string): boolean => {
    try {
      new generated.QueryExecutionResourceLimits(options);
      return false;
    } catch (error) {
      return String((error as Error).message).includes(name);
    }
  };
  const pastMaximumRefused =
    refusedByName({ statements: 4n }, "statements") &&
    refusedByName({ timeoutMilliseconds: 86_400_001n }, "timeout_milliseconds");
  const zero = new generated.QueryExecutionResourceLimits({
    timeoutMilliseconds: 0n,
    items: 0n,
    bytes: 0n,
    graphNodes: 0n,
    attributeValues: 0n,
    collectionMembers: 0n,
    rolePlayers: 0n,
    statements: 0n,
  });
  const values = (limits: any): number[] => [
    Number(limits.timeoutMilliseconds),
    Number(limits.items),
    Number(limits.bytes),
    Number(limits.graphNodes),
    Number(limits.attributeValues),
    Number(limits.collectionMembers),
    Number(limits.rolePlayers),
    Number(limits.statements),
  ];
  const expectedPlus = Object.values(defaults).map((value, index) => (index === 7 ? value : value + 1));
  const plusOneKeptAll = values(plus).every((value, index) => value === expectedPlus[index]);
  const zeroTighteningAll = values(zero).every((value) => value === 0);
  const errors: readonly [string, string][] = await Promise.all(
    [false, true].map(async (remote) => {
      const limits = new generated.QueryExecutionResourceLimits({ rolePlayers: 0n });
      const connection = remote
        ? new generated.ExchangeConnection(advertisement, exchange)
        : database;
      const session = new generated.QuerySession(connection, { resources: limits });
      const relation = session.exact(generated.Membership);
      const query = session.query(relation).where(relation.iid(membershipIid));
      try {
        await query.one();
      } catch (error) {
        const diagnostic = error as any;
        return [diagnostic.sdkCategory, diagnostic.code] as const;
      } finally {
        session.close();
      }
      throw new Error("zero role-player budget accepted a hydrated relation");
    }),
  );
  assert.deepEqual(errors[1], errors[0]);
  // A session asking for more than its connection's answer ceiling runs
  // under the ceiling: the same zero role-player budget, set only on the
  // connection, refuses the hydrated relation the same way.
  const ceilinged = generated.connect(
    new generated.DirectConnectionPolicy(TYPEDB_ADDRESS, INTG_DATABASE, {
      username: TYPEDB_USERNAME,
      password: TYPEDB_PASSWORD,
      httpPort: TYPEDB_HTTP_PORT,
      ...(process.env.TYPEDB_TLS_ROOT_CA === undefined
        ? {}
        : { tls: "custom_root", tlsRootCa: process.env.TYPEDB_TLS_ROOT_CA }),
      answerLimits: new generated.QueryExecutionResourceLimits({ rolePlayers: 0n }),
    }),
  );
  try {
    const ceilingedExchange = new generated.ExchangeConnection(advertisement, exchange, {
      answerLimits: new generated.QueryExecutionResourceLimits({ rolePlayers: 0n }),
    });
    for (const connection of [ceilinged, ceilingedExchange]) {
      const session = new generated.QuerySession(connection, { resources: plus });
      const relation = session.exact(generated.Membership);
      await assert.rejects(
        session.query(relation).where(relation.iid(membershipIid)).one(),
        (error: any) => error.sdkCategory === errors[0][0] && error.code === errors[0][1],
        "a session must not exceed its connection's answer ceiling",
      );
      session.close();
    }
  } finally {
    ceilinged.close();
  }
  return {
    defaults,
    plus_one_kept_all: plusOneKeptAll,
    past_maximum_refused: pastMaximumRefused,
    zero_tightening_all: zeroTighteningAll,
    enforced: {
      dimension: "role_players",
      category: errors[0][0],
      code: errors[0][1],
      no_partial_result: true,
    },
  };
}

async function sdkV2Lifecycle(
  generated: any,
  database: any,
  advertisement: Uint8Array,
  exchange: (request: Uint8Array, signal?: AbortSignal) => Promise<Uint8Array>,
  requests: readonly Uint8Array[],
): Promise<Record<string, unknown>> {
  // One lifecycle program, run on a direct and an exchange connection.
  const lane = async (session: any): Promise<[Record<string, unknown>, any]> => {
    const person = session.exact(generated.Person);
    const identifier = person.field(generated.Person.identifier);
    const scope = identifier.eq(generated.Identifier.create("query-ada"));
    const ancestor = session.query(person).where(scope);
    const sibling = ancestor.clone();
    const closedDescendant = ancestor.where(
      person.field(generated.Person.score).gte(generated.Score.create(1n)),
    );
    closedDescendant.close();
    closedDescendant.close();
    const ancestorUsable = sdkV2Key(await ancestor.one()) === "query-ada";
    const descendant = ancestor.where(
      person.field(generated.Person.score).gte(generated.Score.create(1n)),
    );
    const result = await descendant.one();
    ancestor.close();
    ancestor.close();
    const requestsBeforeRejection = requests.length;
    let rejected = false;
    try {
      await ancestor.one();
    } catch (error) {
      rejected = (error as any).code === "query_resource_closed";
    }
    const postCloseIoCount = requests.length - requestsBeforeRejection;
    const observation = {
      ancestor_usable_after_descendant_close: ancestorUsable,
      close_idempotent: ancestor.isClosed && closedDescendant.isClosed,
      descendant_usable_after_ancestor_close:
        sdkV2Key(await descendant.one()) === "query-ada",
      handle_invalidated: ancestor.isClosed,
      post_close_io_count: postCloseIoCount,
      post_close_rejected: rejected,
      session_usable_after_query_close:
        sdkV2Key(await session.query(person).where(scope).one()) === "query-ada",
      sibling_usable: sdkV2Key(await sibling.one()) === "query-ada",
    };
    descendant.close();
    sibling.close();
    session.close();
    return [observation, result];
  };
  const [direct, directResult] = await lane(new generated.QuerySession(database));
  const [remote, remoteResult] = await lane(
    new generated.QuerySession(new generated.ExchangeConnection(advertisement, exchange)),
  );
  assert.deepEqual(remote, direct);
  return {
    lanes: ["direct", "remote"],
    query: direct,
    result_usable_after_query_close:
      sdkV2Key(directResult) === "query-ada"
      && sdkV2Key(remoteResult) === "query-ada",
  };
}

function sdkV2Reducers(
    values: readonly unknown[],
    binding: readonly any[],
    field: readonly any[],
    tuple: readonly any[],
  ): Record<string, unknown> {
    const bits = (value: unknown): string => {
      assert.equal(typeof value, "number");
      const bytes = Buffer.alloc(8);
      bytes.writeDoubleBE(value as number);
      return bytes.toString("hex");
    };
    return {
      reducers: {
        count: Number(values[0]),
        sum: Number(values[1]),
        min: Number(values[2]),
        max: Number(values[3]),
        mean_bits: bits(values[4]),
        median_bits: bits(values[5]),
        std_bits: bits(values[6]),
      },
      groups: {
        binding: binding.map(([group, result]) => ({
          model: "person",
          key: group.identifier.value,
          count: Number(result[0]),
        })),
        field: field.map(([group, result]) => ({
          key: Number(group.value),
          count: Number(result[0]),
        })),
        field_tuple: tuple.map(([[score, scoreGte], result]) => ({
          key: [Number(score.value), Number(scoreGte.value)],
          count: Number(result[0]),
        })),
      },
    };
}

/**
 * The sdk-v2 query program, run once per connection. Every lane observation
 * comes from this one program, so the direct and remote lanes differ only in
 * the connection the session was opened on.
 */
async function sdkV2QueryLane(
  generated: any,
  session: any,
  requests: readonly Uint8Array[],
  personIids: readonly string[],
  membershipIid: string,
  networkIid: string,
): Promise<{
  observations: Record<string, Record<string, unknown>>;
  oneExchange: number;
}> {
  const {
    Employee,
    Identifier,
    Membership,
    NetworkLink,
    Person,
    Score,
    ValBool,
    aggregate,
    integerInput,
    qualifyingScore,
  } = generated;
  const person = session.exact(Person);
  const identifier = person.field(Person.identifier);
  const scope = identifier.eq(Identifier.create("query-ada"))
    .or(identifier.eq(Identifier.create("query-dana")));
  const query = session.query(person).where(scope);
  assertUnknownEnumValue(() => identifier.asc("none" as never), "missing_order");
  const keys = (await query.rows({
    limit: 2n,
    orderBy: [identifier.asc()],
  })).map((person: any) => person.identifier.value);
  const ada = await session
    .query(person)
    .where(identifier.eq(Identifier.create("query-ada")))
    .one();
  const membershipVar = session.exact(Membership);
  const member = session.exact(Person);
  const membership = await session
    .query(membershipVar)
    .match(member)
    .where(
      membershipVar.iid(membershipIid),
      membershipVar.role(Membership.member).connects(member),
      member.field(Person.identifier).eq(Identifier.create("query-ada")),
    )
    .one();
  const networkVar = session.exact(NetworkLink);
  const origin = session.exact(Person);
  const destination = session.exact(Person);
  const network = await session
    .query(networkVar)
    .match(origin, destination)
    .where(
      networkVar.iid(networkIid),
      networkVar.role(NetworkLink.origin).connects(origin),
      networkVar.role(NetworkLink.destination).connects(destination),
      origin.field(Person.identifier).eq(Identifier.create("query-ada")),
      destination.field(Person.identifier).eq(Identifier.create("query-dana")),
    )
    .one();

  const ownerKeys = sdkV2Keys(
    await session
      .query(person)
      .where(person.field(Person.score).isPresent(), scope)
      .rows({ limit: 2n, orderBy: [identifier.asc()] }),
  );
  const optionalKeys = sdkV2Keys(
    await session
      .query(person)
      .where(person.field(Person.nickname).isPresent(), scope)
      .rows({ limit: 2n, orderBy: [identifier.asc()] }),
  );
  const iidKeys = sdkV2Keys(
    await session
      .query(person)
      .where(person.iidIn(personIids))
      .rows({ limit: 2n, orderBy: [identifier.asc()] }),
  );

  const employeeExact = session.exact(Employee);
  const employeeExactId = employeeExact.field(Employee.identifier);
  const exactValues = await session
    .query(employeeExact)
    .where(
      employeeExactId.eq(Identifier.create("query-employee"))
        .or(employeeExactId.eq(Identifier.create("query-manager"))),
    )
    .rows({ limit: 2n, orderBy: [employeeExactId.asc()] });
  const employeeSubtypes = session.subtypes(Employee);
  const employeeSubtypesId = employeeSubtypes.field(Employee.identifier);
  const subtypeValues = await session
    .query(employeeSubtypes)
    .where(
      employeeSubtypesId.eq(Identifier.create("query-employee"))
        .or(employeeSubtypesId.eq(Identifier.create("query-manager"))),
    )
    .rows({ limit: 2n, orderBy: [employeeSubtypesId.asc()] });

  const score = person.field(Person.score);
  const scoreGte = person.field(Person.scoreGte);
  const boolean = person.field(Person.valBool);
  const andKeys = sdkV2Keys(
    await session
      .query(person)
      .where(
        scope,
        score.gte(Score.create(40n)).and(boolean.eq(ValBool.create(true))),
      )
      .rows({ limit: 2n, orderBy: [identifier.asc()] }),
  );
  const orKeys = sdkV2Keys(
    await session
      .query(person)
      .where(scope)
      .rows({ limit: 2n, orderBy: [identifier.asc()] }),
  );
  const notKeys = sdkV2Keys(
    await session
      .query(person)
      .where(scope, boolean.eq(ValBool.create(true)).not())
      .rows({ limit: 2n, orderBy: [identifier.asc()] }),
  );
  const fieldComparisonKeys = sdkV2Keys(
    await session
      .query(person)
      .where(scope, score.gteField(scoreGte))
      .rows({ limit: 2n, orderBy: [identifier.asc()] }),
  );

  const source = session.exact(Person);
  const target = session.exact(Person);
  const reachable = session.reachable(
    source,
    target,
    NetworkLink,
    NetworkLink.origin,
    NetworkLink.destination,
    { minDepth: 1, maxDepth: 1 },
  );
  const reachablePair = await session
    .query(source, target)
    .where(
      reachable,
      source.field(Person.identifier).eq(Identifier.create("query-ada")),
      target.field(Person.identifier).eq(Identifier.create("query-dana")),
    )
    .one();
  const crossLeft = session.exact(Person);
  const crossRight = session.exact(Person);
  const crossLeftId = crossLeft.field(Person.identifier);
  const crossRightId = crossRight.field(Person.identifier);
  const crossScope = crossLeftId.eq(Identifier.create("query-ada"))
    .or(crossLeftId.eq(Identifier.create("query-dana")))
    .and(
      crossRightId.eq(Identifier.create("query-ada"))
        .or(crossRightId.eq(Identifier.create("query-dana"))),
    );
  const crossPairs = (await session
    .query(crossLeft, crossRight)
    .allowCrossJoin(crossLeft, crossRight)
    .where(crossScope)
    .rows({
      limit: 4n,
      orderBy: [crossLeftId.asc(), crossRightId.asc()],
    }))
    .map(([left, right]: readonly any[]) => [sdkV2Key(left), sdkV2Key(right)]);

  const selectionLink = session.exact(NetworkLink);
  const selectionOrigin = session.exact(Person);
  const selectionParticipant = session.exact(Person);
  const selectionPredicates = [
    selectionLink.iid(networkIid),
    selectionLink.role(NetworkLink.origin).connects(selectionOrigin),
    selectionLink.role(NetworkLink.participant).connects(selectionParticipant),
  ] as const;
  const selectionParticipants = selectionParticipant
    .collect()
    .distinct()
    .orderBy(selectionParticipant.field(Person.identifier).asc());
  const positionalPage = await session
    .query(selectionOrigin, selectionParticipants)
    .match(selectionLink)
    .where(...selectionPredicates)
    .pageBy(selectionOrigin, {
      limit: 1n,
      orderBy: [selectionOrigin.field(Person.identifier).asc()],
      includeTotal: true,
    });
  assert.equal(Number(positionalPage.total), 1);
  assert.equal(positionalPage.items.length, 1);
  const [positionalOrigin, positionalParticipants] =
    positionalPage.items[0];
  const namedPage = await session
    .queryNamed({ origin: selectionOrigin, participants: selectionParticipants })
    .match(selectionLink)
    .where(...selectionPredicates)
    .pageBy(selectionOrigin, {
      limit: 1n,
      orderBy: [selectionOrigin.field(Person.identifier).asc()],
      includeTotal: true,
    });
  assert.equal(Number(namedPage.total), 1);
  assert.equal(namedPage.items.length, 1);
  const named = namedPage.items[0];
  const selection = {
    positional: [
      sdkV2Key(positionalOrigin),
      sdkV2Keys(positionalParticipants),
    ],
    named: {
      origin: sdkV2Key(named.origin),
      participants: sdkV2Keys(named.participants),
    },
    collected_distinct:
      new Set(positionalParticipants.map((value: any) => value.iid)).size
      === positionalParticipants.length,
    collection_order: "identifier_asc",
  };

  const requestsBeforeOne = requests.length;
  const dana = await session
    .query(person)
    .where(identifier.eq(Identifier.create("query-dana")))
    .one();
  const oneExchange = requests.length - requestsBeforeOne;
  const first = await query.first({ orderBy: [identifier.asc()] });
  const page = await query.pageBy(person, {
    limit: 1n,
    orderBy: [identifier.asc()],
    includeTotal: true,
  });
  const terminals = {
    one: sdkV2Key(dana),
    first: sdkV2Key(first),
    rows: keys,
    page: {
      items: sdkV2Keys(page.items),
      offset: Number(page.offset),
      limit: Number(page.limit),
      total: Number(page.total),
    },
    count: Number(await query.countBy(person)),
    exists: await query.existsBy(person),
  };
  const scalarDomainKeys = sdkV2Keys(
    await session
      .query(person)
      .where(scope, score.gte(Score.create(40n)))
      .rows({ limit: 2n, orderBy: [identifier.asc()] }),
  );
  const values = await query.aggregate(person, [
    aggregate.count(),
    aggregate.sum(score),
    aggregate.min(score),
    aggregate.max(score),
    aggregate.mean(score),
    aggregate.median(score),
    aggregate.std(score),
  ] as const);
  const groupPerson = session.exact(Person);
  const groupIdentifier = groupPerson.field(Person.identifier);
  const binding = await query
    .match(groupPerson)
    .where(identifier.eqField(groupIdentifier))
    .groupBy(person, groupPerson)
    .aggregate([aggregate.count()] as const);
  const field = await query
    .groupBy(person, score)
    .aggregate([aggregate.count()] as const);
  const tuple = await query
    .groupBy(person, score, person.field(Person.scoreGte))
    .aggregate([aggregate.count()] as const);
  const reducers = sdkV2Reducers(
    values,
    binding,
    field,
    tuple,
  );

  const minimum = integerInput(session, Score.create(30n));
  const call = qualifyingScore(session, person, minimum);
  const functionValues = (await query
    .where(call.gteField(score))
    .rows({ limit: 2n, orderBy: [identifier.asc()] }))
    .map((person: any) => Number(person.score.value));
  const nested = qualifyingScore(session, person, call);
  const nestedFunctionValues = (await query
    .where(nested.gteField(score))
    .rows({ limit: 2n, orderBy: [identifier.asc()] }))
    .map((person: any) => Number(person.score.value));
  let diagnostic: Record<string, unknown> | undefined;
  try {
    await query.one();
  } catch (error) {
    diagnostic = sdkV2CardinalityDiagnostic(error);
  }
  assert.notEqual(diagnostic, undefined, "one() over two rows must fail");
  return {
    oneExchange,
    observations: {
      model_values_and_references: sdkV2PersonValues(generated, ada, membership),
      owner_iid_set: {
        owner_field: "score",
        owner_keys: ownerKeys,
        optional_field: "nickname",
        optional_present_keys: optionalKeys,
        iid_set_keys: iidKeys,
      },
      exact_subtypes: {
        declared_model: "employee",
        exact: exactValues.map((value: any) => sdkV2ModelKey(generated, value)),
        subtypes: subtypeValues.map((value: any) => sdkV2ModelKey(generated, value)),
      },
      scalar_boolean: {
        and_keys: andKeys,
        or_keys: orKeys,
        not_keys: notKeys,
        field_comparison_keys: fieldComparisonKeys,
      },
      roles: sdkV2RoleObservation(generated, membership, network),
      topology: {
        reachable: [{
          from: sdkV2Key(reachablePair[0]),
          to: sdkV2Key(reachablePair[1]),
          max_hops: 1,
        }],
        cross_join_pairs: crossPairs,
      },
      selection_shapes: selection,
      terminals,
      grouped_reducer: reducers,
      hydrated_result: sdkV2HydratedResult(generated, membership),
      scalar_domain: {
        domain: "long",
        operator: "gte",
        operand: 40,
        keys: scalarDomainKeys,
      },
      schema_function: {
        minimum: 30,
        values: functionValues,
        nested_values: nestedFunctionValues,
      },
      structured_query_diagnostic: diagnostic as Record<string, unknown>,
    },
  };
}


async function runSdkV2Journey(
  generated: any,
  database: any,
  remoteSession: any,
  requests: Buffer[],
  advertisement: Uint8Array,
  exchange: (request: Uint8Array, signal?: AbortSignal) => Promise<Uint8Array>,
  catalog: any,
  journey: any,
  proofObservations: ReadonlyMap<string, Record<string, unknown>>,
): Promise<Record<string, unknown>[]> {
  const {
    Aliases,
    Employee,
    Identifier,
    Manager,
    ManagerNote,
    Membership,
    NetworkLink,
    Nickname,
    PartyName,
    Person,
    Rank,
    Score,
    ScoreGte,
    ValBool,
    ValConstrained,
    ValDate,
    ValDatetime,
    ValDatetimeTz,
    ValDecimal,
    ValDouble,
    ValDuration,
    aggregate,
    integerInput,
    qualifyingScore,
  } = generated;
  const expected = journey.expected_observations;
  const personFromRecord = (record: any): any => {
    const fields = record.fields;
    const doubleBytes = Buffer.from(fields.val_double.bits, "hex");
    assert.equal(doubleBytes.length, 8);
    return Person.create({
      aliases: fields.aliases.map((alias: any) => Aliases.create(alias.value)),
      identifier: Identifier.create(fields.identifier.value),
      ...(fields.nickname === undefined
        ? {}
        : { nickname: Nickname.create(fields.nickname.value) }),
      score: Score.create(BigInt(fields.score.value)),
      scoreGte: ScoreGte.create(BigInt(fields.score__gte.value)),
      valBool: ValBool.create(fields.val_bool.value),
      valConstrained: ValConstrained.create(BigInt(fields.val_constrained.value)),
      valDate: ValDate.create(new Date(`${fields.val_date.value}T00:00:00Z`)),
      valDatetime: ValDatetime.create(new Date(`${fields.val_datetime.value}Z`)),
      valDatetimeTz: ValDatetimeTz.create(new Date(fields.val_datetime_tz.value)),
      valDecimal: ValDecimal.create(fields.val_decimal.value),
      valDouble: ValDouble.create(doubleBytes.readDoubleBE()),
      valDuration: ValDuration.create(fields.val_duration.value),
    });
  };
  const personInputs = journey.records.people.map(personFromRecord);
  const employeeFields = journey.records.employee.fields;
  const managerFields = journey.records.manager.fields;
  const employeeInput = Employee.create({
    identifier: Identifier.create(employeeFields.identifier.value),
    partyName: PartyName.create(employeeFields.party_name.value),
    rank: Rank.create(BigInt(employeeFields.rank.value)),
  });
  const managerInput = Manager.create({
    identifier: Identifier.create(managerFields.identifier.value),
    managerNote: ManagerNote.create(managerFields.manager_note.value),
    partyName: PartyName.create(managerFields.party_name.value),
    rank: Rank.create(BigInt(managerFields.rank.value)),
  });
  const networkFields = journey.records.network_link.fields;
  const personManager = Person.manager(database);
  const employeeManager = Employee.manager(database);
  const managerManager = Manager.manager(database);
  const membershipManager = Membership.manager(database);
  const networkManager = NetworkLink.manager(database);
  const inserted: [any, any][] = [];
  let people: readonly any[] = [];
  let employee: any;
  let manager: any;
  let membership: any;
  let network: any;
  let insertedCount = 0;
  try {
    people = personManager.insertMany(personInputs);
    assert.equal(people.length, personInputs.length);
    for (let index = 0; index < people.length; index += 1) {
      assert.equal(sdkV2Key(people[index]), sdkV2Key(personInputs[index]));
      inserted.push([personManager, people[index]]);
    }
    insertedCount = inserted.length;

    employee = employeeManager.insert(employeeInput);
    assert.equal(sdkV2Key(employee), sdkV2Key(employeeInput));
    inserted.push([employeeManager, employee]);
    insertedCount = inserted.length;

    manager = managerManager.insert(managerInput);
    assert.equal(sdkV2Key(manager), sdkV2Key(managerInput));
    inserted.push([managerManager, manager]);
    insertedCount = inserted.length;

    membership = membershipManager.insert(Membership.create({ member: people[0] }));
    inserted.push([membershipManager, membership]);
    insertedCount = inserted.length;

    network = networkManager.insert(NetworkLink.create({
      destination: people[1],
      identifier: Identifier.create(networkFields.identifier.value),
      nickname: Nickname.create(networkFields.nickname.value),
      origin: people[0],
      participant: people,
    }));
    assert.equal(sdkV2Key(network), networkFields.identifier.value);
    inserted.push([networkManager, network]);
    insertedCount = inserted.length;

    for (const [, value] of inserted) assertIid(value.iid);
    const personIids = people.map((value: any) => value.iid as string);
    const membershipIid = membership.iid as string;
    const networkIid = network.iid as string;
    const entityReadAfterCreate = personManager.getByIid(personIids[0]) !== null;
    const relationReadAfterCreate = membershipManager.getByIid(membershipIid) !== null;

    const lane = (session: any) => sdkV2QueryLane(
      generated,
      session,
      requests,
      personIids,
      membershipIid,
      networkIid,
    );
    const directSession = new generated.QuerySession(database);
    const direct = await lane(directSession).finally(() => directSession.close());
    const remote = await lane(remoteSession);
    // One program, two connections: the lanes must agree observation for
    // observation before either is reported.
    assert.deepEqual(remote.observations, direct.observations);
    assert.deepEqual(direct.observations.grouped_reducer, expected.grouped_reducer);
    assert.deepEqual(
      direct.observations.schema_function,
      {
        minimum: 30,
        values: expected.schema_function.values,
        nested_values: expected.schema_function.nested_values,
      },
    );
    assert.equal(direct.oneExchange, 0);
    assert.equal(remote.oneExchange, 1);
    const resourceLimits = await sdkV2ResourceLimits(
      generated,
      database,
      advertisement,
      exchange,
      membershipIid,
    );
    const queryResourceLifecycle = await sdkV2Lifecycle(
      generated,
      database,
      advertisement,
      exchange,
      requests,
    );

    const observed = new Map<string, Record<string, unknown>>();
    const { structured_query_diagnostic: diagnostic, ...laneObservations } =
      direct.observations;
    for (const [name, observation] of Object.entries(laneObservations)) {
      observed.set(observationKey(name, "direct_runtime"), observation);
      observed.set(
        observationKey(name, "remote_runtime"),
        remote.observations[name] as Record<string, unknown>,
      );
    }
    observed.set(observationKey("remote_one_exchange", "remote_runtime"), {
      exchange_count: remote.oneExchange,
      terminal: "one",
    });
    observed.set(observationKey("structured_query_diagnostic", "diagnostic"), diagnostic);
    observed.set(observationKey("resource_limits", "direct_runtime"), resourceLimits);
    observed.set(observationKey("resource_limits", "remote_runtime"), resourceLimits);
    observed.set(
      observationKey("query_resource_lifecycle", "lifecycle"),
      queryResourceLifecycle,
    );
    assert.equal(observed.size, 29, "sdk-v2 must measure 29 pre-cleanup live lanes");
    assert.equal(proofObservations.size, 3);
    for (const [key, observation] of proofObservations) {
      assert.equal(observed.has(key), false, `proof fragment duplicated live lane ${key}`);
      observed.set(key, observation);
    }
    assert.equal(observed.size, 32);
    let entityDeleted = false;
    let relationDeleted = false;
    for (const [owner, value] of [...inserted].slice(0, insertedCount).reverse()) {
      owner.delete(value);
      insertedCount -= 1;
      if (value === people[0]) entityDeleted = owner.getByIid(value.iid) === null;
      if (value === membership) relationDeleted = owner.getByIid(value.iid) === null;
    }
    observed.set(observationKey("entity_lifecycle", "direct_runtime"), {
      created: true,
      deleted: entityDeleted,
      key: sdkV2Key(people[0]),
      model: sdkV2Model(generated, people[0]),
      read_after_create: entityReadAfterCreate,
    });
    observed.set(observationKey("relation_lifecycle", "direct_runtime"), {
      created: true,
      deleted: relationDeleted,
      model: sdkV2Model(generated, membership),
      player_key: sdkV2Key(people[0]),
      role: "member",
    });
    assert.equal(observed.size, 34, "sdk-v2 requires 31 live and 3 proof lanes");
    assert.equal(relationReadAfterCreate, true);
    return sdkResults(catalog, journey, observed);
  } finally {
    const cleanupFailures: unknown[] = [];
    for (const [owner, value] of [...inserted].slice(0, insertedCount).reverse()) {
      try {
        owner.delete(value);
      } catch (error) {
        cleanupFailures.push(error);
      }
    }
    if (cleanupFailures.length > 0) throw cleanupFailures[0];
  }
}

function run(command: string, args: readonly string[], cwd: string): void {
  const result = spawnSync(command, args, {
    cwd,
    env: {
      ...process.env,
      TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE: REMOTE_PROFILE,
    },
    stdio: "inherit",
  });
  if (result.error !== undefined) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status ?? "unknown"}`);
  }
}

function assertIid(value: unknown): asserts value is string {
  assert.equal(typeof value, "string");
  assert.notEqual(value, "");
}

async function freePort(): Promise<number> {
  return new Promise((resolvePort, reject) => {
    const server = net.createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        server.close(() => reject(new TypeError("failed to reserve a remote query port")));
        return;
      }
      server.close((error) => error === undefined ? resolvePort(address.port) : reject(error));
    });
  });
}

async function waitForPort(port: number, child: ChildProcess, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`generated remote query server exited with code ${child.exitCode}`);
    }
    const connected = await new Promise<boolean>((resolveConnection) => {
      const socket = net.createConnection({ host: "127.0.0.1", port });
      socket.once("connect", () => {
        socket.destroy();
        resolveConnection(true);
      });
      socket.once("error", () => resolveConnection(false));
    });
    if (connected) return;
    await new Promise((resolveWait) => setTimeout(resolveWait, 100));
  }
  throw new Error("timed out waiting for generated remote query server");
}

test("sdk report server-version gate is exact", () => {
  assert.doesNotThrow(() => requireSdkServerVersion("3.12.3"));
  for (const detected of [null, "3.11.5", "3.12.0", "3.12.2", "3.13.0"] as const) {
    assert.throws(
      () => requireSdkServerVersion(detected),
    /actual detected TypeDB server version 3\.12\.3/,
    );
  }
});

test(`generated package round-trips exact models on TypeDB ${TYPEDB_VERSION}`, { timeout: 360_000 }, async () => {
  const suppliedStage = process.env.TYPE_BRIDGE_GENERATED_NODE_STAGE;
  const stage = suppliedStage === undefined
    ? await mkdtemp(join(tmpdir(), "type-bridge-node-projection-"))
    : resolve(suppliedStage);
  const ownsStage = suppliedStage === undefined;
  const generatedDirectory = resolve(stage, "generated_v2");
  const foreignDirectory = resolve(stage, "generated_foreign");
  let database;
  let failure: unknown;
  let preparedSdkReport: Record<string, unknown> | undefined;
  let preparedSdkV2Report: Record<string, unknown> | undefined;
  const sdkReportPath = process.env.TYPE_BRIDGE_SDK_REPORT;
  const sdkV2ReportPath = process.env.TYPE_BRIDGE_SDK_REPORT_V2;
  if (
    sdkReportPath !== undefined
    && sdkV2ReportPath !== undefined
    && sdkReportPath === sdkV2ReportPath
  ) {
    throw new Error("sdk-v1 and sdk-v2 reports require distinct paths");
  }

  try {
    if (ownsStage) {
      run(
        resolve(ROOT, "scripts/ci/prepare_generated_live_fixture.sh"),
        [
          "node",
          stage,
        ],
        ROOT,
      );
    }

    const packageScope = resolve(stage, "node_modules/@type-bridge");
    await mkdir(packageScope, { recursive: true });
    await rm(resolve(packageScope, "node"), { recursive: true, force: true });
    await symlink(
      NODE_RUNTIME_PACKAGE,
      resolve(packageScope, "node"),
      process.platform === "win32" ? "junction" : "dir",
    );
    if (ownsStage) {
      run(
        resolve(NODE_SOURCE_PACKAGE, "node_modules/.bin/tsc"),
        ["--project", resolve(generatedDirectory, "tsconfig.json")],
        generatedDirectory,
      );
      run(
        resolve(NODE_SOURCE_PACKAGE, "node_modules/.bin/tsc"),
        ["--project", resolve(foreignDirectory, "tsconfig.json")],
        foreignDirectory,
      );
    }

    const generated = await import(pathToFileURL(resolve(generatedDirectory, "dist/index.js")).href);
    const foreign = await import(pathToFileURL(resolve(foreignDirectory, "dist/index.js")).href);
    const {
      Actor,
      Aliases,
      Container,
      Counter,
      CounterValue,
      Employee,
      Employment,
      Event,
      FooBar,
      Identifier,
      Interaction,
      Manager,
      ManagerNote,
      Membership,
      NetworkLink,
      Nickname,
      Party,
      PartyName,
      Person,
      PlainActivity,
      QuerySession,
      ExchangeConnection,
      QueryExecutionResourceLimits,
      QueryLimits,
      connectProxy,
      PROJECTION_FINGERPRINT_JSON,
      Rank,
      Robot,
      RobotId,
      RUNTIME_PROJECTION_JSON,
      Score,
      ScoreGte,
      SEMANTIC_SCHEMA_FINGERPRINT_JSON,
      ValBool,
      ValConstrained,
      ValDate,
      ValDatetime,
      ValDatetimeTz,
      ValDecimal,
      ValDouble,
      ValDuration,
      aggregate,
    } = generated;

    const semanticFingerprint = JSON.parse(SEMANTIC_SCHEMA_FINGERPRINT_JSON);
    const projectionFingerprint = JSON.parse(PROJECTION_FINGERPRINT_JSON);
    const runtimeProjection = JSON.parse(RUNTIME_PROJECTION_JSON);
    assert.equal(semanticFingerprint.semantic_profile, REMOTE_PROFILE);
    assert.deepEqual(runtimeProjection.semantic_fingerprint, semanticFingerprint);
    assert.deepEqual(runtimeProjection.projection_fingerprint, projectionFingerprint);

    let sdkCatalogRaw: Buffer | undefined;
    let sdkCatalog: any;
    let sdkJourneyRaw: Buffer | undefined;
    let sdkJourney: any;
    let sdkV2CatalogRaw: Buffer | undefined;
    let sdkV2Catalog: any;
    let sdkV2JourneyRaw: Buffer | undefined;
    let sdkV2Journey: any;
    let sdkV2ProofObservations:
      | ReadonlyMap<string, Record<string, unknown>>
      | undefined;
    if (sdkReportPath !== undefined) {
      if (REMOTE_PROFILE !== "typedb-3.12.1/v1") {
        throw new Error("sdk reports may only be emitted for typedb-3.12.1/v1");
      }
      await validateSdkReportPath(sdkReportPath);
      requireSdkServerVersion(
        await detectTypeDBServerVersion(TYPEDB_ADDRESS, TYPEDB_HTTP_PORT),
      );
      sdkCatalogRaw = await readFile(SDK_CATALOG);
      sdkCatalog = JSON.parse(sdkCatalogRaw.toString("utf8"));
      sdkJourneyRaw = await readFile(SDK_JOURNEY);
      sdkJourney = JSON.parse(sdkJourneyRaw.toString("utf8"));
      assert.equal(sdkJourney.format, "typebridge.sdk-journey/v1");
      assert.equal(sdkJourney.fixture_id, sdkCatalog.fixture.id);
      assert.equal(sdkJourney.version, sdkCatalog.fixture.version);
    }
    if (sdkV2ReportPath !== undefined) {
      if (REMOTE_PROFILE !== "typedb-3.12.1/v1") {
        throw new Error("sdk-v2 reports may only be emitted for typedb-3.12.1/v1");
      }
      await validateSdkReportPath(sdkV2ReportPath);
      requireSdkServerVersion(
        await detectTypeDBServerVersion(TYPEDB_ADDRESS, TYPEDB_HTTP_PORT),
      );
      sdkV2CatalogRaw = await readFile(SDK_V2_CATALOG);
      sdkV2Catalog = JSON.parse(sdkV2CatalogRaw.toString("utf8"));
      sdkV2JourneyRaw = await readFile(SDK_V2_JOURNEY);
      sdkV2Journey = JSON.parse(sdkV2JourneyRaw.toString("utf8"));
      sdkV2ProofObservations = loadSdkV2ProofObservations();
      assert.equal(sdkV2Journey.format, "typebridge.sdk-journey/v2");
      assert.equal(sdkV2Journey.fixture_id, sdkV2Catalog.fixture.id);
      assert.equal(sdkV2Journey.version, sdkV2Catalog.fixture.version);
    }
    database = connectIntegration();
    database.resetDatabase();
    defineSchema(database, await readFile(PROVIDER_SCHEMA, "utf8"));

    const personManager = Person.manager(database);
    const generatedPerson = (identifierValue: string, scoreValue: bigint) => Person.create({
      identifier: Identifier.create(identifierValue),
      score: Score.create(scoreValue),
      valBool: ValBool.create(true),
      valConstrained: ValConstrained.create(20n),
      valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
      valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
      valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
      valDecimal: ValDecimal.create("3.5"),
      valDouble: ValDouble.create(3.5),
      valDuration: ValDuration.create("PT3S"),
    });
    const generatedPersonWithOwnerships = (
      identifierValue: string,
      scoreValue: bigint,
      nicknameValue: string | null,
      aliasValues: readonly string[],
    ) => Person.create({
      aliases: aliasValues.map((value) => Aliases.create(value)),
      identifier: Identifier.create(identifierValue),
      ...(nicknameValue === null ? {} : { nickname: Nickname.create(nicknameValue) }),
      score: Score.create(scoreValue),
      fooBar: FooBar.create(7n),
      scoreGte: ScoreGte.create(8n),
      valBool: ValBool.create(true),
      valConstrained: ValConstrained.create(20n),
      valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
      valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
      valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
      valDecimal: ValDecimal.create("3.5"),
      valDouble: ValDouble.create(3.5),
      valDuration: ValDuration.create("PT3S"),
    });
    const personInput = Person.create({
      identifier: Identifier.create("person-1"),
      nickname: Nickname.create("alice"),
      aliases: [Aliases.create("alpha"), Aliases.create("beta")],
      score: Score.create(3n),
      fooBar: FooBar.create(7n),
      scoreGte: ScoreGte.create(8n),
      valBool: ValBool.create(true),
      valConstrained: ValConstrained.create(20n),
      valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
      valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
      valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
      valDecimal: ValDecimal.create("3.5"),
      valDouble: ValDouble.create(3.5),
      valDuration: ValDuration.create("PT3S"),
    });
    assert.deepEqual(personManager.insertMany([]), []);
    assert.deepEqual(personManager.putMany([]), []);
    const insertedPerson = personManager.insert(personInput);
    assertIid(insertedPerson.iid);
    assert.equal(insertedPerson.__typebridgeModel, Person.typeKey);
    assert.equal(insertedPerson.__typebridgeForm, "complete");
    const putPerson = personManager.put(Person.create({
      identifier: Identifier.create("person-1"),
      nickname: Nickname.create("alice"),
      aliases: [Aliases.create("alpha"), Aliases.create("beta")],
      score: Score.create(3n),
      fooBar: FooBar.create(7n),
      scoreGte: ScoreGte.create(8n),
      valBool: ValBool.create(true),
      valConstrained: ValConstrained.create(20n),
      valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
      valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
      valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
      valDecimal: ValDecimal.create("3.5"),
      valDouble: ValDouble.create(3.5),
      valDuration: ValDuration.create("PT3S"),
    }));
    assert.equal(putPerson.iid, insertedPerson.iid);
    assert.deepEqual(
      personManager.filter({ score__gte: Score.create(3n) }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      personManager.filter({ score__in: [Score.create(2n), Score.create(3n)] })
        .all()
        .map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      personManager.filter({ aliases__isnull: false }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      personManager.filter({ iid__in: [insertedPerson.iid] }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    const filteredManager = personManager.filter({ score__gte: Score.create(3n) });
    assert.throws(
      () => personManager.filter({ identifier: undefined } as any),
      /filter values must be finite JSON scalars/i,
    );
    assert.throws(
      () => personManager.filter({ iid__in: [insertedPerson.iid, undefined] } as any),
      /filter values must be finite JSON scalars/i,
    );
    assert.equal(filteredManager.first()?.iid, insertedPerson.iid);
    assert.equal(filteredManager.count(), 1n);
    assert.equal(filteredManager.exists(), true);
    const missingManager = personManager.filter({ score__gt: Score.create(3n) });
    assert.equal(missingManager.first(), null);
    assert.equal(missingManager.count(), 0n);
    assert.equal(missingManager.exists(), false);
    assert.deepEqual(
      personManager.filter({ scoreGte__eq: ScoreGte.create(8n) }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );
    assertUnknownEnumValue(
      () => personManager.filter({ score__iexact: Score.create(3n) } as never),
      "manager_lookup",
    );
    assert.deepEqual(
      personManager.filter({ fooBar: FooBar.create(7n) }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );

    const storedPerson = personManager.getByIid(insertedPerson.iid);
    assert.notEqual(storedPerson, null);
    assert.equal(storedPerson.iid, insertedPerson.iid);
    assert.equal(storedPerson.__typebridgeModel, Person.typeKey);
    assert.equal(storedPerson.__typebridgeForm, "complete");
    assert.equal(storedPerson.identifier.__typebridgeModel, Identifier.typeKey);
    assert.equal(storedPerson.identifier.value, "person-1");
    assert.equal(storedPerson.nickname.__typebridgeModel, Nickname.typeKey);
    assert.equal(storedPerson.nickname.value, "alice");
    assert.deepEqual(
      new Set(storedPerson.aliases.map((alias: { readonly value: string }) => alias.value)),
      new Set(["alpha", "beta"]),
    );
    assert.ok(storedPerson.aliases.every(
      (alias: { readonly __typebridgeModel: string }) => alias.__typebridgeModel === Aliases.typeKey,
    ));

    const updatedPerson = personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-1", 3n, "ada", ["alpha", "beta"]),
    );
    assert.equal(updatedPerson.iid, insertedPerson.iid);
    const updatedStoredPerson = personManager.getByIid(insertedPerson.iid);
    assert.notEqual(updatedStoredPerson, null);
    assert.equal(updatedStoredPerson.nickname.value, "ada");
    const specialAlias = "quote'\"\\line\nunicode-λ";
    personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-1", 3n, null, [specialAlias]),
    );
    const replacedOwnerships = personManager.getByIid(insertedPerson.iid);
    assert.notEqual(replacedOwnerships, null);
    assert.equal(replacedOwnerships.nickname, null);
    assert.deepEqual(replacedOwnerships.aliases.map((alias) => alias.value), [specialAlias]);
    personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-1", 3n, null, []),
    );
    const clearedOwnerships = personManager.getByIid(insertedPerson.iid);
    assert.notEqual(clearedOwnerships, null);
    assert.equal(clearedOwnerships.nickname, null);
    assert.deepEqual(clearedOwnerships.aliases, []);
    personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-1", 3n, "ada", ["alpha", "beta"]),
    );
    const keyPreserved = personManager.update(
      insertedPerson.iid,
      generatedPersonWithOwnerships("person-key-mutated", 3n, "ada", ["alpha", "beta"]),
    );
    assert.equal(keyPreserved.identifier.value, "person-1");
    assert.equal(personManager.getByIid(insertedPerson.iid)?.identifier.value, "person-1");

    const stalePerson = personManager.insert(generatedPerson("person-stale-update", 4n));
    assertIid(stalePerson.iid);
    personManager.delete(stalePerson.iid);
    assert.throws(
      () => personManager.update(stalePerson.iid, generatedPerson("person-stale-update", 5n)),
      /not found after update/i,
    );

    const batchPeople = [
      generatedPerson("person-2", 5n),
      generatedPerson("person-3", 7n),
    ] as const;
    const insertedBatch = personManager.insertMany(batchPeople);
    assert.ok(insertedBatch.every((candidate) => candidate.iid !== null));
    const batchIids = insertedBatch.map((candidate) => candidate.iid);
    assert.deepEqual(
      personManager.filter({ aliases__isnull: true }).all().map((candidate) => candidate.iid),
      batchIids,
    );
    assert.deepEqual(personManager.putMany(insertedBatch).map((candidate) => candidate.iid), batchIids);

    const insertedEmployee = Employee.manager(database).insert(Employee.create({
      identifier: Identifier.create("employee-1"),
      partyName: PartyName.create("employee"),
      rank: Rank.create(1n),
    }));
    const insertedManager = Manager.manager(database).insert(Manager.create({
      identifier: Identifier.create("manager-1"),
      managerNote: ManagerNote.create("lead"),
      partyName: PartyName.create("manager"),
      rank: Rank.create(2n),
    }));
    assertIid(insertedEmployee.iid);
    assertIid(insertedManager.iid);

    const writeTransaction = database.transaction("write");
    const transactionPerson = generatedPerson("person-4", 9n);
    let transactionPersonIid: string | null = null;
    try {
      const transactionManager = Person.manager(writeTransaction);
      transactionPersonIid = transactionManager.insert(transactionPerson).iid;
      assert.notEqual(transactionPersonIid, null);
      writeTransaction.commit();
    } catch (error) {
      writeTransaction.rollback();
      throw error;
    }
    assertIid(transactionPersonIid);

    const rollbackTransaction = database.transaction("write");
    const rollbackPerson = generatedPerson("person-rollback", 11n);
    const rollbackManager = Person.manager(rollbackTransaction);
    const rollbackIid = rollbackManager.insert(rollbackPerson).iid;
    assertIid(rollbackIid);
    assert.equal(rollbackManager.getByIid(rollbackIid)?.iid, rollbackIid);
    rollbackTransaction.rollback();
    assert.equal(
      personManager.filter({ identifier: Identifier.create("person-rollback") }).exists(),
      false,
    );

    const readTransaction = database.transaction("read");
    try {
      const transactionSession = new QuerySession(readTransaction);
      const transactionPersonVar = transactionSession.var(Person);
      const transactionQuery = transactionSession
        .query(transactionPersonVar)
        .where(
          transactionPersonVar.field(Person.identifier).eq(Identifier.create("person-4")),
      );
      assert.equal(await transactionQuery.countBy(transactionPersonVar), 1n);
      assert.equal((await transactionQuery.first())?.iid, transactionPersonIid);
    } finally {
      readTransaction.close();
    }
    assert.equal(personManager.count(), 4n);

    const membershipManager = Membership.manager(database);
    assert.deepEqual(membershipManager.insertMany([]), []);
    assert.deepEqual(membershipManager.putMany([]), []);
    const insertedMembership = membershipManager.insert(Membership.create({ member: updatedPerson }));
    assertIid(insertedMembership.iid);
    const storedMembership = membershipManager.getByIid(insertedMembership.iid);
    assert.notEqual(storedMembership, null);
    assert.equal(storedMembership.__typebridgeModel, Membership.typeKey);
    assert.equal(storedMembership.member.__typebridgeModel, Person.typeKey);
    assert.equal(storedMembership.member.iid, insertedPerson.iid);

    const employmentManager = Employment.manager(database);
    const insertedEmployment = employmentManager.insert(Employment.create({ employee: updatedPerson }));
    assertIid(insertedEmployment.iid);
    const storedEmployment = employmentManager.getByIid(insertedEmployment.iid);
    assert.notEqual(storedEmployment, null);
    assert.equal(storedEmployment.__typebridgeModel, Employment.typeKey);
    assert.equal(storedEmployment.employee.__typebridgeModel, Person.typeKey);
    assert.equal(storedEmployment.employee.iid, insertedPerson.iid);
    assert.equal(Object.hasOwn(storedEmployment, "member"), false);
    assert.equal(membershipManager.getByIid(insertedEmployment.iid), null);
    assert.ok(membershipManager.all().some((model: { readonly iid: string }) => model.iid === insertedMembership.iid));
    assert.ok(membershipManager.all().every((model: { readonly iid: string }) => model.iid !== insertedEmployment.iid));

    const eventManager = Event.manager(database);
    const insertedEvent = eventManager.insert(Event.create({ subject: updatedPerson }));
    assertIid(insertedEvent.iid);
    const storedEvent = eventManager.getByIid(insertedEvent.iid);
    assert.notEqual(storedEvent, null);
    assert.equal(storedEvent.__typebridgeModel, Event.typeKey);
    assert.equal(storedEvent.subject.__typebridgeModel, Person.typeKey);
    assert.equal(storedEvent.subject.iid, insertedPerson.iid);

    const containerManager = Container.manager(database);
    const insertedContainer = containerManager.insert(Container.create({
      item: [Event.reference(insertedEvent.iid, {})],
    }));
    assertIid(insertedContainer.iid);
    const storedContainer = containerManager.getByIid(insertedContainer.iid);
    assert.notEqual(storedContainer, null);
    assert.equal(storedContainer.__typebridgeModel, Container.typeKey);
    assert.equal(storedContainer.iid, insertedContainer.iid);
    assert.equal(storedContainer.item.length, 1);
    const storedEventReference = storedContainer.item[0];
    assert.equal(storedEventReference.__typebridgeModel, Event.typeKey);
    assert.equal(storedEventReference.__typebridgeForm, "reference");
    assert.equal(storedEventReference.iid, insertedEvent.iid);
    assert.equal(Object.hasOwn(storedEventReference, "subject"), false);

    const networkManager = NetworkLink.manager(database);
    const insertedNetwork = networkManager.insert(NetworkLink.create({
      destination: insertedBatch[0],
      identifier: Identifier.create("network-1"),
      nickname: Nickname.create("primary"),
      origin: insertedPerson,
      participant: [insertedPerson, insertedBatch[0]],
    }));
    assertIid(insertedNetwork.iid);
    const putNetwork = networkManager.put(NetworkLink.create({
      destination: insertedBatch[0],
      identifier: Identifier.create("network-1"),
      nickname: Nickname.create("primary"),
      origin: insertedPerson,
      participant: [insertedPerson, insertedBatch[0]],
    }));
    assert.equal(putNetwork.iid, insertedNetwork.iid);
    const updatedNetwork = networkManager.update(insertedNetwork.iid, NetworkLink.create({
      destination: insertedBatch[0],
      identifier: Identifier.create("network-1"),
      nickname: Nickname.create("updated"),
      origin: insertedPerson,
      participant: [insertedPerson, insertedBatch[0]],
    }));
    assert.equal(updatedNetwork.iid, insertedNetwork.iid);
    assert.equal(networkManager.getByIid(insertedNetwork.iid)?.nickname?.value, "updated");
    const filteredNetworks = networkManager.filter({ identifier: Identifier.create("network-1") });
    assert.deepEqual(filteredNetworks.all().map((candidate) => candidate.iid), [insertedNetwork.iid]);
    assert.equal(filteredNetworks.first()?.iid, insertedNetwork.iid);
    assert.equal(filteredNetworks.count(), 1n);
    assert.equal(filteredNetworks.exists(), true);

    const querySession = new QuerySession(database);
    assert.throws(
      () => querySession.var(foreign.Person),
      /exact package model token/i,
    );

    const scalarDomainVariable = querySession.exact(Person);
    const scalarDomainPerson = await querySession
      .query(scalarDomainVariable)
      .where(
        scalarDomainVariable.field(Person.identifier).eq(Identifier.create("person-1")),
        scalarDomainVariable.field(Person.valBool).eq(ValBool.create(true)),
        scalarDomainVariable.field(Person.valDouble).gte(ValDouble.create(3.5)),
        scalarDomainVariable.field(Person.valDecimal).gte(ValDecimal.create("3.5")),
        scalarDomainVariable
          .field(Person.valDate)
          .gte(ValDate.create(new Date("2026-07-29T00:00:00Z"))),
        scalarDomainVariable
          .field(Person.valDatetime)
          .gte(ValDatetime.create(new Date("2026-07-29T12:34:56Z"))),
        scalarDomainVariable
          .field(Person.valDatetimeTz)
          .gte(ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z"))),
        scalarDomainVariable.field(Person.valDuration).eq(ValDuration.create("PT3S")),
      )
      .one();
    assert.equal(scalarDomainPerson.iid, insertedPerson.iid);
    assert.deepEqual(
      personManager.filter({
        identifier: Identifier.create("person-1"),
        valBool: ValBool.create(true),
        valDate__gte: ValDate.create(new Date("2026-07-29T00:00:00Z")),
        valDatetime__gte: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
        valDatetimeTz__gte: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
        valDecimal__gte: ValDecimal.create("3.5"),
        valDouble__gte: ValDouble.create(3.5),
        valDuration: ValDuration.create("PT3S"),
      }).all().map((candidate) => candidate.iid),
      [insertedPerson.iid],
    );

    const counterManager = Counter.manager(database);
    const detachedCounter = Counter.create({ counterValue: CounterValue.create(42n) });
    assert.throws(
      () => counterManager.delete(detachedCounter),
      /attached TypeDB IID/,
    );
    const insertedCounter = counterManager.insert(detachedCounter);
    assertIid(insertedCounter.iid);
    const storedCounter = counterManager.getByIid(insertedCounter.iid);
    assert.notEqual(storedCounter, null);
    assert.equal(storedCounter.__typebridgeModel, Counter.typeKey);
    assert.equal(storedCounter.counterValue.__typebridgeModel, CounterValue.typeKey);
    assert.equal(storedCounter.counterValue.value, 42n);
    const counterVariable = querySession.exact(Counter);
    const counterQuery = querySession
      .query(counterVariable)
      .where(counterVariable.field(Counter.counterValue).eq(CounterValue.create(42n)));
    assert.equal((await counterQuery.one()).iid, insertedCounter.iid);
    await assert.rejects(
      counterQuery.rows({ limit: 2n }),
      (boundedCounterError: any) => {
        assert.match(
          String(boundedCounterError),
          /The typed query plan does not satisfy the generated query contract/,
        );
        assert.equal(boundedCounterError.queryCategory, "invalid_plan");
        assert.equal(boundedCounterError.code, "missing_stable_unique_key");
        return true;
      },
    );
    counterManager.delete(insertedCounter);
    assert.equal(counterManager.getByIid(insertedCounter.iid), null);
    assert.equal(counterManager.count(), 0n);

    const plainActivityManager = PlainActivity.manager(database);
    const insertedPlainActivity = plainActivityManager.insert(
      PlainActivity.create({ participant: insertedPerson }),
    );
    assertIid(insertedPlainActivity.iid);
    const storedPlainActivity = plainActivityManager.getByIid(insertedPlainActivity.iid);
    assert.notEqual(storedPlainActivity, null);
    assert.equal(storedPlainActivity.__typebridgeModel, PlainActivity.typeKey);
    assert.equal(storedPlainActivity.participant.__typebridgeModel, Person.typeKey);
    assert.equal(storedPlainActivity.participant.iid, insertedPerson.iid);
    const plainActivityVariable = querySession.exact(PlainActivity);
    const plainParticipantVariable = querySession.exact(Person);
    const [queriedPlainActivity, queriedPlainParticipant] = await querySession
      .query(plainActivityVariable, plainParticipantVariable)
      .where(
        plainActivityVariable.role(PlainActivity.participant).connects(plainParticipantVariable),
        plainParticipantVariable
          .field(Person.identifier)
          .eq(Identifier.create("person-1")),
      )
      .one();
    assert.equal(queriedPlainActivity.iid, insertedPlainActivity.iid);
    assert.equal(queriedPlainParticipant.iid, insertedPerson.iid);
    plainActivityManager.delete(insertedPlainActivity);
    assert.equal(plainActivityManager.getByIid(insertedPlainActivity.iid), null);

    const robotManager = Robot.manager(database);
    const integerKeyValues = [-42n, 1n, 100n, 9999n] as const;
    const insertedRobots = robotManager.insertMany(integerKeyValues.map((value, index) => Robot.create({
      ...(value === -42n ? { nickname: Nickname.create("actor-robot") } : {}),
      robotId: RobotId.create(value),
      valConstrained: ValConstrained.create(BigInt(index + 1)),
    })));
    assert.equal(robotManager.count(), BigInt(integerKeyValues.length));
    for (const [index, value] of integerKeyValues.entries()) {
      const integerKeyMatch = robotManager.filter({ robotId: RobotId.create(value) });
      assert.equal(integerKeyMatch.count(), 1n);
      assert.equal(integerKeyMatch.first()?.iid, insertedRobots[index]?.iid);
      assert.equal(integerKeyMatch.first()?.robotId.value, value);
    }
    assert.deepEqual(
      new Set(
        robotManager
          .filter({ robotId__in: [RobotId.create(-42n), RobotId.create(9999n)] })
          .all()
          .map((candidate) => candidate.robotId.value),
      ),
      new Set([-42n, 9999n]),
    );

    const insertedRobot = insertedRobots[0];
    assert.notEqual(insertedRobot, undefined);
    assertIid(insertedRobot.iid);
    const robotMembership = membershipManager.insert(Membership.create({ member: insertedRobot }));
    assertIid(robotMembership.iid);
    const storedRobotMembership = membershipManager.getByIid(robotMembership.iid);
    assert.notEqual(storedRobotMembership, null);
    assert.equal(storedRobotMembership.member.__typebridgeModel, Robot.typeKey);
    assert.equal(storedRobotMembership.member.iid, insertedRobot.iid);
    assert.equal(storedRobotMembership.member.robotId.value, -42n);

    const interactionManager = Interaction.manager(database);
    const insertedInteractions = interactionManager.insertMany([
      Interaction.create({
        actor: insertedRobot,
        identifier: Identifier.create("interaction-robot"),
        nickname: Nickname.create("assist"),
        target: updatedPerson,
      }),
      Interaction.create({
        actor: updatedPerson,
        identifier: Identifier.create("interaction-person"),
        nickname: Nickname.create("read"),
        target: insertedBatch[0],
      }),
    ]);
    const robotInteraction = insertedInteractions[0];
    const personInteraction = insertedInteractions[1];
    assert.notEqual(robotInteraction, undefined);
    assert.notEqual(personInteraction, undefined);
    assertIid(robotInteraction.iid);
    assertIid(personInteraction.iid);

    const actorVariable = querySession.subtypes(Actor);
    const interactionVariable = querySession.exact(Interaction);
    const polymorphicActorRows = await querySession
      .query(interactionVariable)
      .match(actorVariable)
      .where(
        interactionVariable.role(Interaction.actor).connects(actorVariable),
        actorVariable.field(Actor.nickname).contains(Nickname.create("a")),
      )
      .rows({
        limit: 10n,
        orderBy: [interactionVariable.field(Interaction.identifier).asc()],
      });
    assert.deepEqual(
      new Set(polymorphicActorRows.map((relation) => relation.actor?.__typebridgeModel)),
      new Set([Person.typeKey, Robot.typeKey]),
    );
    assert.deepEqual(
      new Set(polymorphicActorRows.map((relation) => relation.iid)),
      new Set([robotInteraction.iid, personInteraction.iid]),
    );

    const robotVariable = querySession.exact(Robot);
    const targetVariable = querySession.exact(Person);
    const [queriedRobotInteraction, queriedRobot, queriedTarget] = await querySession
      .query(interactionVariable, robotVariable, targetVariable)
      .where(
        interactionVariable.role(Interaction.actor).connects(robotVariable),
        interactionVariable.role(Interaction.target).connects(targetVariable),
        interactionVariable.field(Interaction.nickname).eq(Nickname.create("assist")),
        robotVariable.field(Robot.robotId).eq(RobotId.create(-42n)),
        robotVariable.field(Robot.valConstrained).lt(ValConstrained.create(10n)),
        targetVariable.field(Person.identifier).eq(Identifier.create("person-1")),
      )
      .one();
    assert.equal(queriedRobotInteraction.iid, robotInteraction.iid);
    assert.equal(queriedRobot.__typebridgeModel, Robot.typeKey);
    assert.equal(queriedRobot.iid, insertedRobot.iid);
    assert.equal(queriedTarget.iid, insertedPerson.iid);

    const personActorVariable = querySession.exact(Person);
    const [queriedPersonInteraction, queriedPersonActor] = await querySession
      .query(interactionVariable, personActorVariable)
      .where(
        interactionVariable.role(Interaction.actor).connects(personActorVariable),
        interactionVariable.field(Interaction.nickname).eq(Nickname.create("read")),
        personActorVariable.field(Person.score).gte(Score.create(3n)),
      )
      .one();
    assert.equal(queriedPersonInteraction.iid, personInteraction.iid);
    assert.equal(queriedPersonActor.iid, insertedPerson.iid);
    interactionManager.delete(queriedPersonInteraction);
    assert.equal(interactionManager.getByIid(personInteraction.iid), null);

    membershipManager.delete(robotMembership);
    assert.equal(membershipManager.getByIid(robotMembership.iid), null);
    robotManager.delete(insertedRobot);
    assert.equal(robotManager.getByIid(insertedRobot.iid), null);
    const survivingInteraction = interactionManager.getByIid(robotInteraction.iid);
    assert.notEqual(survivingInteraction, null);
    assert.equal(survivingInteraction.actor, null);
    assert.equal(survivingInteraction.target.__typebridgeModel, Person.typeKey);
    assert.equal(survivingInteraction.target.iid, insertedPerson.iid);
    interactionManager.delete(survivingInteraction);
    assert.equal(interactionManager.getByIid(robotInteraction.iid), null);
    for (const remainingRobot of insertedRobots.slice(1)) {
      robotManager.delete(remainingRobot);
    }
    assert.equal(robotManager.count(), 0n);

    // One query program, run on a direct, an exchange, and a proxy
    // connection. `terminal` wraps every terminal call so the exchange lane
    // can prove each terminal is exactly one exchange.
    const iids = (values: readonly { readonly iid: string | null }[]): (string | null)[] =>
      values.map((value) => value.iid);
    const queryProgram = async (
      session: any,
      terminal: (run: () => Promise<any>) => Promise<any>,
    ): Promise<Record<string, unknown>> => {
      const personVariable = session.var(Person);
      const collectedPersonVariable = session.var(Person);
      const eventVariable = session.var(Event);
      const identifierOrder = personVariable.field(Person.identifier).asc();
      const personPredicate = personVariable
        .field(Person.identifier)
        .eq(Identifier.create("person-1"));
      const eventPredicate = eventVariable.role(Event.subject).connects(personVariable);
      const sameIdentifierPredicate = collectedPersonVariable
        .field(Person.identifier)
        .eqField(personVariable.field(Person.identifier));
      const pairQuery = session
        .query(personVariable, eventVariable)
        .where(personPredicate, eventPredicate);
      const personQuery = session.query(personVariable).where(personPredicate);
      const ordered = { limit: 10n, orderBy: [identifierOrder] };

      const [onePerson, oneEvent] = await terminal(() => pairQuery.one());
      const rows = await terminal(() => personQuery.rows(ordered));
      const first = await terminal(() => personQuery.first({ orderBy: [identifierOrder] }));
      const aliasesPresent = await terminal(() => session
        .query(personVariable)
        .where(personVariable.field(Person.aliases).isPresent())
        .rows(ordered));
      const aliasesMissing = await terminal(() => session
        .query(personVariable)
        .where(personVariable.field(Person.aliases).isMissing())
        .rows(ordered));
      const byIid = await terminal(() => session
        .query(personVariable)
        .where(personVariable.iid(insertedPerson.iid))
        .one());
      const byIidSet = await terminal(() => session
        .query(personVariable)
        .where(personVariable.iidIn([insertedPerson.iid, insertedBatch[0].iid]))
        .rows(ordered));
      const offsetRows = await terminal(() => session
        .query(personVariable)
        .rows({ limit: 2n, offset: 1n, orderBy: [identifierOrder] }));
      const identifierField = personVariable.field(Person.identifier);
      const identifierValue = Identifier.create("person-1");
      const generatedExpression = identifierField.startsWith(Identifier.create("person-"))
        .and(identifierField.contains(Identifier.create("son-")))
        .and(identifierField.endsWith(Identifier.create("-1")))
        .and(identifierField.regex(Identifier.create("^person-1$")))
        .and(identifierField.ne(identifierValue).not())
        .and(
          identifierField.eq(identifierValue)
            .or(identifierField.eq(Identifier.create("does-not-exist"))),
        );
      const byExpression = await terminal(() =>
        session.query(personVariable).where(generatedExpression).one());
      const crossLeft = session.exact(Person);
      const crossRight = session.exact(Person);
      const crossPair = await terminal(() => session
        .query(crossLeft, crossRight)
        .allowCrossJoin(crossLeft, crossRight)
        .where(
          crossLeft.field(Person.identifier).eq(Identifier.create("person-1")),
          crossRight.field(Person.identifier).eq(Identifier.create("person-2")),
        )
        .one());

      const partyVariable = session.subtypes(Party);
      const partyRows = await terminal(() => session.query(partyVariable).rows({
        limit: 10n,
        orderBy: [partyVariable.field(Party.identifier).asc()],
      }));
      const membershipSubtypeVariable = session.subtypes(Membership);
      const membershipFamily = session.query(membershipSubtypeVariable);
      const membershipCount = await terminal(() =>
        membershipFamily.countBy(membershipSubtypeVariable));
      const baseRelation = await terminal(() => membershipFamily
        .where(membershipSubtypeVariable.iid(insertedMembership.iid))
        .one());
      const subtypeRelation = await terminal(() => membershipFamily
        .where(membershipSubtypeVariable.iid(insertedEmployment.iid))
        .one());

      const reachableSource = session.exact(Person);
      const reachableTarget = session.exact(Person);
      const reachable = session.reachable(
        reachableSource,
        reachableTarget,
        NetworkLink,
        NetworkLink.origin,
        NetworkLink.destination,
        { minDepth: 1, maxDepth: 1 },
      );
      const reachablePair = await terminal(() => session
        .query(reachableSource, reachableTarget)
        .where(
          reachable,
          reachableSource.field(Person.identifier).eq(Identifier.create("person-1")),
          reachableTarget.field(Person.identifier).eq(Identifier.create("person-2")),
        )
        .one());
      const networkVariable = session.exact(NetworkLink);
      const network = await terminal(() => session
        .query(networkVariable)
        .where(
          networkVariable.iid(insertedNetwork.iid),
          networkVariable.field(NetworkLink.nickname).isPresent(),
        )
        .one());

      const allPeopleQuery = session.query(personVariable);
      const allFirst = await terminal(() =>
        allPeopleQuery.first({ orderBy: [identifierOrder] }));
      const scoreField = personVariable.field(Person.score);
      const reducers = await terminal(() => allPeopleQuery.aggregate(personVariable, [
        aggregate.count(),
        aggregate.sum(scoreField),
        aggregate.min(scoreField),
        aggregate.max(scoreField),
        aggregate.mean(scoreField),
        aggregate.median(scoreField),
        aggregate.std(scoreField),
      ] as const));
      const fieldGrouped = await terminal(() => allPeopleQuery
        .groupBy(personVariable, personVariable.field(Person.valBool))
        .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const));
      const tupleGrouped = await terminal(() => allPeopleQuery
        .groupBy(personVariable, personVariable.field(Person.valBool), scoreField)
        .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const));
      await assert.rejects(
        async () => allPeopleQuery.aggregate(personVariable, [
          aggregate.sum(personVariable.field(Person.identifier)),
        ] as const),
        /long|double|numeric|reduc/i,
      );
      const eventGrouped = await terminal(() => pairQuery
        .groupBy(personVariable, eventVariable)
        .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const));
      const named = await terminal(() => session
        .queryNamed({ person: personVariable })
        .where(personPredicate)
        .one());
      const count = await terminal(() => pairQuery.countBy(personVariable));
      const exists = await terminal(() => pairQuery.existsBy(personVariable));
      const page = await terminal(() => session
        .query(personVariable, collectedPersonVariable.collect().distinct())
        .where(personPredicate, sameIdentifierPredicate)
        .pageBy(personVariable, { limit: 10n, includeTotal: true }));

      return {
        one: [onePerson.iid, oneEvent.iid, oneEvent.subject.iid],
        rows: rows.map((row: any) => [row.iid, row.identifier.value]),
        first: first?.iid,
        aliases_present: iids(aliasesPresent),
        aliases_missing: iids(aliasesMissing),
        by_iid: byIid.iid,
        by_iid_set: iids(byIidSet),
        offset_identifiers: offsetRows.map((row: any) => row.identifier.value),
        by_expression: byExpression.iid,
        cross_pair: iids(crossPair),
        parties: partyRows.map((row: any) => [row.__typebridgeModel, row.iid]),
        membership_count: membershipCount,
        base_relation: [baseRelation.__typebridgeModel, baseRelation.member.iid],
        subtype_relation: [
          subtypeRelation.__typebridgeModel,
          subtypeRelation.employee.iid,
          Object.hasOwn(subtypeRelation, "member"),
        ],
        reachable: iids(reachablePair),
        network: network.iid,
        all_first: allFirst?.iid,
        reducers: reducers.slice(0, 6),
        std_type: typeof reducers[6],
        field_grouped: fieldGrouped.map(([group, values]: any) => [group.value, values]),
        tuple_grouped: tupleGrouped.map(([[boolGroup, scoreGroup], values]: any) => [
          boolGroup.value,
          scoreGroup.value,
          values,
        ]),
        event_grouped: eventGrouped.map(([event, values]: any) => [event.iid, values]),
        named: named.person.iid,
        count,
        exists,
        page: {
          items: page.items.map(([person, collected]: any) => [person.iid, iids(collected)]),
          limit: page.limit,
          offset: page.offset,
          total: page.total,
        },
      };
    };
    const direct = await queryProgram(querySession, (run) => run());
    assert.deepEqual(direct, {
      one: [insertedPerson.iid, insertedEvent.iid, insertedPerson.iid],
      rows: [[insertedPerson.iid, "person-1"]],
      first: insertedPerson.iid,
      aliases_present: [insertedPerson.iid],
      aliases_missing: [...batchIids, transactionPersonIid],
      by_iid: insertedPerson.iid,
      by_iid_set: [insertedPerson.iid, insertedBatch[0].iid],
      offset_identifiers: ["person-2", "person-3"],
      by_expression: insertedPerson.iid,
      cross_pair: [insertedPerson.iid, insertedBatch[0].iid],
      parties: [
        [Employee.typeKey, insertedEmployee.iid],
        [Manager.typeKey, insertedManager.iid],
      ],
      membership_count: 2n,
      base_relation: [Membership.typeKey, insertedPerson.iid],
      subtype_relation: [Employment.typeKey, insertedPerson.iid, false],
      reachable: [insertedPerson.iid, insertedBatch[0].iid],
      network: insertedNetwork.iid,
      all_first: insertedPerson.iid,
      reducers: [4n, 24n, 3n, 9n, 6, 6],
      std_type: "number",
      field_grouped: [[true, [4n, 24n]]],
      tuple_grouped: [
        [true, 3n, [1n, 3n]],
        [true, 5n, [1n, 5n]],
        [true, 7n, [1n, 7n]],
        [true, 9n, [1n, 9n]],
      ],
      event_grouped: [[insertedEvent.iid, [1n, 3n]]],
      named: insertedPerson.iid,
      count: 1n,
      exists: true,
      page: {
        items: [[insertedPerson.iid, [insertedPerson.iid]]],
        limit: 10n,
        offset: 0n,
        total: 1n,
      },
    });

    const authority = await readFile(resolve(stage, "schema-authority.json"));
    const port = await freePort();
    const suppliedServer = process.env.TYPE_BRIDGE_V2_SMOKE_SERVER;
    const server = spawn(
      suppliedServer ?? "cargo",
      suppliedServer === undefined
        ? [
            "run",
            "--quiet",
            "-p",
            "type-bridge-server",
            "--features",
            "v2-query",
            "--example",
            "v2_smoke_server",
          ]
        : [],
      {
        cwd: CORE,
        env: {
          ...process.env,
          SMOKE_TYPEDB_ADDRESS: TYPEDB_ADDRESS,
          SMOKE_TYPEDB_USERNAME: TYPEDB_USERNAME,
          SMOKE_TYPEDB_PASSWORD: TYPEDB_PASSWORD,
          SMOKE_TYPEDB_HTTP_PORT: String(TYPEDB_HTTP_PORT),
          SMOKE_DATABASE: INTG_DATABASE,
          SMOKE_AUTHORITY_B64: authority.toString("base64"),
          SMOKE_PORT: String(port),
        },
        stdio: "ignore",
      },
    );
    try {
      await waitForPort(port, server, 300_000);
      const advertisementResponse = await fetch(`http://127.0.0.1:${port}/v2/capabilities`);
      assert.equal(advertisementResponse.status, 200);
      const advertisement = Buffer.from(await advertisementResponse.arrayBuffer());
      const requests: Buffer[] = [];
      async function exchange(request: Uint8Array): Promise<Buffer> {
        requests.push(Buffer.from(request));
        const response = await fetch(`http://127.0.0.1:${port}/v2/query`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: new Uint8Array(request),
        });
        assert.equal(response.status, 200);
        return Buffer.from(await response.arrayBuffer());
      }
      async function oneRemoteExchange(
        operation: () => Promise<any>,
      ): Promise<any> {
        const requestsBefore = requests.length;
        const result = await operation();
        assert.equal(requests.length, requestsBefore + 1);
        return result;
      }

      // docs: remote-query-node:start
      const remoteSession = new QuerySession(
        new ExchangeConnection(advertisement, exchange),
        {
          resources: new QueryExecutionResourceLimits({
            timeoutMilliseconds: 30_000n,
            items: 100n,
            bytes: 1n << 20n,
            collectionMembers: 100n,
            graphNodes: 1_000n,
            attributeValues: 1_000n,
            rolePlayers: 1_000n,
          }),
        },
      );
      // docs: remote-query-node:end
      assert.deepEqual(await queryProgram(remoteSession, oneRemoteExchange), direct);
      const proxy = await connectProxy(`http://127.0.0.1:${port}`);
      const proxySession = new QuerySession(proxy);
      try {
        assert.deepEqual(await queryProgram(proxySession, (run) => run()), direct);
      } finally {
        proxySession.close();
      }

      // A limit refusal is the same on every connection: category, code,
      // path, and details.
      const limitRefusal = async (connection: any): Promise<Record<string, unknown>> => {
        const session = new QuerySession(connection, {
          resources: new QueryExecutionResourceLimits({ rolePlayers: 0n }),
        });
        const membership = session.exact(Membership);
        try {
          await session.query(membership).where(membership.iid(insertedMembership.iid)).one();
        } catch (error: any) {
          return {
            category: error.sdkCategory,
            queryCategory: error.queryCategory,
            code: error.code,
            path: error.path,
            details: error.details,
          };
        } finally {
          session.close();
        }
        throw new Error("a zero role-player budget accepted a hydrated relation");
      };
      const directRefusal = await limitRefusal(database);
      assert.equal(directRefusal.category, "resource_limit");
      assert.deepEqual(
        await limitRefusal(new ExchangeConnection(advertisement, exchange)),
        directRefusal,
      );
      assert.deepEqual(await limitRefusal(proxy), directRefusal);

      // Windows, caps, group-by keys, and reachability depth are core rules:
      // each corpus case is refused with the corpus code on every
      // connection, and a session with fewer selected slots refuses at its
      // own limit.
      const corpusCases = JSON.parse(
        await readFile(resolve(ROOT, "tests/contracts/typed_query/corpus-v1.json"), "utf8"),
      ).cases as { id: string; expected: { error_category: string; error_code: string } }[];
      const corpusCode = (id: string): [string, string] => {
        const found = corpusCases.find((entry) => entry.id === id);
        assert.ok(found !== undefined, `missing corpus case ${id}`);
        return [found.expected.error_category, found.expected.error_code];
      };
      const coreRefusal = async (
        id: string,
        action: () => unknown,
      ): Promise<Record<string, unknown>> => {
        try {
          await action();
        } catch (error: any) {
          assert.deepEqual([error.category, error.code], corpusCode(id), id);
          return { code: error.code, path: error.path, details: error.details };
        }
        throw new Error(`${id} was not refused`);
      };
      const coreRules = async (connection: any, slots: bigint | undefined) => {
        const session = new QuerySession(
          connection,
          slots === undefined ? {} : { queryLimits: new QueryLimits({ selected_slots: slots }) },
        );
        const width = slots === undefined ? 16 : Number(slots);
        try {
          const person = session.exact(Person);
          const score = person.field(Person.score);
          const query = session.query(person);
          const other = session.exact(Person);
          const reachable = (minDepth: number, maxDepth: number) => () =>
            session.reachable(
              person,
              other,
              NetworkLink,
              NetworkLink.origin,
              NetworkLink.destination,
              { minDepth, maxDepth },
            );
          return {
            selection: await coreRefusal(
              slots === undefined
                ? "selection.seventeen-slot-rejection"
                : "selection.session-slot-ceiling",
              () =>
                session.query(
                  ...(Array.from({ length: width + 1 }, () => session.exact(Person)) as [any]),
                ),
            ),
            groupKeys: await coreRefusal("aggregate.seventeen-group-keys", () =>
              (query.groupBy as any)(
                person,
                ...Array.from({ length: width + 1 }, () => score),
              ),
            ),
            terms: await coreRefusal("aggregate.seventeen-terms", () =>
              query.aggregate(
                person,
                Array.from({ length: width + 1 }, () => aggregate.count()) as any,
              ),
            ),
            emptyTerms: await coreRefusal("aggregate.empty-terms", () =>
              query.aggregate(person, [] as any),
            ),
            zeroLimit: await coreRefusal("bounds.public-invalid-limit", () =>
              query.rows({ limit: 0n }),
            ),
            zeroPage: await coreRefusal("bounds.public-invalid-limit", () =>
              query.pageBy(person, { limit: 0n }),
            ),
            offset: await coreRefusal("bounds.public-invalid-offset", () =>
              query.rows({ offset: (1n << 64n) - 1n, limit: 1n }),
            ),
            overflow: await coreRefusal("bounds.window-overflow", () =>
              query.rows({ offset: (1n << 64n) - 2n, limit: 2n }),
            ),
            reversed: await coreRefusal("reachability.reversed-bounds", reachable(3, 2)),
            deep: await coreRefusal("reachability.depth-ceiling", reachable(0, 65)),
          };
        } finally {
          session.close();
        }
      };
      for (const slots of [undefined, 4n]) {
        const directRules = await coreRules(database, slots);
        const width = slots === undefined ? 16 : Number(slots);
        assert.equal(String((directRules.selection.details as any).maximum.value), String(width));
        assert.equal(String((directRules.groupKeys.details as any).maximum.value), String(width));
        assert.equal(String((directRules.terms.details as any).maximum.value), String(width));
        const requestsBefore = requests.length;
        assert.deepEqual(
          await coreRules(new ExchangeConnection(advertisement, exchange), slots),
          directRules,
        );
        assert.deepEqual(await coreRules(proxy, slots), directRules);
        assert.equal(requests.length, requestsBefore);
      }
      assert.ok(requests.every((request) => request.length > 0));

      if (sdkReportPath !== undefined) {
        if (sdkCatalogRaw === undefined || sdkJourneyRaw === undefined) {
          throw new Error("sdk contracts were not loaded after report preflight");
        }
        const results = await runSdkJourney(
          generated,
          personManager,
          membershipManager,
          querySession,
          remoteSession,
          requests,
          sdkCatalog,
          sdkJourney,
        );
        preparedSdkReport = await sdkReport(
          sdkCatalogRaw,
          sdkCatalog,
          sdkJourneyRaw,
          semanticFingerprint,
          projectionFingerprint,
          results,
        );
      }
      if (sdkV2ReportPath !== undefined) {
        if (
          sdkV2CatalogRaw === undefined
          || sdkV2JourneyRaw === undefined
          || sdkV2ProofObservations === undefined
        ) {
          throw new Error("sdk-v2 contracts were not loaded after report preflight");
        }
        const results = await runSdkV2Journey(
          generated,
          database,
          remoteSession,
          requests,
          advertisement,
          exchange,
          sdkV2Catalog,
          sdkV2Journey,
          sdkV2ProofObservations,
        );
        preparedSdkV2Report = await sdkV2Report(
          sdkV2CatalogRaw,
          sdkV2Catalog,
          sdkV2JourneyRaw,
          semanticFingerprint,
          projectionFingerprint,
          results,
        );
      }
    } finally {
      if (server.exitCode === null) {
        const exited = new Promise((resolveExit) => server.once("exit", resolveExit));
        server.kill("SIGKILL");
        await exited;
      }
    }

    const relationBatch = networkManager.insertMany([
      NetworkLink.create({
        destination: insertedBatch[1],
        identifier: Identifier.create("network-2"),
        origin: insertedBatch[0],
        participant: [insertedBatch[0], insertedBatch[1]],
      }),
      NetworkLink.create({
        destination: insertedPerson,
        identifier: Identifier.create("network-3"),
        origin: insertedBatch[1],
        participant: [insertedBatch[1], insertedPerson],
      }),
    ]);
    assert.ok(relationBatch.every((candidate) => candidate.iid !== null));
    assert.deepEqual(
      networkManager.putMany(relationBatch).map((candidate) => candidate.iid),
      relationBatch.map((candidate) => candidate.iid),
    );
    for (const relation of relationBatch) {
      assertIid(relation.iid);
      networkManager.delete(relation);
      assert.equal(networkManager.getByIid(relation.iid), null);
    }

    membershipManager.delete(insertedMembership);
    assert.equal(membershipManager.getByIid(insertedMembership.iid), null);
    networkManager.delete(insertedNetwork);
    assert.equal(networkManager.getByIid(insertedNetwork.iid), null);
    personManager.delete(transactionPersonIid);
    assert.equal(personManager.getByIid(transactionPersonIid), null);
  } catch (error) {
    failure = error;
  }

  try {
    database?.deleteDatabase();
  } catch (error) {
    failure ??= error;
  }
  try {
    await rm(
      ownsStage ? stage : resolve(stage, "node_modules", "@type-bridge", "node"),
      { recursive: true, force: true },
    );
  } catch (error) {
    failure ??= error;
  }
  if (failure !== undefined) {
    throw failure;
  }
  if (sdkReportPath !== undefined) {
    assert.equal(
      REMOTE_PROFILE,
      "typedb-3.12.1/v1",
      "sdk reports may only be emitted for typedb-3.12.1/v1",
    );
    if (preparedSdkReport === undefined) {
      throw new Error("sdk report was not prepared after the successful journey");
    }
    await publishSdkReport(sdkReportPath, preparedSdkReport);
  }
  if (sdkV2ReportPath !== undefined) {
    assert.equal(
      REMOTE_PROFILE,
      "typedb-3.12.1/v1",
      "sdk-v2 reports may only be emitted for typedb-3.12.1/v1",
    );
    if (preparedSdkV2Report === undefined) {
      throw new Error("sdk-v2 report was not prepared after the successful journey");
    }
    await publishSdkReport(sdkV2ReportPath, preparedSdkV2Report);
  }
});

test(
  "node.generated_data_model_runtime_v3_live",
  { skip: IS_TYPEDB_3_11, timeout: 360_000 },
  async () => {
    const suppliedStage = process.env.TYPE_BRIDGE_GENERATED_NODE_STAGE;
    const stage = suppliedStage === undefined
      ? await mkdtemp(join(tmpdir(), "type-bridge-node-ordered-projection-"))
      : resolve(suppliedStage);
    const ownsStage = suppliedStage === undefined;
    const generatedDirectory = resolve(stage, "generated_ordered");
    let database: ReturnType<typeof connectIntegration> | undefined;
    let failure: unknown;

    try {
      if (ownsStage) {
        run(
          resolve(ROOT, "scripts/ci/prepare_generated_live_fixture.sh"),
          ["node", stage],
          ROOT,
        );
      }

      const packageScope = resolve(stage, "node_modules/@type-bridge");
      await mkdir(packageScope, { recursive: true });
      await rm(resolve(packageScope, "node"), { recursive: true, force: true });
      await symlink(
        NODE_RUNTIME_PACKAGE,
        resolve(packageScope, "node"),
        process.platform === "win32" ? "junction" : "dir",
      );
      if (ownsStage) {
        run(
          resolve(NODE_SOURCE_PACKAGE, "node_modules/.bin/tsc"),
          ["--project", resolve(generatedDirectory, "tsconfig.json")],
          generatedDirectory,
        );
      }

      const generated = await import(
        pathToFileURL(resolve(generatedDirectory, "dist/index.js")).href
      );
      const {
        Counter,
        CounterValue,
        Identifier,
        Membership,
        NetworkLink,
        Person,
        Score,
        ValBool,
        ValConstrained,
        ValDate,
        ValDatetime,
        ValDatetimeTz,
        ValDecimal,
        ValDouble,
        ValDuration,
      } = generated;

      database = connectIntegration();
      database.resetDatabase();
      defineSchema(database, await readFile(SDK_V3_PROVIDER_SCHEMA, "utf8"));

      const personInput = (identifierValue: string, scoreValue: bigint) => Person.create({
        identifier: Identifier.create(identifierValue),
        score: Score.create(scoreValue),
        valBool: ValBool.create(true),
        valConstrained: ValConstrained.create(20n),
        valDate: ValDate.create(new Date("2026-08-14T00:00:00.000Z")),
        valDatetime: ValDatetime.create(new Date("2026-08-14T12:34:56.000Z")),
        valDatetimeTz: ValDatetimeTz.create(new Date("2026-08-14T12:34:56.000Z")),
        valDecimal: ValDecimal.create("3.5"),
        valDouble: ValDouble.create(3.5),
        valDuration: ValDuration.create("PT3S"),
      });
      const frozenBatch = (value: unknown): void => {
        assert.equal(Array.isArray(value), true);
        assert.equal(Object.isFrozen(value), true);
      };
      const nonce = randomUUID();
      const personKeys = [
        `ordered-person-a-${nonce}`,
        `ordered-person-b-${nonce}`,
      ] as const;
      const networkKeys = [
        `ordered-network-a-${nonce}`,
        `ordered-network-b-${nonce}`,
      ] as const;

      const personManager = Person.manager(database);
      for (const empty of [
        personManager.insertMany([]),
        personManager.putMany([]),
        personManager.updateMany([]),
      ]) {
        assert.deepEqual(empty, []);
        frozenBatch(empty);
      }
      assert.equal(personManager.deleteMany([]), undefined);
      assert.equal(typeof personManager.updateMany, "function");
      assert.equal(typeof personManager.deleteMany, "function");

      const insertedPeople = personManager.insertMany([
        personInput(personKeys[0], 1n),
        personInput(personKeys[1], 2n),
      ]);
      frozenBatch(insertedPeople);
      assert.deepEqual(
        insertedPeople.map((person) => person.identifier.value),
        personKeys,
      );
      assert.ok(insertedPeople.every((person) => Object.isFrozen(person)));
      const personIids = insertedPeople.map((person) => {
        assertIid(person.iid);
        return person.iid;
      });

      assert.throws(
        () => personManager.insertMany([
          personInput(`duplicate-person-${nonce}`, 3n),
          personInput(`duplicate-person-${nonce}`, 4n),
        ]),
      );
      assert.equal(
        personManager.filter({ identifier: Identifier.create(`duplicate-person-${nonce}`) }).count(),
        0n,
      );
      personManager.deleteMany([personIids[1]]);
      const putPeople = personManager.putMany(insertedPeople);
      frozenBatch(putPeople);
      assert.equal(putPeople[0].iid, personIids[0]);
      assertIid(putPeople[1].iid);
      assert.notEqual(putPeople[1].iid, personIids[1]);
      personIids[1] = putPeople[1].iid;
      const updatedPeople = personManager.updateMany([
        [personIids[1], personInput(personKeys[1], 12n)],
        [personIids[0], personInput(personKeys[0], 11n)],
      ]);
      frozenBatch(updatedPeople);
      assert.deepEqual(
        updatedPeople.map((person) => person.identifier.value),
        [personKeys[1], personKeys[0]],
      );
      assert.deepEqual(
        updatedPeople.map((person) => person.score.value),
        [12n, 11n],
      );
      const personB = updatedPeople[0];
      const personA = updatedPeople[1];

      assert.throws(
        () => personManager.updateMany([
          [personIids[0], personInput(personKeys[0], 31n)],
          [personIids[0], personInput(personKeys[0], 32n)],
        ]),
      );

      const counterManager = Counter.manager(database);
      const insertedCounters = counterManager.insertMany([
        Counter.create({ counterValue: CounterValue.create(1n) }),
        Counter.create({ counterValue: CounterValue.create(2n) }),
      ]);
      const counterIids = insertedCounters.map((counter) => {
        assertIid(counter.iid);
        return counter.iid;
      });
      assert.equal(counterManager.getByIid(counterIids[0])?.counterValue.value, 1n);
      const updatedCounters = counterManager.updateMany([
        [counterIids[0], Counter.create({ counterValue: CounterValue.create(11n) })],
        [counterIids[1], Counter.create({ counterValue: CounterValue.create(12n) })],
      ]);
      assert.deepEqual(updatedCounters.map((counter) => counter.iid), counterIids);
      assert.throws(() => counterManager.updateMany([
        [counterIids[0], Counter.create({ counterValue: CounterValue.create(13n) })],
        [counterIids[0], Counter.create({ counterValue: CounterValue.create(14n) })],
      ]));

      const membershipManager = Membership.manager(database);
      const insertedMemberships = membershipManager.insertMany([
        Membership.create({ member: personA }),
        Membership.create({ member: personB }),
      ]);
      const membershipIids = insertedMemberships.map((membership) => {
        assertIid(membership.iid);
        return membership.iid;
      });
      assert.equal(membershipManager.getByIid(membershipIids[0])?.member.iid, personA.iid);
      const updatedMemberships = membershipManager.updateMany([
        [membershipIids[0], Membership.create({ member: personB })],
        [membershipIids[1], Membership.create({ member: personA })],
      ]);
      assert.deepEqual(updatedMemberships.map((membership) => membership.iid), membershipIids);
      assert.equal(membershipManager.getByIid(membershipIids[0])?.member.iid, personB.iid);
      assert.throws(() => membershipManager.updateMany([
        [membershipIids[0], Membership.create({ member: personA })],
        [membershipIids[0], Membership.create({ member: personB })],
      ]));

      const networkManager = NetworkLink.manager(database);
      for (const empty of [
        networkManager.insertMany([]),
        networkManager.putMany([]),
        networkManager.updateMany([]),
      ]) {
        assert.deepEqual(empty, []);
        frozenBatch(empty);
      }
      assert.equal(networkManager.deleteMany([]), undefined);
      const insertedNetworks = networkManager.insertMany([
        NetworkLink.create({
          destination: personB,
          identifier: Identifier.create(networkKeys[0]),
          origin: personA,
        }),
        NetworkLink.create({
          destination: personA,
          identifier: Identifier.create(networkKeys[1]),
          origin: personB,
        }),
      ]);
      frozenBatch(insertedNetworks);
      assert.deepEqual(
        insertedNetworks.map((network) => network.identifier.value),
        networkKeys,
      );
      const networkIids = insertedNetworks.map((network) => {
        assertIid(network.iid);
        return network.iid;
      });
      assert.throws(() => networkManager.insertMany([
        NetworkLink.create({ destination: personB, identifier: Identifier.create(`duplicate-network-${nonce}`), origin: personA }),
        NetworkLink.create({ destination: personA, identifier: Identifier.create(`duplicate-network-${nonce}`), origin: personB }),
      ]));
      networkManager.deleteMany([networkIids[1]]);
      const putNetworks = networkManager.putMany(insertedNetworks);
      frozenBatch(putNetworks);
      assert.equal(putNetworks[0].iid, networkIids[0]);
      assertIid(putNetworks[1].iid);
      assert.notEqual(putNetworks[1].iid, networkIids[1]);
      networkIids[1] = putNetworks[1].iid;
      const updatedNetworks = networkManager.updateMany([
        [networkIids[1], NetworkLink.create({
          destination: personB,
          identifier: Identifier.create(networkKeys[1]),
          origin: personA,
        })],
        [networkIids[0], NetworkLink.create({
          destination: personA,
          identifier: Identifier.create(networkKeys[0]),
          origin: personB,
        })],
      ]);
      frozenBatch(updatedNetworks);
      assert.deepEqual(
        updatedNetworks.map((network) => network.identifier.value),
        [networkKeys[1], networkKeys[0]],
      );
      assert.equal(
        networkManager.getByIid(networkIids[1])?.origin.identifier.value,
        personKeys[0],
      );
      assert.equal(networkManager.deleteMany(networkIids), undefined);
      assert.ok(networkIids.every((iid) => networkManager.getByIid(iid) === null));
      assert.equal(membershipManager.deleteMany(membershipIids), undefined);
      assert.ok(membershipIids.every((iid) => membershipManager.getByIid(iid) === null));
      assert.equal(counterManager.deleteMany(counterIids), undefined);
      assert.ok(counterIids.every((iid) => counterManager.getByIid(iid) === null));
      assert.equal(personManager.deleteMany(personIids), undefined);
      assert.ok(personIids.every((iid) => personManager.getByIid(iid) === null));

      const transactionPersonKeys = [
        `ordered-transaction-person-a-${nonce}`,
        `ordered-transaction-person-b-${nonce}`,
      ] as const;
      const transactionNetworkKey = `ordered-transaction-network-${nonce}`;
      const transaction = database.transaction("write");
      let transactionPeople: readonly any[] = [];
      let transactionNetworks: readonly any[] = [];
      try {
        const transactionPersonManager = Person.manager(transaction);
        transactionPeople = transactionPersonManager.insertMany([
          personInput(transactionPersonKeys[0], 21n),
          personInput(transactionPersonKeys[1], 22n),
        ]);
        frozenBatch(transactionPeople);
        const transactionNetworkManager = NetworkLink.manager(transaction);
        transactionNetworks = transactionNetworkManager.insertMany([
          NetworkLink.create({
            destination: transactionPeople[1],
            identifier: Identifier.create(transactionNetworkKey),
            origin: transactionPeople[0],
          }),
        ]);
        frozenBatch(transactionNetworks);
        assert.deepEqual(
          transactionNetworkManager.putMany(transactionNetworks).map(
            (network) => network.iid,
          ),
          transactionNetworks.map((network) => network.iid),
        );
        transaction.commit();
      } catch (error) {
        try {
          transaction.rollback();
        } catch {
          // Preserve the operation or commit failure.
        }
        throw error;
      }

      const transactionPersonIids = transactionPeople.map((person) => {
        assertIid(person.iid);
        assert.equal(personManager.getByIid(person.iid)?.iid, person.iid);
        return person.iid;
      });
      const transactionNetworkIids = transactionNetworks.map((network) => {
        assertIid(network.iid);
        assert.equal(networkManager.getByIid(network.iid)?.iid, network.iid);
        return network.iid;
      });
      assert.equal(networkManager.deleteMany(transactionNetworkIids), undefined);
      assert.equal(personManager.deleteMany(transactionPersonIids), undefined);
      assert.equal(networkManager.count(), 0n);
      assert.equal(personManager.count(), 0n);

      const supplementPath = process.env.TYPE_BRIDGE_SDK_V3_NODE_SUPPLEMENT;
      if (supplementPath !== undefined) {
        const observations = sdkV3SupplementObservations();
        const journey = JSON.parse(await readFile(SDK_V3_JOURNEY, "utf8"));
        const results = [...observations.entries()].map(([key, observation]) => {
          const [observationRef, proofKind] = key.split("\u0000");
          assert.deepEqual(observation, journey.expected_observations[observationRef]);
          return {
            observation_ref: observationRef,
            proof_kind: proofKind,
            outcome: "passed",
            observation,
          };
        }).sort((left, right) =>
          compareText(left.observation_ref, right.observation_ref)
          || compareText(left.proof_kind, right.proof_kind)
        );
        assert.equal(results.length, 8);
        await publishSdkReport(supplementPath, {
          format: "typebridge.sdk-v3-live-supplement/v1",
          binding: "node",
          semantic_profile: "typedb-3.12.1/v1",
          producer: "node.generated-data-model-runtime-v3-live",
          semantic_fingerprint: JSON.parse(generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON),
          projection_fingerprint: JSON.parse(generated.PROJECTION_FINGERPRINT_JSON),
          results,
        });
      }
    } catch (error) {
      failure = error;
    }

    try {
      database?.deleteDatabase();
    } catch (error) {
      failure ??= error;
    }
    try {
      await rm(
        ownsStage ? stage : resolve(stage, "node_modules", "@type-bridge", "node"),
        { recursive: true, force: true },
      );
    } catch (error) {
      failure ??= error;
    }
    if (failure !== undefined) throw failure;
  },
);

test(
  "node.generated_canonical_serialization_v5_live",
  {
    skip: IS_TYPEDB_3_11 || process.env.TYPE_BRIDGE_SDK_V5_NODE_EVIDENCE === undefined,
    timeout: 360_000,
  },
  async () => {
    const evidencePath = process.env.TYPE_BRIDGE_SDK_V5_NODE_EVIDENCE;
    assert.notEqual(evidencePath, undefined);
    const suppliedStage = process.env.TYPE_BRIDGE_GENERATED_NODE_STAGE;
    const stage = suppliedStage === undefined
      ? await mkdtemp(join(tmpdir(), "type-bridge-node-v5-live-"))
      : resolve(suppliedStage);
    const ownsStage = suppliedStage === undefined;
    const generatedDirectory = resolve(stage, "generated_ordered");
    let database: ReturnType<typeof connectIntegration> | undefined;
    let server: ChildProcess | undefined;
    let failure: unknown;

    try {
      if (ownsStage) {
        run(resolve(ROOT, "scripts/ci/prepare_generated_live_fixture.sh"), ["node", stage], ROOT);
      }
      const packageScope = resolve(stage, "node_modules/@type-bridge");
      await mkdir(packageScope, { recursive: true });
      await rm(resolve(packageScope, "node"), { recursive: true, force: true });
      await symlink(
        NODE_RUNTIME_PACKAGE,
        resolve(packageScope, "node"),
        process.platform === "win32" ? "junction" : "dir",
      );
      if (ownsStage) {
        run(
          resolve(NODE_SOURCE_PACKAGE, "node_modules/.bin/tsc"),
          ["--project", resolve(generatedDirectory, "tsconfig.json")],
          generatedDirectory,
        );
      }
      const generated = await import(
        pathToFileURL(resolve(generatedDirectory, "dist/index.js")).href
      );
      const {
        Employment, ExchangeConnection, Identifier, Person, QueryExecutionResourceLimits,
        QuerySession, Score,
        ValBool, ValConstrained, ValDate, ValDatetime, ValDatetimeTz, ValDecimal,
        ValDouble, ValDuration,
      } = generated;

      database = connectIntegration();
      database.resetDatabase();
      requireSdkServerVersion(await detectTypeDBServerVersion(TYPEDB_ADDRESS, TYPEDB_HTTP_PORT));
      defineSchema(database, await readFile(SDK_V3_PROVIDER_SCHEMA, "utf8"));
      const personManager = Person.manager(database);
      const employmentManager = Employment.manager(database);
      const person = personManager.insert(Person.create({
        aliases: [],
        identifier: Identifier.create("v5-live-person"),
        score: Score.create(70n),
        valBool: ValBool.create(true),
        valConstrained: ValConstrained.create(55n),
        valDate: ValDate.create(new Date("2026-08-03T00:00:00.000Z")),
        valDatetime: ValDatetime.create(new Date("2026-08-03T03:55:00.000Z")),
        valDatetimeTz: ValDatetimeTz.create(new Date("2026-08-03T03:55:00.000Z")),
        valDecimal: ValDecimal.create("128.45"),
        valDouble: ValDouble.create(8.14),
        valDuration: ValDuration.create("P6D"),
      }));
      const employment = employmentManager.insert(Employment.create({ employee: person }));

      // One employment query, run on the direct and the exchange connection.
      const employmentPair = async (session: any): Promise<[any, any]> => {
        const employmentVar = session.var(Employment);
        const personVar = session.var(Person);
        try {
          return await session
            .query(employmentVar, personVar)
            .where(
              employmentVar.role(Employment.employee).connects(personVar),
              personVar.field(Person.identifier).eq(Identifier.create("v5-live-person")),
            )
            .one();
        } finally {
          session.close();
        }
      };
      const [directEmployment, directPerson] = await employmentPair(new QuerySession(database));
      const entitySnapshot = Person.encodeSnapshot(directPerson);
      const relationSnapshot = Employment.encodeSnapshot(directEmployment);

      const port = await freePort();
      const authority = await readFile(resolve(stage, "schema-authority-ordered.json"));
      const suppliedServer = process.env.TYPE_BRIDGE_V2_SMOKE_SERVER;
      server = spawn(
        suppliedServer ?? "cargo",
        suppliedServer === undefined
          ? ["run", "--quiet", "-p", "type-bridge-server", "--features", "v2-query", "--example", "v2_smoke_server"]
          : [],
        {
          cwd: CORE,
          env: {
            ...process.env,
            SMOKE_TYPEDB_ADDRESS: TYPEDB_ADDRESS,
            SMOKE_TYPEDB_USERNAME: TYPEDB_USERNAME,
            SMOKE_TYPEDB_PASSWORD: TYPEDB_PASSWORD,
            SMOKE_TYPEDB_HTTP_PORT: String(TYPEDB_HTTP_PORT),
            SMOKE_DATABASE: INTG_DATABASE,
            SMOKE_AUTHORITY_B64: authority.toString("base64"),
            SMOKE_PORT: String(port),
          },
          stdio: "ignore",
        },
      );
      await waitForPort(port, server, 300_000);
      const advertisementResponse = await fetch(`http://127.0.0.1:${port}/v2/capabilities`);
      assert.equal(advertisementResponse.status, 200);
      const advertisement = Buffer.from(await advertisementResponse.arrayBuffer());
      const requests: Buffer[] = [];
      const exchange = new ExchangeConnection(
        advertisement,
        async (request: Uint8Array): Promise<Buffer> => {
          requests.push(Buffer.from(request));
          const response = await fetch(`http://127.0.0.1:${port}/v2/query`, {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: new Uint8Array(request),
          });
          assert.equal(response.status, 200);
          return Buffer.from(await response.arrayBuffer());
        },
      );
      const [remoteEmployment, remotePerson] = await employmentPair(new QuerySession(exchange, {
        resources: new QueryExecutionResourceLimits({
          timeoutMilliseconds: 30_000n,
          items: 10n,
          bytes: 1n << 20n,
          collectionMembers: 100n,
          graphNodes: 30n,
          attributeValues: 1_000n,
          rolePlayers: 30n,
        }),
      }));
      assert.equal(requests.length, 1);
      assert.deepEqual(Person.encodeSnapshot(remotePerson), entitySnapshot);
      assert.deepEqual(Employment.encodeSnapshot(remoteEmployment), relationSnapshot);

      const detached = Person.decodeSnapshot(entitySnapshot);
      let detachedCode: string | undefined;
      try {
        employmentManager.insert(Employment.create({ employee: detached }));
      } catch (error) {
        detachedCode = String(error).match(/"code":"([a-z0-9_]+)"/)?.[1];
      }
      assert.equal(detachedCode, "projected_snapshot_detached");
      const rebound = personManager
        .filter({ identifier: Identifier.create("v5-live-person") })
        .first();
      assert.notEqual(rebound, null);
      employmentManager.update(
        employment.iid,
        Employment.create({ employee: rebound }),
      );
      assert.equal(employmentManager.getByIid(employment.iid)?.iid, employment.iid);

      await publishSdkReport(evidencePath!, {
        binding: "node",
        detached_mutation_code: detachedCode,
        direct_remote_equal: true,
        entity_snapshot_b64: Buffer.from(entitySnapshot).toString("base64"),
        format: "typebridge.sdk-v5-live-codec-evidence/v1",
        rebound_mutation: true,
        relation_snapshot_b64: Buffer.from(relationSnapshot).toString("base64"),
        remote_exchange_count: requests.length,
      });
      employmentManager.delete(employment);
      personManager.delete(person);
    } catch (error) {
      failure ??= error;
    }
    try {
      if (server !== undefined && server.exitCode === null) server.kill("SIGTERM");
    } catch (error) {
      failure ??= error;
    }
    try {
      database?.deleteDatabase();
    } catch (error) {
      failure ??= error;
    }
    try {
      database?.close();
    } catch (error) {
      failure ??= error;
    }
    try {
      await rm(
        ownsStage ? stage : resolve(stage, "node_modules", "@type-bridge", "node"),
        { recursive: true, force: true },
      );
    } catch (error) {
      failure ??= error;
    }
    if (failure !== undefined) throw failure;
  },
);
