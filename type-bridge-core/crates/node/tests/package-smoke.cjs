"use strict";

const assert = require("assert");
const typeBridge = require("../");

// ---------------------------------------------------------------------------
// Low-level facade — DescriptorRegistry, Marshalling, value builders
// ---------------------------------------------------------------------------

const registry = new typeBridge.DescriptorRegistry();

const person = registry.registerEntity({
  type_name: "person",
  is_abstract: false,
  parent_type: null,
  owned_attributes: [
    {
      field_name: "name",
      attr_name: "person-name",
      value_type: "string",
      annotations: ["Key"],
      is_optional: false,
      is_ordered: false,
    },
    {
      field_name: "age",
      attr_name: "age",
      value_type: "long",
      annotations: [],
      is_optional: true,
      is_ordered: false,
    },
  ],
});

const employment = registry.registerRelation({
  type_name: "employment",
  is_abstract: false,
  parent_type: null,
  owned_attributes: [
    {
      field_name: "since",
      attr_name: "since",
      value_type: "date",
      annotations: [],
      is_optional: true,
      is_ordered: false,
    },
  ],
  roles: [
    {
      role_name: "employee",
      player_type_names: ["person"],
      cardinality: [1, 1],
      overrides: null,
      is_abstract: false,
      ordered: false,
      distinct: false,
    },
    {
      role_name: "employer",
      player_type_names: ["company"],
      cardinality: [1, 1],
      overrides: null,
      is_abstract: false,
      ordered: false,
      distinct: false,
    },
  ],
});

assert.strictEqual(person.type_name, "person");
assert.strictEqual(employment.type_name, "employment");
assert.deepStrictEqual(registry.entity("person"), person);
assert.deepStrictEqual(registry.relation("employment"), employment);
assert.strictEqual(registry.snapshot().length, 2);

const accepts = registry.registerRelation({
  type_name: "accepts",
  is_abstract: false,
  parent_type: null,
  owned_attributes: [],
  roles: [
    {
      role_name: "definition",
      player_type_names: [],
      cardinality: null,
    },
    {
      role_name: "allowed_value",
      player_type_names: ["person"],
      cardinality: null,
    },
  ],
});

assert.deepStrictEqual(accepts.roles[0].player_type_names, []);
const acceptsTypeql = typeBridge.generateDefineBlock(registry.schemaInfo());
assert.match(acceptsTypeql, /relates definition/);
assert.match(acceptsTypeql, /person plays accepts:allowed_value;/);
assert.doesNotMatch(acceptsTypeql, /plays accepts:definition/);

const rawPlaysCardSchema = {
  entities: {
    company: {
      type_name: "company",
      is_abstract: false,
      parent_type: null,
      owned_attributes: [],
      plays_cardinalities: {
        "employment:employer": [0, 1],
      },
    },
  },
  relations: {
    employment: {
      type_name: "employment",
      is_abstract: false,
      parent_type: null,
      owned_attributes: [],
      roles: [
        {
          role_name: "employer",
          player_type_names: ["company"],
          cardinality: null,
        },
      ],
      plays_cardinalities: {},
    },
  },
  attributes: {},
};
const rawPlaysCardTypeql = typeBridge.generateDefineBlock(rawPlaysCardSchema);
assert.match(rawPlaysCardTypeql, /company plays employment:employer @card\(0\.\.1\);/);

const rawBarePlaysSchema = structuredClone(rawPlaysCardSchema);
rawBarePlaysSchema.entities.company.plays_cardinalities = {};
const rawBarePlaysTypeql = typeBridge.generateDefineBlock(rawBarePlaysSchema);
assert.match(rawBarePlaysTypeql, /^company plays employment:employer;$/m);
assert.doesNotMatch(rawBarePlaysTypeql, /company plays employment:employer @card/);

const marshalling = new typeBridge.Marshalling();
assert.deepStrictEqual(
  typeBridge.long(9223372036854775807n),
  { value_type: "long", value: "9223372036854775807" },
);
assert.deepStrictEqual(marshalling.attributeValue(typeBridge.long(42n)), { Long: 42 });

assert.throws(() => typeBridge.long(1), /bigint/);

const attrs = marshalling.entityAttributes(person, {
  name: typeBridge.string("Alice"),
  age: typeBridge.long(42n),
});

assert.deepStrictEqual(attrs, [
  ["age", { Long: 42 }],
  ["person-name", { String: "Alice" }],
]);

// ---------------------------------------------------------------------------
// Facade presence — loadNative is a real function
// ---------------------------------------------------------------------------

assert.strictEqual(typeof typeBridge.loadNative, "function", "loadNative must be a function");

// ---------------------------------------------------------------------------
// Typed layer — presence assertions
// ---------------------------------------------------------------------------

assert.strictEqual(typeof typeBridge.Entity, "function", "Entity must be a function");
// attr is a namespace object (attr.String, attr.Integer, …)
assert.strictEqual(typeof typeBridge.attr, "object", "attr must be an object");
assert.strictEqual(typeof typeBridge.attr.String, "function", "attr.String must be a function");
assert.strictEqual(typeof typeBridge.field, "function", "field must be a function");
assert.strictEqual(typeof typeBridge.role, "function", "role must be a function");
assert.strictEqual(typeof typeBridge.Card, "function", "Card must be a function");
// Key/Unique are string flag tokens passed to field(Attr, Key).
assert.strictEqual(typeBridge.Key, "Key", "Key must be the 'Key' flag token");
assert.strictEqual(typeBridge.Unique, "Unique", "Unique must be the 'Unique' flag token");
assert.strictEqual(typeof typeBridge.TypeFlags, "function", "TypeFlags must be a function");
assert.strictEqual(typeof typeBridge.generateModels, "function", "generateModels must be a function");
assert.strictEqual(typeof typeBridge.parseSchema, "function", "parseSchema must be a function");

// ---------------------------------------------------------------------------
// Typed layer — constructive check: define a model class and read its descriptor
// ---------------------------------------------------------------------------

// attr.String("type-name") returns an attribute class (constructor).
class SmokePersonName extends typeBridge.attr.String("smoke-person-name") {}

// field(AttrClass, ...flags) returns a FieldSpec.
const nameField = typeBridge.field(SmokePersonName, typeBridge.Key);

// Entity("type-name", { fieldKey: FieldSpec }) returns a model base class.
class SmokePerson extends typeBridge.Entity("smoke-person", {
  name: nameField,
}) {}

const descriptor = SmokePerson.descriptor();
assert.strictEqual(descriptor.type_name, "smoke-person", "Entity descriptor type_name must match");
assert.ok(Array.isArray(descriptor.owned_attributes), "Entity descriptor owned_attributes must be an array");
assert.strictEqual(descriptor.owned_attributes.length, 1, "SmokePerson must have exactly one owned attribute");
assert.strictEqual(descriptor.owned_attributes[0].field_name, "name", "Field name must be 'name'");
assert.deepStrictEqual(descriptor.owned_attributes[0].annotations, ["Key"], "Key annotation must be present");

// ---------------------------------------------------------------------------
// Published-tarball contents — the publish artifact must ship the compiled
// runtime + the native module, or an installed consumer cannot load them.
// ---------------------------------------------------------------------------

const { execSync } = require("node:child_process");
const packed = JSON.parse(execSync("npm pack --dry-run --json", { encoding: "utf8" }));
const packedFiles = packed[0].files.map((f) => f.path);
assert.ok(packedFiles.includes("dist/index.js"), "tarball must include dist/index.js");
assert.ok(packedFiles.includes("dist/native.js"), "tarball must include dist/native.js (the loader)");
assert.ok(
  packedFiles.some((f) => f.endsWith(".node")),
  "tarball must include the native .node module",
);
assert.ok(
  !packedFiles.some((f) => f.startsWith("dist/typescript/")),
  "tarball must not include stale duplicate dist/typescript outputs",
);
assert.ok(!packedFiles.includes("index.js"), "the deleted root index.js must not be published");
