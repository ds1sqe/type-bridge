"use strict";

const path = require("path");

const typeBridge = require(path.join(__dirname, "..", "..", "..", "type-bridge-core", "crates", "node"));
const { descriptorSnapshot, registerParityDescriptors } = require("./node_parity_descriptors.cjs");

const { registry } = registerParityDescriptors(typeBridge);
process.stdout.write(JSON.stringify(descriptorSnapshot(registry)));
