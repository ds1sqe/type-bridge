"""Retained Python emitters preserve the official AST and caller-owned bytes."""

import json
from datetime import date, datetime, timedelta
from decimal import Decimal as DecimalValue
from pathlib import Path
from typing import Any
from zoneinfo import ZoneInfo

import pytest
import type_bridge_core as core

from tests.unit.migration import test_lowering as models
from tests.utils.handwritten import (
    AttributeFlags,
    Boolean,
    Date,
    DateTime,
    DateTimeTZ,
    Decimal,
    Doc,
    Double,
    Duration,
    Entity,
    Flag,
    Integer,
    Key,
    Meta,
    Ordered,
    Relation,
    Role,
    String,
    TypeFlags,
)
from tests.utils.typeql import assert_typeql_equivalent
from type_bridge.expressions.functions import FunctionQuery, ReturnType
from type_bridge.migration import _operations as ops
from type_bridge.query import Query

BASELINE = json.loads((Path(__file__).parent / "fixtures/python_typeql_pre_ast.json").read_text())


def test_frozen_operation_schema_and_query_outputs_keep_their_asts() -> None:
    operations: dict[str, ops.Operation] = {}
    for mark in getattr(
        models.test_every_current_operation_lowers_to_rust_normalized_spec, "pytestmark"
    ):
        if mark.name == "parametrize":
            operations.update({kind: operation for operation, kind in mark.args[1]})
    queries = {
        "match": Query().match("$p isa person"),
        "fetch": Query().match("$p isa person").fetch("$p"),
        "sort_limit": Query()
        .match("$p isa person")
        .fetch("$p")
        .sort("$p", "desc")
        .offset(2)
        .limit(3),
        "insert": Query().insert('$p isa person, has name "A"'),
    }
    for row in BASELINE["emitters"]:
        case = row["case"]
        if case in operations:
            operation = operations[case]
            for direction, method in (
                ("forward", operation.to_typeql_steps),
                ("reverse", operation.to_rollback_typeql_steps),
            ):
                if direction + "_error" in row:
                    with pytest.raises(NotImplementedError) as error:
                        method()
                    assert str(error.value) == row[direction + "_error"]
                elif isinstance(operation, ops.RunTypeQL):
                    assert method() == row[direction]
                else:
                    assert_typeql_equivalent(method(), row[direction])
        elif case.startswith("schema_"):
            rendered = getattr(models, case.removeprefix("schema_")).to_schema_definition()
            assert_typeql_equivalent("define " + rendered, "define " + row["schema"])
        else:
            assert_typeql_equivalent(queries[case.removeprefix("query_")].build(), row["query"])


def _schema_models() -> dict[str, type[Any]]:
    class Name(String):
        flags = AttributeFlags(
            name="legacy-name", doc='name "doc"', meta={"z": "last", "a": "first"}
        )

    class Age(Integer):
        flags = AttributeFlags(name="legacy-age")
        range_constraint = (0, 150)

    class Parent(Entity):
        flags = TypeFlags(
            name="legacy-parent", abstract=True, doc="parent", meta={"owner": "schema"}
        )
        name: Name = Flag(Key)

    class Child(Parent):
        flags = TypeFlags(name="legacy-child")
        age: Age | None = None

    class Base(Relation):
        flags = TypeFlags(name="legacy-base", abstract=True)
        participant: Role[Parent] = Role("participant", Parent, abstract=True, ordered=True)

    class Review(Base):
        flags = TypeFlags(name="legacy-review", abstract=True)
        reviewer: Role[Child] = Role(
            "reviewer",
            Child,
            overrides="participant[]",
            abstract=True,
            ordered=True,
            doc="reviewer",
        )
        names: list[Name] = Flag(Ordered, Doc("names"), Meta("owner", "bridge"))

    return dict(
        zip(
            (
                "attribute_doc_meta",
                "attribute_range",
                "entity_parent",
                "entity_child",
                "relation_base",
                "relation_child_ordered",
            ),
            (Name, Age, Parent, Child, Base, Review),
            strict=True,
        )
    )


def test_schema_inheritance_and_legacy_operations_keep_local_declarations() -> None:
    classes = _schema_models()
    for row in BASELINE["schemas"]:
        model = classes[row["case"]]
        assert_typeql_equivalent(
            "define " + model.to_schema_definition(), "define " + row["schema"]
        )
        if row["case"].startswith("entity_"):
            assert_typeql_equivalent(ops.AddEntity(model).to_typeql(), "define " + row["schema"])
        elif row["case"] == "relation_child_ordered":
            assert_typeql_equivalent(
                ops.AddRelation(model).to_typeql(),
                "define " + row["schema"] + " legacy-child plays legacy-review:reviewer;",
            )


@pytest.mark.parametrize("index", range(9))
def test_key_literal_domains_keep_the_frozen_match_ast(index: int) -> None:
    domains = [
        (String, "hello"),
        (Integer, 42),
        (Double, 1.5),
        (Boolean, True),
        (Date, date(2026, 10, 16)),
        (DateTime, datetime(2026, 10, 16, 10, 11, 12)),
        (DateTimeTZ, datetime(2026, 10, 16, 10, 11, 12, tzinfo=ZoneInfo("America/New_York"))),
        (Decimal, DecimalValue("1.25")),
        (Duration, timedelta(days=1)),
    ]
    base, value = domains[index]
    row = BASELINE["keys"][index]

    class KeyAttr(base):
        flags = AttributeFlags(name="key-" + row["case"])

    class KeyEntity(Entity):
        flags = TypeFlags(name="key-holder-" + row["case"])
        key: KeyAttr = Flag(Key)

    instance = KeyEntity(key=KeyAttr(value))
    assert_typeql_equivalent(
        "match " + instance.get_match_clause_info().main_clause + ";", row["old"]
    )


def test_relation_and_entity_match_helpers_keep_their_frozen_asts() -> None:
    person = models.LowerPerson(name=models.LowerName("Alice"))
    company = models.LowerCompany(name=models.LowerName("Acme"))
    relation = models.LowerEmployment(employee=person, employer=company)
    for instance, row in zip((person, relation), BASELINE["matches"], strict=True):
        info = instance.get_match_clause_info()
        query = "match " + "; ".join([info.main_clause, *info.extra_clauses]) + ";"
        assert_typeql_equivalent(query, row["old"])


def test_function_queries_compile_modifiers_before_fetch_through_one_ast() -> None:
    for row in BASELINE["functions"]:
        function = FunctionQuery(
            name=row["case"],
            return_type=ReturnType(row["types"], is_stream=row["is_stream"]),
            args=[tuple(value) for value in row["args"]],
        )
        assert_typeql_equivalent(function.to_query(**row["options"]), row["query"])


def test_optional_attribute_clear_keeps_the_released_query_ast() -> None:
    actual = core._clear_attribute_query("person", "0x123", "$e", ["name", "age"])
    old = """match $e isa person, iid 0x123;
try { $e has name $old_attr_0; $old_attr_0 isa name; };
try { $e has age $old_attr_1; $old_attr_1 isa age; };
delete try { $old_attr_0 of $e; }; try { $old_attr_1 of $e; };"""
    assert_typeql_equivalent(actual, old)
