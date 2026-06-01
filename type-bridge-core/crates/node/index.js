"use strict";

const fs = require("fs");
const path = require("path");

let cachedNative = null;

function loadNative() {
  if (cachedNative) {
    return cachedNative;
  }

  const explicitPath = process.env.TYPE_BRIDGE_NODE_NATIVE_PATH;
  const candidates = explicitPath ? [explicitPath] : [];
  candidates.push(...nativeCandidates());

  const tried = [];
  for (const candidate of candidates) {
    tried.push(candidate);
    if (fs.existsSync(candidate)) {
      cachedNative = require(candidate);
      return cachedNative;
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

function nativeCandidates() {
  const dir = __dirname;
  const triple = platformTriple();
  const names = [
    "type_bridge_node.node",
    "type-bridge-node.node",
    "index.node",
  ];

  if (triple) {
    names.unshift(
      `type_bridge_node.${triple}.node`,
      `type-bridge-node.${triple}.node`,
    );
  }

  return names.map((name) => path.join(dir, name));
}

function platformTriple() {
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

function string(value) {
  return { value_type: "string", value };
}

function long(value) {
  if (typeof value !== "bigint") {
    throw new TypeError("long requires a bigint; use longFromNumberUnsafe for explicit number conversion");
  }
  return { value_type: "long", value: value.toString() };
}

function longFromNumberUnsafe(value) {
  if (!Number.isFinite(value) || !Number.isInteger(value)) {
    throw new TypeError("longFromNumberUnsafe requires a finite integer number");
  }
  return { value_type: "long", value: value.toString() };
}

function double(value) {
  if (!Number.isFinite(value)) {
    throw new TypeError("double requires a finite number");
  }
  return { value_type: "double", value };
}

function boolean(value) {
  return { value_type: "boolean", value };
}

function date(value) {
  return { value_type: "date", value };
}

function datetime(value) {
  return { value_type: "datetime", value };
}

function datetimetz(value) {
  return { value_type: "datetime-tz", value };
}

function decimal(value) {
  return { value_type: "decimal", value };
}

function duration(value) {
  return { value_type: "duration", value };
}

class DescriptorRegistry {
  #native;

  constructor(nativeRegistry = null) {
    this.#native = nativeRegistry ?? new (loadNative().NodeDescriptorRegistry)();
  }

  registerEntity(descriptor) {
    return parseJson(this.#native.registerEntityJson(JSON.stringify(descriptor)));
  }

  registerRelation(descriptor) {
    return parseJson(this.#native.registerRelationJson(JSON.stringify(descriptor)));
  }

  entity(typeName) {
    return parseJson(this.#native.entityJson(typeName));
  }

  relation(typeName) {
    return parseJson(this.#native.relationJson(typeName));
  }

  snapshot() {
    return parseJson(this.#native.snapshotJson());
  }
}

class Marshalling {
  #native;

  constructor(nativeMarshalling = null) {
    this.#native = nativeMarshalling ?? loadNative();
  }

  attributeValue(value) {
    return parseJson(this.#native.normalizeAttributeValueJson(JSON.stringify(value)));
  }

  entityAttributes(descriptor, attributes) {
    return parseJson(
      this.#native.normalizeEntityAttributesJson(JSON.stringify(descriptor), JSON.stringify(attributes)),
    );
  }

  relationAttributes(descriptor, attributes) {
    return parseJson(
      this.#native.normalizeRelationAttributesJson(JSON.stringify(descriptor), JSON.stringify(attributes)),
    );
  }

  filters(descriptor, filters) {
    return parseJson(this.#native.normalizeFiltersJson(JSON.stringify(descriptor), JSON.stringify(filters)));
  }

  relationFilters(descriptor, filters) {
    return parseJson(this.#native.normalizeRelationFiltersJson(JSON.stringify(descriptor), JSON.stringify(filters)));
  }

  aggregates(descriptor, aggregates) {
    return parseJson(this.#native.normalizeAggregatesJson(JSON.stringify(descriptor), JSON.stringify(aggregates)));
  }

  rolePlayers(descriptor, rolePlayers) {
    return parseJson(
      this.#native.normalizeRolePlayersJson(JSON.stringify(descriptor), JSON.stringify(rolePlayers)),
    );
  }

  relationWriteBatch(descriptor, batch) {
    return parseJson(this.#native.normalizeRelationWriteBatchJson(JSON.stringify(descriptor), JSON.stringify(batch)));
  }
}

class RustDatabase {
  #native;

  constructor(nativeDatabase) {
    this.#native = nativeDatabase;
  }

  static connect(nativeOrAddress, addressOrDatabase, databaseOrOptions = {}, maybeOptions = {}) {
    const parsed = parseConnectArguments(nativeOrAddress, addressOrDatabase, databaseOrOptions, maybeOptions);
    return new RustDatabase(
      parsed.native.connectRustDatabase(
        parsed.address,
        parsed.database,
        parsed.options.username ?? null,
        parsed.options.password ?? null,
      ),
    );
  }

  isConnected() {
    return this.#native.isConnected();
  }

  databaseName() {
    return this.#native.databaseName();
  }

  transaction(transactionType = "read") {
    return new RustTransactionContext(this.#native.transaction(transactionType));
  }

  entityManager(descriptor) {
    return new RustDynamicEntityManager(this.#native.entityManagerJson(JSON.stringify(descriptor)));
  }

  relationManager(descriptor) {
    return new RustDynamicRelationManager(this.#native.relationManagerJson(JSON.stringify(descriptor)));
  }
}

class RustTransactionContext {
  #native;

  constructor(nativeContext) {
    this.#native = nativeContext;
  }

  query(query) {
    return parseJson(this.#native.queryJson(query));
  }

  commit() {
    this.#native.commit();
  }

  rollback() {
    this.#native.rollback();
  }

  close() {
    this.#native.close();
  }

  transactionType() {
    return this.#native.transactionType();
  }

  entityManager(descriptor) {
    return new RustDynamicEntityManager(this.#native.entityManagerJson(JSON.stringify(descriptor)));
  }

  relationManager(descriptor) {
    return new RustDynamicRelationManager(this.#native.relationManagerJson(JSON.stringify(descriptor)));
  }
}

class RustDynamicEntityManager {
  #native;

  constructor(nativeManager) {
    this.#native = nativeManager;
  }

  insert(attributes) {
    return this.#native.insertJson(JSON.stringify(attributes));
  }

  insertMany(batch) {
    return parseJson(this.#native.insertManyJson(JSON.stringify(batch)));
  }

  put(attributes) {
    return this.#native.putJson(JSON.stringify(attributes));
  }

  putMany(batch) {
    return parseJson(this.#native.putManyJson(JSON.stringify(batch)));
  }

  update(attributes, iid = null) {
    this.#native.updateJson(JSON.stringify(attributes), iid);
  }

  get(filters = null) {
    return parseJson(this.#native.getJson(optionalJson(filters)));
  }

  getByIid(iid) {
    return parseJson(this.#native.getByIidJson(iid));
  }

  all() {
    return parseJson(this.#native.allJson());
  }

  count(filters = null) {
    return BigInt(this.#native.countJson(optionalJson(filters)));
  }

  aggregate(aggregates, filters = null) {
    return parseJson(this.#native.aggregateJson(JSON.stringify(aggregates), optionalJson(filters)));
  }

  groupByAggregate(groupFields, aggregates, filters = null) {
    return parseJson(
      this.#native.groupByAggregateJson(JSON.stringify(groupFields), JSON.stringify(aggregates), optionalJson(filters)),
    );
  }

  deleteByIid(iid) {
    this.#native.deleteByIid(iid);
  }
}

class RustDynamicRelationManager {
  #native;

  constructor(nativeManager) {
    this.#native = nativeManager;
  }

  insert(attributes, rolePlayers) {
    return this.#native.insertJson(JSON.stringify(attributes), JSON.stringify(rolePlayers));
  }

  insertMany(batch) {
    return parseJson(this.#native.insertManyJson(JSON.stringify(batch)));
  }

  put(attributes, rolePlayers) {
    return this.#native.putJson(JSON.stringify(attributes), JSON.stringify(rolePlayers));
  }

  putMany(batch) {
    return parseJson(this.#native.putManyJson(JSON.stringify(batch)));
  }

  update(attributes, rolePlayers, iid = null) {
    this.#native.updateJson(JSON.stringify(attributes), JSON.stringify(rolePlayers), iid);
  }

  get(filters = null) {
    return parseJson(this.#native.getJson(optionalJson(filters)));
  }

  getWithRolePlayers(filters = null, rolePlayers = null) {
    return parseJson(this.#native.getWithRolePlayersJson(optionalJson(filters), optionalJson(rolePlayers)));
  }

  getByIid(iid) {
    return parseJson(this.#native.getByIidJson(iid));
  }

  all() {
    return parseJson(this.#native.allJson());
  }

  count(filters = null) {
    return BigInt(this.#native.countJson(optionalJson(filters)));
  }

  aggregate(aggregates, filters = null) {
    return parseJson(this.#native.aggregateJson(JSON.stringify(aggregates), optionalJson(filters)));
  }

  groupByAggregate(groupFields, aggregates, filters = null) {
    return parseJson(
      this.#native.groupByAggregateJson(JSON.stringify(groupFields), JSON.stringify(aggregates), optionalJson(filters)),
    );
  }

  deleteByIid(iid) {
    this.#native.deleteByIid(iid);
  }
}

function parseConnectArguments(nativeOrAddress, addressOrDatabase, databaseOrOptions, maybeOptions) {
  if (typeof nativeOrAddress === "string") {
    return {
      native: loadNative(),
      address: nativeOrAddress,
      database: addressOrDatabase,
      options: databaseOrOptions ?? {},
    };
  }

  if (typeof addressOrDatabase !== "string" || typeof databaseOrOptions !== "string") {
    throw new TypeError("RustDatabase.connect(native, address, database, options?) requires address and database strings");
  }

  return {
    native: nativeOrAddress,
    address: addressOrDatabase,
    database: databaseOrOptions,
    options: maybeOptions ?? {},
  };
}

function parseJson(value) {
  return JSON.parse(value);
}

function optionalJson(value) {
  return value == null ? null : JSON.stringify(value);
}

function ensureDatabase(address, database, options) {
  loadNative().ensureRustDatabase(
    address,
    database,
    options?.username ?? null,
    options?.password ?? null,
  );
}

module.exports = {
  DescriptorRegistry,
  Marshalling,
  RustDatabase,
  RustDynamicEntityManager,
  RustDynamicRelationManager,
  RustTransactionContext,
  boolean,
  date,
  datetime,
  datetimetz,
  decimal,
  double,
  duration,
  ensureDatabase,
  loadNative,
  long,
  longFromNumberUnsafe,
  string,
};
