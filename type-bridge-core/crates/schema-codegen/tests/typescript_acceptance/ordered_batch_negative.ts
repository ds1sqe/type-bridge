import {
  Identifier,
  Membership,
  Person,
  Score,
  type ProjectedBatchUpdate,
} from "./generated_ordered/src/index.js";
import type { Database } from "@type-bridge/node";

declare const database: Database;

const person = Person.create({
  identifier: Identifier.create("person"),
  score: Score.create(1n),
  tag: [],
});
const membership = Membership.create({ member: [person] });
const personManager = Person.manager(database);

// @ts-expect-error updates use exact readonly IID/replacement tuples, not objects
personManager.updateMany([{ iid: "0xa1", replacement: person }]);
// @ts-expect-error update tuples require both IID and replacement
personManager.updateMany([["0xa1"] as const]);
// @ts-expect-error update tuples contain exactly two elements
personManager.updateMany([["0xa1", person, person] as const]);
// @ts-expect-error an entity manager rejects a relation replacement
personManager.updateMany([["0xa1", membership] as const]);
// @ts-expect-error deleteMany accepts canonical IID strings, not complete models
personManager.deleteMany([person]);
// @ts-expect-error deleteMany accepts canonical IID strings, not numbers
personManager.deleteMany([1]);
// @ts-expect-error exact field values must match the field token's scalar domain
personManager.where(Person.fields.score, "eq", Identifier.create("wrong"));
// @ts-expect-error manager comparisons are the closed six-operator vocabulary
personManager.where(Person.fields.score, "contains", Score.create(1n));
// @ts-expect-error a relation manager rejects a field token owned by Person
Membership.manager(database).where(Person.fields.score, "eq", Score.create(1n));
const canonicalFilter = personManager.where();
// @ts-expect-error canonical filters expose no mutation terminals
canonicalFilter.delete(person);
// @ts-expect-error canonical filters are immutable and expose no object filter escape
canonicalFilter.filter({ score: Score.create(1n) });
// @ts-expect-error batch results are immutable
const mutablePeople: Person[] = personManager.updateMany([["0xa1", person]] as const);
// @ts-expect-error the replacement retains its exact generated complete type
const wrongUpdate: ProjectedBatchUpdate<Person> = ["0xa1", membership];

void mutablePeople;
void wrongUpdate;
