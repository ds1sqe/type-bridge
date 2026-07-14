"""Runtime coverage for native named and collected #175 shapes."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from operator import setitem
from typing import NamedTuple

import pytest
import type_bridge_core

from tests.unit.typed_query._support import (
    corpus_error,
    diagnostic_session,
    invoke_untyped,
    runtime_attribute,
)
from type_bridge import Entity, Flag, Key, Relation, Role, String, TypeFlags
from type_bridge.typed import (
    Page,
    Query,
    TypedQueryConnectionError,
)


class WorkIdentity(String):
    pass


class WorkPerson(Entity):
    flags = TypeFlags(name="typed-work-person")
    name: WorkIdentity = Flag(Key)


class WorkEmployee(WorkPerson):
    flags = TypeFlags(name="typed-work-employee")


class WorkPersonCollision(Entity):
    flags = TypeFlags(name="typed-work-person")
    name: WorkIdentity = Flag(Key)


class PythonOnlyWork(Entity):
    flags = TypeFlags(name="typed-work-python-only", base=True)


class PythonOnlyWorkRelation(Relation):
    flags = TypeFlags(name="typed-work-python-only-relation", base=True)


class WorkCompany(Entity):
    flags = TypeFlags(name="typed-work-company")
    name: WorkIdentity = Flag(Key)


class WorkEmployment(Relation):
    flags = TypeFlags(name="typed-work-employment")
    identifier: WorkIdentity = Flag(Key)
    employee: Role[WorkPerson] = Role("employee", WorkPerson)
    employer: Role[WorkCompany] = Role("employer", WorkCompany)


class WorkNestedRelation(Relation):
    flags = TypeFlags(name="typed-work-nested-relation")
    identifier: WorkIdentity = Flag(Key)
    member: Role[WorkPerson] = Role("member", WorkPerson)


class WorkEnvelope(Relation):
    flags = TypeFlags(name="typed-work-envelope")
    identifier: WorkIdentity = Flag(Key)
    nested: Role[WorkNestedRelation] = Role("nested", WorkNestedRelation)


class UnregisteredWork(Entity):
    flags = TypeFlags(name="typed-work-unregistered")


@dataclass(frozen=True, slots=True)
class PersonRow:
    person: WorkPerson


@dataclass(frozen=True, slots=True)
class PersonWork:
    person: WorkPerson
    employments: tuple[WorkEmployment, ...]
    companies: tuple[WorkCompany, ...]


class PersonWorkTuple(NamedTuple):
    person: WorkPerson
    employments: tuple[WorkEmployment, ...]
    companies: tuple[WorkCompany, ...]


@dataclass(slots=True)
class MutablePersonRow:
    person: WorkPerson


@dataclass(frozen=True, slots=True)
class WrongPersonRow:
    person: WorkCompany


@dataclass(frozen=True, slots=True)
class CollisionPersonRow:
    person: WorkPersonCollision


@dataclass(frozen=True, slots=True)
class CollisionPeopleRow:
    people: tuple[WorkPersonCollision, ...]


@dataclass(frozen=True, slots=True)
class OptionalPersonRow:
    person: WorkPerson | None


@dataclass(frozen=True, slots=True)
class ListCollectionRow:
    employments: list[WorkEmployment]


@dataclass(frozen=True, slots=True)
class WrongCardinalityRow:
    employment: WorkEmployment


@dataclass(frozen=True, slots=True)
class UnknownDescriptorRow:
    person: UnregisteredWork


@dataclass(frozen=True, slots=True)
class DuplicateSelectionRow:
    person: WorkPerson
    people: tuple[WorkPerson, ...]


@dataclass(frozen=True, slots=True)
class EmptyRow:
    pass


class UnsupportedRow:
    person: WorkPerson


def _connected_shape():
    session = diagnostic_session()
    person = session.var(WorkPerson)
    employment = session.var(WorkEmployment)
    company = session.var(WorkCompany)
    predicates = (
        employment.role(WorkEmployment.employee).connects(person),
        employment.role(WorkEmployment.employer).connects(company),
    )
    return session, person, employment, company, predicates


def _assert_connection_required(callback: Callable[[], object]) -> None:
    with pytest.raises(TypedQueryConnectionError) as raised:
        callback()
    assert raised.value.category == "invalid_plan"
    assert raised.value.code == "execution_connection_required"


def test_positional_collections_lower_natively_and_page_by_root() -> None:
    session, person, employment, company, predicates = _connected_shape()
    base_collection = employment.collect()
    distinct_collection = company.collect().distinct()
    shaped = session.query(person, base_collection, distinct_collection).where(*predicates)

    assert isinstance(shaped, Query)
    assert base_collection is not base_collection.distinct()
    _assert_connection_required(lambda: shaped.page_by(person, limit=20, include_total=True))

    non_first_root = session.query(
        employment.collect(), person, company.collect().distinct()
    ).where(*predicates)
    _assert_connection_required(lambda: non_first_root.page_by(person, limit=20))

    for terminal in (shaped.one, lambda: shaped.rows(limit=20)):
        with pytest.raises(type_bridge_core.MatchRequestError) as rejected:
            terminal()
        assert rejected.value.category == "invalid_plan"
        assert rejected.value.code == "collection_requires_page_root"


def test_named_frozen_dataclass_and_named_tuple_preserve_one_query() -> None:
    session, person, employment, company, predicates = _connected_shape()
    scalar = session.query_as(PersonRow, person=person)
    assert isinstance(scalar, Query)
    _assert_connection_required(scalar.one)
    _assert_connection_required(lambda: scalar.rows(limit=1))

    named = session.query_as(
        PersonWork,
        person=person,
        employments=employment.collect(),
        companies=company.collect().distinct(),
    ).where(*predicates)
    named_tuple = session.query_as(
        PersonWorkTuple,
        person=person,
        employments=employment.collect(),
        companies=company.collect().distinct(),
    ).where(*predicates)

    assert named._row_declaration() is PersonWork
    assert named_tuple._row_declaration() is PersonWorkTuple
    _assert_connection_required(lambda: named.page_by(person, limit=10))
    _assert_connection_required(lambda: named_tuple.page_by(person, limit=10, include_total=True))


def test_named_declarations_reject_kind_names_order_and_annotations() -> None:
    session, person, employment, company, _ = _connected_shape()

    with pytest.raises(TypeError, match="must be frozen"):
        session.query_as(MutablePersonRow, person=person)
    with pytest.raises(TypeError, match="frozen dataclass or NamedTuple"):
        session.query_as(UnsupportedRow, person=person)
    with pytest.raises(type_bridge_core.MatchRequestError) as wrong_descriptor:
        session.query_as(WrongPersonRow, person=person)
    assert wrong_descriptor.value.code == "named_declaration_descriptor_mismatch"
    with pytest.raises(TypeError, match="annotation must be Model"):
        session.query_as(OptionalPersonRow, person=person)
    with pytest.raises(TypeError, match="annotation must be Model"):
        session.query_as(ListCollectionRow, employments=employment.collect())
    with pytest.raises(type_bridge_core.MatchRequestError) as wrong_cardinality:
        session.query_as(
            WrongCardinalityRow,
            employment=employment.collect(),
        )
    assert wrong_cardinality.value.code == "named_declaration_cardinality_mismatch"
    with pytest.raises(type_bridge_core.MatchRequestError) as unknown_descriptor:
        session.query_as(UnknownDescriptorRow, person=person)
    assert unknown_descriptor.value.code == "unknown_declared_descriptor"

    with pytest.raises(type_bridge_core.MatchRequestError) as missing:
        session.query_as(
            PersonWork,
            person=person,
            employments=employment.collect(),
        )
    assert missing.value.code == "named_declaration_length_mismatch"
    with pytest.raises(type_bridge_core.MatchRequestError) as extra:
        session.query_as(
            PersonWork,
            person=person,
            employments=employment.collect(),
            companies=company.collect(),
            extra=session.var(WorkEmployee).collect(),
        )
    assert extra.value.code == "named_declaration_length_mismatch"
    with pytest.raises(type_bridge_core.MatchRequestError) as order:
        session.query_as(
            PersonWork,
            person=person,
            companies=company.collect(),
            employments=employment.collect(),
        )
    assert order.value.code == "named_declaration_name_mismatch"


def test_named_declaration_rejects_unrelated_same_label_python_models() -> None:
    session = diagnostic_session()
    person = session.var(WorkPerson)

    with pytest.raises(
        TypeError,
        match="annotation WorkPersonCollision.*selection model WorkPerson",
    ):
        session.query_as(CollisionPersonRow, person=person)
    with pytest.raises(
        TypeError,
        match="annotation WorkPersonCollision.*selection model WorkPerson",
    ):
        session.query_as(CollisionPeopleRow, people=person.collect())

    assert session._model_constructors()[WorkPerson.get_type_name()] is WorkPerson


def test_named_declaration_accepts_nominal_root_for_subtype_inclusive_selection() -> None:
    session = diagnostic_session()
    person = session.subtypes(WorkPerson)

    query = session.query_as(PersonRow, person=person)

    _assert_connection_required(query.one)


def test_empty_duplicate_name_and_duplicate_selection_fail_in_native_handles() -> None:
    session = diagnostic_session()
    person = session.var(WorkPerson)
    company = session.var(WorkCompany)

    with pytest.raises(type_bridge_core.MatchRequestError) as empty:
        session.query_as(EmptyRow)
    assert empty.value.code == "empty_output"

    with pytest.raises(type_bridge_core.MatchRequestError) as duplicate_selection:
        session.query_as(
            DuplicateSelectionRow,
            person=person,
            people=person.collect(),
        )
    assert duplicate_selection.value.code == "duplicate_selection"

    with pytest.raises(type_bridge_core.MatchRequestError) as duplicate_name:
        session._native_session().named(
            ["member", "member"],
            [person._native_selection(), company._native_selection()],
        )
    assert duplicate_name.value.code == "duplicate_output_name"


def test_page_shape_rejects_singular_non_root_and_collected_root() -> None:
    session, person, employment, company, predicates = _connected_shape()
    singular_non_root = session.query(person, company).match(employment).where(*predicates)
    with pytest.raises(type_bridge_core.MatchRequestError) as singular:
        invoke_untyped(runtime_attribute(singular_non_root, "page_by"), person, limit=10)
    assert (singular.value.category, singular.value.code) == corpus_error(
        "shape.page-non-root-singular"
    )

    collected_root = session.query(person.collect())
    with pytest.raises(type_bridge_core.MatchRequestError) as collected:
        collected_root.page_by(person, limit=10)
    assert collected.value.code == "collected_page_root"


def test_query_collection_and_model_registry_metadata_are_immutable() -> None:
    session = diagnostic_session()
    relation = session.var(WorkEmployment)
    collection = relation.collect()
    query = session.query(relation)

    with pytest.raises(AttributeError, match="immutable"):
        setattr(query, "_Query__handle", query._native_query())
    with pytest.raises(AttributeError, match="immutable"):
        setattr(collection, "_Collected__handle", collection._native_selection())

    constructors = session._model_constructors()
    assert constructors[WorkPerson.get_type_name()] is WorkPerson
    assert constructors[WorkEmployee.get_type_name()] is WorkEmployee
    assert constructors[WorkCompany.get_type_name()] is WorkCompany
    assert constructors[WorkEmployment.get_type_name()] is WorkEmployment
    assert query._model_constructors()[WorkPerson.get_type_name()] is WorkPerson
    assert query._row_declaration() is None
    with pytest.raises(TypeError):
        invoke_untyped(setitem, constructors, "forged", WorkPerson)


def test_subtype_bindings_register_loaded_concrete_constructor_closure() -> None:
    session = diagnostic_session()
    person = session.var(WorkPerson, subtypes=True)

    constructors = session._model_constructors()
    assert constructors[WorkPerson.get_type_name()] is WorkPerson
    assert constructors[WorkEmployee.get_type_name()] is WorkEmployee
    assert session.query(person)._model_constructors()[WorkEmployee.get_type_name()] is WorkEmployee


def test_descriptor_closure_never_registers_framework_model_roots() -> None:
    session = diagnostic_session()
    session.var(WorkPerson)
    session.var(WorkEmployment)

    constructors = session._model_constructors()
    assert "Entity" not in constructors
    assert "Relation" not in constructors
    assert "TypeDBType" not in constructors
    snapshot = session._native_registry().snapshot()
    assert {descriptor["descriptor"]["type_name"] for descriptor in snapshot} == {
        "typed-work-person",
        "typed-work-employee",
        "typed-work-company",
        "typed-work-employment",
    }


def test_query_session_rejects_python_only_entity_and_relation_roots() -> None:
    with pytest.raises(TypeError, match="Python-only base=True model"):
        diagnostic_session().var(PythonOnlyWork)
    with pytest.raises(TypeError, match="Python-only base=True model"):
        diagnostic_session().subtypes(PythonOnlyWorkRelation)


def test_query_session_accepts_shallow_nested_relation_player_plan() -> None:
    session = diagnostic_session()
    envelope = session.var(WorkEnvelope)
    nested = session.var(WorkNestedRelation)
    connected = envelope.role(WorkEnvelope.nested).connects(nested)

    query = session.query(envelope).match(nested).where(connected)

    _assert_connection_required(query.one)


def test_existing_query_constructor_cannot_be_replaced_by_same_label_class() -> None:
    session = diagnostic_session()
    person = session.var(WorkPerson)
    query = session.query(person)

    with pytest.raises(TypeError, match="model constructor collision"):
        session.var(WorkPersonCollision)

    assert session._model_constructors()[WorkPerson.get_type_name()] is WorkPerson
    assert query._model_constructors()[WorkPerson.get_type_name()] is WorkPerson
    assert session.var(WorkPerson)


def test_page_envelope_keeps_collections_and_totals_immutable() -> None:
    item = ("person", ("employment",), ("company",))
    page = Page([item], offset=0, limit=10)
    with_total = Page([item], offset=0, limit=10, total=2**63 + 7)

    assert page.items == (item,)
    assert page.total is None
    assert with_total.total == 2**63 + 7
