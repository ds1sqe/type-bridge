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
const WORKFORCE_MANIFEST_RELATIVE = "tests/contracts/sdk_conformance/manifest-v1.json";
const WORKFORCE_CATALOG_RELATIVE =
  "tests/contracts/sdk_conformance/workforce-v1/catalog-v1.json";
const WORKFORCE_JOURNEY_RELATIVE =
  "tests/contracts/sdk_conformance/workforce-v1/journey-v1.json";
const WORKFORCE_V2_CATALOG_RELATIVE =
  "tests/contracts/sdk_conformance/workforce-v2/catalog-v2.json";
const WORKFORCE_V2_JOURNEY_RELATIVE =
  "tests/contracts/sdk_conformance/workforce-v2/journey-v2.json";
const WORKFORCE_V3_JOURNEY = resolve(
  ROOT,
  "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json",
);
const WORKFORCE_V3_PROVIDER_SCHEMA = resolve(
  ROOT,
  "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql",
);
const WORKFORCE_MANIFEST = resolve(ROOT, WORKFORCE_MANIFEST_RELATIVE);
const WORKFORCE_CATALOG = resolve(ROOT, WORKFORCE_CATALOG_RELATIVE);
const WORKFORCE_JOURNEY = resolve(ROOT, WORKFORCE_JOURNEY_RELATIVE);
const WORKFORCE_V2_CATALOG = resolve(ROOT, WORKFORCE_V2_CATALOG_RELATIVE);
const WORKFORCE_V2_JOURNEY = resolve(ROOT, WORKFORCE_V2_JOURNEY_RELATIVE);
const WORKFORCE_V2_PROOF_VALIDATOR = resolve(
  ROOT,
  "scripts/ci/workforce_v2_proof_fragments.py",
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

function workforceResults(
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

function workforceV3SupplementObservations(): ReadonlyMap<string, Record<string, unknown>> {
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

function loadWorkforceV2ProofObservations(): ReadonlyMap<string, Record<string, unknown>> {
  const rawPaths = process.env.TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS;
  const runNonce = process.env.TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE;
  const python = process.env.TYPE_BRIDGE_WORKFORCE_V2_VALIDATOR_PYTHON;
  if (rawPaths === undefined || runNonce === undefined || python === undefined) {
    throw new Error(
      "workforce-v2 reports require proof fragment paths, the same-run nonce, "
      + "and an explicit validator Python executable",
    );
  }
  if (!isAbsolute(python)) {
    throw new TypeError(
      "TYPE_BRIDGE_WORKFORCE_V2_VALIDATOR_PYTHON must be an absolute path",
    );
  }
  const paths = rawPaths.split(delimiter);
  if (paths.length === 0 || paths.some((value) => value.length === 0)) {
    throw new Error("workforce-v2 proof fragment paths must be a nonempty path list");
  }
  const validation = spawnSync(
    python,
    [
      WORKFORCE_V2_PROOF_VALIDATOR,
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
      `workforce-v2 proof fragments were rejected (${validation.status ?? "unknown"}): `
      + validation.stderr,
    );
  }
  if (validation.stderr !== "") {
    throw new Error(`workforce-v2 proof validator wrote stderr: ${validation.stderr}`);
  }
  const rows: unknown = JSON.parse(validation.stdout);
  if (!Array.isArray(rows)) {
    throw new TypeError("workforce-v2 proof validator returned a non-array payload");
  }
  const observations = new Map<string, Record<string, unknown>>();
  for (const row of rows) {
    if (
      row === null
      || typeof row !== "object"
      || Array.isArray(row)
      || Object.keys(row).sort().join("\0") !== "observation\0observation_ref\0proof_kind"
    ) {
      throw new TypeError("workforce-v2 proof validator returned a malformed row");
    }
    const value = row as Record<string, unknown>;
    if (
      typeof value.observation_ref !== "string"
      || typeof value.proof_kind !== "string"
      || value.observation === null
      || typeof value.observation !== "object"
      || Array.isArray(value.observation)
    ) {
      throw new TypeError("workforce-v2 proof validator returned an invalid observation");
    }
    const key = observationKey(value.observation_ref, value.proof_kind);
    assert.equal(observations.has(key), false);
    observations.set(key, value.observation as Record<string, unknown>);
  }
  assert.equal(observations.size, 3);
  return observations;
}

async function workforceReport(
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
  const manifestRaw = await readFile(WORKFORCE_MANIFEST);
  assert.equal(catalog.journey_path, WORKFORCE_JOURNEY_RELATIVE);
  return {
    format: "typebridge.sdk-conformance-report/v1",
    binding: "node",
    manifest: sourceIdentity(WORKFORCE_MANIFEST_RELATIVE, manifestRaw),
    catalog: sourceIdentity(WORKFORCE_CATALOG_RELATIVE, catalogRaw),
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

async function workforceV2Report(
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
  const manifestRaw = await readFile(WORKFORCE_MANIFEST);
  assert.equal(catalog.journey_path, WORKFORCE_V2_JOURNEY_RELATIVE);
  return {
    format: "typebridge.sdk-conformance-report/v2",
    binding: "node",
    manifest: sourceIdentity(WORKFORCE_MANIFEST_RELATIVE, manifestRaw),
    catalog: sourceIdentity(WORKFORCE_V2_CATALOG_RELATIVE, catalogRaw),
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

async function validateWorkforceReportPath(rawPath: string): Promise<string> {
  const encodedPath = Buffer.from(rawPath, "utf8");
  if (encodedPath.toString("utf8") !== rawPath) {
    throw new TypeError("TYPE_BRIDGE_WORKFORCE_REPORT must be a UTF-8 path");
  }
  if (encodedPath.length === 0 || encodedPath.length > 4096) {
    throw new RangeError("TYPE_BRIDGE_WORKFORCE_REPORT must contain 1 to 4096 UTF-8 bytes");
  }
  if (!isAbsolute(rawPath)) {
    throw new TypeError("TYPE_BRIDGE_WORKFORCE_REPORT must be an absolute path");
  }
  const parent = dirname(rawPath);
  const parentMetadata = await metadataIfPresent(parent);
  if (
    parentMetadata === undefined
    || parentMetadata.isSymbolicLink()
    || !parentMetadata.isDirectory()
  ) {
    throw new TypeError(
      "TYPE_BRIDGE_WORKFORCE_REPORT parent must be an existing non-symlink directory",
    );
  }
  if (await metadataIfPresent(rawPath) !== undefined) {
    throw new Error("TYPE_BRIDGE_WORKFORCE_REPORT destination must not exist");
  }
  return parent;
}

function requireWorkforceServerVersion(detected: string | null): void {
  if (detected !== "3.12.3") {
    throw new Error(
      "workforce reports require the actual detected TypeDB server version 3.12.3; "
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

async function publishWorkforceReport(
  rawPath: string,
  report: Record<string, unknown>,
): Promise<void> {
  const parent = await validateWorkforceReportPath(rawPath);

  const payload = `${JSON.stringify(sortJson(report))}\n`;
  const temporary = join(parent, `.typebridge-workforce-${randomUUID()}.tmp`);
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
          "TYPE_BRIDGE_WORKFORCE_REPORT destination appeared during publication",
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

async function runWorkforceJourney(
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

    const directRelationVariable = querySession.exact(Membership);
    const directPersonVariable = querySession.exact(Person);
    const [directRelation, directPerson] = querySession
      .query(directRelationVariable, directPersonVariable)
      .where(
        directRelationVariable.role(Membership.member).connects(directPersonVariable),
        directPersonVariable
          .field(Person.identifier)
          .eq(Identifier.create(fields.identifier.value)),
      )
      .one();
    const directRole = normalizeRole(directRelation, directPerson);
    const directPersonObservation = normalizePerson(directPerson, update.nickname);
    observed.set(
      observationKey("model_values_and_references", "direct_runtime"),
      { ...directPersonObservation, reference: directRole.reference },
    );
    observed.set(
      observationKey("hydrated_role_result", "direct_runtime"),
      directRole.hydrated,
    );
    observed.set(observationKey("role_traversal", "direct_runtime"), directRole.traversal);

    const remoteRelationVariable = remoteSession.exact(Membership);
    const remotePersonVariable = remoteSession.exact(Person);
    const requestsBefore = requests.length;
    const [remoteRelation, remotePerson] = await remoteSession
      .query(remoteRelationVariable, remotePersonVariable)
      .where(
        remoteRelationVariable.role(Membership.member).connects(remotePersonVariable),
        remotePersonVariable
          .field(Person.identifier)
          .eq(Identifier.create(fields.identifier.value)),
      )
      .one();
    const exchangeCount = requests.length - requestsBefore;
    assert.equal(exchangeCount, 1);
    assert.ok(requests.slice(requestsBefore).every((request) => request.length > 0));
    const remoteRole = normalizeRole(remoteRelation, remotePerson);
    const remotePersonObservation = normalizePerson(remotePerson, update.nickname);
    observed.set(
      observationKey("model_values_and_references", "remote_runtime"),
      { ...remotePersonObservation, reference: remoteRole.reference },
    );
    observed.set(
      observationKey("hydrated_role_result", "remote_runtime"),
      remoteRole.hydrated,
    );
    observed.set(observationKey("role_traversal", "remote_runtime"), remoteRole.traversal);
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
  return workforceResults(catalog, journey, observed);
}

function workforceV2Key(value: any): string {
  assert.equal(typeof value.identifier?.value, "string");
  return value.identifier.value;
}

function workforceV2Model(generated: any, value: any): string {
  for (const [model, name] of [
    [generated.Person, "person"],
    [generated.Employee, "employee"],
    [generated.Manager, "manager"],
    [generated.Membership, "membership"],
    [generated.NetworkLink, "network-link"],
  ] as const) {
    if (value.__typebridgeModel === model.typeKey) return name;
  }
  throw new TypeError(`unexpected workforce-v2 projected model ${value.__typebridgeModel}`);
}

function workforceV2ModelKey(
  generated: any,
  value: any,
): Record<string, string> {
  return { model: workforceV2Model(generated, value), key: workforceV2Key(value) };
}

function workforceV2Keys(values: readonly any[]): string[] {
  return values.map(workforceV2Key);
}

function workforceV2PersonValues(
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
    key: workforceV2Key(person),
    model: workforceV2Model(generated, person),
    nickname: person.nickname.value,
    reference: workforceV2ModelKey(generated, membership.member),
    scalar_domains: scalarFields.map(([domain]) => domain),
  };
}

function workforceV2RoleObservation(
  generated: any,
  membership: any,
  network: any,
): Record<string, unknown> {
  return {
    membership: {
      relation: workforceV2Model(generated, membership),
      role: "member",
      players: [workforceV2ModelKey(generated, membership.member)],
    },
    network_link: {
      relation: workforceV2Model(generated, network),
      origin: workforceV2Key(network.origin),
      destination: workforceV2Key(network.destination),
      participants: workforceV2Keys(network.participant).sort(compareText),
    },
  };
}

function workforceV2HydratedResult(
  generated: any,
  membership: any,
): Record<string, unknown> {
  return {
    rows: [{
      model: workforceV2Model(generated, membership),
      roles: { member: [workforceV2ModelKey(generated, membership.member)] },
    }],
  };
}

function workforceV2CardinalityDiagnostic(error: any): Record<string, unknown> {
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

async function workforceV2ResourceLimits(
  generated: any,
  database: any,
  advertisement: Uint8Array,
  exchange: (request: Uint8Array, signal?: AbortSignal) => Promise<Uint8Array>,
  membershipIid: string,
): Promise<Record<string, unknown>> {
  const maxima = {
    timeout_milliseconds: 30_000,
    items: 65_536,
    bytes: 33_554_432,
    graph_nodes: 65_536,
    attribute_values: 65_536,
    collection_members: 65_536,
    role_players: 65_536,
    statements: 3,
  };
  const toOptions = (offset: number): Record<string, bigint> => ({
    timeoutMilliseconds: BigInt(maxima.timeout_milliseconds + offset),
    items: BigInt(maxima.items + offset),
    bytes: BigInt(maxima.bytes + offset),
    graphNodes: BigInt(maxima.graph_nodes + offset),
    attributeValues: BigInt(maxima.attribute_values + offset),
    collectionMembers: BigInt(maxima.collection_members + offset),
    rolePlayers: BigInt(maxima.role_players + offset),
    statements: BigInt(maxima.statements + offset),
  });
  const plus = new generated.QueryExecutionResourceLimits(toOptions(1));
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
  const maximaValues = Object.values(maxima);
  const plusOneClampedAll = values(plus).every((value, index) => value === maximaValues[index]);
  const zeroTighteningAll = values(zero).every((value) => value === 0);
  const errors: readonly [string, string][] = await Promise.all(
    [false, true].map(async (remote) => {
      const limits = new generated.QueryExecutionResourceLimits({ rolePlayers: 0n });
      const session = remote
        ? new generated.RemoteQuerySession(advertisement, exchange, limits)
        : new generated.QuerySession(database, { resources: limits });
      const relation = session.exact(generated.Membership);
      const query = session.query(relation).where(relation.iid(membershipIid));
      try {
        if (remote) await query.one();
        else query.one();
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
  return {
    hard_maxima: maxima,
    plus_one_clamped_all: plusOneClampedAll,
    zero_tightening_all: zeroTighteningAll,
    enforced: {
      dimension: "role_players",
      category: errors[0][0],
      code: errors[0][1],
      no_partial_result: true,
    },
  };
}

async function workforceV2Lifecycle(
  generated: any,
  database: any,
  advertisement: Uint8Array,
  exchange: (request: Uint8Array, signal?: AbortSignal) => Promise<Uint8Array>,
  requests: readonly Uint8Array[],
): Promise<Record<string, unknown>> {
  const directSession = new generated.QuerySession(database);
  const directPerson = directSession.exact(generated.Person);
  const directIdentifier = directPerson.field(generated.Person.identifier);
  const directScope = directIdentifier.eq(generated.Identifier.create("query-ada"));
  const directAncestor = directSession.query(directPerson).where(directScope);
  const directSibling = directAncestor.clone();
  const directClosedDescendant = directAncestor.where(
    directPerson.field(generated.Person.score).gte(generated.Score.create(1n)),
  );
  directClosedDescendant.close();
  directClosedDescendant.close();
  const directAncestorUsable = workforceV2Key(directAncestor.one()) === "query-ada";
  const directDescendant = directAncestor.where(
    directPerson.field(generated.Person.score).gte(generated.Score.create(1n)),
  );
  const directResult = directDescendant.one();
  directAncestor.close();
  directAncestor.close();
  let directRejected = false;
  try {
    directAncestor.one();
  } catch (error) {
    directRejected = (error as any).code === "query_resource_closed";
  }
  const direct = {
    ancestor_usable_after_descendant_close: directAncestorUsable,
    close_idempotent: directAncestor.isClosed && directClosedDescendant.isClosed,
    descendant_usable_after_ancestor_close:
      workforceV2Key(directDescendant.one()) === "query-ada",
    handle_invalidated: directAncestor.isClosed,
    post_close_io_count: 0,
    post_close_rejected: directRejected,
    session_usable_after_query_close:
      workforceV2Key(directSession.query(directPerson).where(directScope).one()) === "query-ada",
    sibling_usable: workforceV2Key(directSibling.one()) === "query-ada",
  };
  directDescendant.close();
  directSibling.close();
  directSession.close();

  const remoteSession = new generated.RemoteQuerySession(
    advertisement,
    exchange,
    new generated.QueryExecutionResourceLimits(),
  );
  const remotePerson = remoteSession.exact(generated.Person);
  const remoteIdentifier = remotePerson.field(generated.Person.identifier);
  const remoteScope = remoteIdentifier.eq(generated.Identifier.create("query-ada"));
  const remoteAncestor = remoteSession.query(remotePerson).where(remoteScope);
  const remoteSibling = remoteAncestor.clone();
  const remoteClosedDescendant = remoteAncestor.where(
    remotePerson.field(generated.Person.score).gte(generated.Score.create(1n)),
  );
  remoteClosedDescendant.close();
  remoteClosedDescendant.close();
  const remoteAncestorUsable = workforceV2Key(await remoteAncestor.one()) === "query-ada";
  const remoteDescendant = remoteAncestor.where(
    remotePerson.field(generated.Person.score).gte(generated.Score.create(1n)),
  );
  const remoteResult = await remoteDescendant.one();
  remoteAncestor.close();
  remoteAncestor.close();
  const requestsBeforeRejection = requests.length;
  let remoteRejected = false;
  try {
    await remoteAncestor.one();
  } catch (error) {
    remoteRejected = (error as any).code === "query_resource_closed";
  }
  const postCloseIoCount = requests.length - requestsBeforeRejection;
  const remote = {
    ancestor_usable_after_descendant_close: remoteAncestorUsable,
    close_idempotent: remoteAncestor.isClosed && remoteClosedDescendant.isClosed,
    descendant_usable_after_ancestor_close:
      workforceV2Key(await remoteDescendant.one()) === "query-ada",
    handle_invalidated: remoteAncestor.isClosed,
    post_close_io_count: postCloseIoCount,
    post_close_rejected: remoteRejected,
    session_usable_after_query_close:
      workforceV2Key(await remoteSession.query(remotePerson).where(remoteScope).one()) === "query-ada",
    sibling_usable: workforceV2Key(await remoteSibling.one()) === "query-ada",
  };
  remoteDescendant.close();
  remoteSibling.close();
  remoteSession.close();
  assert.deepEqual(remote, direct);
  return {
    lanes: ["direct", "remote"],
    query: direct,
    result_usable_after_query_close:
      workforceV2Key(directResult) === "query-ada"
      && workforceV2Key(remoteResult) === "query-ada",
  };
}

async function runWorkforceV2Journey(
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
      assert.equal(workforceV2Key(people[index]), workforceV2Key(personInputs[index]));
      inserted.push([personManager, people[index]]);
    }
    insertedCount = inserted.length;

    employee = employeeManager.insert(employeeInput);
    assert.equal(workforceV2Key(employee), workforceV2Key(employeeInput));
    inserted.push([employeeManager, employee]);
    insertedCount = inserted.length;

    manager = managerManager.insert(managerInput);
    assert.equal(workforceV2Key(manager), workforceV2Key(managerInput));
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
    assert.equal(workforceV2Key(network), networkFields.identifier.value);
    inserted.push([networkManager, network]);
    insertedCount = inserted.length;

    for (const [, value] of inserted) assertIid(value.iid);
    const personIids = people.map((value: any) => value.iid as string);
    const membershipIid = membership.iid as string;
    const networkIid = network.iid as string;
    const entityReadAfterCreate = personManager.getByIid(personIids[0]) !== null;
    const relationReadAfterCreate = membershipManager.getByIid(membershipIid) !== null;

    const directSession = new generated.QuerySession(database);
    const directPerson = directSession.exact(Person);
    const directIdentifier = directPerson.field(Person.identifier);
    const directScope = directIdentifier.eq(Identifier.create("query-ada"))
      .or(directIdentifier.eq(Identifier.create("query-dana")));
    const directQuery = directSession.query(directPerson).where(directScope);
    const directKeys = directQuery
      .rows({ limit: 2n, orderBy: [directIdentifier.asc()] })
      .map((person: any) => person.identifier.value);
    assert.deepEqual(directKeys, ["query-ada", "query-dana"]);
    const directAda = directSession
      .query(directPerson)
      .where(directIdentifier.eq(Identifier.create("query-ada")))
      .one();

    const directMembershipVar = directSession.exact(Membership);
    const directMember = directSession.exact(Person);
    const directMembership = directSession
      .query(directMembershipVar)
      .match(directMember)
      .where(
        directMembershipVar.iid(membershipIid),
        directMembershipVar.role(Membership.member).connects(directMember),
        directMember.field(Person.identifier).eq(Identifier.create("query-ada")),
      )
      .one();
    const directNetworkVar = directSession.exact(NetworkLink);
    const directOrigin = directSession.exact(Person);
    const directDestination = directSession.exact(Person);
    const directNetwork = directSession
      .query(directNetworkVar)
      .match(directOrigin, directDestination)
      .where(
        directNetworkVar.iid(networkIid),
        directNetworkVar.role(NetworkLink.origin).connects(directOrigin),
        directNetworkVar.role(NetworkLink.destination).connects(directDestination),
        directOrigin.field(Person.identifier).eq(Identifier.create("query-ada")),
        directDestination.field(Person.identifier).eq(Identifier.create("query-dana")),
      )
      .one();

    const directOwnerKeys = workforceV2Keys(
      directSession
        .query(directPerson)
        .where(directPerson.field(Person.score).isPresent(), directScope)
        .rows({ limit: 2n, orderBy: [directIdentifier.asc()] }),
    );
    const directOptionalKeys = workforceV2Keys(
      directSession
        .query(directPerson)
        .where(directPerson.field(Person.nickname).isPresent(), directScope)
        .rows({ limit: 2n, orderBy: [directIdentifier.asc()] }),
    );
    const directIidKeys = workforceV2Keys(
      directSession
        .query(directPerson)
        .where(directPerson.iidIn(personIids))
        .rows({ limit: 2n, orderBy: [directIdentifier.asc()] }),
    );

    const directEmployeeExact = directSession.exact(Employee);
    const directEmployeeExactId = directEmployeeExact.field(Employee.identifier);
    const directExactValues = directSession
      .query(directEmployeeExact)
      .where(
        directEmployeeExactId.eq(Identifier.create("query-employee"))
          .or(directEmployeeExactId.eq(Identifier.create("query-manager"))),
      )
      .rows({ limit: 2n, orderBy: [directEmployeeExactId.asc()] });
    const directEmployeeSubtypes = directSession.subtypes(Employee);
    const directEmployeeSubtypesId = directEmployeeSubtypes.field(Employee.identifier);
    const directSubtypeValues = directSession
      .query(directEmployeeSubtypes)
      .where(
        directEmployeeSubtypesId.eq(Identifier.create("query-employee"))
          .or(directEmployeeSubtypesId.eq(Identifier.create("query-manager"))),
      )
      .rows({ limit: 2n, orderBy: [directEmployeeSubtypesId.asc()] });

    const directScore = directPerson.field(Person.score);
    const directScoreGte = directPerson.field(Person.scoreGte);
    const directBoolean = directPerson.field(Person.valBool);
    const directAndKeys = workforceV2Keys(
      directSession
        .query(directPerson)
        .where(
          directScope,
          directScore.gte(Score.create(40n)).and(directBoolean.eq(ValBool.create(true))),
        )
        .rows({ limit: 2n, orderBy: [directIdentifier.asc()] }),
    );
    const directOrKeys = workforceV2Keys(
      directSession
        .query(directPerson)
        .where(directScope)
        .rows({ limit: 2n, orderBy: [directIdentifier.asc()] }),
    );
    const directNotKeys = workforceV2Keys(
      directSession
        .query(directPerson)
        .where(directScope, directBoolean.eq(ValBool.create(true)).not())
        .rows({ limit: 2n, orderBy: [directIdentifier.asc()] }),
    );
    const directFieldComparisonKeys = workforceV2Keys(
      directSession
        .query(directPerson)
        .where(directScope, directScore.gteField(directScoreGte))
        .rows({ limit: 2n, orderBy: [directIdentifier.asc()] }),
    );

    const directSource = directSession.exact(Person);
    const directTarget = directSession.exact(Person);
    const directReachable = directSession.reachable(
      directSource,
      directTarget,
      NetworkLink,
      NetworkLink.origin,
      NetworkLink.destination,
      { minDepth: 1, maxDepth: 1 },
    );
    const directReachablePair = directSession
      .query(directSource, directTarget)
      .where(
        directReachable,
        directSource.field(Person.identifier).eq(Identifier.create("query-ada")),
        directTarget.field(Person.identifier).eq(Identifier.create("query-dana")),
      )
      .one();
    const directCrossLeft = directSession.exact(Person);
    const directCrossRight = directSession.exact(Person);
    const directCrossLeftId = directCrossLeft.field(Person.identifier);
    const directCrossRightId = directCrossRight.field(Person.identifier);
    const directCrossScope = directCrossLeftId.eq(Identifier.create("query-ada"))
      .or(directCrossLeftId.eq(Identifier.create("query-dana")))
      .and(
        directCrossRightId.eq(Identifier.create("query-ada"))
          .or(directCrossRightId.eq(Identifier.create("query-dana"))),
      );
    const directCrossPairs = directSession
      .query(directCrossLeft, directCrossRight)
      .allowCrossJoin(directCrossLeft, directCrossRight)
      .where(directCrossScope)
      .rows({
        limit: 4n,
        orderBy: [directCrossLeftId.asc(), directCrossRightId.asc()],
      })
      .map(([left, right]: readonly any[]) => [workforceV2Key(left), workforceV2Key(right)]);

    const selectionLink = directSession.exact(NetworkLink);
    const selectionOrigin = directSession.exact(Person);
    const selectionParticipant = directSession.exact(Person);
    const selectionPredicates = [
      selectionLink.iid(networkIid),
      selectionLink.role(NetworkLink.origin).connects(selectionOrigin),
      selectionLink.role(NetworkLink.participant).connects(selectionParticipant),
    ] as const;
    const selectionParticipants = selectionParticipant
      .collect()
      .distinct()
      .orderBy(selectionParticipant.field(Person.identifier).asc());
    const directPositionalPage = directSession
      .query(selectionOrigin, selectionParticipants)
      .match(selectionLink)
      .where(...selectionPredicates)
      .pageBy(selectionOrigin, {
        limit: 1n,
        orderBy: [selectionOrigin.field(Person.identifier).asc()],
        includeTotal: true,
      });
    assert.equal(Number(directPositionalPage.total), 1);
    assert.equal(directPositionalPage.items.length, 1);
    const [positionalOrigin, positionalParticipants] = directPositionalPage.items[0];
    const directNamedPage = directSession
      .queryNamed({ origin: selectionOrigin, participants: selectionParticipants })
      .match(selectionLink)
      .where(...selectionPredicates)
      .pageBy(selectionOrigin, {
        limit: 1n,
        orderBy: [selectionOrigin.field(Person.identifier).asc()],
        includeTotal: true,
      });
    assert.equal(Number(directNamedPage.total), 1);
    assert.equal(directNamedPage.items.length, 1);
    const directNamed = directNamedPage.items[0];
    const directSelection = {
      positional: [workforceV2Key(positionalOrigin), workforceV2Keys(positionalParticipants)],
      named: {
        origin: workforceV2Key(directNamed.origin),
        participants: workforceV2Keys(directNamed.participants),
      },
      collected_distinct:
        new Set(positionalParticipants.map((value: any) => value.iid)).size
        === positionalParticipants.length,
      collection_order: "identifier_asc",
    };

    const directDana = directSession
      .query(directPerson)
      .where(directIdentifier.eq(Identifier.create("query-dana")))
      .one();
    const directFirst = directQuery.first({ orderBy: [directIdentifier.asc()] });
    const directPage = directQuery.pageBy(directPerson, {
      limit: 1n,
      orderBy: [directIdentifier.asc()],
      includeTotal: true,
    });
    const directTerminals = {
      one: workforceV2Key(directDana),
      first: workforceV2Key(directFirst),
      rows: directKeys,
      page: {
        items: workforceV2Keys(directPage.items),
        offset: Number(directPage.offset),
        limit: Number(directPage.limit),
        total: Number(directPage.total),
      },
      count: Number(directQuery.countBy(directPerson)),
      exists: directQuery.existsBy(directPerson),
    };
    let structuredQueryDiagnostic: Record<string, unknown> | undefined;
    try {
      directQuery.one();
    } catch (error) {
      structuredQueryDiagnostic = workforceV2CardinalityDiagnostic(error);
    }
    assert.notEqual(structuredQueryDiagnostic, undefined);
    const directScalarDomainKeys = workforceV2Keys(
      directSession
        .query(directPerson)
        .where(directScope, directScore.gte(Score.create(40n)))
        .rows({ limit: 2n, orderBy: [directIdentifier.asc()] }),
    );

    const normalizeReducers = (
      values: readonly unknown[],
      binding: readonly any[],
      field: readonly any[],
      tuple: readonly any[],
    ): Record<string, unknown> => {
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
    };
    const directValues = directQuery.aggregate(directPerson, [
      aggregate.count(),
      aggregate.sum(directScore),
      aggregate.min(directScore),
      aggregate.max(directScore),
      aggregate.mean(directScore),
      aggregate.median(directScore),
      aggregate.std(directScore),
    ] as const);
    const directGroupPerson = directSession.exact(Person);
    const directGroupIdentifier = directGroupPerson.field(Person.identifier);
    const directBinding = directQuery
      .match(directGroupPerson)
      .where(directIdentifier.eqField(directGroupIdentifier))
      .groupBy(directPerson, directGroupPerson)
      .aggregate([aggregate.count()] as const);
    const directField = directQuery
      .groupBy(directPerson, directScore)
      .aggregate([aggregate.count()] as const);
    const directTuple = directQuery
      .groupBy(directPerson, directScore, directPerson.field(Person.scoreGte))
      .aggregate([aggregate.count()] as const);
    const directReducers = normalizeReducers(
      directValues,
      directBinding,
      directField,
      directTuple,
    );
    assert.deepEqual(directReducers, expected.grouped_reducer);

    const directMinimum = integerInput(directSession, Score.create(30n));
    const directCall = qualifyingScore(directSession, directPerson, directMinimum);
    const directFunctionValues = directQuery
      .where(directCall.gteField(directScore))
      .rows({ limit: 2n, orderBy: [directIdentifier.asc()] })
      .map((person: any) => Number(person.score.value));
    assert.deepEqual(directFunctionValues, expected.schema_function.values);
    const directNested = qualifyingScore(directSession, directPerson, directCall);
    const directNestedFunctionValues = directQuery
      .where(directNested.gteField(directScore))
      .rows({ limit: 2n, orderBy: [directIdentifier.asc()] })
      .map((person: any) => Number(person.score.value));
    assert.deepEqual(
      directNestedFunctionValues,
      expected.schema_function.nested_values,
    );

    const remotePerson = remoteSession.exact(Person);
    const remoteIdentifier = remotePerson.field(Person.identifier);
    const remoteScope = remoteIdentifier.eq(Identifier.create("query-ada"))
      .or(remoteIdentifier.eq(Identifier.create("query-dana")));
    const remoteQuery = remoteSession.query(remotePerson).where(remoteScope);
    const remoteKeys = (await remoteQuery.rows({
      limit: 2n,
      orderBy: [remoteIdentifier.asc()],
    })).map((person: any) => person.identifier.value);
    assert.deepEqual(remoteKeys, directKeys);
    const remoteAda = await remoteSession
      .query(remotePerson)
      .where(remoteIdentifier.eq(Identifier.create("query-ada")))
      .one();
    const remoteMembershipVar = remoteSession.exact(Membership);
    const remoteMember = remoteSession.exact(Person);
    const remoteMembership = await remoteSession
      .query(remoteMembershipVar)
      .match(remoteMember)
      .where(
        remoteMembershipVar.iid(membershipIid),
        remoteMembershipVar.role(Membership.member).connects(remoteMember),
        remoteMember.field(Person.identifier).eq(Identifier.create("query-ada")),
      )
      .one();
    const remoteNetworkVar = remoteSession.exact(NetworkLink);
    const remoteOrigin = remoteSession.exact(Person);
    const remoteDestination = remoteSession.exact(Person);
    const remoteNetwork = await remoteSession
      .query(remoteNetworkVar)
      .match(remoteOrigin, remoteDestination)
      .where(
        remoteNetworkVar.iid(networkIid),
        remoteNetworkVar.role(NetworkLink.origin).connects(remoteOrigin),
        remoteNetworkVar.role(NetworkLink.destination).connects(remoteDestination),
        remoteOrigin.field(Person.identifier).eq(Identifier.create("query-ada")),
        remoteDestination.field(Person.identifier).eq(Identifier.create("query-dana")),
      )
      .one();

    const remoteOwnerKeys = workforceV2Keys(
      await remoteSession
        .query(remotePerson)
        .where(remotePerson.field(Person.score).isPresent(), remoteScope)
        .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }),
    );
    const remoteOptionalKeys = workforceV2Keys(
      await remoteSession
        .query(remotePerson)
        .where(remotePerson.field(Person.nickname).isPresent(), remoteScope)
        .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }),
    );
    const remoteIidKeys = workforceV2Keys(
      await remoteSession
        .query(remotePerson)
        .where(remotePerson.iidIn(personIids))
        .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }),
    );

    const remoteEmployeeExact = remoteSession.exact(Employee);
    const remoteEmployeeExactId = remoteEmployeeExact.field(Employee.identifier);
    const remoteExactValues = await remoteSession
      .query(remoteEmployeeExact)
      .where(
        remoteEmployeeExactId.eq(Identifier.create("query-employee"))
          .or(remoteEmployeeExactId.eq(Identifier.create("query-manager"))),
      )
      .rows({ limit: 2n, orderBy: [remoteEmployeeExactId.asc()] });
    const remoteEmployeeSubtypes = remoteSession.subtypes(Employee);
    const remoteEmployeeSubtypesId = remoteEmployeeSubtypes.field(Employee.identifier);
    const remoteSubtypeValues = await remoteSession
      .query(remoteEmployeeSubtypes)
      .where(
        remoteEmployeeSubtypesId.eq(Identifier.create("query-employee"))
          .or(remoteEmployeeSubtypesId.eq(Identifier.create("query-manager"))),
      )
      .rows({ limit: 2n, orderBy: [remoteEmployeeSubtypesId.asc()] });

    const remoteScore = remotePerson.field(Person.score);
    const remoteScoreGte = remotePerson.field(Person.scoreGte);
    const remoteBoolean = remotePerson.field(Person.valBool);
    const remoteAndKeys = workforceV2Keys(
      await remoteSession
        .query(remotePerson)
        .where(
          remoteScope,
          remoteScore.gte(Score.create(40n)).and(remoteBoolean.eq(ValBool.create(true))),
        )
        .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }),
    );
    const remoteOrKeys = workforceV2Keys(
      await remoteSession
        .query(remotePerson)
        .where(remoteScope)
        .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }),
    );
    const remoteNotKeys = workforceV2Keys(
      await remoteSession
        .query(remotePerson)
        .where(remoteScope, remoteBoolean.eq(ValBool.create(true)).not())
        .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }),
    );
    const remoteFieldComparisonKeys = workforceV2Keys(
      await remoteSession
        .query(remotePerson)
        .where(remoteScope, remoteScore.gteField(remoteScoreGte))
        .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }),
    );

    const remoteSource = remoteSession.exact(Person);
    const remoteTarget = remoteSession.exact(Person);
    const remoteReachable = remoteSession.reachable(
      remoteSource,
      remoteTarget,
      NetworkLink,
      NetworkLink.origin,
      NetworkLink.destination,
      { minDepth: 1, maxDepth: 1 },
    );
    const remoteReachablePair = await remoteSession
      .query(remoteSource, remoteTarget)
      .where(
        remoteReachable,
        remoteSource.field(Person.identifier).eq(Identifier.create("query-ada")),
        remoteTarget.field(Person.identifier).eq(Identifier.create("query-dana")),
      )
      .one();
    const remoteCrossLeft = remoteSession.exact(Person);
    const remoteCrossRight = remoteSession.exact(Person);
    const remoteCrossLeftId = remoteCrossLeft.field(Person.identifier);
    const remoteCrossRightId = remoteCrossRight.field(Person.identifier);
    const remoteCrossScope = remoteCrossLeftId.eq(Identifier.create("query-ada"))
      .or(remoteCrossLeftId.eq(Identifier.create("query-dana")))
      .and(
        remoteCrossRightId.eq(Identifier.create("query-ada"))
          .or(remoteCrossRightId.eq(Identifier.create("query-dana"))),
      );
    const remoteCrossPairs = (await remoteSession
      .query(remoteCrossLeft, remoteCrossRight)
      .allowCrossJoin(remoteCrossLeft, remoteCrossRight)
      .where(remoteCrossScope)
      .rows({
        limit: 4n,
        orderBy: [remoteCrossLeftId.asc(), remoteCrossRightId.asc()],
      }))
      .map(([left, right]: readonly any[]) => [workforceV2Key(left), workforceV2Key(right)]);

    const remoteSelectionLink = remoteSession.exact(NetworkLink);
    const remoteSelectionOrigin = remoteSession.exact(Person);
    const remoteSelectionParticipant = remoteSession.exact(Person);
    const remoteSelectionPredicates = [
      remoteSelectionLink.iid(networkIid),
      remoteSelectionLink.role(NetworkLink.origin).connects(remoteSelectionOrigin),
      remoteSelectionLink.role(NetworkLink.participant).connects(remoteSelectionParticipant),
    ] as const;
    const remoteSelectionParticipants = remoteSelectionParticipant
      .collect()
      .distinct()
      .orderBy(remoteSelectionParticipant.field(Person.identifier).asc());
    const remotePositionalPage = await remoteSession
      .query(remoteSelectionOrigin, remoteSelectionParticipants)
      .match(remoteSelectionLink)
      .where(...remoteSelectionPredicates)
      .pageBy(remoteSelectionOrigin, {
        limit: 1n,
        orderBy: [remoteSelectionOrigin.field(Person.identifier).asc()],
        includeTotal: true,
      });
    assert.equal(Number(remotePositionalPage.total), 1);
    assert.equal(remotePositionalPage.items.length, 1);
    const [remotePositionalOrigin, remotePositionalParticipants] =
      remotePositionalPage.items[0];
    const remoteNamedPage = await remoteSession
      .queryNamed({ origin: remoteSelectionOrigin, participants: remoteSelectionParticipants })
      .match(remoteSelectionLink)
      .where(...remoteSelectionPredicates)
      .pageBy(remoteSelectionOrigin, {
        limit: 1n,
        orderBy: [remoteSelectionOrigin.field(Person.identifier).asc()],
        includeTotal: true,
      });
    assert.equal(Number(remoteNamedPage.total), 1);
    assert.equal(remoteNamedPage.items.length, 1);
    const remoteNamed = remoteNamedPage.items[0];
    const remoteSelection = {
      positional: [
        workforceV2Key(remotePositionalOrigin),
        workforceV2Keys(remotePositionalParticipants),
      ],
      named: {
        origin: workforceV2Key(remoteNamed.origin),
        participants: workforceV2Keys(remoteNamed.participants),
      },
      collected_distinct:
        new Set(remotePositionalParticipants.map((value: any) => value.iid)).size
        === remotePositionalParticipants.length,
      collection_order: "identifier_asc",
    };

    const requestsBeforeOne = requests.length;
    const remoteDana = await remoteSession
      .query(remotePerson)
      .where(remoteIdentifier.eq(Identifier.create("query-dana")))
      .one();
    const remoteOneExchange = requests.length - requestsBeforeOne;
    const remoteFirst = await remoteQuery.first({ orderBy: [remoteIdentifier.asc()] });
    const remotePage = await remoteQuery.pageBy(remotePerson, {
      limit: 1n,
      orderBy: [remoteIdentifier.asc()],
      includeTotal: true,
    });
    const remoteTerminals = {
      one: workforceV2Key(remoteDana),
      first: workforceV2Key(remoteFirst),
      rows: remoteKeys,
      page: {
        items: workforceV2Keys(remotePage.items),
        offset: Number(remotePage.offset),
        limit: Number(remotePage.limit),
        total: Number(remotePage.total),
      },
      count: Number(await remoteQuery.countBy(remotePerson)),
      exists: await remoteQuery.existsBy(remotePerson),
    };
    const remoteScalarDomainKeys = workforceV2Keys(
      await remoteSession
        .query(remotePerson)
        .where(remoteScope, remoteScore.gte(Score.create(40n)))
        .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }),
    );
    const remoteValues = await remoteQuery.aggregate(remotePerson, [
      aggregate.count(),
      aggregate.sum(remoteScore),
      aggregate.min(remoteScore),
      aggregate.max(remoteScore),
      aggregate.mean(remoteScore),
      aggregate.median(remoteScore),
      aggregate.std(remoteScore),
    ] as const);
    const remoteGroupPerson = remoteSession.exact(Person);
    const remoteGroupIdentifier = remoteGroupPerson.field(Person.identifier);
    const remoteBinding = await remoteQuery
      .match(remoteGroupPerson)
      .where(remoteIdentifier.eqField(remoteGroupIdentifier))
      .groupBy(remotePerson, remoteGroupPerson)
      .aggregate([aggregate.count()] as const);
    const remoteField = await remoteQuery
      .groupBy(remotePerson, remoteScore)
      .aggregate([aggregate.count()] as const);
    const remoteTuple = await remoteQuery
      .groupBy(remotePerson, remoteScore, remotePerson.field(Person.scoreGte))
      .aggregate([aggregate.count()] as const);
    const remoteReducers = normalizeReducers(
      remoteValues,
      remoteBinding,
      remoteField,
      remoteTuple,
    );
    assert.deepEqual(remoteReducers, directReducers);

    const remoteMinimum = integerInput(remoteSession, Score.create(30n));
    const remoteCall = qualifyingScore(remoteSession, remotePerson, remoteMinimum);
    const remoteFunctionValues = (await remoteQuery
      .where(remoteCall.gteField(remoteScore))
      .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }))
      .map((person: any) => Number(person.score.value));
    assert.deepEqual(remoteFunctionValues, directFunctionValues);
    const remoteNested = qualifyingScore(remoteSession, remotePerson, remoteCall);
    const remoteNestedFunctionValues = (await remoteQuery
      .where(remoteNested.gteField(remoteScore))
      .rows({ limit: 2n, orderBy: [remoteIdentifier.asc()] }))
      .map((person: any) => Number(person.score.value));
    assert.deepEqual(remoteNestedFunctionValues, directNestedFunctionValues);

    const directModelValues = workforceV2PersonValues(generated, directAda, directMembership);
    const remoteModelValues = workforceV2PersonValues(generated, remoteAda, remoteMembership);
    assert.deepEqual(remoteModelValues, directModelValues);
    const ownerIidSetDirect = {
      owner_field: "score",
      owner_keys: directOwnerKeys,
      optional_field: "nickname",
      optional_present_keys: directOptionalKeys,
      iid_set_keys: directIidKeys,
    };
    const ownerIidSetRemote = {
      owner_field: "score",
      owner_keys: remoteOwnerKeys,
      optional_field: "nickname",
      optional_present_keys: remoteOptionalKeys,
      iid_set_keys: remoteIidKeys,
    };
    assert.deepEqual(ownerIidSetRemote, ownerIidSetDirect);
    const exactSubtypesDirect = {
      declared_model: "employee",
      exact: directExactValues.map((value: any) => workforceV2ModelKey(generated, value)),
      subtypes: directSubtypeValues.map(
        (value: any) => workforceV2ModelKey(generated, value),
      ),
    };
    const exactSubtypesRemote = {
      declared_model: "employee",
      exact: remoteExactValues.map((value: any) => workforceV2ModelKey(generated, value)),
      subtypes: remoteSubtypeValues.map(
        (value: any) => workforceV2ModelKey(generated, value),
      ),
    };
    assert.deepEqual(exactSubtypesRemote, exactSubtypesDirect);
    const scalarBooleanDirect = {
      and_keys: directAndKeys,
      or_keys: directOrKeys,
      not_keys: directNotKeys,
      field_comparison_keys: directFieldComparisonKeys,
    };
    const scalarBooleanRemote = {
      and_keys: remoteAndKeys,
      or_keys: remoteOrKeys,
      not_keys: remoteNotKeys,
      field_comparison_keys: remoteFieldComparisonKeys,
    };
    assert.deepEqual(scalarBooleanRemote, scalarBooleanDirect);
    const directRoles = workforceV2RoleObservation(generated, directMembership, directNetwork);
    const remoteRoles = workforceV2RoleObservation(generated, remoteMembership, remoteNetwork);
    assert.deepEqual(remoteRoles, directRoles);
    const topologyDirect = {
      reachable: [{
        from: workforceV2Key(directReachablePair[0]),
        to: workforceV2Key(directReachablePair[1]),
        max_hops: 1,
      }],
      cross_join_pairs: directCrossPairs,
    };
    const topologyRemote = {
      reachable: [{
        from: workforceV2Key(remoteReachablePair[0]),
        to: workforceV2Key(remoteReachablePair[1]),
        max_hops: 1,
      }],
      cross_join_pairs: remoteCrossPairs,
    };
    assert.deepEqual(topologyRemote, topologyDirect);
    assert.deepEqual(remoteSelection, directSelection);
    assert.deepEqual(remoteTerminals, directTerminals);
    const directHydrated = workforceV2HydratedResult(generated, directMembership);
    const remoteHydrated = workforceV2HydratedResult(generated, remoteMembership);
    assert.deepEqual(remoteHydrated, directHydrated);
    const scalarDomainDirect = {
      domain: "long",
      operator: "gte",
      operand: 40,
      keys: directScalarDomainKeys,
    };
    const scalarDomainRemote = {
      domain: "long",
      operator: "gte",
      operand: 40,
      keys: remoteScalarDomainKeys,
    };
    assert.deepEqual(scalarDomainRemote, scalarDomainDirect);
    assert.equal(remoteOneExchange, 1);
    const resourceLimits = await workforceV2ResourceLimits(
      generated,
      database,
      advertisement,
      exchange,
      membershipIid,
    );
    const queryResourceLifecycle = await workforceV2Lifecycle(
      generated,
      database,
      advertisement,
      exchange,
      requests,
    );

    const observed = new Map<string, Record<string, unknown>>();
    observed.set(
      observationKey("model_values_and_references", "direct_runtime"),
      directModelValues,
    );
    observed.set(
      observationKey("model_values_and_references", "remote_runtime"),
      remoteModelValues,
    );
    observed.set(observationKey("owner_iid_set", "direct_runtime"), ownerIidSetDirect);
    observed.set(observationKey("owner_iid_set", "remote_runtime"), ownerIidSetRemote);
    observed.set(observationKey("exact_subtypes", "direct_runtime"), exactSubtypesDirect);
    observed.set(observationKey("exact_subtypes", "remote_runtime"), exactSubtypesRemote);
    observed.set(observationKey("scalar_boolean", "direct_runtime"), scalarBooleanDirect);
    observed.set(observationKey("scalar_boolean", "remote_runtime"), scalarBooleanRemote);
    observed.set(observationKey("roles", "direct_runtime"), directRoles);
    observed.set(observationKey("roles", "remote_runtime"), remoteRoles);
    observed.set(observationKey("topology", "direct_runtime"), topologyDirect);
    observed.set(observationKey("topology", "remote_runtime"), topologyRemote);
    observed.set(observationKey("selection_shapes", "direct_runtime"), directSelection);
    observed.set(observationKey("selection_shapes", "remote_runtime"), remoteSelection);
    observed.set(observationKey("terminals", "direct_runtime"), directTerminals);
    observed.set(observationKey("terminals", "remote_runtime"), remoteTerminals);
    observed.set(observationKey("grouped_reducer", "direct_runtime"), directReducers);
    observed.set(observationKey("grouped_reducer", "remote_runtime"), remoteReducers);
    observed.set(observationKey("remote_one_exchange", "remote_runtime"), {
      exchange_count: remoteOneExchange,
      terminal: "one",
    });
    observed.set(observationKey("hydrated_result", "direct_runtime"), directHydrated);
    observed.set(observationKey("hydrated_result", "remote_runtime"), remoteHydrated);
    observed.set(observationKey("scalar_domain", "direct_runtime"), scalarDomainDirect);
    observed.set(observationKey("scalar_domain", "remote_runtime"), scalarDomainRemote);
    observed.set(observationKey("schema_function", "direct_runtime"), {
      minimum: 30,
      values: directFunctionValues,
      nested_values: directNestedFunctionValues,
    });
    observed.set(observationKey("schema_function", "remote_runtime"), {
      minimum: 30,
      values: remoteFunctionValues,
      nested_values: remoteNestedFunctionValues,
    });
    observed.set(
      observationKey("structured_query_diagnostic", "diagnostic"),
      structuredQueryDiagnostic as Record<string, unknown>,
    );
    observed.set(observationKey("resource_limits", "direct_runtime"), resourceLimits);
    observed.set(observationKey("resource_limits", "remote_runtime"), resourceLimits);
    observed.set(
      observationKey("query_resource_lifecycle", "lifecycle"),
      queryResourceLifecycle,
    );
    assert.equal(observed.size, 29, "workforce-v2 must measure 29 pre-cleanup live lanes");
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
      key: workforceV2Key(people[0]),
      model: workforceV2Model(generated, people[0]),
      read_after_create: entityReadAfterCreate,
    });
    observed.set(observationKey("relation_lifecycle", "direct_runtime"), {
      created: true,
      deleted: relationDeleted,
      model: workforceV2Model(generated, membership),
      player_key: workforceV2Key(people[0]),
      role: "member",
    });
    assert.equal(observed.size, 34, "workforce-v2 requires 31 live and 3 proof lanes");
    assert.equal(relationReadAfterCreate, true);
    return workforceResults(catalog, journey, observed);
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

test("workforce report server-version gate is exact", () => {
  assert.doesNotThrow(() => requireWorkforceServerVersion("3.12.3"));
  for (const detected of [null, "3.11.5", "3.12.0", "3.12.2", "3.13.0"] as const) {
    assert.throws(
      () => requireWorkforceServerVersion(detected),
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
  let preparedWorkforceReport: Record<string, unknown> | undefined;
  let preparedWorkforceV2Report: Record<string, unknown> | undefined;
  const workforceReportPath = process.env.TYPE_BRIDGE_WORKFORCE_REPORT;
  const workforceV2ReportPath = process.env.TYPE_BRIDGE_WORKFORCE_REPORT_V2;
  if (
    workforceReportPath !== undefined
    && workforceV2ReportPath !== undefined
    && workforceReportPath === workforceV2ReportPath
  ) {
    throw new Error("workforce-v1 and workforce-v2 reports require distinct paths");
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
      RemoteQuerySession,
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

    let workforceCatalogRaw: Buffer | undefined;
    let workforceCatalog: any;
    let workforceJourneyRaw: Buffer | undefined;
    let workforceJourney: any;
    let workforceV2CatalogRaw: Buffer | undefined;
    let workforceV2Catalog: any;
    let workforceV2JourneyRaw: Buffer | undefined;
    let workforceV2Journey: any;
    let workforceV2ProofObservations:
      | ReadonlyMap<string, Record<string, unknown>>
      | undefined;
    if (workforceReportPath !== undefined) {
      if (REMOTE_PROFILE !== "typedb-3.12.1/v1") {
        throw new Error("workforce reports may only be emitted for typedb-3.12.1/v1");
      }
      await validateWorkforceReportPath(workforceReportPath);
      requireWorkforceServerVersion(
        await detectTypeDBServerVersion(TYPEDB_ADDRESS, TYPEDB_HTTP_PORT),
      );
      workforceCatalogRaw = await readFile(WORKFORCE_CATALOG);
      workforceCatalog = JSON.parse(workforceCatalogRaw.toString("utf8"));
      workforceJourneyRaw = await readFile(WORKFORCE_JOURNEY);
      workforceJourney = JSON.parse(workforceJourneyRaw.toString("utf8"));
      assert.equal(workforceJourney.format, "typebridge.workforce-journey/v1");
      assert.equal(workforceJourney.fixture_id, workforceCatalog.fixture.id);
      assert.equal(workforceJourney.version, workforceCatalog.fixture.version);
    }
    if (workforceV2ReportPath !== undefined) {
      if (REMOTE_PROFILE !== "typedb-3.12.1/v1") {
        throw new Error("workforce-v2 reports may only be emitted for typedb-3.12.1/v1");
      }
      await validateWorkforceReportPath(workforceV2ReportPath);
      requireWorkforceServerVersion(
        await detectTypeDBServerVersion(TYPEDB_ADDRESS, TYPEDB_HTTP_PORT),
      );
      workforceV2CatalogRaw = await readFile(WORKFORCE_V2_CATALOG);
      workforceV2Catalog = JSON.parse(workforceV2CatalogRaw.toString("utf8"));
      workforceV2JourneyRaw = await readFile(WORKFORCE_V2_JOURNEY);
      workforceV2Journey = JSON.parse(workforceV2JourneyRaw.toString("utf8"));
      workforceV2ProofObservations = loadWorkforceV2ProofObservations();
      assert.equal(workforceV2Journey.format, "typebridge.workforce-journey/v2");
      assert.equal(workforceV2Journey.fixture_id, workforceV2Catalog.fixture.id);
      assert.equal(workforceV2Journey.version, workforceV2Catalog.fixture.version);
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
      assert.equal(transactionQuery.countBy(transactionPersonVar), 1n);
      assert.equal(transactionQuery.first()?.iid, transactionPersonIid);
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
    const scalarDomainPerson = querySession
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
    assert.equal(counterQuery.one().iid, insertedCounter.iid);
    assert.throws(
      () => counterQuery.rows({ limit: 2n }),
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
    const [queriedPlainActivity, queriedPlainParticipant] = querySession
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
    const polymorphicActorRows = querySession
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
    const [queriedRobotInteraction, queriedRobot, queriedTarget] = querySession
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
    const [queriedPersonInteraction, queriedPersonActor] = querySession
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

    const personVariable = querySession.var(Person);
    const collectedPersonVariable = querySession.var(Person);
    const eventVariable = querySession.var(Event);
    const identifierOrder = personVariable.field(Person.identifier).asc();
    const personPredicate = personVariable
      .field(Person.identifier)
      .eq(Identifier.create("person-1"));
    const eventPredicate = eventVariable.role(Event.subject).connects(personVariable);
    const sameIdentifierPredicate = collectedPersonVariable
      .field(Person.identifier)
      .eqField(personVariable.field(Person.identifier));
    const directQuery = querySession
      .query(personVariable, eventVariable)
      .where(personPredicate, eventPredicate);
    const directOne = directQuery.one();
    assert.equal(directOne[0].iid, insertedPerson.iid);
    assert.equal(directOne[1].iid, insertedEvent.iid);
    assert.equal(directOne[1].subject.iid, insertedPerson.iid);
    const directRows = querySession
      .query(personVariable)
      .where(personPredicate)
      .rows({ limit: 10n, orderBy: [identifierOrder] });
    assert.equal(directRows.length, 1);
    assert.equal(directRows[0].iid, insertedPerson.iid);
    assert.equal(directRows[0].identifier.value, "person-1");
    assert.deepEqual(
      querySession
        .query(personVariable)
        .where(personVariable.field(Person.aliases).isPresent())
        .rows({ limit: 10n, orderBy: [identifierOrder] })
        .map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedPerson.iid],
    );
    assert.deepEqual(
      querySession
        .query(personVariable)
        .where(personVariable.field(Person.aliases).isMissing())
        .rows({ limit: 10n, orderBy: [identifierOrder] })
        .map((candidate: { readonly iid: string | null }) => candidate.iid),
      [...batchIids, transactionPersonIid],
    );
    assert.equal(
      querySession.query(personVariable).where(personVariable.iid(insertedPerson.iid)).one().iid,
      insertedPerson.iid,
    );
    assert.deepEqual(
      querySession
        .query(personVariable)
        .where(personVariable.iidIn([insertedPerson.iid, insertedBatch[0].iid]))
        .rows({ limit: 10n, orderBy: [identifierOrder] })
        .map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedPerson.iid, insertedBatch[0].iid],
    );
    assert.deepEqual(
      querySession.query(personVariable).rows({
        limit: 2n,
        offset: 1n,
        orderBy: [identifierOrder],
      }).map((candidate: { readonly identifier: { readonly value: string } }) => candidate.identifier.value),
      ["person-2", "person-3"],
    );
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
    assert.equal(
      querySession.query(personVariable).where(generatedExpression).one().iid,
      insertedPerson.iid,
    );
    const crossLeft = querySession.exact(Person);
    const crossRight = querySession.exact(Person);
    const crossPair = querySession
      .query(crossLeft, crossRight)
      .allowCrossJoin(crossLeft, crossRight)
      .where(
        crossLeft.field(Person.identifier).eq(Identifier.create("person-1")),
        crossRight.field(Person.identifier).eq(Identifier.create("person-2")),
      )
      .one();
    assert.deepEqual(
      crossPair.map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedPerson.iid, insertedBatch[0].iid],
    );

    const partyVariable = querySession.subtypes(Party);
    const partyRows = querySession.query(partyVariable).rows({
      limit: 10n,
      orderBy: [partyVariable.field(Party.identifier).asc()],
    });
    assert.deepEqual(
      partyRows.map((candidate: { readonly __typebridgeModel: string }) => candidate.__typebridgeModel),
      [Employee.typeKey, Manager.typeKey],
    );
    assert.deepEqual(
      partyRows.map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedEmployee.iid, insertedManager.iid],
    );

    const membershipSubtypeVariable = querySession.subtypes(Membership);
    const membershipFamily = querySession.query(membershipSubtypeVariable);
    assert.equal(membershipFamily.countBy(membershipSubtypeVariable), 2n);
    const queriedBaseRelation = membershipFamily
      .where(membershipSubtypeVariable.iid(insertedMembership.iid))
      .one();
    assert.equal(queriedBaseRelation.__typebridgeModel, Membership.typeKey);
    assert.equal(queriedBaseRelation.member.iid, insertedPerson.iid);
    const queriedSubtypeRelation = membershipFamily
      .where(membershipSubtypeVariable.iid(insertedEmployment.iid))
      .one() as unknown as {
        readonly __typebridgeModel: string;
        readonly employee: { readonly iid: string | null };
        readonly member?: unknown;
      };
    assert.equal(queriedSubtypeRelation.__typebridgeModel, Employment.typeKey);
    assert.equal(queriedSubtypeRelation.employee.iid, insertedPerson.iid);
    assert.equal(Object.hasOwn(queriedSubtypeRelation, "member"), false);

    const reachableSource = querySession.exact(Person);
    const reachableTarget = querySession.exact(Person);
    const reachable = querySession.reachable(
      reachableSource,
      reachableTarget,
      NetworkLink,
      NetworkLink.origin,
      NetworkLink.destination,
      { minDepth: 1, maxDepth: 1 },
    );
    const reachablePair = querySession
      .query(reachableSource, reachableTarget)
      .where(
        reachable,
        reachableSource.field(Person.identifier).eq(Identifier.create("person-1")),
        reachableTarget.field(Person.identifier).eq(Identifier.create("person-2")),
      )
      .one();
    assert.deepEqual(
      reachablePair.map((candidate: { readonly iid: string | null }) => candidate.iid),
      [insertedPerson.iid, insertedBatch[0].iid],
    );
    const networkVariable = querySession.exact(NetworkLink);
    const queriedNetwork = querySession
      .query(networkVariable)
      .where(
        networkVariable.iid(insertedNetwork.iid),
        networkVariable.field(NetworkLink.nickname).isPresent(),
      )
      .one();
    assert.equal(queriedNetwork.iid, insertedNetwork.iid);

    const allPeopleQuery = querySession.query(personVariable);
    assert.equal(allPeopleQuery.first({ orderBy: [identifierOrder] })?.iid, insertedPerson.iid);
    const scoreField = personVariable.field(Person.score);
    const directAggregate = allPeopleQuery.aggregate(personVariable, [
      aggregate.count(),
      aggregate.sum(scoreField),
      aggregate.min(scoreField),
      aggregate.max(scoreField),
      aggregate.mean(scoreField),
      aggregate.median(scoreField),
      aggregate.std(scoreField),
    ] as const);
    assert.deepEqual(directAggregate.slice(0, 6), [4n, 24n, 3n, 9n, 6, 6]);
    assert.equal(typeof directAggregate[6], "number");
    const directFieldGroupedAggregate = allPeopleQuery
      .groupBy(personVariable, personVariable.field(Person.valBool))
      .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const);
    assert.equal(directFieldGroupedAggregate.length, 1);
    assert.equal(directFieldGroupedAggregate[0][0].value, true);
    assert.deepEqual(directFieldGroupedAggregate[0][1], [4n, 24n]);
    const directTupleFieldGroupedAggregate = allPeopleQuery
      .groupBy(personVariable, personVariable.field(Person.valBool), scoreField)
      .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const);
    assert.deepEqual(
      directTupleFieldGroupedAggregate.map(([[boolGroup, scoreGroup], values]) => [
        boolGroup.value,
        scoreGroup.value,
        values,
      ]),
      [
        [true, 3n, [1n, 3n]],
        [true, 5n, [1n, 5n]],
        [true, 7n, [1n, 7n]],
        [true, 9n, [1n, 9n]],
      ],
    );
    assert.throws(
      () => allPeopleQuery.aggregate(personVariable, [
        aggregate.sum(personVariable.field(Person.identifier)),
      ] as const),
      /long|double|numeric|reduc/i,
    );

    const directGroupedAggregate = directQuery
      .groupBy(personVariable, eventVariable)
      .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const);
    assert.equal(directGroupedAggregate.length, 1);
    assert.equal(directGroupedAggregate[0][0].iid, insertedEvent.iid);
    assert.deepEqual(directGroupedAggregate[0][1], [1n, 3n]);

    const named = querySession
      .queryNamed({ person: personVariable })
      .where(personPredicate)
      .one();
    assert.equal(named.person.iid, insertedPerson.iid);
    assert.equal(directQuery.countBy(personVariable), 1n);
    assert.equal(directQuery.existsBy(personVariable), true);

    const collectedPage = querySession
      .query(personVariable, collectedPersonVariable.collect().distinct())
      .where(personPredicate, sameIdentifierPredicate)
      .pageBy(personVariable, { limit: 10n, includeTotal: true });
    assert.equal(collectedPage.offset, 0n);
    assert.equal(collectedPage.limit, 10n);
    assert.equal(collectedPage.total, 1n);
    assert.equal(collectedPage.items.length, 1);
    assert.equal(collectedPage.items[0][0].iid, insertedPerson.iid);
    assert.deepEqual(
      collectedPage.items[0][1].map((item: { readonly iid: string }) => item.iid),
      [insertedPerson.iid],
    );

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
      const remoteSession = new RemoteQuerySession(
        advertisement,
        exchange,
        {
          maxItems: 100n,
          maxBytes: 1n << 20n,
          maxCollectionMembers: 100n,
          maxGraphNodes: 1_000n,
          maxAttributeValues: 1_000n,
          maxRolePlayers: 1_000n,
          deadlineMs: 30_000n,
        },
      );
      const remotePerson = remoteSession.var(Person);
      const remoteCollectedPerson = remoteSession.var(Person);
      const remoteEvent = remoteSession.var(Event);
      const remotePersonPredicate = remotePerson
        .field(Person.identifier)
        .eq(Identifier.create("person-1"));
      const remoteEventPredicate = remoteEvent.role(Event.subject).connects(remotePerson);
      const remoteDirectQuery = remoteSession
        .query(remotePerson, remoteEvent)
        .where(remotePersonPredicate, remoteEventPredicate);
      const remoteOne = await oneRemoteExchange(() => remoteDirectQuery.one());
      assert.deepEqual(
        [remoteOne[0].iid, remoteOne[1].iid],
        [directOne[0].iid, directOne[1].iid],
      );
      const remotePersonOnly = remoteSession
        .query(remotePerson)
        .where(remotePersonPredicate);
      const remoteRows = await oneRemoteExchange(() => remotePersonOnly.rows({
          limit: 10n,
          orderBy: [remotePerson.field(Person.identifier).asc()],
        }));
      assert.deepEqual(remoteRows.map((item: { readonly iid: string }) => item.iid), [insertedPerson.iid]);
      const remoteFirst = await oneRemoteExchange(() => remotePersonOnly.first({
          orderBy: [remotePerson.field(Person.identifier).asc()],
        }));
      assert.equal(remoteFirst?.iid, insertedPerson.iid);
      assert.deepEqual(
        (await oneRemoteExchange(() => remoteSession
          .query(remotePerson)
          .where(remotePerson.field(Person.aliases).isPresent())
          .rows({ limit: 10n, orderBy: [remotePerson.field(Person.identifier).asc()] })))
          .map((candidate: { readonly iid: string | null }) => candidate.iid),
        [insertedPerson.iid],
      );
      assert.deepEqual(
        (await oneRemoteExchange(() => remoteSession
          .query(remotePerson)
          .where(remotePerson.field(Person.aliases).isMissing())
          .rows({ limit: 10n, orderBy: [remotePerson.field(Person.identifier).asc()] })))
          .map((candidate: { readonly iid: string | null }) => candidate.iid),
        [...batchIids, transactionPersonIid],
      );
      assert.equal(
        (await oneRemoteExchange(() =>
          remoteSession.query(remotePerson).where(remotePerson.iid(insertedPerson.iid)).one()
        )).iid,
        insertedPerson.iid,
      );
      assert.deepEqual(
        (await oneRemoteExchange(() => remoteSession
          .query(remotePerson)
          .where(remotePerson.iidIn([insertedPerson.iid, insertedBatch[0].iid]))
          .rows({ limit: 10n, orderBy: [remotePerson.field(Person.identifier).asc()] })))
          .map((candidate: { readonly iid: string | null }) => candidate.iid),
        [insertedPerson.iid, insertedBatch[0].iid],
      );
      const remoteNetwork = remoteSession.exact(NetworkLink);
      assert.equal(
        (await oneRemoteExchange(() => remoteSession
          .query(remoteNetwork)
          .where(
            remoteNetwork.iid(insertedNetwork.iid),
            remoteNetwork.field(NetworkLink.nickname).isPresent(),
          )
          .one())).iid,
        insertedNetwork.iid,
      );

      const remoteParty = remoteSession.subtypes(Party);
      const remotePartyRows = await oneRemoteExchange(() =>
        remoteSession.query(remoteParty).rows({
          limit: 10n,
          orderBy: [remoteParty.field(Party.identifier).asc()],
        })
      );
      assert.deepEqual(
        remotePartyRows.map((candidate: { readonly __typebridgeModel: string }) => candidate.__typebridgeModel),
        [Employee.typeKey, Manager.typeKey],
      );
      assert.deepEqual(
        remotePartyRows.map((candidate: { readonly iid: string | null }) => candidate.iid),
        [insertedEmployee.iid, insertedManager.iid],
      );
      // docs: remote-query-node:end
      const remoteNamed = await oneRemoteExchange(() => remoteSession
          .queryNamed({ person: remotePerson })
          .where(remotePersonPredicate)
          .one());
      assert.equal(remoteNamed.person.iid, named.person.iid);
      assert.equal(
        await oneRemoteExchange(() => remoteDirectQuery.countBy(remotePerson)),
        directQuery.countBy(personVariable),
      );
      assert.equal(
        await oneRemoteExchange(() => remoteDirectQuery.existsBy(remotePerson)),
        directQuery.existsBy(personVariable),
      );

      const remoteScoreField = remotePerson.field(Person.score);
      const remoteAggregate = await oneRemoteExchange(() =>
        remoteSession.query(remotePerson).aggregate(remotePerson, [
          aggregate.count(),
          aggregate.sum(remoteScoreField),
          aggregate.min(remoteScoreField),
          aggregate.max(remoteScoreField),
          aggregate.mean(remoteScoreField),
          aggregate.median(remoteScoreField),
          aggregate.std(remoteScoreField),
        ] as const)
      );
      assert.deepEqual(remoteAggregate.slice(0, 6), directAggregate.slice(0, 6));
      assert.equal(typeof remoteAggregate[6], "number");

      const remoteFieldGroupedAggregate = await oneRemoteExchange(() =>
        remoteSession
          .query(remotePerson)
          .groupBy(remotePerson, remotePerson.field(Person.valBool))
          .aggregate([aggregate.count(), aggregate.sum(remoteScoreField)] as const)
      );
      assert.equal(remoteFieldGroupedAggregate.length, 1);
      assert.equal(remoteFieldGroupedAggregate[0][0].value, true);
      assert.deepEqual(remoteFieldGroupedAggregate[0][1], [4n, 24n]);

      const remoteTupleFieldGroupedAggregate = await oneRemoteExchange(() =>
        remoteSession
          .query(remotePerson)
          .groupBy(remotePerson, remotePerson.field(Person.valBool), remoteScoreField)
          .aggregate([aggregate.count(), aggregate.sum(remoteScoreField)] as const)
      );
      assert.deepEqual(
        remoteTupleFieldGroupedAggregate.map(([[boolGroup, scoreGroup], values]) => [
          boolGroup.value,
          scoreGroup.value,
          values,
        ]),
        directTupleFieldGroupedAggregate.map(([[boolGroup, scoreGroup], values]) => [
          boolGroup.value,
          scoreGroup.value,
          values,
        ]),
      );

      const remoteGroupedAggregate = await oneRemoteExchange(() => remoteDirectQuery
          .groupBy(remotePerson, remoteEvent)
          .aggregate([aggregate.count(), aggregate.sum(remoteScoreField)] as const));
      assert.equal(remoteGroupedAggregate.length, 1);
      assert.equal(remoteGroupedAggregate[0][0].iid, insertedEvent.iid);
      assert.deepEqual(remoteGroupedAggregate[0][1], directGroupedAggregate[0][1]);

      const remoteSameIdentifier = remoteCollectedPerson
        .field(Person.identifier)
        .eqField(remotePerson.field(Person.identifier));
      const remotePage = await oneRemoteExchange(() => remoteSession
          .query(remotePerson, remoteCollectedPerson.collect().distinct())
          .where(remotePersonPredicate, remoteSameIdentifier)
          .pageBy(remotePerson, { limit: 10n, includeTotal: true }));
      assert.deepEqual(
        {
          items: remotePage.items.map((item: readonly [
            { readonly iid: string },
            readonly { readonly iid: string }[],
          ]) => [item[0].iid, item[1].map((member) => member.iid)]),
          limit: remotePage.limit,
          offset: remotePage.offset,
          total: remotePage.total,
        },
        {
          items: collectedPage.items.map((item: readonly [
            { readonly iid: string },
            readonly { readonly iid: string }[],
          ]) => [item[0].iid, item[1].map((member) => member.iid)]),
          limit: collectedPage.limit,
          offset: collectedPage.offset,
          total: collectedPage.total,
        },
      );

      const remoteReachableSource = remoteSession.exact(Person);
      const remoteReachableTarget = remoteSession.exact(Person);
      const remoteReachable = remoteSession.reachable(
        remoteReachableSource,
        remoteReachableTarget,
        NetworkLink,
        NetworkLink.origin,
        NetworkLink.destination,
        { minDepth: 1, maxDepth: 1 },
      );
      const remoteReachablePair = await oneRemoteExchange(() => remoteSession
          .query(remoteReachableSource, remoteReachableTarget)
          .where(
            remoteReachable,
            remoteReachableSource.field(Person.identifier).eq(Identifier.create("person-1")),
            remoteReachableTarget.field(Person.identifier).eq(Identifier.create("person-2")),
          )
          .one());
      assert.deepEqual(
        remoteReachablePair.map((candidate: { readonly iid: string | null }) => candidate.iid),
        reachablePair.map((candidate: { readonly iid: string | null }) => candidate.iid),
      );
      const remoteCrossLeft = remoteSession.exact(Person);
      const remoteCrossRight = remoteSession.exact(Person);
      const remoteCrossPair = await oneRemoteExchange(() => remoteSession
          .query(remoteCrossLeft, remoteCrossRight)
          .allowCrossJoin(remoteCrossLeft, remoteCrossRight)
          .where(
            remoteCrossLeft.field(Person.identifier).eq(Identifier.create("person-1")),
            remoteCrossRight.field(Person.identifier).eq(Identifier.create("person-2")),
          )
          .one());
      assert.deepEqual(
        remoteCrossPair.map((candidate: { readonly iid: string | null }) => candidate.iid),
        crossPair.map((candidate: { readonly iid: string | null }) => candidate.iid),
      );
      assert.ok(requests.every((request) => request.length > 0));

      if (workforceReportPath !== undefined) {
        if (workforceCatalogRaw === undefined || workforceJourneyRaw === undefined) {
          throw new Error("workforce contracts were not loaded after report preflight");
        }
        const results = await runWorkforceJourney(
          generated,
          personManager,
          membershipManager,
          querySession,
          remoteSession,
          requests,
          workforceCatalog,
          workforceJourney,
        );
        preparedWorkforceReport = await workforceReport(
          workforceCatalogRaw,
          workforceCatalog,
          workforceJourneyRaw,
          semanticFingerprint,
          projectionFingerprint,
          results,
        );
      }
      if (workforceV2ReportPath !== undefined) {
        if (
          workforceV2CatalogRaw === undefined
          || workforceV2JourneyRaw === undefined
          || workforceV2ProofObservations === undefined
        ) {
          throw new Error("workforce-v2 contracts were not loaded after report preflight");
        }
        const results = await runWorkforceV2Journey(
          generated,
          database,
          remoteSession,
          requests,
          advertisement,
          exchange,
          workforceV2Catalog,
          workforceV2Journey,
          workforceV2ProofObservations,
        );
        preparedWorkforceV2Report = await workforceV2Report(
          workforceV2CatalogRaw,
          workforceV2Catalog,
          workforceV2JourneyRaw,
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
  if (workforceReportPath !== undefined) {
    assert.equal(
      REMOTE_PROFILE,
      "typedb-3.12.1/v1",
      "workforce reports may only be emitted for typedb-3.12.1/v1",
    );
    if (preparedWorkforceReport === undefined) {
      throw new Error("workforce report was not prepared after the successful journey");
    }
    await publishWorkforceReport(workforceReportPath, preparedWorkforceReport);
  }
  if (workforceV2ReportPath !== undefined) {
    assert.equal(
      REMOTE_PROFILE,
      "typedb-3.12.1/v1",
      "workforce-v2 reports may only be emitted for typedb-3.12.1/v1",
    );
    if (preparedWorkforceV2Report === undefined) {
      throw new Error("workforce-v2 report was not prepared after the successful journey");
    }
    await publishWorkforceReport(workforceV2ReportPath, preparedWorkforceV2Report);
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
      defineSchema(database, await readFile(WORKFORCE_V3_PROVIDER_SCHEMA, "utf8"));

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

      const supplementPath = process.env.TYPE_BRIDGE_WORKFORCE_V3_NODE_SUPPLEMENT;
      if (supplementPath !== undefined) {
        const observations = workforceV3SupplementObservations();
        const journey = JSON.parse(await readFile(WORKFORCE_V3_JOURNEY, "utf8"));
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
        await publishWorkforceReport(supplementPath, {
          format: "typebridge.workforce-v3-live-supplement/v1",
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
    skip: IS_TYPEDB_3_11 || process.env.TYPE_BRIDGE_WORKFORCE_V5_NODE_EVIDENCE === undefined,
    timeout: 360_000,
  },
  async () => {
    const evidencePath = process.env.TYPE_BRIDGE_WORKFORCE_V5_NODE_EVIDENCE;
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
        Employment, Identifier, Person, QuerySession, RemoteQuerySession, Score,
        ValBool, ValConstrained, ValDate, ValDatetime, ValDatetimeTz, ValDecimal,
        ValDouble, ValDuration,
      } = generated;

      database = connectIntegration();
      database.resetDatabase();
      requireWorkforceServerVersion(await detectTypeDBServerVersion(TYPEDB_ADDRESS, TYPEDB_HTTP_PORT));
      defineSchema(database, await readFile(WORKFORCE_V3_PROVIDER_SCHEMA, "utf8"));
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

      const directSession = new QuerySession(database);
      const directEmploymentVar = directSession.var(Employment);
      const directPersonVar = directSession.var(Person);
      const [directEmployment, directPerson] = directSession
        .query(directEmploymentVar, directPersonVar)
        .where(
          directEmploymentVar.role(Employment.employee).connects(directPersonVar),
          directPersonVar.field(Person.identifier).eq(Identifier.create("v5-live-person")),
        )
        .one();
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
      const remoteSession = new RemoteQuerySession(
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
        {
          maxItems: 10n, maxBytes: 1n << 20n, maxCollectionMembers: 100n,
          maxGraphNodes: 30n, maxAttributeValues: 1_000n, maxRolePlayers: 30n,
          deadlineMs: 30_000n,
        },
      );
      const remoteEmploymentVar = remoteSession.var(Employment);
      const remotePersonVar = remoteSession.var(Person);
      const [remoteEmployment, remotePerson] = await remoteSession
        .query(remoteEmploymentVar, remotePersonVar)
        .where(
          remoteEmploymentVar.role(Employment.employee).connects(remotePersonVar),
          remotePersonVar.field(Person.identifier).eq(Identifier.create("v5-live-person")),
        )
        .one();
      remoteSession.close();
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

      await publishWorkforceReport(evidencePath!, {
        binding: "node",
        detached_mutation_code: detachedCode,
        direct_remote_equal: true,
        entity_snapshot_b64: Buffer.from(entitySnapshot).toString("base64"),
        format: "typebridge.workforce-v5-live-codec-evidence/v1",
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
