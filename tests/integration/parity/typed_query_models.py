"""Python models for the shared live typed-query parity fixture."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from type_bridge import (
    AttributeFlags,
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)

FIXTURE_DIR = Path(__file__).with_name("fixtures") / "typed-query"
CONTRACT_PATH = FIXTURE_DIR / "contract.json"
SCHEMA_PATH = FIXTURE_DIR / "schema.tql"
DATA_PATH = FIXTURE_DIR / "data.tql"


def load_typed_query_contract() -> dict[str, Any]:
    """Load the shared labels and stable ordering oracle."""
    return json.loads(CONTRACT_PATH.read_text(encoding="utf-8"))


def _string_mapping(value: object) -> dict[str, str]:
    if not isinstance(value, dict):
        raise TypeError("typed-query labels must be an object")
    result: dict[str, str] = {}
    for key, item in value.items():
        if not isinstance(key, str) or not isinstance(item, str):
            raise TypeError("typed-query labels must map strings to strings")
        result[key] = item
    return result


_LABELS = _string_mapping(load_typed_query_contract()["labels"])


class ParityQueryPersonId(String):
    flags = AttributeFlags(name=_LABELS["person_id"])


class ParityQueryPersonName(String):
    flags = AttributeFlags(name=_LABELS["person_name"])


class ParityQueryRank(Integer):
    flags = AttributeFlags(name=_LABELS["rank"])


class ParityQuerySpecialty(String):
    flags = AttributeFlags(name=_LABELS["specialty"])


class ParityQueryCompanyId(String):
    flags = AttributeFlags(name=_LABELS["company_id"])


class ParityQueryCompanyName(String):
    flags = AttributeFlags(name=_LABELS["company_name"])


class ParityQueryEmploymentCode(String):
    flags = AttributeFlags(name=_LABELS["employment_code"])


class ParityQueryEnvelopeCode(String):
    flags = AttributeFlags(name=_LABELS["envelope_code"])


class ParityQueryPerson(Entity):
    flags = TypeFlags(name=_LABELS["person"])
    person_id: ParityQueryPersonId = Flag(Key)
    name: ParityQueryPersonName


class ParityQueryEmployee(ParityQueryPerson):
    flags = TypeFlags(name=_LABELS["employee"])
    rank: ParityQueryRank


class ParityQueryContractor(ParityQueryPerson):
    flags = TypeFlags(name=_LABELS["contractor"])
    specialty: ParityQuerySpecialty


class ParityQueryCompany(Entity):
    flags = TypeFlags(name=_LABELS["company"])
    company_id: ParityQueryCompanyId = Flag(Key)
    name: ParityQueryCompanyName


class ParityQueryEmployment(Relation):
    flags = TypeFlags(name=_LABELS["employment"])
    code: ParityQueryEmploymentCode = Flag(Key)
    employee: Role[ParityQueryPerson] = Role(
        "employee",
        ParityQueryPerson,
        cardinality=Card(1, 1),
    )
    employer: Role[ParityQueryCompany] = Role(
        "employer",
        ParityQueryCompany,
        cardinality=Card(1, 1),
    )


class ParityQueryEnvelope(Relation):
    flags = TypeFlags(name=_LABELS["envelope"])
    code: ParityQueryEnvelopeCode = Flag(Key)
    nested: Role[ParityQueryEmployment] = Role(
        "nested",
        ParityQueryEmployment,
        cardinality=Card(1, 1),
    )


PERSON_NAME = ParityQueryPersonName
COMPANY_NAME = ParityQueryCompanyName
EMPLOYMENT_CODE = ParityQueryEmploymentCode
