import {
  Entity,
  Key,
  Relation,
  RustDatabase,
  attr,
  field,
  role,
  type RustTransactionContext,
} from "@type-bridge/node";
import {
  QuerySession,
  references,
  type Page,
  type Query,
} from "@type-bridge/node/typed";

class Name extends attr.String("name") {}
class Age extends attr.Integer("age") {}
class Industry extends attr.String("industry") {}
class Position extends attr.String("position") {}

class Person extends Entity("person", {
  name: field(Name, Key),
  age: field(Age),
}) {}

class Company extends Entity("company", {
  name: field(Name, Key),
  industry: field(Industry),
}) {}

class Employment extends Relation("employment", {
  employee: role(Person),
  employer: role(Company),
  position: field(Position),
}) {}

const db = RustDatabase.connect("localhost:1729", "typed_query_example");
const session = new QuerySession(db);
const person = session.var(Person);
const employment = session.var(Employment);
const company = session.var(Company);
const personRefs = references(Person);
const companyRefs = references(Company);
const employmentRefs = references(Employment);

const onePerson: Query<readonly [Person]> = session.query(person);
const scalar: Person = onePerson.one();
const scalarRows: readonly Person[] = onePerson.rows({ limit: 20 });

const twoSlots: Query<readonly [Person, Company]> = session
  .query(person, company)
  .match(employment)
  .where(
    employment.role(employmentRefs.roles.employee).is(person),
    employment.role(employmentRefs.roles.employer).is(company),
  );
const pairRows: readonly (readonly [Person, Company])[] = twoSlots.rows({
  limit: 20,
});

const colleague = session.var(Person);
const otherEmployment = session.var(Employment);
const fiveSlots: Query<
  readonly [Person, Employment, Company, Employment, Person]
> = session
  .query(person, employment, company, otherEmployment, colleague)
  .where(
    employment.role(employmentRefs.roles.employee).is(person),
    employment.role(employmentRefs.roles.employer).is(company),
    otherEmployment.role(employmentRefs.roles.employee).is(colleague),
    otherEmployment.role(employmentRefs.roles.employer).is(company),
  );

const personPair: Query<readonly [Person, Person]> = session
  .query(person, colleague)
  .match(employment, otherEmployment, company)
  .where(
    employment.role(employmentRefs.roles.employee).is(person),
    employment.role(employmentRefs.roles.employer).is(company),
    otherEmployment.role(employmentRefs.roles.employee).is(colleague),
    otherEmployment.role(employmentRefs.roles.employer).is(company),
  );

void scalar;
void scalarRows;
void pairRows;
void fiveSlots;
void personPair;
// query() accepts 1..16 selections. Seventeen is a tsc and Rust diagnostic.

const adultsInAi = twoSlots.where(
  person.field(personRefs.fields.age).gte(new Age(18n)),
  company.field(companyRefs.fields.industry).eq(new Industry("AI")),
  person.field(personRefs.fields.name).startsWith("Al"),
  company.field(companyRefs.fields.name).contains("Research"),
  company.field(companyRefs.fields.name).endsWith("Labs"),
  person.field(personRefs.fields.name).regex(String.raw`^A[[:alpha:]]+$`),
);

const literalPercent = onePerson.where(
  person.field(personRefs.fields.name).contains("50%"),
);

const independentPairs: Query<readonly [Person, Company]> = session
  .query(person, company)
  .allowCrossJoin(person, company);

void adultsInAi;
void literalPercent;
void independentPairs;

const orderedPeople: readonly Person[] = onePerson.rows({
  limit: 50,
  offset: 0,
  orderBy: [person.field(personRefs.fields.name).asc()],
});

const personCount: bigint = twoSlots.countBy(person);
const anyPerson: boolean = twoSlots.existsBy(person);

const work: Query<readonly [Readonly<{
  person: Person;
  employments: readonly Employment[];
  companies: readonly Company[];
}>]> = session.queryNamed({
  person,
  employments: employment.collect(),
  companies: company.collect().distinct(),
}).where(
  employment.role(employmentRefs.roles.employee).is(person),
  employment.role(employmentRefs.roles.employer).is(company),
);

const page: Page<Readonly<{
  person: Person;
  employments: readonly Employment[];
  companies: readonly Company[];
}>> = work.pageBy(person, {
  limit: 50,
  offset: 0,
  orderBy: [person.field(personRefs.fields.name).asc()],
  includeTotal: true,
});

void orderedPeople;
void personCount;
void anyPerson;
void page;

// Owned: rows() opens and closes one read transaction on every exit path.
const ownedSession = new QuerySession(db);
const ownedPerson = ownedSession.var(Person);
const ownedRows = ownedSession.query(ownedPerson).rows({ limit: 10 });

// Borrowed: only the caller closes the context, which remains reusable.
const tx: RustTransactionContext = db.transaction("read");
try {
  const borrowed = new QuerySession(tx);
  const borrowedPerson = borrowed.var(Person);
  const firstPage = borrowed.query(borrowedPerson).rows({ limit: 10 });
  const secondPage = borrowed.query(borrowedPerson).rows({
    limit: 10,
    offset: 10,
  });
  void firstPage;
  void secondPage;
} finally {
  tx.close();
}

void ownedRows;
