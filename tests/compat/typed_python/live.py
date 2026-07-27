#!/usr/bin/env python3
"""Live typed-query consumer loaded only from extracted wheel artifacts."""

from __future__ import annotations

import argparse
import json
import os
import sys
import warnings
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any


def _within(path: str | Path, root: str | Path) -> bool:
    candidate = Path(path).expanduser().resolve()
    boundary = Path(root).expanduser().resolve()
    return candidate == boundary or boundary in candidate.parents


def _activate_artifact(artifact_root: Path, source_root: Path) -> None:
    retained: list[str] = []
    for entry in sys.path:
        if not entry:
            continue
        candidate = Path(entry).expanduser().resolve()
        if _within(candidate, source_root) and (
            (candidate / "type_bridge").exists() or (candidate / "type_bridge_core").exists()
        ):
            continue
        retained.append(entry)
    sys.path[:] = [str(artifact_root), *retained]


def _module_paths(module: ModuleType) -> list[Path]:
    paths: list[Path] = []
    module_file = getattr(module, "__file__", None)
    if module_file:
        paths.append(Path(module_file).resolve())
    module_path = getattr(module, "__path__", None)
    if module_path is not None:
        paths.extend(Path(entry).resolve() for entry in module_path)
    return paths


def _assert_artifact_imports(artifact_root: Path, source_root: Path) -> None:
    for name, module in sys.modules.items():
        if name != "type_bridge" and not name.startswith(("type_bridge.", "type_bridge_core")):
            continue
        paths = _module_paths(module)
        if not paths or not all(_within(path, artifact_root) for path in paths):
            raise AssertionError(f"live wheel import escaped artifact root: {name} -> {paths}")
        if any(_within(path, source_root) for path in paths):
            raise AssertionError(f"live wheel import leaked to checkout: {name} -> {paths}")


def _run(args: argparse.Namespace) -> dict[str, Any]:
    artifact_root = args.artifact_root.resolve()
    source_root = args.source_root.resolve()
    _activate_artifact(artifact_root, source_root)

    from type_bridge import (
        AttributeFlags,
        Card,
        Database,
        Entity,
        Flag,
        Integer,
        Key,
        Relation,
        Role,
        String,
        TypeFlags,
    )
    from type_bridge.models.base import TypeDBType
    from type_bridge.session import TransactionContext, TypeDBServerDeprecationWarning
    from type_bridge.typed import QuerySession

    contract = json.loads(args.fixture.read_text(encoding="utf-8"))
    labels: dict[str, str] = contract["labels"]
    expected: dict[str, Any] = contract["expected"]

    # Postponed model annotations resolve through module globals.
    globals().update({"Role": Role})

    class PersonId(String):
        flags = AttributeFlags(name=labels["person_id"])

    class PersonName(String):
        flags = AttributeFlags(name=labels["person_name"])

    class Rank(Integer):
        flags = AttributeFlags(name=labels["rank"])

    class Specialty(String):
        flags = AttributeFlags(name=labels["specialty"])

    class CompanyId(String):
        flags = AttributeFlags(name=labels["company_id"])

    class CompanyName(String):
        flags = AttributeFlags(name=labels["company_name"])

    class EmploymentCode(String):
        flags = AttributeFlags(name=labels["employment_code"])

    class EnvelopeCode(String):
        flags = AttributeFlags(name=labels["envelope_code"])

    globals().update(
        {
            "PersonId": PersonId,
            "PersonName": PersonName,
            "Rank": Rank,
            "Specialty": Specialty,
            "CompanyId": CompanyId,
            "CompanyName": CompanyName,
            "EmploymentCode": EmploymentCode,
            "EnvelopeCode": EnvelopeCode,
        }
    )

    class Person(Entity):
        flags = TypeFlags(name=labels["person"])
        person_id: PersonId = Flag(Key)
        name: PersonName

    globals()["Person"] = Person

    class Employee(Person):
        flags = TypeFlags(name=labels["employee"])
        rank: Rank

    class Contractor(Person):
        flags = TypeFlags(name=labels["contractor"])
        specialty: Specialty

    class Company(Entity):
        flags = TypeFlags(name=labels["company"])
        company_id: CompanyId = Flag(Key)
        name: CompanyName

    globals().update({"Employee": Employee, "Contractor": Contractor, "Company": Company})

    class Employment(Relation):
        flags = TypeFlags(name=labels["employment"])
        code: EmploymentCode = Flag(Key)
        employee: Role[Person] = Role("employee", Person, cardinality=Card(1, 1))
        employer: Role[Company] = Role("employer", Company, cardinality=Card(1, 1))

    globals()["Employment"] = Employment

    class Envelope(Relation):
        flags = TypeFlags(name=labels["envelope"])
        code: EnvelopeCode = Flag(Key)
        nested: Role[Employment] = Role(
            "nested",
            Employment,
            cardinality=Card(1, 1),
        )

    globals()["Envelope"] = Envelope

    @dataclass(frozen=True, slots=True)
    class PersonWork:
        person: Person
        employments: tuple[Employment, ...]
        companies: tuple[Company, ...]

    def graph_query(connection: Database | TransactionContext):
        session = QuerySession(connection)
        person = session.var(Person, subtypes=True)
        employment = session.var(Employment)
        company = session.var(Company)
        query = session.query_as(
            PersonWork,
            person=person,
            employments=employment.collect().order_by(employment.field(EmploymentCode).asc()),
            companies=company.collect().distinct().order_by(company.field(CompanyName).asc()),
        ).where(
            employment.role(Employment.employee).connects(person),
            employment.role(Employment.employer).connects(company),
        )
        return person, query

    def identity(thing: TypeDBType) -> dict[str, str]:
        if not isinstance(thing._iid, str):
            raise AssertionError("wheel-hydrated thing has no IID")
        if isinstance(thing, Employee):
            kind = "employee"
        elif isinstance(thing, Contractor):
            kind = "contractor"
        elif isinstance(thing, Company):
            kind = "company"
        elif isinstance(thing, Employment):
            kind = "employment"
        else:
            raise AssertionError(f"unexpected wheel constructor: {type(thing).__name__}")
        return {"kind": kind, "iid": thing._iid}

    def relation_player_contract(connection: Database) -> dict[str, str]:
        try:
            QuerySession(connection).var(Envelope)
        except TypeError as error:
            if "cannot materialize nested relation role" not in str(error):
                raise AssertionError("wheel rejected F8 with the wrong contract") from error
            return {"contract": "planning-time-rejection"}
        raise AssertionError("wheel accepted unsupported nested relation-player planning")

    database = Database(
        args.address,
        args.database,
        username=args.username,
        password=args.password,
        http_port=args.http_port,
    )
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", TypeDBServerDeprecationWarning)
        database.connect()
    legacy_notices = [
        {
            "message": str(item.message),
            "type": item.category.__name__,
            "code": getattr(item.category, "code", None),
        }
        for item in caught
        if item.category is TypeDBServerDeprecationWarning
    ]
    expected_given = os.environ.get("TYPE_BRIDGE_PARITY_EXPECT_GIVEN")
    if expected_given is not None:
        if expected_given not in {"0", "1"}:
            raise AssertionError("TYPE_BRIDGE_PARITY_EXPECT_GIVEN must be either '0' or '1'")
        if database.supports_given_stage() is not (expected_given == "1"):
            raise AssertionError("wheel given-stage capability drifted from the release lane")
    person, query = graph_query(database)
    page = query.page_by(
        person,
        limit=10,
        order_by=(person.field(PersonName).asc(),),
        include_total=True,
    )
    semantic_page = query.page_by(
        person,
        limit=1,
        order_by=(person.field(PersonName).asc(),),
        include_total=True,
    )
    if [row.person.person_id.value for row in page.items] != expected["root_keys"]:
        raise AssertionError("wheel page roots drifted from the shared oracle")
    for row in page.items:
        key = row.person.person_id.value
        if [item.code.value for item in row.employments] != expected["employment_keys"][key]:
            raise AssertionError(f"wheel employment collection drifted for {key}")
        if [item.company_id.value for item in row.companies] != expected["company_keys"][key]:
            raise AssertionError(f"wheel company collection drifted for {key}")

    count = query.count_by(person)
    exists = query.exists_by(person)
    semantic_projection = {
        "source_fixture": contract["semantic_corpus_projection"]["source_fixture"],
        "distinct_roots": [f"person:{row.person.person_id.value}" for row in page.items],
        "page_by_person_offset_0_limit_1": {
            "roots": [f"person:{row.person.person_id.value}" for row in semantic_page.items],
            "offset": semantic_page.offset,
            "limit": semantic_page.limit,
            "total": semantic_page.total,
        },
        "alice_collect_count": len(page.items[0].employments),
        "alice_collect_distinct_count": len(page.items[0].companies),
        "count_by_person": count,
        "exists_by_person": exists,
    }
    if semantic_projection != contract["semantic_corpus_projection"]:
        raise AssertionError("wheel semantic projection drifted from the #171 identity manifest")
    relation_player = relation_player_contract(database)
    with database.transaction("read") as transaction:
        borrowed_person, borrowed_query = graph_query(transaction)
        borrowed = {
            "counts": [
                borrowed_query.count_by(borrowed_person),
                borrowed_query.count_by(borrowed_person),
            ],
            "exists": [
                borrowed_query.exists_by(borrowed_person),
                borrowed_query.exists_by(borrowed_person),
            ],
        }

    summary = {
        "version": contract["version"],
        "page": {
            "offset": page.offset,
            "limit": page.limit,
            "total": page.total,
            "items": [
                {
                    "person": identity(row.person),
                    "employments": [identity(item) for item in row.employments],
                    "companies": [identity(item) for item in row.companies],
                    "role_players": [
                        {
                            "employment": identity(item),
                            "employee": identity(item.employee),
                            "employer": identity(item.employer),
                        }
                        for item in row.employments
                    ],
                }
                for row in page.items
            ],
        },
        "count": count,
        "exists": exists,
        "semantic_corpus_projection": semantic_projection,
        "relation_player": relation_player,
        "borrowed": borrowed,
    }
    _assert_artifact_imports(artifact_root, source_root)
    return {
        "status": "ok",
        "artifact": "wheel",
        "legacy_notices": legacy_notices,
        "summary": summary,
    }


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact-root", type=Path, required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--address", required=True)
    parser.add_argument("--database", required=True)
    parser.add_argument("--http-port", type=int, required=True)
    parser.add_argument("--username", default="admin")
    parser.add_argument("--password", default="password")
    return parser


if __name__ == "__main__":
    print(json.dumps(_run(_parser().parse_args()), sort_keys=True))
