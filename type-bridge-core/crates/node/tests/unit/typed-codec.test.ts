import assert from "node:assert/strict";
import { describe, test } from "node:test";

import {
  Entity,
  Key,
  TypedCodecError,
  attr,
  field,
  hydrateAttributes,
  lowerAttributes,
  type AttributeInput,
  type DynamicEntityRow,
  type RuntimeAttributeValue,
  type ValueType,
} from "../../typescript/index.js";

class Id extends attr.String("codec-id") {}
class Name extends attr.String("codec-name") {}
class Age extends attr.Integer("codec-age") {}
class Score extends attr.Double("codec-score") {}
class Active extends attr.Boolean("codec-active") {}
class Birthday extends attr.Date("codec-birthday") {}
class LoginAt extends attr.DateTime("codec-login-at") {}
class SeenAt extends attr.DateTimeTZ("codec-seen-at") {}
class Balance extends attr.Decimal("codec-balance") {}
class SessionLength extends attr.Duration("codec-session-length") {}

class AllValues extends Entity("codec-all-values", {
  id: field(Id, Key),
  name: field(Name),
  age: field(Age),
  score: field(Score),
  active: field(Active),
  birthday: field(Birthday),
  loginAt: field(LoginAt),
  seenAt: field(SeenAt),
  balance: field(Balance),
  sessionLength: field(SessionLength),
}) {}

const cases = [
  {
    fieldName: "name",
    attrName: "codec-name",
    valueType: "string",
    attr: new Name("Alice"),
    wire: { value_type: "string", value: "Alice" },
    runtime: { String: "Alice" },
  },
  {
    fieldName: "age",
    attrName: "codec-age",
    valueType: "long",
    attr: new Age(9223372036854775807n),
    wire: { value_type: "long", value: "9223372036854775807" },
    runtime: { Long: "9223372036854775807" },
  },
  {
    fieldName: "score",
    attrName: "codec-score",
    valueType: "double",
    attr: new Score(91.25),
    wire: { value_type: "double", value: 91.25 },
    runtime: { Double: 91.25 },
  },
  {
    fieldName: "active",
    attrName: "codec-active",
    valueType: "boolean",
    attr: new Active(true),
    wire: { value_type: "boolean", value: true },
    runtime: { Boolean: true },
  },
  {
    fieldName: "birthday",
    attrName: "codec-birthday",
    valueType: "date",
    attr: new Birthday("1990-01-02"),
    wire: { value_type: "date", value: "1990-01-02" },
    runtime: { Date: "1990-01-02" },
  },
  {
    fieldName: "loginAt",
    attrName: "codec-login-at",
    valueType: "datetime",
    attr: new LoginAt("2026-06-04T10:30:00"),
    wire: { value_type: "datetime", value: "2026-06-04T10:30:00" },
    runtime: { DateTime: "2026-06-04T10:30:00" },
  },
  {
    fieldName: "seenAt",
    attrName: "codec-seen-at",
    valueType: "datetime-tz",
    attr: new SeenAt("2026-06-04T10:30:00+00:00"),
    wire: { value_type: "datetime-tz", value: "2026-06-04T10:30:00+00:00" },
    runtime: { DateTimeTZ: "2026-06-04T10:30:00+00:00" },
  },
  {
    fieldName: "balance",
    attrName: "codec-balance",
    valueType: "decimal",
    attr: new Balance("1234567890.123456789"),
    wire: { value_type: "decimal", value: "1234567890.123456789" },
    runtime: { Decimal: "1234567890.123456789" },
  },
  {
    fieldName: "sessionLength",
    attrName: "codec-session-length",
    valueType: "duration",
    attr: new SessionLength("PT2H30M"),
    wire: { value_type: "duration", value: "PT2H30M" },
    runtime: { Duration: "PT2H30M" },
  },
] as const satisfies readonly {
  readonly fieldName: keyof AllValues;
  readonly attrName: string;
  readonly valueType: ValueType;
  readonly attr: AllValues[keyof AllValues];
  readonly wire: AttributeInput[string];
  readonly runtime: RuntimeAttributeValue;
}[];

describe("typed value codec", () => {
  test("all 9 value types lower to AttributeInput and hydrate back to branded attributes", () => {
    const instance = new AllValues({
      id: new Id("all-1"),
      name: new Name("Alice"),
      age: new Age(9223372036854775807n),
      score: new Score(91.25),
      active: new Active(true),
      birthday: new Birthday("1990-01-02"),
      loginAt: new LoginAt("2026-06-04T10:30:00"),
      seenAt: new SeenAt("2026-06-04T10:30:00+00:00"),
      balance: new Balance("1234567890.123456789"),
      sessionLength: new SessionLength("PT2H30M"),
    });

    const lowered: AttributeInput = lowerAttributes(instance, AllValues.schema);
    for (const item of cases) {
      assert.deepEqual(lowered[item.fieldName], item.wire);
    }

    const row: DynamicEntityRow = {
      iid: "0xcodec",
      type_name: AllValues.typeName,
      attributes: cases.map((item) => [item.attrName, item.runtime]),
    };
    const hydrated = hydrateAttributes(row, AllValues.schema);
    for (const item of cases) {
      const actual = hydrated[item.fieldName];
      assert.ok(actual.constructor === item.attr.constructor);
      assert.equal(actual.value, item.attr.value);
    }
  });

  test("attr_name differing from field_name hydrates to the schema field", () => {
    const hydrated = hydrateAttributes(
      {
        attributes: [["codec-login-at", { DateTime: "2026-06-04T10:30:00" }]],
      },
      AllValues.schema,
    );
    assert.ok(hydrated.loginAt instanceof LoginAt);
    assert.equal(hydrated.loginAt.value, "2026-06-04T10:30:00");
  });

  test("unknown runtime tags fail loudly", () => {
    assert.throws(
      () =>
        hydrateAttributes(
          {
            attributes: [["codec-name", { Unknown: "Alice" } as never]],
          },
          AllValues.schema,
        ),
      TypedCodecError,
    );
  });
});
