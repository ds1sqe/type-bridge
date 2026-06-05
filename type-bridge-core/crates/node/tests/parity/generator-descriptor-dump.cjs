"use strict";

// Cross-language generator-parity helper.
//
// Two modes, driven by the Python test `test_generator_cross_language.py`:
//   generate <schema.tql> <outDir>  — run the TS generator into <outDir>
//   dump                            — load the COMPILED generated package and
//                                     print its descriptor snapshot as JSON
//
// `dump` resolves the generated package's `@type-bridge/node` import to the
// compiled surface via a module resolver patch. The descriptor() output is offline.

const fs = require("fs");
const path = require("path");
const Module = require("module");

const cwd = process.cwd();
const TMP = path.join(cwd, "..", "..", "..", "tmp");

function generate(schemaPath, outDir) {
  const gen = require(path.join(TMP, "node-typed-integration", "typescript", "generator", "index.js"));
  const native = require(cwd).loadNative();
  const tql = fs.readFileSync(schemaPath, "utf8");
  fs.rmSync(outDir, { recursive: true, force: true });
  fs.mkdirSync(outDir, { recursive: true });
  gen.generateModels(tql, outDir, { native });
  process.stderr.write(`generated: ${fs.readdirSync(outDir).join(", ")}\n`);
}

function dump() {
  const surface = path.join(TMP, "node-gen-parity", "typescript", "index.js");
  const resolve = Module._resolveFilename;
  Module._resolveFilename = function (request, ...rest) {
    return resolve.call(this, request === "@type-bridge/node" ? surface : request, ...rest);
  };
  const generated = require(path.join(TMP, "node-gen-parity", "tests", "parity", "generated", "index.js"));
  const entities = [];
  const relations = [];
  for (const value of Object.values(generated)) {
    if (typeof value !== "function" || typeof value.descriptor !== "function") continue;
    let descriptor;
    try {
      descriptor = value.descriptor();
    } catch {
      continue; // attribute classes etc. — not a model descriptor
    }
    if (!descriptor || typeof descriptor.type_name !== "string" || !Array.isArray(descriptor.owned_attributes)) {
      continue;
    }
    (Array.isArray(descriptor.roles) ? relations : entities).push(descriptor);
  }
  process.stdout.write(JSON.stringify({ version: "1.0.0", entities, relations }));
}

const mode = process.argv[2];
if (mode === "generate") {
  generate(process.argv[3], process.argv[4]);
} else if (mode === "dump") {
  dump();
} else {
  process.stderr.write("usage: generator-descriptor-dump.cjs generate <schema> <outDir> | dump\n");
  process.exit(2);
}
