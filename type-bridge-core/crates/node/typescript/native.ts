import * as fs from "fs";
import * as path from "path";
import type {
  NativeModule,
  NativeRustDatabase,
  NativeRustTransactionContext,
} from "./index.js";
import { ownedByteSnapshot } from "./owned-bytes.js";
import type { NativeProjectedManager } from "./runtime-projection.js";

type NativeRegistryHandle = InstanceType<NativeModule["NodeDescriptorRegistry"]>;

type NativeMatchComparison =
  | "equal"
  | "not_equal"
  | "less_than"
  | "less_than_or_equal"
  | "greater_than"
  | "greater_than_or_equal"
  | "contains"
  | "starts_with"
  | "ends_with"
  | "regex";

type NativeMatchDirection = "ascending" | "descending";
type NativeMatchMissingOrder = "reject" | "first" | "last";
type NativeMatchRowCardinality = "exactly_one" | "bounded_many";
type NativeMatchReduction =
  | "count"
  | "sum"
  | "min"
  | "max"
  | "mean"
  | "median"
  | "std";
declare const nativeMatchHandleKind: unique symbol;

interface NativeMatchSessionHandle {
  readonly [nativeMatchHandleKind]: "session";
  exact(typeName: string): NativeMatchBindingHandle;
  subtypes(typeName: string): NativeMatchBindingHandle;
  reachable(
    relationType: string,
    roleFrom: string,
    roleTo: string,
    source: NativeMatchBindingHandle,
    target: NativeMatchBindingHandle,
    minDepth: number,
    maxDepth: number,
  ): NativeMatchPredicateHandle;
  positional(selections: NativeMatchSelectionHandle[]): NativeMatchShapeHandle;
  named(names: string[], selections: NativeMatchSelectionHandle[]): NativeMatchShapeHandle;
  query(shape: NativeMatchShapeHandle): NativeMatchQueryHandle;
}

interface NativeMatchBindingHandle {
  readonly [nativeMatchHandleKind]: "binding";
  field(fieldName: string): NativeMatchFieldHandle;
  fieldOwnedBy(ownerType: string, fieldName: string): NativeMatchFieldHandle;
  role(roleName: string): NativeMatchRoleHandle;
  roleOwnedBy(ownerType: string, roleName: string): NativeMatchRoleHandle;
  one(): NativeMatchSelectionHandle;
  collect(): NativeMatchSelectionHandle;
}

interface NativeMatchFieldHandle {
  readonly [nativeMatchHandleKind]: "field";
  compareValueJson(comparison: NativeMatchComparison, valueJson: string): NativeMatchPredicateHandle;
  compareField(comparison: NativeMatchComparison, other: NativeMatchFieldHandle): NativeMatchPredicateHandle;
  order(direction: NativeMatchDirection, missing: NativeMatchMissingOrder): NativeMatchOrderHandle;
}

interface NativeMatchRoleHandle {
  readonly [nativeMatchHandleKind]: "role";
  connects(player: NativeMatchBindingHandle): NativeMatchPredicateHandle;
}

interface NativeMatchPredicateHandle {
  readonly [nativeMatchHandleKind]: "predicate";
  and(other: NativeMatchPredicateHandle): NativeMatchPredicateHandle;
  or(other: NativeMatchPredicateHandle): NativeMatchPredicateHandle;
  not(): NativeMatchPredicateHandle;
}

interface NativeMatchOrderHandle {
  readonly [nativeMatchHandleKind]: "order";
}

interface NativeMatchSelectionHandle {
  readonly [nativeMatchHandleKind]: "selection";
  distinct(distinct: boolean): NativeMatchSelectionHandle;
  orderBy(order: NativeMatchOrderHandle): NativeMatchSelectionHandle;
}

interface NativeMatchShapeHandle {
  readonly [nativeMatchHandleKind]: "shape";
}

interface NativeMatchQueryHandle {
  readonly [nativeMatchHandleKind]: "query";
  addHidden(binding: NativeMatchBindingHandle): NativeMatchQueryHandle;
  wherePredicate(predicate: NativeMatchPredicateHandle): NativeMatchQueryHandle;
  allowCrossJoin(left: NativeMatchBindingHandle, right: NativeMatchBindingHandle): NativeMatchQueryHandle;
  fetchRowsDiagnostic(
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): string;
  executeFetchRowsOwned(
    database: NativeRustDatabase,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): NativeValidatedMatchResultHandle;
  executeFetchRowsBorrowed(
    transaction: NativeRustTransactionContext,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): NativeValidatedMatchResultHandle;
  executePageByOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): NativeValidatedMatchResultHandle;
  executePageByBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): NativeValidatedMatchResultHandle;
  executeCountByOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
  ): NativeValidatedMatchResultHandle;
  executeCountByBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
  ): NativeValidatedMatchResultHandle;
  executeExistsByOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
  ): NativeValidatedMatchResultHandle;
  executeExistsByBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
  ): NativeValidatedMatchResultHandle;
  pageByDiagnostic(
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): string;
  countByDiagnostic(root: NativeMatchBindingHandle): string;
  existsByDiagnostic(root: NativeMatchBindingHandle): string;
  reduceByDiagnostic(
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): string;
  executeReduceByOwned(
    database: NativeRustDatabase,
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): NativeValidatedMatchResultHandle;
  executeReduceByBorrowed(
    transaction: NativeRustTransactionContext,
    root: NativeMatchBindingHandle,
    group: NativeMatchBindingHandle | null,
    reducers: NativeMatchReduction[],
    inputs: (NativeMatchFieldHandle | null)[],
  ): NativeValidatedMatchResultHandle;
}

interface NativeValidatedMatchResultHandle {
  readonly [nativeMatchHandleKind]: "validated-result";
  outputSlotCount(query: NativeMatchQueryHandle): number;
  outputSlotIsCollection(query: NativeMatchQueryHandle, slotIndex: number): boolean;
  rowCount(query: NativeMatchQueryHandle): number;
  slotCount(query: NativeMatchQueryHandle, rowIndex: number): number;
  outputNames(query: NativeMatchQueryHandle): string[] | null;
  slotThing(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    slotIndex: number,
  ): NativeValidatedThingHandle;
  pageEntryCount(query: NativeMatchQueryHandle): number;
  pageSlotCount(query: NativeMatchQueryHandle, entryIndex: number): number;
  pageSlotValueCount(
    query: NativeMatchQueryHandle,
    entryIndex: number,
    slotIndex: number,
  ): number;
  pageSlotThing(
    query: NativeMatchQueryHandle,
    entryIndex: number,
    slotIndex: number,
    valueIndex: number,
  ): NativeValidatedThingHandle;
  pageOffset(query: NativeMatchQueryHandle): bigint;
  pageLimit(query: NativeMatchQueryHandle): bigint;
  pageTotal(query: NativeMatchQueryHandle): bigint | null;
  countValue(query: NativeMatchQueryHandle): bigint;
  existsValue(query: NativeMatchQueryHandle): boolean;
  reductionRowCount(query: NativeMatchQueryHandle): number;
  reductionValueCount(query: NativeMatchQueryHandle, rowIndex: number): number;
  reductionValueKind(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    valueIndex: number,
  ): "count" | "long" | "double";
  reductionCountValue(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    valueIndex: number,
  ): bigint;
  reductionLongValue(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    valueIndex: number,
  ): bigint | null;
  reductionDoubleValue(
    query: NativeMatchQueryHandle,
    rowIndex: number,
    valueIndex: number,
  ): number | null;
  reductionGroup(
    query: NativeMatchQueryHandle,
    rowIndex: number,
  ): NativeValidatedThingHandle;
}

interface NativeValidatedThingHandle {
  readonly [nativeMatchHandleKind]: "validated-thing";
  iid(): string;
  concreteDescriptor(): string;
  thingKind(): "entity" | "relation";
  fieldNames(): string[];
  fieldValuesJson(fieldName: string): string | null;
  roleDataComplete(): boolean;
  roleNames(): string[];
  rolePlayerCount(roleName: string): number;
  rolePlayer(roleName: string, playerIndex: number): NativeValidatedThingHandle;
}

interface NativeMatchModule {
  NodeMatchSessionHandle: new (registry: NativeRegistryHandle) => NativeMatchSessionHandle;
  revalidateMatchDiagnostic(registry: NativeRegistryHandle, diagnosticJson: string): string;
  validateMatchOrderTermCount(actual: number): void;
}

interface NativeRemoteModelQueryContext {}

interface NativePendingRemoteModelQuery {
  requestBytes(): Uint8Array;
  decodeReply(response: Uint8Array): Promise<NativeValidatedMatchResultHandle>;
}

interface NativeRemoteModelQueryModule {
  queryV2RemoteModelContext(
    authority: ReturnType<NativeModule["queryV2Authority"]>,
    advertisement: Uint8Array,
    maxItems: bigint,
    maxBytes: bigint,
    maxCollectionMembers: bigint,
    maxGraphNodes: bigint,
    maxAttributeValues: bigint,
    maxRolePlayers: bigint,
    deadlineMs?: bigint | null,
  ): NativeRemoteModelQueryContext;
  queryV2PrepareRemoteModelRows(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    cardinality: NativeMatchRowCardinality,
  ): NativePendingRemoteModelQuery;
  queryV2PrepareRemoteModelPage(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
    orders: NativeMatchOrderHandle[],
    offset: bigint,
    limit: bigint,
    includeTotal: boolean,
  ): NativePendingRemoteModelQuery;
  queryV2PrepareRemoteModelCount(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
  ): NativePendingRemoteModelQuery;
  queryV2PrepareRemoteModelExists(
    query: NativeMatchQueryHandle,
    context: NativeRemoteModelQueryContext,
    root: NativeMatchBindingHandle,
  ): NativePendingRemoteModelQuery;
}

interface NativeRuntimeProjectionHandle {
  managerForDatabase(typeKey: string, database: NativeRustDatabase): NativeProjectedManager;
  managerForTransaction(typeKey: string, transaction: NativeRustTransactionContext): NativeProjectedManager;
}

interface NativeRuntimeProjectionModule {
  NodeRuntimeProjection: new (
    projectionJson: string,
    semanticFingerprintJson: string,
    projectionFingerprintJson: string,
    registrationsJson: string,
  ) => NativeRuntimeProjectionHandle;
}

type LoadedNativeModule =
  & NativeModule
  & NativeMatchModule
  & NativeRemoteModelQueryModule
  & NativeRuntimeProjectionModule;
type LoadedNativePendingQueryV2Remote = ReturnType<
  LoadedNativeModule["queryV2PrepareRemote"]
>;
type LoadedNativePendingRemoteModelQuery = ReturnType<
  LoadedNativeModule["queryV2PrepareRemoteModelRows"]
>;

const MAX_CANONICAL_BYTES = 16 * 1024 * 1024;
const MAX_REMOTE_ENVELOPE_BYTES = 32 * 1024 * 1024;

// This module compiles to dist/native.js (CommonJS). __dirname is therefore
// the dist/ directory. The .node artifacts are placed at the package root
// (one level up, i.e. dist/..) by build-native.js, which writes
// type_bridge_node.<triple>.node beside package.json. The candidates list
// probes the package root first (primary), then dist/ itself as a robustness
// fallback for atypical build layouts.

let _cached: LoadedNativeModule | null = null;

function protectedPendingQueryV2Remote(
  pending: LoadedNativePendingQueryV2Remote,
): LoadedNativePendingQueryV2Remote {
  const requestBytes = pending.requestBytes.bind(pending);
  const decodeReply = pending.decodeReply.bind(pending);
  let decodeStarted = false;
  return Object.freeze({
    requestBytes: (): Uint8Array => requestBytes(),
    decodeReply: (response: Uint8Array): Promise<string> => {
      if (decodeStarted) {
        // The native one-shot claim rejects replay before inspecting this
        // argument, so do not snapshot attacker-sized replay input.
        return decodeReply(response);
      }
      decodeStarted = true;
      let snapshot: Buffer;
      try {
        snapshot = ownedByteSnapshot(response, MAX_REMOTE_ENVELOPE_BYTES);
      } catch {
        // Preserve the native claim-first contract for an invalid first
        // argument. The addon consumes the one-shot before its metadata check,
        // while a replay returns without inspecting this value.
        return decodeReply(response);
      }
      // Do not catch a synchronous native rejection here. The claim has
      // already been consumed, so retrying would mask the original diagnostic
      // with the one-shot replay error.
      return decodeReply(snapshot);
    },
  });
}

function protectedPendingRemoteModelQuery(
  pending: LoadedNativePendingRemoteModelQuery,
): LoadedNativePendingRemoteModelQuery {
  const requestBytes = pending.requestBytes.bind(pending);
  const decodeReply = pending.decodeReply.bind(pending);
  let decodeStarted = false;
  return Object.freeze({
    requestBytes: (): Uint8Array => requestBytes(),
    decodeReply: (response: Uint8Array) => {
      if (decodeStarted) {
        return decodeReply(response);
      }
      decodeStarted = true;
      let snapshot: Buffer;
      try {
        snapshot = ownedByteSnapshot(response, MAX_REMOTE_ENVELOPE_BYTES);
      } catch {
        return decodeReply(response);
      }
      return decodeReply(snapshot);
    },
  });
}

function protectedMethodDescriptor(
  native: LoadedNativeModule,
  name: keyof LoadedNativeModule,
  value: (...args: never[]) => unknown,
): PropertyDescriptor {
  const original = Object.getOwnPropertyDescriptor(native, name);
  return {
    configurable: original?.configurable ?? true,
    enumerable: original?.enumerable ?? true,
    value,
    writable: original !== undefined && "writable" in original
      ? original.writable
      : false,
  };
}

/**
 * Hide every raw N-API V2 byte boundary behind an owned JavaScript snapshot.
 *
 * A Node Buffer may alias SharedArrayBuffer storage that another Worker can
 * mutate. N-API exposes that storage to Rust as an ordinary borrowed slice,
 * for which concurrent mutation would be undefined behaviour. The public
 * loader therefore returns a facade whose byte-bearing V2 calls take bounded
 * copies before entering the addon. The addon independently rejects shared
 * backing storage so direct artifact loads remain safe; the facade preserves
 * Uint8Array convenience and reflective property access without weakening
 * that native boundary.
 */
function protectNativeV2ByteInputs(native: LoadedNativeModule): LoadedNativeModule {
  const queryV2Authority = native.queryV2Authority.bind(native);
  const queryV2QueryOnlyAuthority = native.queryV2QueryOnlyAuthority.bind(native);
  const queryV2ExecuteLocal = native.queryV2ExecuteLocal.bind(native);
  const queryV2RemoteCapabilities = native.queryV2RemoteCapabilities.bind(native);
  const queryV2PrepareRemote = native.queryV2PrepareRemote.bind(native);
  const queryV2RemoteModelContext = native.queryV2RemoteModelContext.bind(native);
  const queryV2PrepareRemoteModelRows =
    native.queryV2PrepareRemoteModelRows.bind(native);
  const queryV2PrepareRemoteModelPage =
    native.queryV2PrepareRemoteModelPage.bind(native);
  const queryV2PrepareRemoteModelCount =
    native.queryV2PrepareRemoteModelCount.bind(native);
  const queryV2PrepareRemoteModelExists =
    native.queryV2PrepareRemoteModelExists.bind(native);
  const descriptors = Object.getOwnPropertyDescriptors(native);

  descriptors["queryV2Authority"] = protectedMethodDescriptor(
    native,
    "queryV2Authority",
    ((declaredSchema: Uint8Array, scope: string, profile: string) =>
      queryV2Authority(
        ownedByteSnapshot(declaredSchema, MAX_CANONICAL_BYTES),
        scope,
        profile,
      )) as (
      ...args: never[]
    ) => unknown,
  );
  descriptors["queryV2QueryOnlyAuthority"] = protectedMethodDescriptor(
    native,
    "queryV2QueryOnlyAuthority",
    ((
      database: NativeRustDatabase,
      declaredSchema: Uint8Array,
      scope: string,
      profile: string,
    ) =>
      queryV2QueryOnlyAuthority(
        database,
        ownedByteSnapshot(declaredSchema, MAX_CANONICAL_BYTES),
        scope,
        profile,
      )) as (...args: never[]) => unknown,
  );
  descriptors["queryV2ExecuteLocal"] = protectedMethodDescriptor(
    native,
    "queryV2ExecuteLocal",
    ((
      database: NativeRustDatabase,
      authority: Parameters<LoadedNativeModule["queryV2ExecuteLocal"]>[1],
      plan: Uint8Array,
      invocationJson: string,
      deadlineMs?: bigint | null,
    ) =>
      queryV2ExecuteLocal(
        database,
        authority,
        ownedByteSnapshot(plan, MAX_CANONICAL_BYTES),
        invocationJson,
        deadlineMs,
      )) as (...args: never[]) => unknown,
  );
  descriptors["queryV2RemoteCapabilities"] = protectedMethodDescriptor(
    native,
    "queryV2RemoteCapabilities",
    ((advertisement: Uint8Array) =>
      queryV2RemoteCapabilities(
        ownedByteSnapshot(advertisement, MAX_REMOTE_ENVELOPE_BYTES),
      )) as (
      ...args: never[]
    ) => unknown,
  );
  descriptors["queryV2PrepareRemote"] = protectedMethodDescriptor(
    native,
    "queryV2PrepareRemote",
    ((
      authority: Parameters<LoadedNativeModule["queryV2PrepareRemote"]>[0],
      plan: Uint8Array,
      invocationJson: string,
      advertisement: Uint8Array,
      maxItems: bigint,
      maxBytes: bigint,
      maxCollectionMembers: bigint,
      deadlineMs?: bigint | null,
    ) =>
      protectedPendingQueryV2Remote(
        queryV2PrepareRemote(
          authority,
          ownedByteSnapshot(plan, MAX_CANONICAL_BYTES),
          invocationJson,
          ownedByteSnapshot(advertisement, MAX_REMOTE_ENVELOPE_BYTES),
          maxItems,
          maxBytes,
          maxCollectionMembers,
          deadlineMs,
        ),
      )) as (...args: never[]) => unknown,
  );
  descriptors["queryV2RemoteModelContext"] = protectedMethodDescriptor(
    native,
    "queryV2RemoteModelContext",
    ((
      authority: Parameters<LoadedNativeModule["queryV2RemoteModelContext"]>[0],
      advertisement: Uint8Array,
      maxItems: bigint,
      maxBytes: bigint,
      maxCollectionMembers: bigint,
      maxGraphNodes: bigint,
      maxAttributeValues: bigint,
      maxRolePlayers: bigint,
      deadlineMs?: bigint | null,
    ) =>
      queryV2RemoteModelContext(
        authority,
        ownedByteSnapshot(advertisement, MAX_REMOTE_ENVELOPE_BYTES),
        maxItems,
        maxBytes,
        maxCollectionMembers,
        maxGraphNodes,
        maxAttributeValues,
        maxRolePlayers,
        deadlineMs,
      )) as (...args: never[]) => unknown,
  );
  descriptors["queryV2PrepareRemoteModelRows"] = protectedMethodDescriptor(
    native,
    "queryV2PrepareRemoteModelRows",
    ((...args: Parameters<LoadedNativeModule["queryV2PrepareRemoteModelRows"]>) =>
      protectedPendingRemoteModelQuery(
        queryV2PrepareRemoteModelRows(...args),
      )) as (...args: never[]) => unknown,
  );
  descriptors["queryV2PrepareRemoteModelPage"] = protectedMethodDescriptor(
    native,
    "queryV2PrepareRemoteModelPage",
    ((...args: Parameters<LoadedNativeModule["queryV2PrepareRemoteModelPage"]>) =>
      protectedPendingRemoteModelQuery(
        queryV2PrepareRemoteModelPage(...args),
      )) as (...args: never[]) => unknown,
  );
  descriptors["queryV2PrepareRemoteModelCount"] = protectedMethodDescriptor(
    native,
    "queryV2PrepareRemoteModelCount",
    ((...args: Parameters<LoadedNativeModule["queryV2PrepareRemoteModelCount"]>) =>
      protectedPendingRemoteModelQuery(
        queryV2PrepareRemoteModelCount(...args),
      )) as (...args: never[]) => unknown,
  );
  descriptors["queryV2PrepareRemoteModelExists"] = protectedMethodDescriptor(
    native,
    "queryV2PrepareRemoteModelExists",
    ((...args: Parameters<LoadedNativeModule["queryV2PrepareRemoteModelExists"]>) =>
      protectedPendingRemoteModelQuery(
        queryV2PrepareRemoteModelExists(...args),
      )) as (...args: never[]) => unknown,
  );

  return Object.create(Object.getPrototypeOf(native), descriptors) as LoadedNativeModule;
}

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
export function loadNative(): LoadedNativeModule {
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
      const native = require(candidate) as LoadedNativeModule;
      _cached = protectNativeV2ByteInputs(native);
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
