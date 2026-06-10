import assert from "node:assert/strict";
import { describe, test } from "node:test";

import { toClassName, toFieldName } from "../../typescript/generator/naming.js";

describe("toClassName", () => {
  test("single word", () => {
    assert.equal(toClassName("person"), "Person");
  });

  test("kebab-separated words", () => {
    assert.equal(toClassName("order-line"), "OrderLine");
  });

  test("kebab with prefix", () => {
    assert.equal(toClassName("parity-person"), "ParityPerson");
  });

  test("trailing number segment", () => {
    assert.equal(toClassName("isbn-13"), "Isbn13");
  });

  test("underscore is treated as kebab separator", () => {
    assert.equal(toClassName("login_at"), "LoginAt");
  });

  test("all-uppercase input is lower-cased except first char (capitalize semantics)", () => {
    // Python's capitalize() lower-cases everything after the first character.
    assert.equal(toClassName("URL"), "Url");
  });
});

describe("toFieldName", () => {
  test("simple kebab converts to snake_case", () => {
    assert.equal(toFieldName("parity-id"), "parity_id");
  });

  test("multi-segment kebab", () => {
    assert.equal(toFieldName("parity-birth-date"), "parity_birth_date");
  });

  test("single segment with prefix", () => {
    assert.equal(toFieldName("parity-tag"), "parity_tag");
  });
});
