import {
  Container,
  Employment,
  Event,
  Identifier,
  Person,
  QuerySession,
  Score,
  type Page,
  type Query,
} from "./generated_v2/src/index.js";
import {
  RustDatabase,
  type RustTransactionContext,
} from "@type-bridge/node";

const db = RustDatabase.connect("localhost:1729", "typed_query_example");
const session = new QuerySession(db);
const person = session.exact(Person);
const event = session.exact(Event);
const container = session.exact(Container);
const employment = session.exact(Employment);

const employee = employment.role(Employment.employee).connects(person);
const subject = event.role(Event.subject).connects(person);
const contained = container.role(Container.item).connects(event);

const onePerson: Query<Person> = session.query(person);
const scalar: Person = onePerson.one();
const scalarRows: readonly Person[] = onePerson.rows({ limit: 20n });

const twoSlots: Query<readonly [Person, Event]> = session
  .query(person, event)
  .where(subject);
const pairRows: readonly (readonly [Person, Event])[] = twoSlots.rows({
  limit: 20n,
});

const colleague = session.exact(Person);
const otherEvent = session.exact(Event);
const fiveSlots: Query<
  readonly [Person, Event, Container, Event, Person]
> = session
  .query(person, event, container, otherEvent, colleague)
  .where(
    subject,
    contained,
    otherEvent.role(Event.subject).connects(colleague),
    container.role(Container.item).connects(otherEvent),
  );

const personPair: Query<readonly [Person, Person]> = session
  .query(person, colleague)
  .match(event, otherEvent, container)
  .where(
    subject,
    contained,
    otherEvent.role(Event.subject).connects(colleague),
    container.role(Container.item).connects(otherEvent),
  );

void scalar;
void scalarRows;
void pairRows;
void fiveSlots;
void personPair;
// Generated query() accepts 1..16 selections. Seventeen is a tsc and Rust
// diagnostic.

const adults = onePerson.where(
  person.field(Person.score).gte(Score.create(18n)),
  person.field(Person.identifier).startsWith(Identifier.create("Al")),
  person.field(Person.identifier).contains(Identifier.create("Research")),
  person.field(Person.identifier).endsWith(Identifier.create("Labs")),
  person.field(Person.identifier).regex(Identifier.create(String.raw`^A[[:alpha:]]+$`)),
);

const literalPercent = onePerson.where(
  person.field(Person.identifier).contains(Identifier.create("50%")),
);

const independentPairs: Query<readonly [Person, Container]> = session
  .query(person, container)
  .allowCrossJoin(person, container);

void adults;
void literalPercent;
void independentPairs;

const orderedPeople: readonly Person[] = onePerson.rows({
  limit: 50n,
  offset: 0n,
  orderBy: [person.field(Person.identifier).asc()],
});

const personCount: bigint = twoSlots.countBy(person);
const anyPerson: boolean = twoSlots.existsBy(person);

const work: Query<Readonly<{
  person: Person;
  events: readonly Event[];
}>> = session.queryNamed({
  person,
  events: event.collect().distinct(),
}).where(subject);

const page: Page<Readonly<{
  person: Person;
  events: readonly Event[];
}>> = work.pageBy(person, {
  limit: 50n,
  offset: 0n,
  orderBy: [person.field(Person.identifier).asc()],
  includeTotal: true,
});

void orderedPeople;
void personCount;
void anyPerson;
void page;

// Owned: rows() opens and closes one read transaction on every exit path.
const ownedSession = new QuerySession(db);
const ownedPerson = ownedSession.exact(Person);
const ownedRows = ownedSession.query(ownedPerson).rows({ limit: 10n });

// Borrowed: only the caller closes the context, which remains reusable.
const tx: RustTransactionContext = db.transaction("read");
try {
  const borrowed = new QuerySession(tx);
  const borrowedPerson = borrowed.exact(Person);
  const firstPage = borrowed.query(borrowedPerson).rows({ limit: 10n });
  const secondPage = borrowed.query(borrowedPerson).rows({
    limit: 10n,
    offset: 10n,
  });
  void firstPage;
  void secondPage;
} finally {
  tx.close();
}

void employee;
void ownedRows;
