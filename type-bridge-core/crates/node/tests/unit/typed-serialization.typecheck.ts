/**
 * Compile-only type-level checks for toDict() / fromDict() (Phase 1).
 *
 * These are NOT runtime tests — they exist to prove that the TypeScript type
 * system catches incorrect usage of the serialization API. Each
 * @ts-expect-error annotation asserts that the expression is a type error;
 * removing the annotation would cause tsc to report an unused @ts-expect-error.
 *
 * Verified invariants:
 *   - toDict() return type is NOT `any` (non-schema key indexing errors).
 *   - fromDict() rejects wrong-brand values at compile time.
 *   - fromDict() rejects missing required fields at compile time.
 */

import { Entity, Key, Unique, attr, field } from "../../typescript/index.js";

// ---------------------------------------------------------------------------
// Model setup
// ---------------------------------------------------------------------------

class Name extends attr.String("name") {}
class Email extends attr.String("email") {}
class Age extends attr.Integer("age") {}

class Person extends Entity("tc-person", {
  name: field(Name, Key),
  email: field(Email, Unique),
  age: field(Age).optional(),
}) {}

const alice = new Person({ name: new Name("Alice"), email: new Email("alice@x.test") });

// ---------------------------------------------------------------------------
// Positive cases: toDict() returns the typed dict shape
// ---------------------------------------------------------------------------

// toDict() result is typed; known keys are accessible without type errors.
const dict = alice.toDict();

// `name` is a required field → PlainFieldValue<FieldSpec<Name, false>> = string
const nameVal: string = dict.name;
void nameVal;

// `age` is optional → string | bigint | undefined depending on Attr type; Age
// wraps bigint so PlainFieldValue is bigint | undefined.
const ageVal: bigint | undefined = dict.age;
void ageVal;

// email is required string field.
const emailVal: string = dict.email;
void emailVal;

// fromDict() from a valid plain dict compiles.
const roundTripped = Person.fromDict({ name: "Alice", email: "alice@x.test" });
void roundTripped;

// ---------------------------------------------------------------------------
// Negative cases: toDict() return is NOT `any`
// ---------------------------------------------------------------------------

// @ts-expect-error indexing a non-schema key is a compile-time error (not `any`)
const _nonSchema: unknown = dict.nonExistentField;
void _nonSchema;

// The return of toDict() cannot be assigned to Record<string, never> because
// the values are real types (string, bigint, etc.), not `never`.
// @ts-expect-error toDict() does not return Record<string, never>
const _neverRecord: Record<string, never> = alice.toDict();
void _neverRecord;

// ---------------------------------------------------------------------------
// Negative cases: fromDict() rejects wrong-brand values
// ---------------------------------------------------------------------------

// `name` expects a string (unwrapped from Name attribute); passing a bigint
// (which would be appropriate for `age`) should be a type error.
// @ts-expect-error wrong primitive type for `name` field (bigint vs string)
Person.fromDict({ name: 42n, email: "x@x.test" });

// Passing a branded Attribute instance (not a plain primitive) is rejected.
// @ts-expect-error Attribute instance is not accepted in fromDict (expects plain primitive)
Person.fromDict({ name: new Name("Alice"), email: "x@x.test" });

// ---------------------------------------------------------------------------
// Negative cases: fromDict() rejects missing required fields
// ---------------------------------------------------------------------------

// `name` is required (Key flag, non-optional FieldSpec) — omitting it is an error.
// @ts-expect-error missing required field `name`
Person.fromDict({ email: "alice@x.test" });

// `email` is also required — omitting it is an error.
// @ts-expect-error missing required field `email`
Person.fromDict({ name: "Alice" });
