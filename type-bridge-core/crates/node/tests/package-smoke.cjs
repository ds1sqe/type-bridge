"use strict";

const assert = require("assert");
const typeBridge = require("../");

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
    },
    {
      field_name: "age",
      attr_name: "age",
      value_type: "long",
      annotations: [],
      is_optional: true,
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
    },
  ],
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
});

assert.strictEqual(person.type_name, "person");
assert.strictEqual(employment.type_name, "employment");
assert.deepStrictEqual(registry.entity("person"), person);
assert.deepStrictEqual(registry.relation("employment"), employment);
assert.strictEqual(registry.snapshot().length, 2);

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
