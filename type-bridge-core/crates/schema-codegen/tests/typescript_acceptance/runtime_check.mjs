import assert from "node:assert/strict";
import { installRuntimeProjection } from "@type-bridge/node/runtime-projection";

import {
  Aliases,
  Container,
  Employment,
  Event,
  Identifier,
  PLAYING_FACTS,
  PROJECTION_FINGERPRINT_JSON,
  Person,
  PlayerStats,
  RUNTIME_PROJECTION_JSON,
  SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  Score,
  ValBool,
  ValConstrained,
  ValDate,
  ValDatetime,
  ValDatetimeTz,
  ValDecimal,
  ValDouble,
  ValDuration,
} from "./generated_v2/dist/index.js";

const identifier = Identifier.create("person-1");
assert.equal(identifier.value, "person-1");
assert.equal(identifier.iid, null);
const score = Score.create(3n);
assert.equal(score.value, 3n);
assert.equal(score.iid, null);
const personValues = {
  identifier,
  score,
  valBool: ValBool.create(true),
  valConstrained: ValConstrained.create(20n),
  valDate: ValDate.create(new Date("2026-07-29T00:00:00Z")),
  valDatetime: ValDatetime.create(new Date("2026-07-29T12:34:56Z")),
  valDatetimeTz: ValDatetimeTz.create(new Date("2026-07-29T12:34:56Z")),
  valDecimal: ValDecimal.create("3.5"),
  valDouble: ValDouble.create(3.5),
  valDuration: ValDuration.create("PT3S"),
};
const person = Person.create({
  ...personValues,
  aliases: [Aliases.create("first"), Aliases.create("second")],
});
assert.equal(person.__typebridgeForm, "complete");
assert.equal(person.iid, null);
assert.equal(person.nickname, null);
assert(Object.isFrozen(person));
assert.equal(Person.identifier.kind, "field");
assert.equal(Person.identifier.key, true);
assert.equal(Person.identifier.unique, true);
assert.equal(Person.identifier.multiplicity.cardinality.min, "1");
assert.equal(Person.identifier.multiplicity.cardinality.max, "1");

const event = Event.create({ subject: person });
assert.equal(event.subject, person);
const reference = Person.reference("person-iid", { identifier });
assert.equal(reference.__typebridgeForm, "reference");
assert.equal(reference.iid, "person-iid");
assert.throws(() => Identifier.create({}), TypeError);
assert.throws(() => Score.create(3), TypeError);
assert.throws(() => Score.create(1n << 63n), TypeError);

const hydrateComplete = Object.getOwnPropertySymbols(Identifier).find(
  (symbol) => symbol.description === "typebridge.hydrate-complete",
);
assert(hydrateComplete);
const hydratedIdentifier = Identifier[hydrateComplete]("identifier-iid", "provider-value");
assert.equal(hydratedIdentifier.iid, "identifier-iid");
assert.equal(hydratedIdentifier.value, "provider-value");
assert(Object.isFrozen(hydratedIdentifier));
const hydratedPerson = Person[hydrateComplete]("person-iid", {
  ...personValues,
  aliases: [],
  nickname: null,
});
assert.equal(hydratedPerson.iid, "person-iid");
assert.equal(hydratedPerson.nickname, null);
assert.deepEqual(hydratedPerson.aliases, []);
Employment.create({ employee: person });

const eventReference = Event.reference("event-iid", {});
Container.create({ item: [eventReference] });
assert.throws(
  () => Container.create({ item: [eventReference, eventReference, eventReference] }),
  RangeError,
);
assert.throws(() => Employment.create({ employee: event }), TypeError);
assert.throws(
  () => Employment.create({ employee: person, member: person }),
  TypeError,
);

assert.equal(Employment.employee.kind, "role");
assert.notEqual(Employment.employee.specializes, null);
assert(JSON.stringify(Employment.metadata).includes("role_upcasts"));

const stats = PlayerStats({ wins: 3n });
assert.equal(stats.nickname, null);
assert(Object.isFrozen(stats));

assert.equal(PLAYING_FACTS.length, 8);
assert(PLAYING_FACTS.every((fact) => fact.kind === "plays"));
const personId = '{"kind":"entity","label":"person"}';
const robotId = '{"kind":"entity","label":"robot"}';
const membershipMemberId = '{"declaring_relation":"membership","label":"member"}';
const eventSubjectId = '{"declaring_relation":"event","label":"subject"}';
const membershipFacts = PLAYING_FACTS.filter(
  (fact) => fact.role === membershipMemberId,
);
assert.equal(membershipFacts.length, 2);
const membershipPersonFact = membershipFacts.find(
  (fact) => fact.player === personId,
);
const membershipRobotFact = membershipFacts.find(
  (fact) => fact.player === robotId,
);
assert(membershipPersonFact);
assert(membershipRobotFact);
assert.equal(membershipPersonFact.multiplicity.cardinality.max, "2");
assert.equal(membershipRobotFact.multiplicity.cardinality.max, "1");
assert(JSON.stringify(membershipPersonFact.metadata).includes("membership player"));
assert(JSON.stringify(membershipRobotFact.metadata).includes("robot membership player"));
assert.notDeepEqual(membershipPersonFact.metadata.id, membershipRobotFact.metadata.id);
const eventSubjectFacts = PLAYING_FACTS.filter(
  (fact) => fact.role === eventSubjectId,
);
assert.equal(eventSubjectFacts.length, 1);
const eventSubjectFact = eventSubjectFacts[0];
assert.equal(eventSubjectFact.player, personId);
assert.equal(eventSubjectFact.multiplicity.cardinality.max, "1");
assert(JSON.stringify(eventSubjectFact.metadata).includes("event subject player"));
assert.notDeepEqual(eventSubjectFact.metadata.id, membershipPersonFact.metadata.id);
const projection = JSON.parse(RUNTIME_PROJECTION_JSON);
assert.deepEqual(JSON.parse(SEMANTIC_SCHEMA_FINGERPRINT_JSON), projection.semantic_fingerprint);
assert.deepEqual(JSON.parse(PROJECTION_FINGERPRINT_JSON), projection.projection_fingerprint);
const bindings = projection.models.map((model) => ({
  typeKey: JSON.stringify(model.id),
  targetName: model.target_name,
  create: model.create.enabled,
  reference: model.reference_read.target_name !== null,
}));
installRuntimeProjection({
  projectionJson: RUNTIME_PROJECTION_JSON,
  semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
  bindings,
});
assert.throws(() => installRuntimeProjection({
  projectionJson: RUNTIME_PROJECTION_JSON.replace('"target_name":"Aliases"', '"target_name":"AliasesTampered"'),
  semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
  bindings,
}), /fingerprint|canonical/i);
assert.throws(() => installRuntimeProjection({
  projectionJson: RUNTIME_PROJECTION_JSON,
  semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
  bindings: bindings.slice(1),
}), /exactly|coverage/i);
assert.throws(() => installRuntimeProjection({
  projectionJson: RUNTIME_PROJECTION_JSON,
  semanticFingerprintJson: SEMANTIC_SCHEMA_FINGERPRINT_JSON,
  projectionFingerprintJson: PROJECTION_FINGERPRINT_JSON,
  bindings: [{ ...bindings[0], targetName: "WrongTarget" }, ...bindings.slice(1)],
}), /registration|facet/i);
