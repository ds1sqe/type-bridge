import {
  Entity,
  Relation,
  TypeFlags,
  attr,
  field,
  role,
} from "../../typescript/index.js";
import {
  QuerySession,
  references,
  type BoundVar,
  type Predicate,
  type QueryOrder,
  type Selection,
} from "../../typescript/typed/index.js";
import { diagnosticQuerySession } from "../../typescript/typed/session.js";

class SharedName extends attr.String("owner-shared-name") {}
class Age extends attr.Integer("owner-age") {}
class Active extends attr.Boolean("owner-active") {}
class Salary extends attr.Integer("owner-salary") {}

class Party extends Entity("owner-party", {
  name: field(SharedName),
}) {}

class Person extends Entity("owner-person", {
  name: field(SharedName),
  age: field(Age),
  active: field(Active),
}) {}

// Deliberately same-shaped and sharing the same attribute class as Person.
class ForeignPerson extends Entity("owner-foreign-person", {
  name: field(SharedName),
  age: field(Age),
  active: field(Active),
}) {}

// TypeFlags is emitted for abstract/doc/meta models. Explicit names must carry
// the same exact owner identity as the string factory overload.
class FlaggedPerson extends Entity(
  TypeFlags({ name: "owner-flagged-person", abstract: true }),
  { name: field(SharedName) },
) {}
class ForeignFlaggedPerson extends Entity(
  TypeFlags({ name: "owner-foreign-flagged-person", abstract: true }),
  { name: field(SharedName) },
) {}
class FlaggedEmployee extends Entity(
  TypeFlags({ name: "owner-flagged-employee" }),
  {},
  { parent: FlaggedPerson },
) {}
class ForeignFlaggedEmployee extends Entity(
  TypeFlags({ name: "owner-foreign-flagged-employee" }),
  {},
  { parent: ForeignFlaggedPerson },
) {}

class Employee extends Entity(
  "owner-employee",
  { salary: field(Salary) },
  { parent: Party },
) {}

class Employment extends Relation("owner-employment", {
  employee: role(Person),
  employer: role(ForeignPerson),
  participant: role(Person, ForeignPerson),
}) {}

class PartyLink extends Relation("owner-party-link", {
  party: role(Party),
}) {}
class FlaggedLink extends Relation(
  TypeFlags({ name: "owner-flagged-link", abstract: true }),
  { participant: role(FlaggedPerson) },
) {}
class ForeignFlaggedLink extends Relation(
  TypeFlags({ name: "owner-foreign-flagged-link", abstract: true }),
  { participant: role(ForeignFlaggedPerson) },
) {}
class FlaggedLinkSubtype extends Relation(
  TypeFlags({ name: "owner-flagged-link-subtype" }),
  {},
  { parent: FlaggedLink },
) {}
class RelationEnvelope extends Relation("owner-relation-envelope", {
  nested: role(FlaggedLink),
}) {}

declare const selectedFlaggedLink: InstanceType<typeof FlaggedLink>;
const flaggedTypeName: "owner-flagged-person" = FlaggedPerson.typeName;
const selectedParticipants:
  | InstanceType<typeof FlaggedPerson>
  | readonly InstanceType<typeof FlaggedPerson>[] = selectedFlaggedLink.participant;
type ShallowFlaggedLink = Exclude<
  InstanceType<typeof RelationEnvelope>["nested"],
  readonly unknown[]
>;
declare const shallowFlaggedLink: ShallowFlaggedLink;
const absentNestedParticipants: undefined = shallowFlaggedLink.participant;
void selectedParticipants;
void absentNestedParticipants;
void flaggedTypeName;

const partyRefs = references(Party);
const personRefs = references(Person);
const foreignRefs = references(ForeignPerson);
const flaggedRefs = references(FlaggedPerson);
const foreignFlaggedRefs = references(ForeignFlaggedPerson);
const employeeRefs = references(Employee);
const employmentRefs = references(Employment);
const partyLinkRefs = references(PartyLink);
const flaggedLinkRefs = references(FlaggedLink);
const foreignFlaggedLinkRefs = references(ForeignFlaggedLink);

const session = diagnosticQuerySession();
const firstPerson = session.var(Person);
const secondPerson = session.var(Person, "subtypes");
const foreignPerson = session.var(ForeignPerson);
const flaggedPerson = session.var(FlaggedPerson);
const foreignFlaggedPerson = session.var(ForeignFlaggedPerson);
const flaggedEmployee = session.var(FlaggedEmployee);
const foreignFlaggedEmployee = session.var(ForeignFlaggedEmployee);
const employee = session.var(Employee);
const employment = session.var(Employment);
const partyLink = session.var(PartyLink);
const flaggedLink = session.var(FlaggedLink);
const flaggedLinkSubtype = session.var(FlaggedLinkSubtype);

const firstName = firstPerson.field(personRefs.fields.name);
const secondName = secondPerson.field(personRefs.fields.name);
const foreignName = foreignPerson.field(foreignRefs.fields.name);
const age = firstPerson.field(personRefs.fields.age);
const active = firstPerson.field(personRefs.fields.active);

const literalPredicate: Predicate = firstName.eq(new SharedName("Alice"));
const sameModelComparison: Predicate = firstName.ne(secondName);
const sameCategoryComparison: Predicate = firstName.eq(foreignName);
const explicitFieldComparison: Predicate = firstName.eqField(secondName);
const rangePredicate: Predicate = age.gte(new Age(18n));
const stringPredicate: Predicate = firstName.startsWith("A");
const booleanPredicate: Predicate = active.eq(new Active(true));
const order: QueryOrder = age.desc("last");
const selection: Selection<InstanceType<typeof Person>> = firstPerson;
const collected: Selection<readonly InstanceType<typeof Person>[]> = firstPerson
  .collect()
  .distinct()
  .orderBy(firstName.asc());

literalPredicate.and(sameModelComparison).or(sameCategoryComparison.not());
void rangePredicate;
void stringPredicate;
void booleanPredicate;
void explicitFieldComparison;
void order;
void selection;
void collected;

employment.role(employmentRefs.roles.employee).connects(firstPerson);
employment.role(employmentRefs.roles.employee).is(firstPerson);
employment.role(employmentRefs.roles.employer).connects(foreignPerson);
employment.role(employmentRefs.roles.participant).connects(firstPerson);
employment.role(employmentRefs.roles.participant).connects(foreignPerson);
flaggedPerson.field(flaggedRefs.fields.name);
flaggedEmployee.field(flaggedRefs.fields.name);
flaggedLink.role(flaggedLinkRefs.roles.participant).connects(flaggedPerson);
flaggedLinkSubtype.role(flaggedLinkRefs.roles.participant).connects(flaggedEmployee);
void foreignFlaggedEmployee;

// A base-owned field/role may bind to a declared subtype.
employee.field(partyRefs.fields.name).eq(new SharedName("Pat"));
employee.field(employeeRefs.fields.salary).gt(new Salary(10n));
partyLink.role(partyLinkRefs.roles.party).connects(employee);

// Two variables have one static model type while retaining different native tokens.
const sameModelVariables: readonly [
  BoundVar<InstanceType<typeof Person>>,
  BoundVar<InstanceType<typeof Person>>,
] = [firstPerson, secondPerson];
void sameModelVariables;

// @ts-expect-error same-shaped foreign owners remain nominally incompatible
firstPerson.field(foreignRefs.fields.name);
// @ts-expect-error TypeFlags explicit names retain nominal owner identity
flaggedPerson.field(foreignFlaggedRefs.fields.name);
// @ts-expect-error unrelated TypeFlags parent lineages remain incompatible
flaggedEmployee.field(foreignFlaggedRefs.fields.name);
// @ts-expect-error TypeFlags relation references retain nominal owner identity
flaggedLink.role(foreignFlaggedLinkRefs.roles.participant);
// @ts-expect-error TypeFlags relation lineage retains the declaring owner
flaggedLinkSubtype.role(foreignFlaggedLinkRefs.roles.participant);
// @ts-expect-error a flagged role rejects a foreign flagged player
flaggedLink.role(flaggedLinkRefs.roles.participant).connects(foreignFlaggedPerson);
// @ts-expect-error a base variable cannot use a subtype-only field
session.var(Party).field(employeeRefs.fields.salary);
// @ts-expect-error boolean fields do not expose range operators
active.gt(new Active(true));
// @ts-expect-error boolean fields do not expose ordering
active.asc();
// @ts-expect-error numeric fields do not expose string matching
age.contains("18");
// @ts-expect-error a value from another category cannot be compared
age.eq(new SharedName("18"));
// @ts-expect-error field-to-field comparison requires the same value category
age.eq(firstName);
// @ts-expect-error employee role accepts Person, not same-shaped ForeignPerson
employment.role(employmentRefs.roles.employee).connects(foreignPerson);
// @ts-expect-error employer role accepts ForeignPerson, not Person
employment.role(employmentRefs.roles.employer).connects(firstPerson);
// @ts-expect-error BoundVar is invariant even across a declared model subtype
const widenedVariable: BoundVar<InstanceType<typeof Party>> = employee;
void widenedVariable;
