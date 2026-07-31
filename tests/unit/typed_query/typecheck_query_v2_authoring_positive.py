"""Positive Pyright fixture for the complete low-level V2 authoring surface."""

from __future__ import annotations

from typing import Literal, assert_type

from type_bridge_core import QueryV2Error

from type_bridge.query_v2 import (
    AuthoredQueryInvocation,
    AuthoredQueryPlan,
    QueryPlanBuilder,
    QueryV2Authority,
)


def positive_query_v2_authoring(
    declared_schema: bytes,
    authority: QueryV2Authority,
) -> None:
    constructed = QueryV2Authority(
        declared_schema,
        "typecheck-query-v2",
        "typedb-3.12.1/v1",
    )
    assert_type(constructed, QueryV2Authority)

    builder = QueryPlanBuilder(authority)
    person = builder.binding("person")
    friend = builder.binding("friend")
    name = builder.binding("name")
    result = builder.binding("result")
    input_column = builder.input("prefix", "string", True)

    person_operand = builder.binding_operand(person)
    name_operand = builder.binding_operand(name)
    input_operand = builder.input_operand(input_column)
    text = builder.literal_operand("string", "Ada")
    builder.literal_operand("long", 1)
    builder.literal_operand("double", 1.0)
    builder.literal_operand("boolean", True)
    builder.literal_operand("date", "2026-07-24")
    builder.literal_operand("datetime", "2026-07-24T00:00:00")
    builder.literal_operand("datetime_tz", "2026-07-24T00:00:00Z")
    builder.literal_operand("decimal", "1.25")
    builder.literal_operand("duration", "P1DT2H")

    person_isa = builder.isa(person, "entity", "person", True)
    friend_isa = builder.isa(friend, "entity", "person", True)
    has_name = builder.has(person, name, "name")
    relation = builder.binding("friendship")
    links = builder.links(
        relation,
        "friendship",
        ("friend", "friend"),
        (person, friend),
    )
    builder.isa(relation, "relation", "friendship", False)
    builder.isa(name, "attribute", "name", False)
    equals = builder.value("equal", name_operand, text)
    builder.value("not_equal", name_operand, text)
    builder.value("less", name_operand, text)
    builder.value("less_or_equal", name_operand, text)
    builder.value("greater", name_operand, text)
    builder.value("greater_or_equal", name_operand, text)
    input_equals = builder.value("equal", name_operand, input_operand)
    builder.not_((input_equals,))
    builder.or_(((friend_isa,), (links,)))
    builder.try_((friend_isa,))
    builder.reachable(
        person,
        friend,
        "friendship",
        "friend",
        "friend",
        0,
        3,
    )
    builder.function_call(result, (name_operand,), "score")

    local_return = builder.local_return("count", name, "long")
    builder.local_return("sum", name, "long")
    builder.local_return("sum", name, "double")
    local = builder.local_function(
        "local_score",
        (name,),
        (name,),
        ("name",),
        (equals,),
        local_return,
    )
    builder.function_call(result, (person_operand,), None, local)
    builder.function_call(result, (person_operand,), local_function=local)

    builder.match((person_isa, has_name))
    builder.select((person, name))
    builder.require((person, name))
    builder.distinct()
    count = builder.reduce_assignment(result, "count")
    builder.reduce_assignment(result, "max", name)
    builder.reduce_assignment(result, "mean", name)
    builder.reduce_assignment(result, "median", name)
    builder.reduce_assignment(result, "min", name)
    builder.reduce_assignment(result, "std", name)
    builder.reduce_assignment(result, "sum", name)
    builder.reduce((count,), (person,))
    builder.sort((builder.order(person, "ascending"),))
    builder.order(person, "descending")
    builder.offset(0)
    builder.limit(10)
    plan = builder.finalize_rows((person, result))
    assert_type(plan, AuthoredQueryPlan)
    assert_type(plan.canonical_bytes, bytes)
    assert_type(plan.format, Literal["typebridge.query-plan/v2"])
    assert_type(plan.fingerprint, str)
    assert_type(plan.required_capabilities, tuple[str, ...])
    invocation = plan.rows(())
    assert_type(invocation, AuthoredQueryInvocation)
    assert_type(invocation.canonical_bytes, bytes)
    assert_type(invocation.operation, Literal["rows", "count", "exists"])
    assert_type(plan.count(()), AuthoredQueryInvocation)
    assert_type(plan.exists(()), AuthoredQueryInvocation)


def positive_query_v2_documents(authority: QueryV2Authority) -> AuthoredQueryPlan:
    builder = QueryPlanBuilder(authority)
    person = builder.binding("person")
    builder.match((builder.isa(person, "entity", "person", False),))
    scalar = builder.document_binding("person", person)
    names = builder.document_attribute_list("names", person, "name")
    plan = builder.finalize_documents((scalar, names))
    assert_type(plan.documents(()), AuthoredQueryInvocation)
    return plan


def positive_query_v2_structured_diagnostic(error: QueryV2Error) -> None:
    assert_type(
        error.category,
        Literal[
            "invalid_contract",
            "unsupported_capability",
            "resource_limit",
            "integrity",
        ],
    )
    for segment in error.path:
        if segment["kind"] == "index":
            assert_type(segment["value"], int)
        else:
            assert_type(segment["kind"], Literal["field", "identifier"])
            assert_type(segment["value"], str)
    for detail in error.details.values():
        if detail["kind"] == "boolean":
            assert_type(detail["value"], bool)
        elif detail["kind"] == "text_list":
            assert_type(detail["value"], list[str])
        else:
            assert_type(detail["kind"], Literal["text", "long"])
            assert_type(detail["value"], str)
