import {
  Aliases,
  Container,
  Employment,
  Event,
  Identifier,
  Membership,
  Person,
  PlayerStats,
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
  findEvents,
  type EventRef,
  type FunctionToken,
  type ProjectedModelManager,
  type PersonRef,
} from "./generated_v2/src/index.js";
import type { RustDatabase } from "@type-bridge/node";

const identifier = Identifier.create("person-1");
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
const reference: PersonRef = Person.reference("person-iid", { identifier });
const eventReference: EventRef = Event.reference("event-iid", {});

Employment.create({ employee: person });
Container.create({ item: [eventReference] });
Membership.create({ member: Robot.create({ valConstrained: ValConstrained.create(20n) }) });

const stats: PlayerStats = PlayerStats({ wins: 3n, nickname: null });
const identifierValue: string = identifier.value;
const scoreValue: bigint = score.value;
const completeIid: string | null = person.iid;
declare const database: RustDatabase;
const personManager: ProjectedModelManager<Person> = Person.manager(database);
personManager.insert(person);
const functionToken: FunctionToken<
  "find-events",
  readonly [event: Event],
  AsyncIterable<Event>
> = findEvents;

void reference;
void event;
void stats;
void identifierValue;
void scoreValue;
void completeIid;
void personManager;
void functionToken;
void Employment.employee;
void Event.subject;
void Person.identifier;
