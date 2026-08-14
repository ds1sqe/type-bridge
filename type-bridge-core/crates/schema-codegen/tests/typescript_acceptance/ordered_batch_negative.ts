import { Person as LegacyPerson } from "./generated_v2/src/index.js";
// @ts-expect-error unordered V1 packages do not export ordered successor types
import type { OrderedModelToken, OrderedProjectedModelManager, ProjectedBatchUpdate as LegacyBatchUpdate } from "./generated_v2/src/index.js";
import {
  Identifier,
  Membership,
  Person,
  Score,
  type ProjectedBatchUpdate,
} from "./generated_ordered/src/index.js";
import type { RustDatabase } from "@type-bridge/node";

declare const database: RustDatabase;
declare const legacyManager: ReturnType<typeof LegacyPerson.manager>;

const person = Person.create({
  identifier: Identifier.create("person"),
  score: Score.create(1n),
  tag: [],
});
const membership = Membership.create({ member: [person] });
const personManager = Person.manager(database);

// @ts-expect-error unordered managers do not expose successor update batches
legacyManager.updateMany([]);
// @ts-expect-error unordered managers do not expose successor delete batches
legacyManager.deleteMany([]);
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
// @ts-expect-error batch results are immutable
const mutablePeople: Person[] = personManager.updateMany([["0xa1", person]] as const);
// @ts-expect-error the replacement retains its exact generated complete type
const wrongUpdate: ProjectedBatchUpdate<Person> = ["0xa1", membership];

void mutablePeople;
void wrongUpdate;
