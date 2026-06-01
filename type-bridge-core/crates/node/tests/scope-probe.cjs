"use strict";

const fs = require("fs");
const path = require("path");

const root = path.resolve(__dirname, "..");
const scannedPaths = [
  "Cargo.toml",
  "package.json",
  "index.js",
  "src",
  "typescript",
].map((entry) => path.join(root, entry));

const directDriver = /\btypedb[-_]driver\b/i;
const queryPolicy = /\b(TypeQL|typeql|QueryCompiler|execute_query)\b/;
const failures = [];

for (const file of files(scannedPaths)) {
  const relative = path.relative(root, file);
  const text = fs.readFileSync(file, "utf8");
  if (directDriver.test(text)) {
    failures.push(`${relative}: direct TypeDB driver reference`);
  }
  if (queryPolicy.test(text)) {
    failures.push(`${relative}: query compiler policy reference`);
  }
}

if (failures.length > 0) {
  throw new Error(`Node facade scope probe failed:\n${failures.join("\n")}`);
}

function* files(paths) {
  for (const entry of paths) {
    const stat = fs.statSync(entry);
    if (stat.isDirectory()) {
      for (const child of fs.readdirSync(entry)) {
        yield* files([path.join(entry, child)]);
      }
    } else if (entry.endsWith(".rs") || entry.endsWith(".ts") || entry.endsWith(".js") || entry.endsWith(".toml") || entry.endsWith(".json")) {
      yield entry;
    }
  }
}
