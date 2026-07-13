"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

function nativeLibraryName() {
  switch (process.platform) {
    case "linux":
      return "libtype_bridge_node.so";
    case "darwin":
      return "libtype_bridge_node.dylib";
    case "win32":
      return "type_bridge_node.dll";
    default:
      throw new Error(`unsupported native test platform: ${process.platform}`);
  }
}

let cachedNative = null;

function loadNative() {
  if (cachedNative !== null) {
    return cachedNative;
  }
  const explicit = process.env.TYPE_BRIDGE_NODE_NATIVE_PATH;
  const source = explicit
    ? path.resolve(explicit)
    : path.resolve(__dirname, "../../../target/debug", nativeLibraryName());
  assert.ok(
    fs.existsSync(source),
    `native artifact missing at ${source}; run cargo build -p type-bridge-node`,
  );

  const tempDir = path.resolve(__dirname, "../../../../tmp/node-native-match");
  fs.mkdirSync(tempDir, { recursive: true });
  const loadable = path.join(tempDir, "type_bridge_node.node");
  fs.copyFileSync(source, loadable);
  cachedNative = require(loadable);
  return cachedNative;
}

function registerGraph(native) {
  const registry = new native.NodeDescriptorRegistry();
  for (const [typeName, attrName] of [
    ["person", "person-name"],
    ["company", "company-name"],
  ]) {
    registry.registerEntityJson(JSON.stringify({
      type_name: typeName,
      is_abstract: false,
      parent_type: null,
      owned_attributes: [{
        field_name: "name",
        attr_name: attrName,
        value_type: "string",
        annotations: ["Key"],
        is_optional: false,
        is_ordered: false,
      }],
    }));
  }
  registry.registerRelationJson(JSON.stringify({
    type_name: "employment",
    is_abstract: false,
    parent_type: null,
    owned_attributes: [],
    roles: [
      {
        role_name: "employee",
        player_type_names: ["person"],
        cardinality: [1, 1],
      },
      {
        role_name: "employer",
        player_type_names: ["company"],
        cardinality: [1, 1],
      },
    ],
  }));
  return registry;
}

test("opaque native handles build persistent canonical diagnostics", () => {
  const native = loadNative();
  const registry = registerGraph(native);
  const session = new native.NodeMatchSessionHandle(registry);
  const person = session.exact("person");
  const company = session.exact("company");
  const employment = session.exact("employment");
  const shape = session.positional([person.one(), company.one()]);
  const base = session.query(shape);
  const connected = employment
    .role("employee")
    .connects(person)
    .and(employment.role("employer").connects(company));
  const valuePredicate = person.field("name").compareValueJson(
    "starts_with",
    JSON.stringify({ value_type: "string", value: "A" }),
  );
  const fieldPredicate = person
    .field("name")
    .compareField("not_equal", company.field("name"));
  const secondValuePredicate = person.field("name").compareValueJson(
    "contains",
    JSON.stringify({ value_type: "string", value: "li" }),
  );
  const derived = base
    .addHidden(employment)
    .wherePredicate(connected)
    .wherePredicate(
      valuePredicate
        .or(secondValuePredicate)
        .and(fieldPredicate)
        .and(valuePredicate.not()),
    );
  const order = person.field("name").order("ascending", "reject");

  const diagnostic = derived.fetchRowsDiagnostic(
    [order],
    0n,
    25n,
    "bounded_many",
  );
  assert.equal(native.revalidateMatchDiagnostic(registry, diagnostic), diagnostic);

  const wire = JSON.parse(diagnostic);
  assert.deepEqual(
    wire.request.plan.bindings.map((binding) => binding.id),
    [0, 1, 2],
  );
  assert.equal(wire.request.operation.output.kind, "positional");
  assert.deepEqual(
    JSON.parse(base.countByDiagnostic(person)).request.plan.bindings.map(
      (binding) => binding.id,
    ),
    [0, 1],
  );

  const companyCollection = company
    .collect()
    .distinct(true)
    .orderBy(company.field("name").order("ascending", "reject"));
  const namedPage = session
    .query(session.named(["person", "companies"], [person.one(), companyCollection]))
    .addHidden(employment)
    .wherePredicate(connected)
    .pageByDiagnostic(person, [order], 10n, 10n, true);
  assert.equal(native.revalidateMatchDiagnostic(registry, namedPage), namedPage);
  assert.equal(JSON.parse(namedPage).request.operation.kind, "page_by");
  const publicNamedPageGolden = fs.readFileSync(
    path.resolve(
      __dirname,
      "../../orm/tests/fixtures/match_request/public-named-page.json",
    ),
    "utf8",
  ).trim();
  assert.equal(namedPage, publicNamedPageGolden);

  const crossJoin = base.allowCrossJoin(person, company).existsByDiagnostic(person);
  assert.equal(native.revalidateMatchDiagnostic(registry, crossJoin), crossJoin);

  const polymorphic = session.subtypes("person");
  const subtypeCount = session
    .query(session.positional([polymorphic.one()]))
    .countByDiagnostic(polymorphic);
  assert.equal(native.revalidateMatchDiagnostic(registry, subtypeCount), subtypeCount);
  assert.match(subtypeCount, /SUBTYPE_ENTITY_TARGET/);

  const singleCount = session
    .query(session.positional([person.one()]))
    .countByDiagnostic(person);
  const rustGolden = fs.readFileSync(
    path.resolve(
      __dirname,
      "../../orm/tests/fixtures/match_request/single-count.json",
    ),
    "utf8",
  ).trim();
  assert.equal(singleCount, rustGolden);
});

test("native match errors are structured and bigint windows are lossless", () => {
  const native = loadNative();
  const registry = registerGraph(native);
  const session = new native.NodeMatchSessionHandle(registry);

  assert.throws(
    () => session.exact("missing"),
    (error) => {
      const payload = JSON.parse(error.message);
      assert.equal(payload.category, "invalid_plan");
      assert.equal(payload.code, "unknown_descriptor");
      assert.deepEqual(payload.path, []);
      assert.deepEqual(payload.details, {});
      return true;
    },
  );

  const person = session.exact("person");
  const query = session.query(session.positional([person.one()]));
  const maximum = query.fetchRowsDiagnostic(
    [],
    18446744073709551615n,
    18446744073709551615n,
    "exactly_one",
  );
  assert.match(maximum, /18446744073709551615/);
  assert.throws(
    () => query.fetchRowsDiagnostic([], -1n, 1n, "exactly_one"),
    /non-negative bigint within the u64 range/,
  );
});

test("validated result handles are nonconstructible and expose no semantic DTO", () => {
  const native = loadNative();
  const queryMethods = Object.getOwnPropertyNames(
    native.NodeMatchQueryHandle.prototype,
  );
  assert.ok(queryMethods.includes("executeFetchRowsOwned"));
  assert.ok(queryMethods.includes("executeFetchRowsBorrowed"));
  assert.ok(queryMethods.includes("executePageByOwned"));
  assert.ok(queryMethods.includes("executePageByBorrowed"));
  assert.ok(queryMethods.includes("executeCountByOwned"));
  assert.ok(queryMethods.includes("executeCountByBorrowed"));
  assert.ok(queryMethods.includes("executeExistsByOwned"));
  assert.ok(queryMethods.includes("executeExistsByBorrowed"));
  assert.equal(queryMethods.includes("executeFetchRowsOwnedJson"), false);
  assert.equal(queryMethods.includes("executeFetchRowsBorrowedJson"), false);

  assert.throws(
    () => new native.NodeValidatedMatchResultHandle(),
    /contains no `constructor`/,
  );
  assert.throws(
    () => new native.NodeValidatedThingHandle(),
    /contains no `constructor`/,
  );
  assert.deepEqual(
    Object.getOwnPropertyNames(native.NodeValidatedMatchResultHandle.prototype)
      .filter((name) => name !== "constructor")
      .sort(),
    [
      "countValue",
      "existsValue",
      "outputNames",
      "outputSlotCount",
      "outputSlotIsCollection",
      "pageEntryCount",
      "pageLimit",
      "pageOffset",
      "pageSlotCount",
      "pageSlotThing",
      "pageSlotValueCount",
      "pageTotal",
      "rowCount",
      "slotCount",
      "slotThing",
    ].sort(),
  );
  assert.deepEqual(
    Object.getOwnPropertyNames(native.NodeValidatedThingHandle.prototype)
      .filter((name) => name !== "constructor")
      .sort(),
    [
      "concreteDescriptor",
      "fieldNames",
      "fieldValuesJson",
      "iid",
      "roleDataComplete",
      "roleNames",
      "rolePlayer",
      "rolePlayerCount",
      "thingKind",
    ].sort(),
  );
});
