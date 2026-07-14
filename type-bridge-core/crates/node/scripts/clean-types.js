"use strict";

const fs = require("node:fs");
const path = require("node:path");

// TypeScript does not delete outputs that no longer map to a source file. A
// clean output root keeps local/retried packs byte-for-byte aligned with the
// fresh-checkout release build and prevents old dist/typescript trees leaking.
const dist = path.resolve(__dirname, "..", "dist");
fs.rmSync(dist, { recursive: true, force: true });
