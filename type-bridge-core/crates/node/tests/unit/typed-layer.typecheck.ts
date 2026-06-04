import { Entity, Key, attr, field } from "../../typescript/index.js";

class Name extends attr.String("parity-name") {}
class Email extends attr.String("parity-email") {}
class Age extends attr.Integer("parity-age") {}

class Person extends Entity("parity-person", {
  name: field(Name, Key),
  age: field(Age).optional(),
}) {}

new Person({ name: new Name("Alice") });
new Person({ name: new Name("Alice"), age: new Age(30n) });

// @ts-expect-error missing required field
new Person({ age: new Age(30n) });

// @ts-expect-error extra field rejected on object literals
new Person({ name: new Name("Alice"), email: new Email("alice@example.com") });

// @ts-expect-error wrong branded attribute class
new Person({ name: new Email("alice@example.com") });

// @ts-expect-error raw primitives are not accepted
new Person({ name: "Alice" });

const person = new Person({ name: new Name("Alice") });
const maybeAge: Age | undefined = person.age;

// @ts-expect-error Name and Email carry distinct brands
const email: Email = new Name("Alice");

// @ts-expect-error branded attributes are not raw strings
const raw: string = person.name;

void maybeAge;
