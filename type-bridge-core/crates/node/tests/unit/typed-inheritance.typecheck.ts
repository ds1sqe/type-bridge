// Compile-only type-level checks for inheritance (Phase 1).
// These are NOT runtime tests — they exist purely to prove that the TS type
// system catches wrong-brand usage of inherited fields. The file is compiled
// by tsconfig.unit.json; failures appear as type errors, not test failures.

import { Entity, Key, TypeFlags, Unique, attr, field } from "../../typescript/index.js";

class ParityId extends attr.String("parity-id") {}
class ParityName extends attr.String("parity-name") {}
class ParityEmail extends attr.String("parity-email") {}
class ParityAge extends attr.Integer("parity-age") {}

class ParityParty extends Entity(TypeFlags({ name: "parity-party", abstract: true }), {
  id: field(ParityId, Key),
  name: field(ParityName).optional(),
}) {}

class ParityPerson extends Entity(
  "parity-person",
  {
    email: field(ParityEmail, Unique),
    age: field(ParityAge).optional(),
  },
  { parent: ParityParty },
) {}

// --- Positive cases (must compile) ---

const person = new ParityPerson({
  id: new ParityId("person-1"),
  email: new ParityEmail("p@example.com"),
});

// Inherited field is accessible with the parent's brand.
const id: ParityId = person.id;
const name: ParityName | undefined = person.name;
const email: ParityEmail = person.email;
const age: ParityAge | undefined = person.age;

void id;
void name;
void email;
void age;

// --- Negative cases (must NOT compile) ---

// @ts-expect-error wrong brand: ParityEmail used where ParityId is required (inherited field)
new ParityPerson({ id: new ParityEmail("x@example.com"), email: new ParityEmail("p@example.com") });

// @ts-expect-error missing inherited required field 'id'
new ParityPerson({ email: new ParityEmail("p@example.com") });

// @ts-expect-error wrong brand on inherited field read: ParityEmail is not assignable to ParityId
const wrongBrand: ParityEmail = person.id;
void wrongBrand;
