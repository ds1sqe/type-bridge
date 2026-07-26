"""Live selected-query parity through public Python and packed Node surfaces."""

from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Any

import pytest
import type_bridge_core

from tests.integration.parity.cross_language import (
    read_typed_query_with_packed_node,
    read_typed_query_with_wheel_python,
)
from tests.integration.parity.typed_query_models import (
    COMPANY_NAME,
    DATA_PATH,
    EMPLOYMENT_CODE,
    PERSON_NAME,
    SCHEMA_PATH,
    ParityQueryCompany,
    ParityQueryContractor,
    ParityQueryEmployee,
    ParityQueryEmployment,
    ParityQueryEnvelope,
    ParityQueryPerson,
    load_typed_query_contract,
)
from type_bridge.models.base import TypeDBType
from type_bridge.session import Database, TransactionContext
from type_bridge.typed import QuerySession


@dataclass(frozen=True, slots=True)
class ParityPersonPage:
    person: ParityQueryPerson
    employments: tuple[ParityQueryEmployment, ...]
    companies: tuple[ParityQueryCompany, ...]


def _graph_query(connection: Database | TransactionContext):
    session = QuerySession(connection)
    person = session.var(ParityQueryPerson, subtypes=True)
    employment = session.var(ParityQueryEmployment)
    company = session.var(ParityQueryCompany)
    query = session.query_as(
        ParityPersonPage,
        person=person,
        employments=employment.collect().order_by(employment.field(EMPLOYMENT_CODE).asc()),
        companies=company.collect().distinct().order_by(company.field(COMPANY_NAME).asc()),
    ).where(
        employment.role(ParityQueryEmployment.employee).connects(person),
        employment.role(ParityQueryEmployment.employer).connects(company),
    )
    return person, query


def _identity(thing: TypeDBType) -> dict[str, str]:
    assert isinstance(thing._iid, str)
    if isinstance(thing, ParityQueryEmployee):
        kind = "employee"
    elif isinstance(thing, ParityQueryContractor):
        kind = "contractor"
    elif isinstance(thing, ParityQueryCompany):
        kind = "company"
    elif isinstance(thing, ParityQueryEmployment):
        kind = "employment"
    else:  # pragma: no cover - a materializer defect should fail loudly
        raise TypeError(f"unknown typed-query parity constructor: {type(thing).__name__}")
    return {"kind": kind, "iid": thing._iid}


def _python_relation_player_contract(
    connection: Database | TransactionContext,
) -> dict[str, str]:
    session = QuerySession(connection)
    with pytest.raises(TypeError, match="cannot materialize nested relation role"):
        session.var(ParityQueryEnvelope)
    return {"contract": "planning-time-rejection"}


def _python_summary(db: Database) -> dict[str, Any]:
    contract = load_typed_query_contract()
    expected = contract["expected"]
    person, query = _graph_query(db)
    page = query.page_by(
        person,
        limit=10,
        order_by=(person.field(PERSON_NAME).asc(),),
        include_total=True,
    )
    semantic_page = query.page_by(
        person,
        limit=1,
        order_by=(person.field(PERSON_NAME).asc(),),
        include_total=True,
    )

    assert [row.person.person_id.value for row in page.items] == expected["root_keys"]
    for row in page.items:
        root_key = row.person.person_id.value
        assert [item.code.value for item in row.employments] == expected["employment_keys"][
            root_key
        ]
        assert [item.company_id.value for item in row.companies] == expected["company_keys"][
            root_key
        ]

    count = query.count_by(person)
    exists = query.exists_by(person)
    assert count == expected["total"]
    assert exists is True
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
    assert semantic_projection == contract["semantic_corpus_projection"]
    relation_player = _python_relation_player_contract(db)

    with db.transaction("read") as transaction:
        borrowed_person, borrowed_query = _graph_query(transaction)
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

    return {
        "version": contract["version"],
        "page": {
            "offset": page.offset,
            "limit": page.limit,
            "total": page.total,
            "items": [
                {
                    "person": _identity(row.person),
                    "employments": [_identity(item) for item in row.employments],
                    "companies": [_identity(item) for item in row.companies],
                    "role_players": [
                        {
                            "employment": _identity(item),
                            "employee": _identity(item.employee),
                            "employer": _identity(item.employer),
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


@pytest.mark.integration
def test_live_typed_query_summary_and_f8_contract_match_built_artifacts(
    clean_db,
    monkeypatch,
) -> None:
    """Built Python rejects F8 while packed Node preserves its shallow V1 result."""
    monkeypatch.delenv("TYPE_BRIDGE_BACKEND", raising=False)
    expected_given = os.environ.get("TYPE_BRIDGE_PARITY_EXPECT_GIVEN")
    if expected_given is not None:
        assert expected_given in {"0", "1"}
        assert clean_db.supports_given_stage() is (expected_given == "1")
    expected_legacy_warning = os.environ.get("TYPE_BRIDGE_PARITY_EXPECT_LEGACY_WARNING")
    if os.environ.get("TYPE_BRIDGE_PARITY_STRICT") == "1":
        assert expected_legacy_warning is not None, (
            "strict artifact parity requires TYPE_BRIDGE_PARITY_EXPECT_LEGACY_WARNING"
        )
    if expected_legacy_warning is not None:
        assert expected_legacy_warning in {"0", "1"}
    server_version = clean_db.detected_server_version()
    notice_message = type_bridge_core.typedb_server_deprecation_notice(server_version)
    if expected_legacy_warning is not None:
        assert (notice_message is not None) is (expected_legacy_warning == "1")
    expected_code = type_bridge_core.TYPEDB_LEGACY_SERVER_DEPRECATION_CODE
    expected_python_notices = (
        []
        if notice_message is None
        else [
            {
                "message": notice_message,
                "type": "TypeDBServerDeprecationWarning",
                "code": expected_code,
            }
        ]
    )
    expected_node_notices = (
        []
        if notice_message is None
        else [
            {
                "message": notice_message,
                "type": "DeprecationWarning",
                "code": expected_code,
            }
        ]
    )
    clean_db.execute_query(SCHEMA_PATH.read_text(encoding="utf-8"), transaction_type="schema")
    clean_db.execute_query(DATA_PATH.read_text(encoding="utf-8"), transaction_type="write")

    python_summary = _python_summary(clean_db)
    wheel_result = read_typed_query_with_wheel_python(
        clean_db.address,
        clean_db.database_name,
        http_port=clean_db.http_port,
    )
    if wheel_result is not None:
        assert wheel_result["artifact"] == "wheel"
        assert wheel_result["legacy_notices"] == expected_python_notices
        assert wheel_result["summary"] == python_summary

    node_result = read_typed_query_with_packed_node(
        clean_db.address,
        clean_db.database_name,
        http_port=clean_db.http_port,
    )

    assert node_result["artifact"] == "packed"
    assert node_result["legacy_notices"] == expected_node_notices
    node_summary = node_result["summary"]
    assert python_summary["relation_player"] == {"contract": "planning-time-rejection"}
    assert node_summary["relation_player"]["contract"] == "shallow-nonrecursive"
    assert node_summary["relation_player"]["shallow"]["roles_materialized"] is False
    assert node_summary["relation_player"]["selected"]["roles_materialized"] is True
    assert node_summary["relation_player"]["same_iid"] is True
    assert node_summary["relation_player"]["distinct_objects"] is True
    assert {key: value for key, value in node_summary.items() if key != "relation_player"} == {
        key: value for key, value in python_summary.items() if key != "relation_player"
    }
