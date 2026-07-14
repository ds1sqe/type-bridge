import type {
  RustDatabase,
  RustTransactionContext,
  TypedQuery,
} from "../../typescript/index.js";
import { Entity, Key, Relation, attr, field, role } from "../../typescript/index.js";
import {
  QuerySession,
  references,
  type Page,
  type Query,
  type QueryRow,
} from "../../typescript/typed/index.js";
import { diagnosticQuerySession } from "../../typescript/typed/session.js";
// @ts-expect-error broad selection tuples are internal and must not erase public output types
import type { QuerySelections } from "../../typescript/typed/index.js";
// @ts-expect-error broad output tuples are internal and must not erase public output types
import type { QuerySlots } from "../../typescript/typed/index.js";

class QueryName extends attr.String("query-v2-name") {}
class QueryPerson extends Entity("query-v2-person", {
  name: field(QueryName, Key),
}) {}
class QueryCompany extends Entity("query-v2-company", {
  name: field(QueryName, Key),
}) {}
class QueryPersonSibling extends Entity("query-v2-person-sibling", {
  name: field(QueryName, Key),
}) {}
class QueryEmployment extends Relation("query-v2-employment", {
  code: field(QueryName, Key),
  employee: role(QueryPerson),
  employer: role(QueryCompany),
}) {}
class QueryParty extends Entity("query-v2-party", {
  name: field(QueryName, Key),
}) {}
class QueryEmployee extends Entity(
  "query-v2-employee",
  {},
  { parent: QueryParty },
) {}

declare const database: RustDatabase;
declare const transaction: RustTransactionContext;

void (null as unknown as QuerySelections);
void (null as unknown as QuerySlots);

// @ts-expect-error public sessions require an execution connection
new QuerySession();
const diagnosticSession = diagnosticQuerySession();
const databaseSession = new QuerySession(database);
const transactionSession = new QuerySession(transaction);
const subtypeSession: QuerySession = new QuerySession(database)
  .registerModels(QueryEmployee);
const party = subtypeSession.var(QueryParty, "subtypes");
const subtypeQuery: Query<readonly [QueryParty]> = subtypeSession.query(party);
void subtypeQuery;
void diagnosticSession;
void transactionSession;

const person = databaseSession.var(QueryPerson);
const secondPerson = databaseSession.var(QueryPerson);
const company = databaseSession.var(QueryCompany);
const sibling = databaseSession.var(QueryPersonSibling);
const employment = databaseSession.var(QueryEmployment);
const personRefs = references(QueryPerson);
const employmentRefs = references(QueryEmployment);

declare const legacyTypedQuery: TypedQuery<QueryPerson, { readonly name: string }>;
void legacyTypedQuery;

const one: Query<readonly [QueryPerson]> = databaseSession.query(person);
const oneResult: QueryPerson = one.one();
const oneRows: readonly QueryPerson[] = one.rows({ limit: 10 });
const onePage: Page<QueryPerson> = one.pageBy(person, {
  limit: 10,
  orderBy: [person.field(personRefs.fields.name).asc()],
  includeTotal: true,
});
const oneCount: bigint = one.countBy(person);
const oneExists: boolean = one.existsBy(person);
void oneResult;
void oneRows;
void onePage;
void oneCount;
void oneExists;

const pair: Query<readonly [QueryPerson, QueryCompany]> = databaseSession.query(
  person,
  company,
);
const pairResult: readonly [QueryPerson, QueryCompany] = pair.one();
const pairRows: readonly (readonly [QueryPerson, QueryCompany])[] = pair.rows({
  limit: 10,
});
const pairCount: bigint = pair.countBy(person);
const pairExists: boolean = pair.existsBy(company);
void pairResult;
void pairRows;
void pairCount;
void pairExists;

const repeated: Query<readonly [QueryPerson, QueryPerson]> = databaseSession.query(
  person,
  secondPerson,
);
const repeatedResult: readonly [QueryPerson, QueryPerson] = repeated.one();
void repeatedResult;

const siblings: Query<readonly [QueryPerson, QueryPersonSibling]> =
  databaseSession.query(person, sibling);
void siblings;

const five = databaseSession.query(person, employment, company, secondPerson, sibling);
const fiveResult: readonly [
  QueryPerson,
  QueryEmployment,
  QueryCompany,
  QueryPerson,
  QueryPersonSibling,
] = five.one();
void fiveResult;

const hiddenAndFiltered: Query<readonly [QueryPerson, QueryCompany]> = pair
  .match(employment)
  .where(
    employment.role(employmentRefs.roles.employee).connects(person),
    employment.role(employmentRefs.roles.employer).connects(company),
    person.field(personRefs.fields.name).startsWith("A"),
  )
  .allowCrossJoin(person, company);
void hiddenAndFiltered;

const orderedEmployments = employment
  .collect()
  .orderBy(employment.field(employmentRefs.fields.code).asc());
const orderedCompanies = company
  .collect()
  .distinct()
  .orderBy(company.field(references(QueryCompany).fields.name).asc());

const positionalCollected: Query<readonly [
  QueryPerson,
  readonly QueryEmployment[],
  readonly QueryCompany[],
]> = databaseSession
  .query(person, orderedEmployments, orderedCompanies)
  .where(
    employment.role(employmentRefs.roles.employee).connects(person),
    employment.role(employmentRefs.roles.employer).connects(company),
  );
const positionalPage: Page<readonly [
  QueryPerson,
  readonly QueryEmployment[],
  readonly QueryCompany[],
]> = positionalCollected.pageBy(person, {
  limit: 10,
  includeTotal: true,
});
void positionalPage;

type PersonWork = Readonly<{
  person: QueryPerson;
  employments: readonly QueryEmployment[];
  companies: readonly QueryCompany[];
}>;
const namedCollected: Query<readonly [PersonWork]> = databaseSession
  .queryNamed({
    person,
    employments: orderedEmployments,
    companies: orderedCompanies,
  })
  .where(
    employment.role(employmentRefs.roles.employee).connects(person),
    employment.role(employmentRefs.roles.employer).connects(company),
  );
const namedPage: Page<PersonWork> = namedCollected.pageBy(person, {
  limit: 10,
  includeTotal: true,
});
const namedPageTotal: bigint | undefined = namedPage.total;
const namedPagePerson: QueryPerson = namedPage.items[0]!.person;
const namedPageEmployments: readonly QueryEmployment[] =
  namedPage.items[0]!.employments;
const namedCollectedCount: bigint = namedCollected.countBy(person);
const namedCollectedExists: boolean = namedCollected.existsBy(person);
void namedPageTotal;
void namedPagePerson;
void namedPageEmployments;
void namedCollectedCount;
void namedCollectedExists;

const namedSingular: Query<readonly [Readonly<{
  person: QueryPerson;
  company: QueryCompany;
}>]> = databaseSession.queryNamed({ person, company });
const namedSingularOne: Readonly<{
  person: QueryPerson;
  company: QueryCompany;
}> = namedSingular.one();
const namedSingularRows: readonly Readonly<{
  person: QueryPerson;
  company: QueryCompany;
}>[] = namedSingular.rows({ limit: 10 });
const namedPersonCount: bigint = namedSingular.countBy(person);
const namedCompanyExists: boolean = namedSingular.existsBy(company);
void namedSingularOne;
void namedSingularRows;
void namedPersonCount;
void namedCompanyExists;

const protoNamed = databaseSession.queryNamed({ ["__proto__"]: person });
const protoNamedRow: Readonly<{ __proto__: QueryPerson }> = protoNamed.one();
void protoNamedRow;

const repeatedNamed = databaseSession.queryNamed({
  first: person,
  second: secondPerson,
});
const repeatedNamedRow: Readonly<{
  first: QueryPerson;
  second: QueryPerson;
}> = repeatedNamed.one();
void repeatedNamedRow;

const singleNamed = databaseSession.queryNamed({ person });
const singleNamedRow: Readonly<{ person: QueryPerson }> = singleNamed.one();
const singleNamedPage: Page<Readonly<{ person: QueryPerson }>> = singleNamed.pageBy(
  person,
  { limit: 10 },
);
void singleNamedRow;
void singleNamedPage;

const p01 = databaseSession.var(QueryPerson);
const p02 = databaseSession.var(QueryPerson);
const p03 = databaseSession.var(QueryPerson);
const p04 = databaseSession.var(QueryPerson);
const p05 = databaseSession.var(QueryPerson);
const p06 = databaseSession.var(QueryPerson);
const p07 = databaseSession.var(QueryPerson);
const p08 = databaseSession.var(QueryPerson);
const p09 = databaseSession.var(QueryPerson);
const p10 = databaseSession.var(QueryPerson);
const p11 = databaseSession.var(QueryPerson);
const p12 = databaseSession.var(QueryPerson);
const p13 = databaseSession.var(QueryPerson);
const p14 = databaseSession.var(QueryPerson);
const p15 = databaseSession.var(QueryPerson);
const p16 = databaseSession.var(QueryPerson);
const p17 = databaseSession.var(QueryPerson);

const sixteen: Query<readonly [
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
]> = databaseSession.query(
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
void sixteen;

type PairRow = QueryRow<readonly [QueryPerson, QueryCompany]>;
const mappedPair: PairRow = pairResult;
void mappedPair;

// @ts-expect-error Query itself cannot be instantiated with an empty tuple
type EmptyQuery = Query<readonly []>;
void (null as unknown as EmptyQuery);

// @ts-expect-error Query itself cannot be widened to an arbitrary readonly array
type WidenedQuery = Query<readonly QueryPerson[]>;
void (null as unknown as WidenedQuery);

// @ts-expect-error query() requires at least one selected output
databaseSession.query();

const seventeenSelections = [
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
  p17,
] as const;
// @ts-expect-error the canonical public selection cap is sixteen
databaseSession.query(...seventeenSelections);

// @ts-expect-error the Query generic itself enforces the sixteen-slot cap
type SeventeenSlotQuery = Query<readonly [
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
  QueryPerson,
]>;
void (null as unknown as SeventeenSlotQuery);

// @ts-expect-error PageBy is executable only for one singular root slot in #174
pair.pageBy(person, { limit: 10 });
// @ts-expect-error a root must be one of this query's selected model types
one.countBy(company);
// @ts-expect-error a sibling model is nominally distinct despite the same schema shape
one.existsBy(sibling);

// @ts-expect-error collection-bearing rows are page-only
positionalCollected.one();
// @ts-expect-error collection-bearing rows are page-only
positionalCollected.rows({ limit: 10 });
// @ts-expect-error named collection-bearing rows are page-only
namedCollected.one();
// @ts-expect-error named collection-bearing rows are page-only
namedCollected.rows({ limit: 10 });
// @ts-expect-error a collected binding is not a count root
namedCollected.countBy(employment);
// @ts-expect-error two singular named members cannot form a page in #175
namedSingular.pageBy(person, { limit: 10 });
// @ts-expect-error repeated same-model singular members are still two page roots
repeatedNamed.pageBy(person, { limit: 10 });
// @ts-expect-error exact named rows do not grow undeclared keys
namedSingularOne.missing;
// @ts-expect-error page items are readonly
namedPage.items.push(namedPage.items[0]!);
// @ts-expect-error named rows are readonly
namedPage.items[0]!.person = person as never;
// @ts-expect-error collected arrays are readonly
namedPage.items[0]!.companies.push(null as never);

// @ts-expect-error named output must contain at least one selection
databaseSession.queryNamed({});
// @ts-expect-error output names must be non-empty
databaseSession.queryNamed({ "": person });
// @ts-expect-error every exact named member must be a Selection
databaseSession.queryNamed({ person, invalid: 1 });

const widenedNamedSelections: Readonly<Record<string, typeof person>> = { person };
// @ts-expect-error queryNamed rejects a widened Record key set
databaseSession.queryNamed(widenedNamedSelections);

const seventeenNamedSelections = {
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
  p17,
} as const;
// @ts-expect-error named output uses the same sixteen-selection cap
databaseSession.queryNamed(seventeenNamedSelections);

// @ts-expect-error registerModels requires at least one concrete constructor
databaseSession.registerModels();
// @ts-expect-error model instances are not constructor metadata
databaseSession.registerModels(new QueryPerson({ name: new QueryName("Alice") }));
