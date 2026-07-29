import { Entity, Relation, attr, field, role } from "@type-bridge/node";

export class ContractName extends attr.String("contract-name") {}
export class ContractAge extends attr.Integer("contract-age") {}

export class ContractPerson extends Entity("contract-person", {
  name: field(ContractName),
  age: field(ContractAge),
}) {}

export class ContractCompany extends Entity("contract-company", {
  name: field(ContractName),
}) {}

export class ContractSkill extends Entity("contract-skill", {
  name: field(ContractName),
}) {}

export class ContractEmployment extends Relation("contract-employment", {
  employee: role(ContractPerson),
  employer: role(ContractCompany),
}) {}

export class ContractPersonSkill extends Relation("contract-person-skill", {
  person: role(ContractPerson),
  skill: role(ContractSkill),
}) {}
