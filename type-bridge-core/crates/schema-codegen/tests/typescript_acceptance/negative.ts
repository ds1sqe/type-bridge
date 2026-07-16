import {
  Container,
  Employment,
  Event,
  Identifier,
  Membership,
  Person,
  Score,
} from "./generated_v2/src/index.js";

const identifier = Identifier.create("person-1");
const personReference = Person.reference("person-iid", { identifier });
const eventReference = Event.reference("event-iid", {});
declare const exactPersonManager: ReturnType<typeof Person.manager>;

// @ts-expect-error subject is required
Event.create({});
// @ts-expect-error employee is required
Employment.create({});
// @ts-expect-error a reference facet is not a complete player
Employment.create({ employee: personReference });
// @ts-expect-error the specialized-away parent role is not a create member
Employment.create({ employee: Person.create({ identifier }), member: Person.create({ identifier }) });
// @ts-expect-error owner and role brands differ
const wrongOwner: typeof Employment.employee = Membership.member;
// @ts-expect-error a sequence role cannot receive a scalar reference
Container.create({ item: eventReference });
// @ts-expect-error owns values use the projected attribute model
Person.create({ identifier: 7 });
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
exactPersonManager.insert(Event.create({ subject: Person.create({ identifier }) }));
