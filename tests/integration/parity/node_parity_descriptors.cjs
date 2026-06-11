"use strict";

function attr(fieldName, attrName, valueType, annotations = [], isOptional = false) {
  return {
    field_name: fieldName,
    attr_name: attrName,
    value_type: valueType,
    annotations,
    is_optional: isOptional,
    is_ordered: false,
  };
}

function role(roleName, playerTypeNames, cardinality, { overrides = null, isAbstract = false } = {}) {
  return {
    role_name: roleName,
    player_type_names: playerTypeNames,
    cardinality,
    plays_cardinality: null,
    overrides,
    is_abstract: isAbstract,
    ordered: false,
    distinct: false,
  };
}

function registerParityDescriptors(typeBridge) {
  const registry = new typeBridge.DescriptorRegistry();

  const parityParty = registry.registerEntity({
    type_name: "parity-party",
    is_abstract: true,
    parent_type: null,
    owned_attributes: [
      attr("id", "parity-id", "string", ["Key"]),
      attr("name", "parity-name", "string", [], true),
    ],
  });

  const partyAttributes = parityParty.owned_attributes;
  const descriptors = {
    entities: {},
    relations: {},
  };

  descriptors.entities.parityParty = parityParty;
  descriptors.entities.parityPerson = registry.registerEntity({
    type_name: "parity-person",
    is_abstract: false,
    parent_type: "parity-party",
    owned_attributes: [
      ...partyAttributes,
      attr("email", "parity-email", "string", ["Unique"]),
      attr("age", "parity-age", "long", [], true),
      attr("score", "parity-score", "double", [], true),
      attr("active", "parity-active", "boolean", [], true),
      attr("birth_date", "parity-birth-date", "date", [], true),
      attr("login_at", "parity-login-at", "datetime", [], true),
      attr("seen_at", "parity-seen-at", "datetime-tz", [], true),
      attr("balance", "parity-balance", "decimal", [], true),
      attr("session_length", "parity-session-length", "duration", [], true),
      attr("tags", "parity-tag", "string", [{ Card: [0, 5] }], true),
    ],
  });

  descriptors.entities.parityCompany = registry.registerEntity({
    type_name: "parity-company",
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      attr("id", "parity-id", "string", ["Key"]),
      attr("name", "parity-name", "string"),
    ],
  });

  descriptors.entities.parityEmailMessage = registry.registerEntity({
    type_name: "parity-email-message",
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      attr("id", "parity-id", "string", ["Key"]),
      attr("note", "parity-note", "string"),
    ],
  });

  descriptors.relations.parityMembership = registry.registerRelation({
    type_name: "parity-membership",
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      attr("since", "parity-since", "date"),
      attr("confidence", "parity-confidence", "long", [], true),
    ],
    roles: [
      role("member", ["parity-person"], [1, 1]),
      role("organization", ["parity-company"], [1, 1]),
      role("evidence", ["parity-person", "parity-email-message"], [0, 5]),
    ],
  });

  descriptors.relations.parityTokenOrigin = registry.registerRelation({
    type_name: "parity-token-origin",
    is_abstract: false,
    parent_type: null,
    owned_attributes: [
      attr("kind", "parity-kind", "string"),
    ],
    roles: [
      role("token", ["parity-party", "parity-person"], [1, 1]),
      role("issue", ["parity-company"], [1, 1]),
    ],
  });

  descriptors.relations.parityContribution = registry.registerRelation({
    type_name: "parity-contribution",
    is_abstract: false,
    parent_type: null,
    owned_attributes: [],
    roles: [
      role("contributor", ["parity-person"], null, { isAbstract: true }),
      role("work", ["parity-email-message"], null),
    ],
  });

  descriptors.relations.parityAuthoring = registry.registerRelation({
    type_name: "parity-authoring",
    is_abstract: false,
    parent_type: "parity-contribution",
    owned_attributes: [],
    roles: [
      role("work", ["parity-email-message"], null),
      role("author", ["parity-person"], null, { overrides: "contributor" }),
    ],
  });

  return { registry, descriptors };
}

function descriptorSnapshot(registry) {
  const snapshot = registry.snapshot();
  const entities = [];
  const relations = [];

  for (const item of snapshot) {
    if (item.kind === "entity") {
      entities.push(item.descriptor);
    } else if (item.kind === "relation") {
      relations.push(item.descriptor);
    }
  }

  return { version: 1, entities, relations };
}

module.exports = {
  descriptorSnapshot,
  registerParityDescriptors,
};
