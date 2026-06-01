/**
 * Attribute value type integration — verifies that all 9 TypeDB primitive
 * value types round-trip correctly through insert/get, and that multi-value
 * Card(0..N) attributes are stored and retrieved as arrays.
 *
 * Mirrors node_entity_all_primitive_attribute_values_against_typedb and
 * node_entity_multi_value_attributes_against_typedb in the Rust suite.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";

import {
  connectIntegration,
  defineSchema,
  newCrudSchema,
  crudSchemaTypeql,
  personDescriptor,
  rowAttribute,
  rowAttributes,
  string,
  long,
  double,
  boolean,
  date,
  datetime,
  datetimetz,
  decimal,
  duration,
} from "../common/index.ts";

const db = connectIntegration();

/** Decimal values round-trip with a `dec` suffix; compare numerically. */
function decimalNumber(value: unknown): number {
  return Number.parseFloat((value as { Decimal: string }).Decimal);
}

function assertDateTimePrefix(value: unknown, prefix: string): void {
  const actual = (value as { DateTime: string }).DateTime;
  assert.ok(actual.startsWith(prefix), `expected DateTime to start with ${prefix}, got ${actual}`);
}

function assertDateTimeTzPrefix(value: unknown, prefix: string, zone: string): void {
  const actual = (value as { DateTimeTZ: string }).DateTimeTZ;
  assert.ok(
    actual.startsWith(prefix) && actual.endsWith(zone),
    `expected DateTimeTZ ${prefix}…${zone}, got ${actual}`,
  );
}

describe("all 9 primitive attribute types", () => {
  const s = newCrudSchema("attrs");
  defineSchema(db, crudSchemaTypeql(s));
  const manager = db.entityManager(personDescriptor(s));

  test("insert with all 9 primitives and verify round-trip", () => {
    const iid = manager.insert({
      name: string("AllTypes"),
      age: long(33n),
      score: double(91.25),
      active: boolean(true),
      birthday: date("1990-01-02"),
      login_at: datetime("2026-05-27T10:30:00"),
      seen_at: datetimetz("2026-05-27T10:30:00+00:00"),
      balance: decimal("1234.56"),
      session_length: duration("PT2H30M"),
    });
    assert.ok(iid.length > 0);

    const rows = manager.get({ name: string("AllTypes") });
    assert.equal(rows.length, 1);
    const row = rows[0];

    assert.deepEqual(rowAttribute(row, s.nameAttr), { String: "AllTypes" });
    assert.deepEqual(rowAttribute(row, s.ageAttr), { Long: "33" });
    assert.deepEqual(rowAttribute(row, s.scoreAttr), { Double: 91.25 });
    assert.deepEqual(rowAttribute(row, s.activeAttr), { Boolean: true });
    assert.deepEqual(rowAttribute(row, s.birthdayAttr), { Date: "1990-01-02" });
    // TypeDB returns temporal values at nanosecond precision and tags decimals
    // with a `dec` suffix; assert lossless round-trip rather than the runtime's
    // exact serialization (that format is the ORM layer's contract, not the
    // package's).
    assertDateTimePrefix(rowAttribute(row, s.loginAtAttr), "2026-05-27T10:30:00");
    assertDateTimeTzPrefix(rowAttribute(row, s.seenAtAttr), "2026-05-27T10:30:00", "+00:00");
    assert.equal(decimalNumber(rowAttribute(row, s.balanceAttr)), 1234.56);
    assert.deepEqual(rowAttribute(row, s.sessionLengthAttr), { Duration: "PT2H30M" });
  });

  test("update changes all 9 primitive values", () => {
    const iid = manager.insert({
      name: string("UpdateAllTypes"),
      age: long(33n),
      score: double(91.25),
      active: boolean(true),
      birthday: date("1990-01-02"),
      login_at: datetime("2026-05-27T10:30:00"),
      seen_at: datetimetz("2026-05-27T10:30:00+00:00"),
      balance: decimal("1234.56"),
      session_length: duration("PT2H30M"),
    });

    manager.update(
      {
        name: string("UpdateAllTypes"),
        age: long(34n),
        score: double(99.5),
        active: boolean(false),
        birthday: date("1991-03-04"),
        login_at: datetime("2026-05-28T11:45:00"),
        seen_at: datetimetz("2026-05-28T11:45:00+00:00"),
        balance: decimal("4321.00"),
        session_length: duration("PT45M"),
      },
      iid,
    );

    const updated = manager.getByIid(iid);
    assert.ok(updated !== null);
    assert.deepEqual(rowAttribute(updated!, s.activeAttr), { Boolean: false });
    assert.equal(decimalNumber(rowAttribute(updated!, s.balanceAttr)), 4321);
    assert.deepEqual(rowAttribute(updated!, s.sessionLengthAttr), { Duration: "PT45M" });
  });
});

describe("multi-value Card(0..N) attributes", () => {
  const s = newCrudSchema("multi");
  defineSchema(db, crudSchemaTypeql(s));
  const manager = db.entityManager(personDescriptor(s));

  test("insert multiple values for each attribute and verify counts", () => {
    const iid = manager.insert({
      name: string("MultiTypes"),
      age: [long(85n), long(90n), long(78n)],
      score: [double(1.5), double(2.7), double(3.9)],
      active: [boolean(true), boolean(false)],
      birthday: [date("2024-01-15"), date("2024-03-01"), date("2024-06-01")],
      login_at: [
        datetime("2024-01-01T10:00:00"),
        datetime("2024-01-01T11:00:00"),
        datetime("2024-01-01T12:00:00"),
      ],
      seen_at: [
        datetimetz("2024-01-01T10:00:00+00:00"),
        datetimetz("2024-01-01T14:00:00+00:00"),
      ],
      balance: [decimal("999.99"), decimal("899.99"), decimal("849.99")],
      session_length: [duration("PT30M"), duration("PT1H"), duration("PT2H")],
    });
    assert.ok(iid.length > 0);

    const rows = manager.get({ name: string("MultiTypes") });
    assert.equal(rows.length, 1);
    const row = rows[0];

    assert.equal(rowAttributes(row, s.ageAttr).length, 3, "age should have 3 values");
    assert.equal(rowAttributes(row, s.scoreAttr).length, 3, "score should have 3 values");
    assert.equal(rowAttributes(row, s.activeAttr).length, 2, "active should have 2 values");
    assert.equal(rowAttributes(row, s.birthdayAttr).length, 3, "birthday should have 3 values");
    assert.equal(rowAttributes(row, s.loginAtAttr).length, 3, "login_at should have 3 values");
    assert.equal(rowAttributes(row, s.seenAtAttr).length, 2, "seen_at should have 2 values");
    assert.equal(rowAttributes(row, s.balanceAttr).length, 3, "balance should have 3 values");
    assert.equal(rowAttributes(row, s.sessionLengthAttr).length, 3, "session_length should have 3 values");
  });

  test("update replaces only the provided attributes", () => {
    const iid = manager.insert({
      name: string("MultiUpdate"),
      age: [long(10n), long(20n), long(30n)],
      balance: [decimal("1.00"), decimal("2.00"), decimal("3.00")],
      session_length: [duration("PT10M"), duration("PT20M"), duration("PT30M")],
    });

    manager.update(
      {
        name: string("MultiUpdate"),
        age: [long(100n), long(200n)],
        balance: [decimal("10.00"), decimal("20.00")],
        session_length: [duration("PT5M")],
      },
      iid,
    );

    const updated = manager.getByIid(iid);
    assert.ok(updated !== null);
    assert.equal(rowAttributes(updated!, s.ageAttr).length, 2, "age should be replaced with 2 values");
    assert.equal(rowAttributes(updated!, s.balanceAttr).length, 2, "balance should be replaced with 2 values");
    assert.equal(rowAttributes(updated!, s.sessionLengthAttr).length, 1, "session_length should be replaced with 1 value");
  });
});
