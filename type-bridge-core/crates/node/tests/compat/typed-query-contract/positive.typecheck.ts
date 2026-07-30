import type { RustDatabase, RustTransactionContext } from "@type-bridge/node";

import {
  aggregate,
  type BoundVar,
  type Page,
  type Query,
  QuerySession,
  references,
} from "@type-bridge/node/typed";
import {
  ContractCompany,
  ContractEmployment,
  ContractPerson,
  ContractPersonSkill,
  ContractSkill,
} from "./models.js";

declare const database: RustDatabase;
declare const transaction: RustTransactionContext;

const databaseSession = new QuerySession(database);
const transactionSession = new QuerySession(transaction);
void transactionSession;

const person = databaseSession.var(ContractPerson);
const employment = databaseSession.var(ContractEmployment);
const company = databaseSession.var(ContractCompany);
const skill = databaseSession.var(ContractSkill);
const personSkill = databaseSession.var(ContractPersonSkill);
const secondPerson = databaseSession.var(ContractPerson);

const personBinding: BoundVar<ContractPerson> = person;
void personBinding;

const people: Query<readonly [ContractPerson]> = databaseSession.query(person);
const personRefs = references(ContractPerson);
const onePerson: ContractPerson = people.one();
const personRows: readonly ContractPerson[] = people.rows({ limit: 25 });
const personPage: Page<ContractPerson> = people.pageBy(person, {
  limit: 25,
  includeTotal: true,
});
const pagePerson: ContractPerson = personPage.items[0];
const pageTotal: bigint | undefined = personPage.total;
void onePerson;
void personRows;
void pagePerson;
void pageTotal;

const age = person.field(personRefs.fields.age);
const ageSummary: readonly [
  bigint,
  bigint,
  bigint | null,
  number | null,
  number | null,
  number | null,
] = people.aggregate(person, [
  aggregate.count(),
  aggregate.sum(age),
  aggregate.max(age),
  aggregate.mean(age),
  aggregate.median(age),
  aggregate.std(age),
]);
const groupedCounts: readonly (readonly [
  ContractCompany,
  readonly [bigint],
])[] = people
  .groupBy(person, company)
  .allowCrossJoin(person, company)
  .aggregate([aggregate.count()]);
void ageSummary;
void groupedCounts;

const pair: Query<readonly [ContractPerson, ContractEmployment]> =
  databaseSession.query(person, employment);
const pairOne: readonly [ContractPerson, ContractEmployment] = pair.one();
const pairRows: readonly (readonly [ContractPerson, ContractEmployment])[] = pair.rows({
  limit: 25,
  offset: 5,
});
void pairOne;
void pairRows;

const five: Query<
  readonly [
    ContractPerson,
    ContractEmployment,
    ContractCompany,
    ContractSkill,
    ContractPersonSkill,
  ]
> = databaseSession.query(person, employment, company, skill, personSkill);
const fiveOne: readonly [
  ContractPerson,
  ContractEmployment,
  ContractCompany,
  ContractSkill,
  ContractPersonSkill,
] = five.one();
const fiveRows: readonly (readonly [
  ContractPerson,
  ContractEmployment,
  ContractCompany,
  ContractSkill,
  ContractPersonSkill,
])[] = five.rows({ limit: 10 });
void fiveOne;
void fiveRows;

const repeated: Query<
  readonly [ContractPerson, ContractEmployment, ContractCompany, ContractPerson]
> = databaseSession.query(person, employment, company, secondPerson);
const repeatedOne: readonly [
  ContractPerson,
  ContractEmployment,
  ContractCompany,
  ContractPerson,
] = repeated.one();
const repeatedRows: readonly (readonly [
  ContractPerson,
  ContractEmployment,
  ContractCompany,
  ContractPerson,
])[] = repeated.rows({ limit: 10 });
void repeatedOne;
void repeatedRows;

const p01 = databaseSession.var(ContractPerson);
const p02 = databaseSession.var(ContractPerson);
const p03 = databaseSession.var(ContractPerson);
const p04 = databaseSession.var(ContractPerson);
const p05 = databaseSession.var(ContractPerson);
const p06 = databaseSession.var(ContractPerson);
const p07 = databaseSession.var(ContractPerson);
const p08 = databaseSession.var(ContractPerson);
const p09 = databaseSession.var(ContractPerson);
const p10 = databaseSession.var(ContractPerson);
const p11 = databaseSession.var(ContractPerson);
const p12 = databaseSession.var(ContractPerson);
const p13 = databaseSession.var(ContractPerson);
const p14 = databaseSession.var(ContractPerson);
const p15 = databaseSession.var(ContractPerson);
const p16 = databaseSession.var(ContractPerson);

const sixteen: Query<
  readonly [
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
    ContractPerson,
  ]
> = databaseSession.query(
  p01,
  p02,
  p03,
  p04,
  p05,
  p06,
  p07,
  p08,
  p09,
  p10,
  p11,
  p12,
  p13,
  p14,
  p15,
  p16,
);
const sixteenOne: readonly [
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
  ContractPerson,
] = sixteen.one();
void sixteenOne;
