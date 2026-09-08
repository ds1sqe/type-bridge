#!/usr/bin/env node
/** Remove compiled unit output so deleted tests cannot execute from a stale tree. */

"use strict";

const fs = require("node:fs");
const path = require("node:path");

const root = path.resolve(__dirname, "..");
const output = path.resolve(root, "../../../tmp/node-unit");
fs.rmSync(output, { recursive: true, force: true });
