import { Iid } from "@type-bridge/node";
import {
  Actor,
  Container,
  Employment,
  Event,
  FunctionCall,
  Identifier,
  Membership,
  Person,
  Robot,
  Score,
  ValBool,
  ValConstrained,
  ValDate,
  ValDatetime,
  ValDatetimeTz,
  ValDecimal,
  ValDouble,
  ValDuration,
  ExchangeConnection,
  QuerySession,
  aggregate,
  qualifyingScore,
} from "./generated_v2/src/index.js";
import { Person as ForeignPerson } from "./generated_foreign/src/index.js";
import { type StagedTransaction, Plan, type Database } from "@type-bridge/node";

const identifier = Identifier.create("person-1");
const person = (value: Identifier): Person => Person.create({
  identifier: value,
  score: Score.create(3n),
  valBool: ValBool.create(true),
  valConstrained: ValConstrained.create(20n),
  valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
  valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
  valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
  valDecimal: ValDecimal.create("3.5"),
  valDouble: ValDouble.create(3.5),
  valDuration: ValDuration.create("PT3S"),
});
const personReference = Person.reference(new Iid("0xa1"), { identifier });
const eventReference = Event.reference(new Iid("0xe1"), {});
declare const exactPersonManager: ReturnType<typeof Person.manager>;
declare const maybeIdentifier: string | undefined;
declare const database: Database;
const querySession = new QuerySession(database);
const personVar = querySession.var(Person);
const eventVar = querySession.var(Event);
const employmentVar = querySession.var(Employment);
const exactActorVar = querySession.exact(Actor);
const exactRobotVar = querySession.exact(Robot);
const subtypeRobotVar = querySession.subtypes(Robot);
declare const booleanCall: FunctionCall<boolean>;

// @ts-expect-error subject is required
Event.create({});
// @ts-expect-error employee is required
Employment.create({});
// @ts-expect-error a reference facet is not a complete player
Employment.create({ employee: personReference });
// @ts-expect-error the specialized-away parent role is not a create member
Employment.create({ employee: person(identifier), member: person(identifier) });
// @ts-expect-error owner and role brands differ
const wrongOwner: typeof Employment.employee = Membership.member;
// @ts-expect-error a sequence role cannot receive a scalar reference
Container.create({ item: eventReference });
// @ts-expect-error owns values use the projected attribute model
person(7);
// @ts-expect-error string attributes take their canonical scalar directly
Identifier.create({});
// @ts-expect-error long attributes require bigint rather than number
Score.create(7);
// @ts-expect-error complete attribute values are readonly
identifier.value = "replacement";
// @ts-expect-error complete IIDs are readonly
identifier.iid = "replacement-iid";
// @ts-expect-error projected managers reject structurally forged connections
Person.manager({});
// @ts-expect-error an exact person manager cannot insert an event
exactPersonManager.insert(Event.create({ subject: person(identifier) }));
// @ts-expect-error undefined filter values would be omitted by JSON serialization
exactPersonManager.filter({ identifier: maybeIdentifier });
// @ts-expect-error field tokens retain their exact generated owner
eventVar.field(Person.identifier);
// @ts-expect-error comparisons retain the exact generated attribute wrapper
personVar.field(Person.identifier).eq(Score.create(3n));
// @ts-expect-error prior function calls retain their exact scalar domain
qualifyingScore(querySession, personVar, booleanCall);
// @ts-expect-error generated relation roles retain their accepted player union
employmentVar.role(Employment.employee).links(eventVar);
// @ts-expect-error an exact abstract ancestor is not itself an accepted player
eventVar.role(Event.subject).links(exactActorVar);
// @ts-expect-error an unrelated concrete subtype remains rejected
eventVar.role(Event.subject).links(exactRobotVar);
// @ts-expect-error a subtype root whose closure has no accepted player remains rejected
eventVar.role(Event.subject).links(subtypeRobotVar);
// @ts-expect-error generated queries require at least one selection
querySession.query();
// @ts-expect-error generated named queries require at least one selection
querySession.queryNamed({});
// @ts-expect-error generated query tokens are nominal to one emitted package
querySession.var(ForeignPerson);
// @ts-expect-error generated field tokens are nominal to one emitted package
personVar.field(ForeignPerson.identifier);
// @ts-expect-error generated positional query typing is capped at sixteen selections
querySession.query(personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar, personVar);
// @ts-expect-error aggregate fields must carry a generated long or double value
aggregate.mean(personVar.field(Person.identifier));
// @ts-expect-error generated aggregate terms are non-empty
querySession.query(personVar).aggregate(personVar, [] as const);
// @ts-expect-error generated aggregate typing is capped at sixteen terms
querySession.query(personVar).aggregate(personVar, [
  aggregate.count(), aggregate.count(), aggregate.count(), aggregate.count(),
  aggregate.count(), aggregate.count(), aggregate.count(), aggregate.count(),
  aggregate.count(), aggregate.count(), aggregate.count(), aggregate.count(),
  aggregate.count(), aggregate.count(), aggregate.count(), aggregate.count(),
  aggregate.count(),
] as const);
// @ts-expect-error terminals return promises on every connection kind
const synchronousCount: bigint = querySession.query(personVar).countBy(personVar);
// @ts-expect-error QuerySession requires a QueryConnection
new QuerySession({ advertisement: new Uint8Array() });
// @ts-expect-error ExchangeConnection answer limits are QueryExecutionResourceLimits
new ExchangeConnection(new Uint8Array(), async () => new Uint8Array(), { answerLimits: {} });
const negativePlan = Plan.builder();
const negativePerson = negativePlan.exact(Person);
// @ts-expect-error a plan field belongs to the thing's own model
negativePlan.field(negativePerson, Robot.robotId);
// @ts-expect-error plan literals are typed: a number is not a long
negativePlan.field(negativePerson, Person.score).gt(1);

declare const stagedTransaction: StagedTransaction;
const stagedSession = new QuerySession(stagedTransaction);
const stagedRoot = stagedSession.exact(Person);
const stagedPerson = await stagedSession.query(stagedRoot).one();
// @ts-expect-error a pending model is not a committed model
const pendingAsModel: Person = stagedPerson;
// @ts-expect-error a pending answer is not a scalar field input
stagedRoot.field(Person.score).eq(stagedPerson);

// @ts-expect-error IID reference arguments are nominal.
Person.reference("0xa1", { identifier });
// @ts-expect-error IID predicates reject strings.
personVar.iid("0xa1");
// @ts-expect-error IID lists reject string members.
personVar.iidIn(["0xa1"]);
// @ts-expect-error Manager IID keywords reject strings.
exactPersonManager.filter({ iid: "0xa1" });
// @ts-expect-error Manager IID lists reject string members.
exactPersonManager.filter({ iid__in: ["0xa1"] });
// @ts-expect-error By-IID reads reject strings.
exactPersonManager.getByIid("0xa1");

// @ts-expect-error Model IIDs are nominal values, not strings.
const plainReferenceIid: string | null = personReference.iid;
const iidBuilder = Plan.builder();
const textColumn = iidBuilder.input("target", "string");
const iidTarget = iidBuilder.exact(Person);
// @ts-expect-error IID input predicates require nominal IID columns.
iidBuilder.byIidInput(iidTarget, textColumn);
// @ts-expect-error Authored by-IID helpers reject strings.
Plan.build(() => Person.byIid("0xa1"));
