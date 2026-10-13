import {
  Actor,
  Aliases,
  Container,
  Counter,
  CounterValue,
  Employment,
  Event,
  Identifier,
  Interaction,
  Membership,
  Nickname,
  Party,
  Person,
  PlainActivity,
  PlayerStats,
  Robot,
  RobotId,
  Score,
  ValBool,
  ValConstrained,
  ValDate,
  ValDatetime,
  ValDatetimeTz,
  ValDecimal,
  ValDouble,
  ValDuration,
  aggregate,
  findEvents,
  integerInput,
  qualifyingScore,
  type EventRef,
  type FunctionToken,
  type IntegerCall,
  type IntegerInput,
  type Page,
  type Predicate,
  type ProjectedModelManager,
  type PersonRef,
  type Query,
  QuerySession,
  ExchangeConnection,
  RemoteQuerySession,
  type RemoteQuery,
  type RemoteQueryExchange,
  type RemoteQueryLimits,
  type ProxyConnection,
  type QueryConnection,
  QueryExecutionResourceLimits,
  connectProxy,
} from "./generated_v2/src/index.js";
import type { RustDatabase } from "@type-bridge/node";

const identifier = Identifier.create("person-1");
const identifierBytes: Uint8Array = Identifier.encodeAttribute(identifier);
const decodedIdentifier: Identifier = Identifier.decodeAttribute(identifierBytes);
const score = Score.create(3n);
const person: Person = Person.create({
  identifier,
  aliases: [Aliases.create("first"), Aliases.create("second")],
  score,
  valBool: ValBool.create(true),
  valConstrained: ValConstrained.create(20n),
  valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
  valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
  valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
  valDecimal: ValDecimal.create("3.5"),
  valDouble: ValDouble.create(3.5),
  valDuration: ValDuration.create("PT3S"),
});
const event: Event = Event.create({ subject: person });
const counter: Counter = Counter.create({ counterValue: CounterValue.create(1n) });
const robot: Robot = Robot.create({
  nickname: Nickname.create("actor-robot"),
  robotId: RobotId.create(1n),
  valConstrained: ValConstrained.create(20n),
});
const interaction: Interaction = Interaction.create({
  actor: robot,
  identifier: Identifier.create("interaction-1"),
  target: person,
});
const plainActivity: PlainActivity = PlainActivity.create({ participant: person });
const reference: PersonRef = Person.reference("person-iid", { identifier });
const eventReference: EventRef = Event.reference("event-iid", {});

Employment.create({ employee: person });
Container.create({ item: [eventReference] });
Membership.create({
  member: Robot.create({
    robotId: RobotId.create(1n),
    valConstrained: ValConstrained.create(20n),
  }),
});

const stats: PlayerStats = PlayerStats({ wins: 3n, nickname: null });
const identifierValue: string = identifier.value;
const scoreValue: bigint = score.value;
const completeIid: string | null = person.iid;
declare const database: RustDatabase;
const personManager: ProjectedModelManager<Person> = Person.manager(database);
personManager.insert(person);
personManager.put(person);
const insertedPeople: readonly Person[] = personManager.insertMany([person]);
const putPeople: readonly Person[] = personManager.putMany([person]);
personManager.update("0x1", person);
personManager.delete(person);
personManager.delete("0x1");
const filteredPersonManager = personManager.filter({ score__gte: Score.create(3n) });
const inPersonManager: ProjectedModelManager<Person> = personManager.filter({
  score__in: [Score.create(3n), Score.create(4n)],
});
const missingPersonManager: ProjectedModelManager<Person> = personManager.filter({
  aliases__isnull: true,
});
const iidPersonManager: ProjectedModelManager<Person> = personManager.filter({
  iid__in: ["0x1", "0x2"],
});
const filteredPeople: readonly Person[] = filteredPersonManager.all();
const firstFilteredPerson: Person | null = filteredPersonManager.first();
const filteredPersonCount: bigint = filteredPersonManager.count();
const filteredPersonExists: boolean = filteredPersonManager.exists();
const functionToken: FunctionToken<
  "find-events",
  readonly [event: Event],
  AsyncIterable<Event>
> = findEvents;
const querySession = new QuerySession(database);
const personVar = querySession.exact(Person);
const minimum: IntegerInput = integerInput(querySession, Score.create(2n));
const qualifying: IntegerCall = qualifyingScore(querySession, personVar, minimum);
const qualifyingPredicate: Predicate = qualifying.gteField(personVar.field(Person.score));
const nestedQualifying: IntegerCall = qualifyingScore(querySession, personVar, qualifying);
const nestedPredicate: Predicate = qualifying.gteCall(nestedQualifying);
const eventVar = querySession.var(Event);
const employmentVar = querySession.var(Employment);
const partyVar = querySession.subtypes(Party);
const actorVar = querySession.subtypes(Actor);
const interactionVar = querySession.exact(Interaction);
const identifierPredicate = personVar.field(Person.identifier).eq(identifier);
const presentPredicate: Predicate = personVar.field(Person.score).isPresent();
const missingPredicate: Predicate = personVar.field(Person.score).isMissing();
const iidPredicate: Predicate = personVar.iid("0x1");
const iidSetPredicate: Predicate = personVar.iidIn(["0x1", "0x2"]);
const employeePredicate = employmentVar.role(Employment.employee).connects(personVar);
const actorPredicate = interactionVar.role(Interaction.actor).connects(actorVar);
const actorInteractionQuery: Query<Interaction> = querySession
  .query(interactionVar)
  .match(actorVar)
  .where(
    actorPredicate,
    actorVar.field(Actor.nickname).contains(Nickname.create("actor")),
  );
const personQuery: Query<Person> = querySession
  .query(personVar)
  .where(identifierPredicate, qualifyingPredicate, nestedPredicate);
const clonedPersonQuery: Query<Person> = personQuery.clone();
const personQueryClosed: boolean = personQuery.isClosed;
personQuery.close();
const querySessionClosed: boolean = querySession.isClosed;
querySession.close();
const tupleQuery: Query<readonly [Person, Event]> = querySession.query(personVar, eventVar);
const collectedQuery: Query<readonly Person[]> = querySession.query(
  personVar.collect().distinct(),
);
const namedQuery: Query<Readonly<{ person: Person; events: readonly Event[] }>> =
  querySession.queryNamed({
    person: personVar,
    events: eventVar.collect(),
  });
const sixteenQuery = querySession.query(
  personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar,
  personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar,
);
const personOne: Person = await personQuery.one();
const personFirst: Person | null = await personQuery.first();
const personRows: readonly Person[] = await personQuery.rows({ limit: 10n });
const partyRows: readonly Party[] = await querySession.query(partyVar).rows({ limit: 10n });
const personPage: Page<Person> = await personQuery.pageBy(personVar, { limit: 10n });
const personCount: bigint = await personQuery.countBy(personVar);
const personExists: boolean = await personQuery.existsBy(personVar);
const scoreField = personVar.field(Person.score);
const boolField = personVar.field(Person.valBool);
const personAggregate: readonly [bigint, bigint, number | null] = await personQuery.aggregate(
  personVar,
  [aggregate.count(), aggregate.sum(scoreField), aggregate.mean(scoreField)] as const,
);
const groupedAggregate: readonly (
  readonly [Employment, readonly [bigint, bigint | null]]
)[] = await querySession
  .query(personVar, employmentVar)
  .where(employeePredicate)
  .groupBy(personVar, employmentVar)
  .aggregate([aggregate.count(), aggregate.max(scoreField)] as const);
const fieldGroupedAggregate: readonly (
  readonly [ValBool, readonly [bigint, bigint]]
)[] = await personQuery
  .groupBy(personVar, boolField)
  .aggregate([aggregate.count(), aggregate.sum(scoreField)] as const);
const tupleFieldGroupedAggregate: readonly (
  readonly [readonly [ValBool, Score], readonly [bigint]]
)[] = await personQuery
  .groupBy(personVar, boolField, scoreField)
  .aggregate([aggregate.count()] as const);
declare const advertisement: Uint8Array;
declare const exchange: (
  request: Uint8Array,
  signal?: AbortSignal,
) => Promise<Uint8Array>;
const exchangeConnection = new ExchangeConnection(advertisement, exchange, {
  answerLimits: new QueryExecutionResourceLimits({ items: 100n }),
});
const exchangeAdvertisement: Uint8Array = exchangeConnection.advertisement;
const proxy: ProxyConnection = await connectProxy("http://127.0.0.1:8080", {
  answerLimits: new QueryExecutionResourceLimits({ items: 100n }),
});
const connections: readonly QueryConnection[] = [database, proxy, exchangeConnection];
const proxySession = new QuerySession(proxy);
const proxyRows: readonly Person[] = await proxySession
  .query(proxySession.exact(Person))
  .rows({ limit: 10n });
const remoteSession = new QuerySession(exchangeConnection, {
  resources: new QueryExecutionResourceLimits({ items: 100n }),
});
// The 2.2.4 guide's remote example still compiles against the deprecated
// aliases.
const advertisementBytes: Uint8Array = advertisement;
const legacyExchange: RemoteQueryExchange = exchange;
const legacyLimits: RemoteQueryLimits = {
  maxItems: 100n,
  maxBytes: 8_388_608n,
  maxCollectionMembers: 1_000n,
  maxGraphNodes: 1_000n,
  maxAttributeValues: 1_000n,
  maxRolePlayers: 1_000n,
  deadlineMs: 30_000n,
};
const remote = new RemoteQuerySession(
  advertisementBytes,
  legacyExchange,
  legacyLimits,
);
const legacyPerson = remote.exact(Person);
const legacyQuery: RemoteQuery<Person> = remote.query(legacyPerson);
const legacyRows: readonly Person[] = await legacyQuery.rows({ limit: 50n });
const legacySession: QuerySession = remote;
const remotePersonVar = remoteSession.var(Person);
const remotePartyVar = remoteSession.subtypes(Party);
const remoteMinimum: IntegerInput = integerInput(remoteSession, Score.create(2n));
const remoteQualifying: IntegerCall = qualifyingScore(
  remoteSession,
  remotePersonVar,
  remoteMinimum,
);
const remoteNestedQualifying: IntegerCall = qualifyingScore(
  remoteSession,
  remotePersonVar,
  remoteQualifying,
);
const remotePersonQuery: Query<Person> = remoteSession.query(remotePersonVar);
const clonedRemotePersonQuery: Query<Person> = remotePersonQuery.clone();
const remotePersonQueryClosed: boolean = remotePersonQuery.isClosed;
remotePersonQuery.close();
const remoteSessionClosed: boolean = remoteSession.isClosed;
remoteSession.close();
const remoteNamedQuery: Query<Readonly<{ person: Person }>> =
  remoteSession.queryNamed({ person: remotePersonVar });
const remoteOne: Promise<Person> = remotePersonQuery.one();
const remoteRows: Promise<readonly Person[]> = remotePersonQuery.rows({ limit: 10n });
const remotePartyRows: Promise<readonly Party[]> = remoteSession
  .query(remotePartyVar)
  .rows({ limit: 10n });
const remotePage: Promise<Page<Person>> = remotePersonQuery.pageBy(remotePersonVar, {
  limit: 10n,
});
const remoteCount: Promise<bigint> = remotePersonQuery.countBy(remotePersonVar);
const remoteExists: Promise<boolean> = remotePersonQuery.existsBy(remotePersonVar);
const remoteFirst: Promise<Person | null> = remotePersonQuery.first();
const remoteScoreField = remotePersonVar.field(Person.score);
const remoteAggregate: Promise<readonly [bigint, bigint]> = remotePersonQuery.aggregate(
  remotePersonVar,
  [aggregate.count(), aggregate.sum(remoteScoreField)] as const,
);
const remoteGroupedAggregate: Promise<readonly (
  readonly [Person, readonly [number | null]]
)[]> = remotePersonQuery
  .groupBy(remotePersonVar, remotePersonVar)
  .aggregate([aggregate.mean(remoteScoreField)] as const);

void reference;
void event;
void counter;
void interaction;
void plainActivity;
void stats;
void identifierValue;
void scoreValue;
void completeIid;
void personManager;
void functionToken;
void filteredPeople;
void employeePredicate;
void actorInteractionQuery;
void tupleQuery;
void collectedQuery;
void namedQuery;
void sixteenQuery;
void exchangeAdvertisement;
void connections;
void proxyRows;
void personOne;
void clonedPersonQuery;
void personQueryClosed;
void querySessionClosed;
void personFirst;
void personRows;
void partyRows;
void personPage;
void personCount;
void personExists;
void personAggregate;
void groupedAggregate;
void remoteNamedQuery;
void clonedRemotePersonQuery;
void remotePersonQueryClosed;
void remoteSessionClosed;
void remoteOne;
void remoteRows;
void remotePartyRows;
void remotePage;
void remoteCount;
void remoteExists;
void remoteFirst;
void remoteAggregate;
void remoteGroupedAggregate;
void Employment.employee;
void Event.subject;
void Person.identifier;
void legacyRows;
void legacySession;
