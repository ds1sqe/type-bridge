"use strict";

const fs = require("fs");
const path = require("path");

const typeBridge = require(path.join(__dirname, "..", "..", "..", "type-bridge-core", "crates", "node"));
const { registerParityDescriptors } = require("./node_parity_descriptors.cjs");

function readRows() {
  const { descriptors } = registerParityDescriptors(typeBridge);
  const address = process.env.TYPEDB_ADDRESS || "localhost:1730";
  const database = process.env.TYPE_BRIDGE_PARITY_DATABASE || process.env.TYPE_BRIDGE_NODE_INTG_DATABASE || "type_bridge_test";
  const username = process.env.TYPEDB_USERNAME || "admin";
  const password = process.env.TYPEDB_PASSWORD || "password";
  const db = typeBridge.RustDatabase.connect(address, database, { username, password });

  const entityDescriptors = [
    descriptors.entities.parityPerson,
    descriptors.entities.parityCompany,
    descriptors.entities.parityEmailMessage,
  ];
  const relationDescriptors = [
    descriptors.relations.parityMembership,
    descriptors.relations.parityTokenOrigin,
  ];

  const entities = entityDescriptors.map((descriptor) => ({
    type_name: descriptor.type_name,
    rows: db.entityManager(descriptor).all(),
  }));
  const entityIids = entityIidsByStableId(entities);
  const writeData = JSON.parse(
    fs.readFileSync(path.join(__dirname, "fixtures", "write-data.json"), "utf8"),
  );

  const relations = relationDescriptors.map((descriptor) => ({
    type_name: descriptor.type_name,
    rows: readRelationRows(db.relationManager(descriptor), descriptor, writeData, entityIids),
  }));

  return { version: 1, entities, relations };
}

function readRelationRows(manager, descriptor, writeData, entityIids) {
  const rows = [];
  for (const relation of writeData.relations.filter((row) => row.type === descriptor.type_name)) {
    for (const rolePlayers of rolePlayerFilterBatches(relation, entityIids)) {
      rows.push(...manager.getWithRolePlayers(null, rolePlayers));
    }
  }
  return rows;
}

function rolePlayerFilterBatches(relation, entityIids) {
  const singlePlayers = [];
  const repeatedPlayers = [];
  for (const [roleName, players] of Object.entries(relation.roles)) {
    const inputs = players.map((player) => ({
      role_name: roleName,
      player_type_name: player.type,
      iid: entityIids[player.stable_id],
    }));
    if (inputs.length <= 1) {
      singlePlayers.push(...inputs);
    } else {
      repeatedPlayers.push(...inputs);
    }
  }

  if (repeatedPlayers.length === 0) {
    return [singlePlayers];
  }
  return repeatedPlayers.map((player) => [...singlePlayers, player]);
}

function entityIidsByStableId(entitySections) {
  const byStableId = {};
  for (const section of entitySections) {
    for (const row of section.rows) {
      byStableId[stableIdFromAttributes(row.attributes)] = row.iid;
    }
  }
  return byStableId;
}

function stableIdFromAttributes(attributes) {
  for (const [attrName, value] of attributes) {
    if (attrName === "parity-id") {
      return String(unwrapNodeValue(value));
    }
  }
  throw new Error(`row is missing parity-id: ${JSON.stringify(attributes)}`);
}

function unwrapNodeValue(value) {
  if (value && typeof value === "object") {
    for (const key of ["String", "Long", "Double", "Boolean", "Date", "DateTime", "DateTimeTZ", "Decimal", "Duration"]) {
      if (Object.prototype.hasOwnProperty.call(value, key)) {
        return value[key];
      }
    }
    if (Object.prototype.hasOwnProperty.call(value, "value")) {
      return unwrapNodeValue(value.value);
    }
  }
  return value;
}

process.stdout.write(JSON.stringify(readRows()));
