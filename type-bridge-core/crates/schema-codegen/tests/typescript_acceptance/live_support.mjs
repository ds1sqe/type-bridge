import crypto from "node:crypto";
import {
  closeSync,
  constants,
  fsyncSync,
  linkSync,
  lstatSync,
  openSync,
  rmSync,
  writeSync,
} from "node:fs";
import { basename, dirname, isAbsolute, resolve } from "node:path";

export const MAX_REPORT_BYTES = 256 * 1024;

export class ProducerError extends Error {
  constructor(code, message, options = undefined) {
    super(`${code}: ${message}`, options);
    this.name = "ProducerError";
    this.code = code;
  }
}

export function requireCondition(condition, code, message) {
  if (!condition) throw new ProducerError(code, message);
}

function canonicalValue(value, ancestors = new Set()) {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      throw new ProducerError(
        "invalid_report_value",
        "report contains a non-finite number",
      );
    }
    return value;
  }
  if (typeof value !== "object") {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a non-JSON value",
    );
  }
  if (ancestors.has(value)) {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a recursive value",
    );
  }
  const nextAncestors = new Set(ancestors);
  nextAncestors.add(value);
  if (Array.isArray(value)) {
    return value.map((member) => canonicalValue(member, nextAncestors));
  }
  if (Object.getPrototypeOf(value) !== Object.prototype) {
    throw new ProducerError(
      "invalid_report_value",
      "report contains a non-plain object",
    );
  }
  return Object.fromEntries(
    Object.entries(value)
      .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
      .map(([key, member]) => [key, canonicalValue(member, nextAncestors)]),
  );
}

export function canonicalString(value) {
  let encoded;
  try {
    encoded = JSON.stringify(canonicalValue(value));
  } catch (error) {
    if (error instanceof ProducerError) throw error;
    throw new ProducerError(
      "invalid_report_value",
      "report cannot be canonicalized",
      { cause: error },
    );
  }
  if (encoded === undefined) {
    throw new ProducerError(
      "invalid_report_value",
      "report cannot be canonicalized",
    );
  }
  return encoded;
}

export function canonicalBytes(value) {
  return Buffer.from(`${canonicalString(value)}\n`);
}

export function realDirectory(value, label) {
  if (!isAbsolute(value)) {
    throw new ProducerError("invalid_directory", `${label} must be absolute`);
  }
  let metadata;
  try {
    metadata = lstatSync(value);
  } catch (error) {
    throw new ProducerError(
      "invalid_directory",
      `${label} cannot be inspected`,
      { cause: error },
    );
  }
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    throw new ProducerError(
      "invalid_directory",
      `${label} must be a real directory`,
    );
  }
  return resolve(value);
}

export function validateOutputTarget(path) {
  let parent;
  try {
    parent = lstatSync(dirname(path));
  } catch (error) {
    throw new ProducerError(
      "invalid_output_parent",
      "report parent cannot be inspected",
      { cause: error },
    );
  }
  if (parent.isSymbolicLink() || !parent.isDirectory()) {
    throw new ProducerError(
      "invalid_output_parent",
      "report parent must be a real directory",
    );
  }
  try {
    lstatSync(path);
  } catch (error) {
    if (error?.code === "ENOENT") return;
    throw new ProducerError(
      "invalid_output_path",
      "report path cannot be inspected",
      { cause: error },
    );
  }
  throw new ProducerError(
    "output_exists",
    "report destination already exists",
  );
}

export function publishReport(path, report) {
  const payload = canonicalBytes(report);
  if (payload.length > MAX_REPORT_BYTES) {
    throw new ProducerError(
      "report_size_limit",
      "canonical report is too large",
    );
  }
  validateOutputTarget(path);
  const temporary = resolve(
    dirname(path),
    `.${basename(path)}.${process.pid}.${crypto.randomUUID()}.tmp`,
  );
  let descriptor;
  let published = false;
  try {
    descriptor = openSync(
      temporary,
      constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL,
      0o600,
    );
    let offset = 0;
    while (offset < payload.length) {
      const count = writeSync(
        descriptor,
        payload,
        offset,
        payload.length - offset,
        offset,
      );
      if (count === 0) {
        throw new ProducerError(
          "output_write_failed",
          "report could not be written completely",
        );
      }
      offset += count;
    }
    fsyncSync(descriptor);
    closeSync(descriptor);
    descriptor = undefined;
    linkSync(temporary, path);
    published = true;
  } catch (error) {
    if (error?.code === "EEXIST") {
      throw new ProducerError(
        "output_exists",
        "report destination already exists",
        { cause: error },
      );
    }
    if (error instanceof ProducerError) throw error;
    throw new ProducerError(
      "output_write_failed",
      "report could not be written",
      { cause: error },
    );
  } finally {
    if (descriptor !== undefined) closeSync(descriptor);
    try {
      rmSync(temporary, { force: true });
    } catch (error) {
      if (!published) {
        throw new ProducerError(
          "output_write_failed",
          "temporary report could not be removed",
          { cause: error },
        );
      }
    }
  }
}
