import {
  Identifier,
  Membership,
  Person,
  Score,
  Tag,
  type OrderedModelToken,
  type OrderedProjectedModelManager,
  type ProjectedBatchUpdate,
  type ProjectedManagerComparison,
  type ProjectedModelFilter,
  type ProjectedModelManager,
} from "./generated_ordered/src/index.js";
import type { RustDatabase } from "@type-bridge/node";

declare const database: RustDatabase;

const first = Person.create({
  identifier: Identifier.create("first"),
  score: Score.create(1n),
  tag: [Tag.create("one"), Tag.create("two")],
});
const second = Person.create({
  identifier: Identifier.create("second"),
  score: Score.create(2n),
  tag: [],
});
const membership = Membership.create({ member: [first, second] });

const personToken: OrderedModelToken<
  typeof Person.typeKey,
  Person,
  typeof Person.create,
  typeof Person.reference,
  typeof Person.fields,
  typeof Person.roles
> = Person;
const personManager: OrderedProjectedModelManager<Person, typeof Person.typeKey> =
  personToken.manager(database);
const legacyPersonManager: ProjectedModelManager<Person> = personManager;

const insertedPeople: readonly Person[] = personManager.insertMany([
  first,
  second,
] as const);
const putPeople: readonly Person[] = personManager.putMany([
  first,
  second,
] as const);
const personUpdates = [
  ["0xa1", first],
  ["0xa2", second],
] as const satisfies readonly ProjectedBatchUpdate<Person>[];
const updatedPeople: readonly Person[] = personManager.updateMany(personUpdates);
personManager.deleteMany(["0xa1", "0xa2"] as const);

const filtered: OrderedProjectedModelManager<Person> = personManager.filter({
  score__gte: Score.create(1n),
});
const filteredUpdates: readonly Person[] = filtered.updateMany(personUpdates);
filtered.deleteMany(["0xa1"] as const);

const comparison: ProjectedManagerComparison = "gte";
const canonicalFilter: ProjectedModelFilter<Person, typeof Person.typeKey> =
  personManager
    .where(Person.fields.score, comparison, Score.create(1n))
    .where(Person.fields.tag, "eq", Tag.create("one"));
const canonicalRoot = personManager.where();
const canonicalPeople: readonly Person[] = canonicalFilter.all();
const canonicalFirst: Person | null = personManager
  .where(Person.fields.identifier, "eq", Identifier.create("first"))
  .first();
const canonicalCount: bigint = canonicalFilter.count();
const canonicalExists: boolean = canonicalRoot.exists();

const membershipManager: OrderedProjectedModelManager<
  Membership,
  typeof Membership.typeKey
> =
  Membership.manager(database);
const insertedMemberships: readonly Membership[] =
  membershipManager.insertMany([membership] as const);
const putMemberships: readonly Membership[] =
  membershipManager.putMany([membership] as const);
const membershipUpdates = [
  ["0xb1", membership],
] as const satisfies readonly ProjectedBatchUpdate<Membership>[];
const updatedMemberships: readonly Membership[] =
  membershipManager.updateMany(membershipUpdates);
membershipManager.deleteMany(["0xb1"] as const);

const emptyPeople: readonly Person[] = personManager.insertMany([]);
const emptyPuts: readonly Person[] = personManager.putMany([]);
const emptyUpdates: readonly Person[] = personManager.updateMany([]);
personManager.deleteMany([]);

void legacyPersonManager;
void insertedPeople;
void putPeople;
void updatedPeople;
void filteredUpdates;
void canonicalPeople;
void canonicalFirst;
void canonicalCount;
void canonicalExists;
void insertedMemberships;
void putMemberships;
void updatedMemberships;
void emptyPeople;
void emptyPuts;
void emptyUpdates;
