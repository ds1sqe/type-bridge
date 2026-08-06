"""High-level model construction from fine-grained validated-result accessors."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import FrozenInstanceError, dataclass
from typing import NamedTuple, Protocol, runtime_checkable

import pytest
import type_bridge_core

import type_bridge.typed.results as typed_results
from tests.utils.handwritten import (
    Card,
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    String,
    TypeDBType,
    TypeFlags,
)
from type_bridge.typed.page import Page
from type_bridge.typed.results import TypedQueryMaterializationError


class ResultName(String):
    pass


class ResultTag(String):
    pass


class ResultIdentity(String):
    pass


class ResultPerson(Entity):
    flags = TypeFlags(name="result-person")
    name: ResultName = Flag(Key)
    tags: list[ResultTag] = Flag(Card(min=0))


class ResultEmployee(ResultPerson):
    flags = TypeFlags(name="result-employee")


class ResultCompany(Entity):
    flags = TypeFlags(name="result-company")
    name: ResultName = Flag(Key)


class ResultBrokenIid(Entity):
    flags = TypeFlags(name="result-broken-iid")
    name: ResultName = Flag(Key)

    def _set_backend_iid(self, iid: str | None) -> None:
        del iid


class ResultEmployment(Relation):
    flags = TypeFlags(name="result-employment")
    identifier: ResultIdentity = Flag(Key)
    employee: Role[ResultPerson] = Role("employee", ResultPerson)
    reviewers: Role[ResultPerson] = Role("reviewer", ResultPerson, cardinality=Card(min=0))
    employer: Role[ResultCompany] = Role("employer", ResultCompany)


class ResultNestedRelation(Relation):
    flags = TypeFlags(name="result-nested-relation")
    identifier: ResultIdentity = Flag(Key)
    member: Role[ResultPerson] = Role("member", ResultPerson)


class ResultEnvelope(Relation):
    flags = TypeFlags(name="result-envelope")
    identifier: ResultIdentity = Flag(Key)
    nested: Role[ResultNestedRelation] = Role("nested", ResultNestedRelation)


@dataclass(frozen=True, slots=True)
class NamedEmployment:
    person: ResultPerson
    employment: ResultEmployment


class TupleEmployment(NamedTuple):
    person: ResultPerson
    employment: ResultEmployment


@dataclass(frozen=True, slots=True)
class NamedEmploymentPage:
    person: ResultPerson
    employments: tuple[ResultEmployment, ...]


@runtime_checkable
class _NativeResultView(Protocol):
    """Structural boundary implemented by the native handle and test double."""

    def row_count(self) -> int: ...

    def row(self, index: int) -> object: ...

    def page_entry_count(self) -> int: ...

    def page_entry(self, index: int) -> object: ...

    def page_offset(self) -> int: ...

    def page_limit(self) -> int: ...

    def page_total(self) -> int | None: ...

    def count_value(self) -> object: ...

    def exists_value(self) -> object: ...


@runtime_checkable
class _OneMaterializer(Protocol):
    def __call__(
        self,
        result: _NativeResultView,
        models: Mapping[str, type[TypeDBType]],
        declaration: type[object] | None,
    ) -> object: ...


@runtime_checkable
class _RowsMaterializer(Protocol):
    def __call__(
        self,
        result: _NativeResultView,
        models: Mapping[str, type[TypeDBType]],
        declaration: type[object] | None,
    ) -> list[object]: ...


@runtime_checkable
class _PageMaterializer(Protocol):
    def __call__(
        self,
        result: _NativeResultView,
        models: Mapping[str, type[TypeDBType]],
        declaration: type[object] | None,
    ) -> Page[object]: ...


@runtime_checkable
class _CountMaterializer(Protocol):
    def __call__(self, result: _NativeResultView) -> int: ...


@runtime_checkable
class _ExistsMaterializer(Protocol):
    def __call__(self, result: _NativeResultView) -> bool: ...


@runtime_checkable
class _PersonRowMutationTarget(Protocol):
    """Writable structural view used to exercise runtime immutability."""

    person: ResultPerson


def _checked_instance[ValueT](value: object, expected: type[ValueT]) -> ValueT:
    if not isinstance(value, expected):
        raise AssertionError(f"expected {expected.__name__}, got {type(value).__name__}")
    return value


_materialize_one = _checked_instance(typed_results._materialize_one, _OneMaterializer)
_materialize_rows = _checked_instance(typed_results._materialize_rows, _RowsMaterializer)
_materialize_page = _checked_instance(typed_results._materialize_page, _PageMaterializer)
_materialize_count = _checked_instance(typed_results._materialize_count, _CountMaterializer)
_materialize_exists = _checked_instance(typed_results._materialize_exists, _ExistsMaterializer)


def _checked_pair(value: object) -> tuple[object, object]:
    if not isinstance(value, tuple) or len(value) != 2:
        raise AssertionError("expected a two-member positional result")
    return value[0], value[1]


def _checked_list[ValueT](value: object, item_type: type[ValueT]) -> list[ValueT]:
    if not isinstance(value, list):
        raise AssertionError(f"expected list, got {type(value).__name__}")
    checked: list[ValueT] = []
    for item in value:
        checked.append(_checked_instance(item, item_type))
    return checked


def _checked_tuple[ValueT](value: object, item_type: type[ValueT]) -> tuple[ValueT, ...]:
    if not isinstance(value, tuple):
        raise AssertionError(f"expected tuple, got {type(value).__name__}")
    return tuple(_checked_instance(item, item_type) for item in value)


def _mutation_target(
    value: object,
    expected_type: type[object],
) -> _PersonRowMutationTarget:
    if type(value) is not expected_type:
        raise AssertionError(f"expected exact {expected_type.__name__}, got {type(value).__name__}")
    if not isinstance(value, _PersonRowMutationTarget):
        raise AssertionError("materialized row has no person member")
    return value


class _Attribute:
    def __init__(self, field_name: str, *values: tuple[str, object]) -> None:
        self._field_name = field_name
        self._values = values

    def field_name(self) -> str:
        return self._field_name

    def value_count(self) -> int:
        return len(self._values)

    def value_type(self, index: int) -> str:
        return self._values[index][0]

    def value(self, index: int) -> object:
        return self._values[index][1]


class _RolePlayer:
    def __init__(
        self,
        iid: str,
        declared: str,
        concrete: str,
        kind: str,
        attributes: list[_Attribute],
    ) -> None:
        self._iid = iid
        self._declared = declared
        self._concrete = concrete
        self._kind = kind
        self._attributes = attributes

    def iid(self) -> str:
        return self._iid

    def declared_type_name(self) -> str:
        return self._declared

    def concrete_type_name(self) -> str:
        return self._concrete

    def kind(self) -> str:
        return self._kind

    def attribute_count(self) -> int:
        return len(self._attributes)

    def attribute(self, index: int) -> _Attribute:
        return self._attributes[index]


class _Role:
    def __init__(self, name: str, players: list[_RolePlayer]) -> None:
        self._name = name
        self._players = players

    def role_name(self) -> str:
        return self._name

    def player_count(self) -> int:
        return len(self._players)

    def player(self, index: int) -> _RolePlayer:
        return self._players[index]


class _Thing(_RolePlayer):
    def __init__(
        self,
        iid: str,
        declared: str,
        concrete: str,
        kind: str,
        attributes: list[_Attribute],
        roles: list[_Role] | None = None,
    ) -> None:
        super().__init__(iid, declared, concrete, kind, attributes)
        self._roles = roles or []

    def role_count(self) -> int:
        return len(self._roles)

    def role(self, index: int) -> _Role:
        return self._roles[index]


class _Slot:
    def __init__(self, thing: _Thing, name: str | None = None) -> None:
        self._thing = thing
        self._name = name

    def name(self) -> str | None:
        return self._name

    def is_collection(self) -> bool:
        return False

    def thing_count(self) -> int:
        return 1

    def thing(self, index: int) -> _Thing:
        if index != 0:
            raise IndexError(index)
        return self._thing


class _CollectionSlot:
    def __init__(self, things: list[_Thing], name: str | None = None) -> None:
        self._things = things
        self._name = name

    def name(self) -> str | None:
        return self._name

    def is_collection(self) -> bool:
        return True

    def thing_count(self) -> int:
        return len(self._things)

    def thing(self, index: int) -> _Thing:
        return self._things[index]


class _Row:
    def __init__(self, slots: list[_Slot | _CollectionSlot]) -> None:
        self._slots = slots

    def slot_count(self) -> int:
        return len(self._slots)

    def slot(self, index: int) -> _Slot | _CollectionSlot:
        return self._slots[index]


class _Result:
    def __init__(
        self,
        rows: list[_Row],
        *,
        offset: int = 0,
        limit: int = 1,
        total: int | None = None,
        count: object = 0,
        exists: object = False,
    ) -> None:
        self._rows = rows
        self._offset = offset
        self._limit = limit
        self._total = total
        self._count = count
        self._exists = exists

    def row_count(self) -> int:
        return len(self._rows)

    def row(self, index: int) -> _Row:
        return self._rows[index]

    def page_entry_count(self) -> int:
        return len(self._rows)

    def page_entry(self, index: int) -> _Row:
        return self._rows[index]

    def page_offset(self) -> int:
        return self._offset

    def page_limit(self) -> int:
        return self._limit

    def page_total(self) -> int | None:
        return self._total

    def count_value(self) -> object:
        return self._count

    def exists_value(self) -> object:
        return self._exists


def _person(
    iid: str = "0x01",
    *,
    declared: str = "result-person",
    concrete: str = "result-employee",
) -> _Thing:
    return _Thing(
        iid,
        declared,
        concrete,
        "entity",
        [
            _Attribute("name", ("string", "Alice")),
            _Attribute("tags", ("string", "engineer"), ("string", "reviewer")),
        ],
    )


def _company(iid: str = "0x02") -> _RolePlayer:
    return _RolePlayer(
        iid,
        "result-company",
        "result-company",
        "entity",
        [_Attribute("name", ("string", "Acme"))],
    )


def _employment(iid: str = "0x10") -> _Thing:
    employee = _person()
    reviewer = _person("0x03")
    return _Thing(
        iid,
        "result-employment",
        "result-employment",
        "relation",
        [_Attribute("identifier", ("string", "employment-1"))],
        [
            _Role("employee", [employee]),
            _Role("reviewer", [reviewer, reviewer]),
            _Role("employer", [_company()]),
        ],
    )


def _models() -> dict[str, type[TypeDBType]]:
    return {
        model.get_type_name(): model
        for model in (
            ResultPerson,
            ResultEmployee,
            ResultCompany,
            ResultBrokenIid,
            ResultEmployment,
            ResultNestedRelation,
            ResultEnvelope,
        )
    }


def _native(result: _Result) -> _NativeResultView:
    if not isinstance(result, _NativeResultView):
        raise AssertionError("test result double does not implement the native result view")
    return result


def test_packaged_result_views_are_nonconstructible_and_operation_specific() -> None:
    for name in (
        "ValidatedMatchResultHandle",
        "ValidatedMatchRowHandle",
        "ValidatedMatchSlotHandle",
        "ValidatedMatchThingHandle",
        "ValidatedMatchAttributeHandle",
        "ValidatedMatchRoleHandle",
        "ValidatedMatchRolePlayerHandle",
    ):
        with pytest.raises(TypeError):
            getattr(type_bridge_core, name)()

    methods = set(dir(type_bridge_core.ValidatedMatchResultHandle))
    assert {
        "row_count",
        "row",
        "page_entry_count",
        "page_entry",
        "page_offset",
        "page_limit",
        "page_total",
        "count_value",
        "exists_value",
    } <= methods


def test_result_materializers_are_not_public_module_exports() -> None:
    assert typed_results.__all__ == ["TypedQueryMaterializationError"]
    for name in (
        "materialize_count",
        "materialize_exists",
        "materialize_one",
        "materialize_page",
        "materialize_rows",
    ):
        assert not hasattr(typed_results, name)


def test_scalar_uses_concrete_subclass_attributes_and_iid() -> None:
    person = _materialize_one(_native(_Result([_Row([_Slot(_person())])])), _models(), None)

    assert type(person) is ResultEmployee
    assert person._iid == "0x01"
    assert person.name == ResultName("Alice")
    assert person.tags == [ResultTag("engineer"), ResultTag("reviewer")]


def test_positional_rows_preserve_order_and_do_not_share_model_instances() -> None:
    thing = _person()
    result = _Result([_Row([_Slot(thing), _Slot(thing)])])

    rows = _materialize_rows(_native(result), _models(), None)
    left_value, right_value = _checked_pair(rows[0])
    left = _checked_instance(left_value, ResultEmployee)
    right = _checked_instance(right_value, ResultEmployee)

    assert left is not right
    assert left._iid == right._iid == "0x01"
    left.name = ResultName("Changed")
    assert right.name == ResultName("Alice")


def test_relation_materializes_complete_roles_subtypes_and_multiplicity() -> None:
    relation = _checked_instance(
        _materialize_one(
            _native(_Result([_Row([_Slot(_employment())])])),
            _models(),
            None,
        ),
        ResultEmployment,
    )

    assert type(relation) is ResultEmployment
    assert relation._iid == "0x10"
    assert relation.identifier == ResultIdentity("employment-1")
    assert type(relation.employee) is ResultEmployee
    assert relation.employee._iid == "0x01"
    assert relation.employer._iid == "0x02"
    reviewers = _checked_list(relation.reviewers, ResultPerson)
    assert [reviewer._iid for reviewer in reviewers] == ["0x03", "0x03"]
    assert reviewers[0] is not reviewers[1]


@pytest.mark.parametrize("declaration", [NamedEmployment, TupleEmployment])
def test_named_rows_keep_exact_declared_immutable_shape(declaration: type[object]) -> None:
    result = _Result(
        [
            _Row(
                [
                    _Slot(_person(), "person"),
                    _Slot(_employment(), "employment"),
                ]
            )
        ]
    )

    named = _mutation_target(
        _materialize_one(_native(result), _models(), declaration),
        declaration,
    )

    assert type(named) is declaration
    assert type(named.person) is ResultEmployee
    with pytest.raises((FrozenInstanceError, AttributeError)):
        named.person = ResultPerson(name=ResultName("Other"), tags=[])


def test_page_preserves_collections_window_total_and_named_immutable_shape() -> None:
    positional = _Result(
        [
            _Row(
                [
                    _Slot(_person()),
                    _CollectionSlot([_employment("0x10"), _employment("0x11")]),
                ]
            )
        ],
        offset=7,
        limit=20,
        total=2**63 + 9,
    )

    page = _materialize_page(_native(positional), _models(), None)

    person_value, employments_value = _checked_pair(page.items[0])
    person = _checked_instance(person_value, ResultEmployee)
    employments = _checked_tuple(employments_value, ResultEmployment)
    assert type(person) is ResultEmployee
    assert [employment._iid for employment in employments] == ["0x10", "0x11"]
    assert page.offset == 7
    assert page.limit == 20
    assert page.total == 2**63 + 9
    assert isinstance(page.items, tuple)
    assert isinstance(employments, tuple)

    named = _Result(
        [
            _Row(
                [
                    _Slot(_person(), "person"),
                    _CollectionSlot([_employment()], "employments"),
                ]
            )
        ],
        limit=10,
    )
    named_page = _materialize_page(_native(named), _models(), NamedEmploymentPage)
    named_item = _mutation_target(named_page.items[0], NamedEmploymentPage)
    assert isinstance(named_item, NamedEmploymentPage)
    assert isinstance(named_item.employments, tuple)
    with pytest.raises(FrozenInstanceError):
        named_item.person = ResultPerson(name=ResultName("Other"), tags=[])


def test_count_and_exists_materializers_retain_lossless_scalar_types() -> None:
    assert _materialize_count(_native(_Result([], count=2**64 - 1))) == 2**64 - 1
    assert _materialize_exists(_native(_Result([], exists=True))) is True

    with pytest.raises(TypedQueryMaterializationError) as count:
        _materialize_count(_native(_Result([], count=True)))
    assert count.value.code == "invalid_count_value"

    with pytest.raises(TypedQueryMaterializationError) as exists:
        _materialize_exists(_native(_Result([], exists=1)))
    assert exists.value.code == "invalid_exists_value"


def test_materializer_fails_closed_on_constructor_field_and_value_type_gaps() -> None:
    missing_constructor = _Result([_Row([_Slot(_person(concrete="result-unregistered"))])])
    with pytest.raises(TypedQueryMaterializationError) as constructor:
        _materialize_one(_native(missing_constructor), _models(), None)
    assert constructor.value.code == "missing_concrete_model_constructor"

    missing_field = _person()
    missing_field._attributes.append(_Attribute("forged", ("string", "value")))
    with pytest.raises(TypedQueryMaterializationError) as field:
        _materialize_one(_native(_Result([_Row([_Slot(missing_field)])])), _models(), None)
    assert field.value.code == "missing_model_field"

    wrong_type = _person()
    wrong_type._attributes[0] = _Attribute("name", ("long", 7))
    with pytest.raises(TypedQueryMaterializationError) as value_type:
        _materialize_one(_native(_Result([_Row([_Slot(wrong_type)])])), _models(), None)
    assert value_type.value.code == "model_field_type_mismatch"

    wrong_declared = _models()
    wrong_declared[ResultPerson.get_type_name()] = ResultCompany
    with pytest.raises(TypedQueryMaterializationError) as declared:
        _materialize_one(_native(_Result([_Row([_Slot(_person())])])), wrong_declared, None)
    assert declared.value.code == "declared_model_name_mismatch"


def test_materializer_fails_if_constructor_does_not_retain_validated_iid() -> None:
    thing = _Thing(
        "0x99",
        "result-broken-iid",
        "result-broken-iid",
        "entity",
        [_Attribute("name", ("string", "Broken"))],
    )

    with pytest.raises(TypedQueryMaterializationError) as raised:
        _materialize_one(_native(_Result([_Row([_Slot(thing)])])), _models(), None)
    assert raised.value.code == "model_iid_assignment_mismatch"


def test_nested_relation_role_player_fails_closed_during_materialization() -> None:
    nested = _RolePlayer(
        "0x20",
        "result-nested-relation",
        "result-nested-relation",
        "relation",
        [_Attribute("identifier", ("string", "nested-1"))],
    )
    relation = _Thing(
        "0x30",
        "result-envelope",
        "result-envelope",
        "relation",
        [_Attribute("identifier", ("string", "envelope-1"))],
        [_Role("nested", [nested])],
    )

    with pytest.raises(TypedQueryMaterializationError) as raised:
        _materialize_one(_native(_Result([_Row([_Slot(relation)])])), _models(), None)

    assert raised.value.code == "nested_relation_role_player_unsupported"
