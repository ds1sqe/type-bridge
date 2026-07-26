"use strict";

const fs = require("node:fs");
const path = require("node:path");
const ts = require("typescript");

const CRATE_ROOT = path.resolve(__dirname, "..");
const SCANNED_PATHS = [
  "Cargo.toml",
  "package.json",
  "src",
  "typescript",
].map((entry) => path.join(CRATE_ROOT, entry));

const HOST_QUERY_POLICY_IDENTIFIERS = new Set([
  "QueryCompiler",
  "compileQuery",
  "compile_query",
  "executeQuery",
  "execute_query",
  "queryCompiler",
  "query_compiler",
]);
const DEPENDENCY_SECTIONS = [
  "dependencies",
  "devDependencies",
  "optionalDependencies",
  "peerDependencies",
  "overrides",
  "resolutions",
];

function isDirectDriverPackage(name) {
  return /^(?:npm:)?(?:typedb[-_]driver|@typedb\/driver)(?:@|\/|$)/i.test(
    name,
  );
}

function lineAt(text, index) {
  return text.slice(0, Math.max(index, 0)).split("\n").length;
}

function finding(file, text, index, code, message) {
  return {
    code,
    file,
    line: lineAt(text, index),
    message,
  };
}

function constantString(node) {
  if (ts.isStringLiteralLike(node)) {
    return node.text;
  }
  if (
    ts.isParenthesizedExpression(node) ||
    ts.isAsExpression(node) ||
    ts.isTypeAssertionExpression(node) ||
    ts.isNonNullExpression(node)
  ) {
    return constantString(node.expression);
  }
  if (
    ts.isBinaryExpression(node) &&
    node.operatorToken.kind === ts.SyntaxKind.PlusToken
  ) {
    const left = constantString(node.left);
    const right = constantString(node.right);
    return left === null || right === null ? null : left + right;
  }
  return null;
}

function inspectTypeScript(file, text) {
  const source = ts.createSourceFile(
    file,
    text,
    ts.ScriptTarget.Latest,
    true,
    file.endsWith(".js") ? ts.ScriptKind.JS : ts.ScriptKind.TS,
  );
  const findings = [];

  function add(node, code, message) {
    findings.push(finding(file, text, node.getStart(source), code, message));
  }

  function inspectModuleExpression(node) {
    const moduleName = constantString(node);
    if (moduleName !== null && isDirectDriverPackage(moduleName)) {
      add(
        node,
        "direct_driver_module",
        `direct TypeDB driver module ownership: ${moduleName}`,
      );
    }
  }

  function inspectPolicyProperty(node, expression) {
    const property = constantString(expression);
    if (property !== null && HOST_QUERY_POLICY_IDENTIFIERS.has(property)) {
      add(
        node,
        "host_query_policy",
        `host-side query compiler/execution property: ${property}`,
      );
    }
  }

  function visit(node) {
    if (
      ts.isIdentifier(node) &&
      HOST_QUERY_POLICY_IDENTIFIERS.has(node.text)
    ) {
      add(
        node,
        "host_query_policy",
        `host-side query compiler/execution identifier: ${node.text}`,
      );
    }
    if (
      ts.isIdentifier(node) &&
      /^(?:typedb_driver|typedbDriver|TypeDBDriver)$/.test(node.text)
    ) {
      add(
        node,
        "direct_driver_identifier",
        `direct TypeDB driver identifier: ${node.text}`,
      );
    }

    if (
      (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) &&
      node.moduleSpecifier !== undefined
    ) {
      inspectModuleExpression(node.moduleSpecifier);
    } else if (
      ts.isImportEqualsDeclaration(node) &&
      ts.isExternalModuleReference(node.moduleReference) &&
      node.moduleReference.expression !== undefined
    ) {
      inspectModuleExpression(node.moduleReference.expression);
    } else if (ts.isImportTypeNode(node)) {
      const argument = ts.isLiteralTypeNode(node.argument)
        ? node.argument.literal
        : node.argument;
      inspectModuleExpression(argument);
    } else if (ts.isCallExpression(node) && node.arguments.length > 0) {
      const isRequire =
        ts.isIdentifier(node.expression) && node.expression.text === "require";
      const isDynamicImport =
        node.expression.kind === ts.SyntaxKind.ImportKeyword;
      if (isRequire || isDynamicImport) {
        inspectModuleExpression(node.arguments[0]);
      }
    }

    if (ts.isElementAccessExpression(node)) {
      inspectPolicyProperty(node, node.argumentExpression);
    } else if (ts.isComputedPropertyName(node)) {
      inspectPolicyProperty(node, node.expression);
    } else if (
      (ts.isPropertyAssignment(node) ||
        ts.isMethodDeclaration(node) ||
        ts.isPropertyDeclaration(node) ||
        ts.isPropertySignature(node) ||
        ts.isGetAccessorDeclaration(node) ||
        ts.isSetAccessorDeclaration(node)) &&
      ts.isStringLiteralLike(node.name)
    ) {
      inspectPolicyProperty(node, node.name);
    }

    ts.forEachChild(node, visit);
  }

  visit(source);
  return findings;
}

function maskRange(output, text, start, end) {
  for (let index = start; index < end; index += 1) {
    if (text[index] !== "\n" && text[index] !== "\r") {
      output[index] = " ";
    }
  }
}

function maskRustCommentsAndLiterals(text) {
  const output = text.split("");
  let index = 0;

  while (index < text.length) {
    if (text.startsWith("//", index)) {
      const end = text.indexOf("\n", index + 2);
      const boundedEnd = end === -1 ? text.length : end;
      maskRange(output, text, index, boundedEnd);
      index = boundedEnd;
      continue;
    }
    if (text.startsWith("/*", index)) {
      let depth = 1;
      let cursor = index + 2;
      while (cursor < text.length && depth > 0) {
        if (text.startsWith("/*", cursor)) {
          depth += 1;
          cursor += 2;
        } else if (text.startsWith("*/", cursor)) {
          depth -= 1;
          cursor += 2;
        } else {
          cursor += 1;
        }
      }
      maskRange(output, text, index, cursor);
      index = cursor;
      continue;
    }

    const raw = text.slice(index).match(/^(?:b|c)?r(#{0,255})"/);
    if (raw !== null) {
      const terminator = `"${raw[1]}`;
      const contentStart = index + raw[0].length;
      const found = text.indexOf(terminator, contentStart);
      const end = found === -1 ? text.length : found + terminator.length;
      maskRange(output, text, index, end);
      index = end;
      continue;
    }

    const stringPrefix =
      text[index] === '"'
        ? 0
        : (text[index] === "b" || text[index] === "c") &&
            text[index + 1] === '"'
          ? 1
          : -1;
    if (stringPrefix >= 0) {
      let cursor = index + stringPrefix + 1;
      while (cursor < text.length) {
        if (text[cursor] === "\\") {
          cursor += 2;
        } else if (text[cursor] === '"') {
          cursor += 1;
          break;
        } else {
          cursor += 1;
        }
      }
      maskRange(output, text, index, cursor);
      index = cursor;
      continue;
    }

    const character = text.slice(index).match(/^(?:b)?'(?:\\.|[^\\'\r\n])'/);
    if (character !== null) {
      const end = index + character[0].length;
      maskRange(output, text, index, end);
      index = end;
      continue;
    }
    index += 1;
  }

  return output.join("");
}

function inspectRust(file, text) {
  const code = maskRustCommentsAndLiterals(text);
  const findings = [];
  const checks = [
    {
      code: "direct_driver_identifier",
      message: "direct TypeDB driver identifier: typedb_driver",
      pattern: /\btypedb_driver\b/g,
    },
    {
      code: "host_query_policy",
      message: "host-side query compiler/execution identifier",
      pattern:
        /\b(?:QueryCompiler|compileQuery|compile_query|executeQuery|execute_query|queryCompiler|query_compiler)\b/g,
    },
  ];

  for (const check of checks) {
    for (const match of code.matchAll(check.pattern)) {
      findings.push(
        finding(
          file,
          text,
          match.index,
          check.code,
          `${check.message}: ${match[0]}`,
        ),
      );
    }
  }
  return findings;
}

function maskTomlComments(text) {
  const output = text.split("");
  let quote = null;
  let escaped = false;

  for (let index = 0; index < text.length; index += 1) {
    const character = text[index];
    if (quote !== null) {
      if (quote === '"' && character === "\\" && !escaped) {
        escaped = true;
        continue;
      }
      if (character === quote && !escaped) {
        quote = null;
      }
      escaped = false;
      continue;
    }
    if (character === '"' || character === "'") {
      quote = character;
    } else if (character === "#") {
      const end = text.indexOf("\n", index + 1);
      const boundedEnd = end === -1 ? text.length : end;
      maskRange(output, text, index, boundedEnd);
      index = boundedEnd - 1;
    }
  }
  return output.join("");
}

function inspectCargoManifest(file, text) {
  const manifest = maskTomlComments(text);
  const findings = [];
  const dependencyTables = new Set([
    "build-dependencies",
    "dependencies",
    "dev-dependencies",
  ]);
  const tablePattern = /^[ \t]*\[([^\]\r\n]+)\][ \t]*$/gm;
  for (const table of manifest.matchAll(tablePattern)) {
    const segments = tomlDottedSegments(table[1]);
    const dependencyIndex = segments.findIndex((segment) =>
      dependencyTables.has(segment),
    );
    if (
      dependencyIndex >= 0 &&
      segments
        .slice(dependencyIndex + 1)
        .some((segment) => /^(?:typedb[-_]driver)$/i.test(segment))
    ) {
      findings.push(
        finding(
          file,
          text,
          table.index,
          "direct_driver_dependency",
          "direct TypeDB driver Cargo dependency table",
        ),
      );
    }
  }
  const checks = [
    /^(?:[ \t]*)(?:"typedb[-_]driver"|'typedb[-_]driver'|typedb[-_]driver)[ \t]*=/gim,
    /\bpackage[ \t]*=[ \t]*(?:"typedb[-_]driver"|'typedb[-_]driver')/gi,
  ];
  for (const pattern of checks) {
    for (const match of manifest.matchAll(pattern)) {
      findings.push(
        finding(
          file,
          text,
          match.index,
          "direct_driver_dependency",
          "direct TypeDB driver Cargo dependency",
        ),
      );
    }
  }
  return findings;
}

function tomlDottedSegments(key) {
  const segments = [];
  let current = "";
  let quote = null;
  let escaped = false;

  for (const character of key) {
    if (quote !== null) {
      if (quote === '"' && character === "\\" && !escaped) {
        escaped = true;
        current += character;
        continue;
      }
      if (character === quote && !escaped) {
        quote = null;
      } else {
        current += character;
      }
      escaped = false;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === ".") {
      segments.push(current.trim());
      current = "";
    } else {
      current += character;
    }
  }
  segments.push(current.trim());
  return segments;
}

function inspectPackageManifest(file, text) {
  let manifest;
  try {
    manifest = JSON.parse(text);
  } catch (error) {
    return [
      finding(
        file,
        text,
        0,
        "invalid_package_manifest",
        `package manifest is not valid JSON: ${error.message}`,
      ),
    ];
  }

  const findings = [];
  function inspectDependencies(value, trail) {
    if (Array.isArray(value)) {
      for (const entry of value) {
        if (typeof entry === "string" && isDirectDriverPackage(entry)) {
          findings.push(
            finding(
              file,
              text,
              text.indexOf(entry),
              "direct_driver_dependency",
              `direct TypeDB driver package at ${trail}: ${entry}`,
            ),
          );
        }
      }
      return;
    }
    if (value === null || typeof value !== "object") {
      return;
    }
    for (const [name, requirement] of Object.entries(value)) {
      const dependencyTrail = `${trail}.${name}`;
      if (
        isDirectDriverPackage(name) ||
        (typeof requirement === "string" &&
          isDirectDriverPackage(requirement))
      ) {
        findings.push(
          finding(
            file,
            text,
            text.indexOf(name),
            "direct_driver_dependency",
            `direct TypeDB driver package at ${dependencyTrail}`,
          ),
        );
      }
      if (trail === "overrides" || trail === "resolutions") {
        inspectDependencies(requirement, dependencyTrail);
      }
    }
  }

  for (const section of DEPENDENCY_SECTIONS) {
    inspectDependencies(manifest[section], section);
  }
  return findings;
}

function inspectFile(file, text) {
  const basename = path.basename(file);
  if (basename === "Cargo.toml") {
    return inspectCargoManifest(file, text);
  }
  if (basename === "package.json") {
    return inspectPackageManifest(file, text);
  }
  if (file.endsWith(".rs")) {
    return inspectRust(file, text);
  }
  if (file.endsWith(".ts") || file.endsWith(".js")) {
    return inspectTypeScript(file, text);
  }
  return [];
}

function* files(paths) {
  for (const entry of paths) {
    const stat = fs.statSync(entry);
    if (stat.isDirectory()) {
      for (const child of fs.readdirSync(entry).sort()) {
        yield* files([path.join(entry, child)]);
      }
    } else if (
      entry.endsWith(".rs") ||
      entry.endsWith(".ts") ||
      entry.endsWith(".js") ||
      entry.endsWith(".toml") ||
      entry.endsWith(".json")
    ) {
      yield entry;
    }
  }
}

function inspectProject(scannedPaths = SCANNED_PATHS) {
  const findings = [];
  let fileCount = 0;
  for (const file of files(scannedPaths)) {
    fileCount += 1;
    findings.push(...inspectFile(file, fs.readFileSync(file, "utf8")));
  }
  findings.sort(
    (left, right) =>
      left.file.localeCompare(right.file) ||
      left.line - right.line ||
      left.code.localeCompare(right.code),
  );
  return { fileCount, findings };
}

function main() {
  const result = inspectProject();
  if (result.findings.length > 0) {
    const rendered = result.findings.map(
      ({ code, file, line, message }) =>
        `${path.relative(CRATE_ROOT, file)}:${line} [${code}] ${message}`,
    );
    throw new Error(`Node facade scope probe failed:\n${rendered.join("\n")}`);
  }
  console.log(
    `[scope-probe] ${result.fileCount} source/manifest files satisfy the thin-binding boundary`,
  );
}

module.exports = {
  inspectFile,
  inspectProject,
  isDirectDriverPackage,
  maskRustCommentsAndLiterals,
};

if (require.main === module) {
  main();
}
