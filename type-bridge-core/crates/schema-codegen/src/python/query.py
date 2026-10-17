"""Projection-owned query references emitted with a generated Python package."""

from __future__ import annotations

import asyncio
import json
import warnings
from collections.abc import Awaitable, Callable, Iterable, Iterator, Sequence
from dataclasses import fields, is_dataclass
from datetime import date, datetime, timedelta
from decimal import Decimal
from os import PathLike
from typing import Literal, Protocol, TypeGuard, get_args, get_origin, get_type_hints, overload

from type_bridge_core import (
    DynamicValue,
    MatchBindingHandle,
    MatchFieldHandle,
    MatchFunctionArgumentHandle,
    MatchFunctionCallHandle,
    MatchFunctionValueHandle,
    MatchOrderHandle,
    MatchPredicateHandle,
    MatchQueryHandle,
    MatchRoleHandle,
    MatchSelectionHandle,
    MatchSessionHandle,
    PendingRemoteModelQuery,
    ProxyConnection,
    PyRuntimeProjection,
    PyRustDatabase,
    PyRustTransactionContext,
    QueryCancellation,
    QueryExecutionResourceLimits,
    QueryLimits,
    RemoteModelQueryContext,
    ValidatedMatchResultHandle,
    ValidatedMatchRowHandle,
    query_v2_authority_from_schema_authority,
    query_v2_prepare_remote_model_count,
    query_v2_prepare_remote_model_exists,
    query_v2_prepare_remote_model_page,
    query_v2_prepare_remote_model_reduce,
    query_v2_prepare_remote_model_reduce_by_field,
    query_v2_prepare_remote_model_reduce_by_fields,
    query_v2_prepare_remote_model_rows,
    query_v2_remote_model_context_with_resources,
)
from type_bridge_core import (
    QueryV2Authority as _QueryV2Authority,
)

from type_bridge._rust_runtime import rust_database_for, rust_transaction_for
from type_bridge.session import Database, TransactionContext

from ._authority import SCHEMA_AUTHORITY_BYTES
from ._runtime import (
    AttributeBase,
    EntityBase,
    FieldToken,
    FunctionRef,
    ModelBase,
    RelationBase,
    RoleToken,
    attribute_model_for_query_label,
    function_identity_for_query,
)
from ._schema import SEMANTIC_SCHEMA_FINGERPRINT_JSON


def _model_label(model: type[ModelBase]) -> str:
    identity: object = json.loads(model.__type_id__)
    if not _is_object_dict(identity):
        raise TypeError("generated model identity must be an object")
    label = identity.get("label")
    if not isinstance(label, str):
        raise TypeError("generated model identity has no string label")
    return label


def _exact_projection(model: type[ModelBase]) -> PyRuntimeProjection:
    projection = model.__dict__.get("__runtime_projection__")
    if not isinstance(projection, PyRuntimeProjection):
        raise TypeError("generated query requires an exact installed generated model class")
    return projection


_package_projection: PyRuntimeProjection | None = None
_package_query_authority: _QueryV2Authority = query_v2_authority_from_schema_authority(
    SCHEMA_AUTHORITY_BYTES,
    SEMANTIC_SCHEMA_FINGERPRINT_JSON.encode("utf-8"),
)


def install_projection(projection: PyRuntimeProjection) -> None:
    global _package_projection
    if _package_projection is not None:
        raise RuntimeError("generated query projection is already installed")
    _package_projection = projection


def _installed_projection() -> PyRuntimeProjection:
    if _package_projection is None:
        raise RuntimeError("generated package runtime projection is not installed")
    return _package_projection


def _installed_query_authority() -> _QueryV2Authority:
    return _package_query_authority


def _attribute_model(label: str) -> type[AttributeBase]:
    return attribute_model_for_query_label(label)


class _Frozen:
    __slots__ = ()

    def __setattr__(self, name: str, value: object) -> None:
        del name, value
        raise AttributeError(f"{type(self).__name__} values are immutable")


class Predicate(_Frozen):
    __slots__ = ("__handle", "__projection")
    __handle: MatchPredicateHandle
    __projection: PyRuntimeProjection

    def __init__(self, handle: MatchPredicateHandle, projection: PyRuntimeProjection) -> None:
        object.__setattr__(self, "_Predicate__handle", handle)
        object.__setattr__(self, "_Predicate__projection", projection)

    def and_(self, other: Predicate) -> Predicate:
        _require_projection(other.projection_identity(), self.__projection, "predicate")
        return Predicate(self.__handle.and_(other.native_handle()), self.__projection)

    def or_(self, other: Predicate) -> Predicate:
        _require_projection(other.projection_identity(), self.__projection, "predicate")
        return Predicate(self.__handle.or_(other.native_handle()), self.__projection)

    def not_(self) -> Predicate:
        return Predicate(self.__handle.not_(), self.__projection)

    def __and__(self, other: Predicate) -> Predicate:
        return self.and_(other)

    def __or__(self, other: Predicate) -> Predicate:
        return self.or_(other)

    def __invert__(self) -> Predicate:
        return self.not_()

    def native_handle(self) -> MatchPredicateHandle:
        return self.__handle

    def projection_identity(self) -> PyRuntimeProjection:
        return self.__projection


class FunctionInput[DomainT](_Frozen):
    """One exact session/projection-branded scalar schema-function input."""

    __slots__ = ("__handle", "__projection")
    __handle: MatchFunctionValueHandle
    __projection: PyRuntimeProjection

    def __init__(
        self,
        handle: MatchFunctionValueHandle,
        projection: PyRuntimeProjection,
    ) -> None:
        object.__setattr__(self, "_FunctionInput__handle", handle)
        object.__setattr__(self, "_FunctionInput__projection", projection)

    def native_argument(self) -> MatchFunctionArgumentHandle:
        return self.__handle.function_argument()

    def native_handle(self) -> MatchFunctionValueHandle:
        return self.__handle

    def projection_identity(self) -> PyRuntimeProjection:
        return self.__projection


class FunctionCall[DomainT](_Frozen):
    """One immutable scalar schema-function call."""

    __slots__ = ("__handle", "__projection")
    __handle: MatchFunctionCallHandle
    __projection: PyRuntimeProjection

    def __init__(
        self,
        handle: MatchFunctionCallHandle,
        projection: PyRuntimeProjection,
    ) -> None:
        object.__setattr__(self, "_FunctionCall__handle", handle)
        object.__setattr__(self, "_FunctionCall__projection", projection)

    def eq_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("equal", field)

    def neq_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("not_equal", field)

    def lt_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("less_than", field)

    def lte_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("less_than_or_equal", field)

    def gt_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("greater_than", field)

    def gte_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("greater_than_or_equal", field)

    def eq_value(self, value: FunctionInput[DomainT]) -> Predicate:
        return self.__compare_value("equal", value)

    def neq_value(self, value: FunctionInput[DomainT]) -> Predicate:
        return self.__compare_value("not_equal", value)

    def lt_value(self, value: FunctionInput[DomainT]) -> Predicate:
        return self.__compare_value("less_than", value)

    def lte_value(self, value: FunctionInput[DomainT]) -> Predicate:
        return self.__compare_value("less_than_or_equal", value)

    def gt_value(self, value: FunctionInput[DomainT]) -> Predicate:
        return self.__compare_value("greater_than", value)

    def gte_value(self, value: FunctionInput[DomainT]) -> Predicate:
        return self.__compare_value("greater_than_or_equal", value)

    def eq_call(self, other: FunctionCall[DomainT]) -> Predicate:
        return self.__compare_call("equal", other)

    def neq_call(self, other: FunctionCall[DomainT]) -> Predicate:
        return self.__compare_call("not_equal", other)

    def lt_call(self, other: FunctionCall[DomainT]) -> Predicate:
        return self.__compare_call("less_than", other)

    def lte_call(self, other: FunctionCall[DomainT]) -> Predicate:
        return self.__compare_call("less_than_or_equal", other)

    def gt_call(self, other: FunctionCall[DomainT]) -> Predicate:
        return self.__compare_call("greater_than", other)

    def gte_call(self, other: FunctionCall[DomainT]) -> Predicate:
        return self.__compare_call("greater_than_or_equal", other)

    def native_argument(self) -> MatchFunctionArgumentHandle:
        return self.__handle.function_argument()

    def native_handle(self) -> MatchFunctionCallHandle:
        return self.__handle

    def projection_identity(self) -> PyRuntimeProjection:
        return self.__projection

    def __compare_field(self, operator: str, field: object) -> Predicate:
        if not isinstance(field, BoundField):
            raise TypeError("schema-function field comparison requires a BoundField")
        _require_projection(field.projection_identity(), self.__projection, "function field")
        return Predicate(
            self.__handle.compare_field(operator, field.native_handle()),
            self.__projection,
        )

    def __compare_value(self, operator: str, value: FunctionInput[DomainT]) -> Predicate:
        if type(value) is not FunctionInput:
            raise TypeError("schema-function value comparison requires a FunctionInput")
        _require_projection(value.projection_identity(), self.__projection, "function value")
        return Predicate(
            self.__handle.compare_value(operator, value.native_handle()),
            self.__projection,
        )

    def __compare_call(self, operator: str, other: FunctionCall[DomainT]) -> Predicate:
        if type(other) is not FunctionCall:
            raise TypeError("schema-function call comparison requires a FunctionCall")
        _require_projection(other.projection_identity(), self.__projection, "function call")
        return Predicate(
            self.__handle.compare_call(operator, other.native_handle()),
            self.__projection,
        )


class QueryOrder(_Frozen):
    __slots__ = ("__handle", "__projection")
    __handle: MatchOrderHandle
    __projection: PyRuntimeProjection

    def __init__(self, handle: MatchOrderHandle, projection: PyRuntimeProjection) -> None:
        object.__setattr__(self, "_QueryOrder__handle", handle)
        object.__setattr__(self, "_QueryOrder__projection", projection)

    def native_handle(self) -> MatchOrderHandle:
        return self.__handle

    def projection_identity(self) -> PyRuntimeProjection:
        return self.__projection


_AGGREGATE_TOKEN = object()
type _Reducer = Literal["count", "sum", "min", "max", "mean", "median", "std"]


class Aggregate(_Frozen):
    __slots__ = ("__input", "__projection", "__reducer")
    __input: MatchFieldHandle | None
    __projection: PyRuntimeProjection | None
    __reducer: _Reducer

    def __init__(
        self,
        reducer: _Reducer,
        input_: MatchFieldHandle | None,
        projection: PyRuntimeProjection | None,
        token: object,
    ) -> None:
        if token is not _AGGREGATE_TOKEN:
            raise TypeError("generated aggregate terms must come from aggregate constructors")
        object.__setattr__(self, "_Aggregate__reducer", reducer)
        object.__setattr__(self, "_Aggregate__input", input_)
        object.__setattr__(self, "_Aggregate__projection", projection)

    def native_reducer(self) -> _Reducer:
        return self.__reducer

    def native_input(self) -> MatchFieldHandle | None:
        return self.__input

    def projection_identity(self) -> PyRuntimeProjection | None:
        return self.__projection


class Page(_Frozen, Sequence[object]):
    __slots__ = ("__items", "__limit", "__offset", "__total")
    __items: tuple[object, ...]
    __limit: int
    __offset: int
    __total: int | None

    def __init__(
        self,
        items: Iterable[object],
        *,
        offset: int,
        limit: int,
        total: int | None,
    ) -> None:
        object.__setattr__(self, "_Page__items", tuple(items))
        object.__setattr__(self, "_Page__offset", offset)
        object.__setattr__(self, "_Page__limit", limit)
        object.__setattr__(self, "_Page__total", total)

    @property
    def items(self) -> tuple[object, ...]:
        return self.__items

    @property
    def offset(self) -> int:
        return self.__offset

    @property
    def limit(self) -> int:
        return self.__limit

    @property
    def total(self) -> int | None:
        return self.__total

    def __len__(self) -> int:
        return len(self.__items)

    @overload
    def __getitem__(self, index: int) -> object: ...

    @overload
    def __getitem__(self, index: slice) -> Sequence[object]: ...

    def __getitem__(self, index: int | slice) -> object | Sequence[object]:
        return self.__items[index]

    def __iter__(self) -> Iterator[object]:
        return iter(self.__items)


class BoundField(_Frozen):
    __slots__ = ("__attribute_label", "__attribute_model", "__handle", "__projection")
    __attribute_label: str
    __handle: MatchFieldHandle
    __projection: PyRuntimeProjection

    def __init__(
        self,
        handle: MatchFieldHandle,
        projection: PyRuntimeProjection,
        attribute_label: str,
        attribute_model: type[AttributeBase],
    ) -> None:
        object.__setattr__(self, "_BoundField__attribute_label", attribute_label)
        object.__setattr__(self, "_BoundField__attribute_model", attribute_model)
        object.__setattr__(self, "_BoundField__handle", handle)
        object.__setattr__(self, "_BoundField__projection", projection)

    def eq(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("equal", value)

    def eq_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("equal", field)

    def neq(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("not_equal", value)

    def neq_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("not_equal", field)

    def lt(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("less_than", value)

    def lt_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("less_than", field)

    def lte(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("less_than_or_equal", value)

    def lte_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("less_than_or_equal", field)

    def gt(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("greater_than", value)

    def gt_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("greater_than", field)

    def gte(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("greater_than_or_equal", value)

    def gte_field(self, field: BoundField) -> Predicate:
        return self.__compare_field("greater_than_or_equal", field)

    def contains(self, value: AttributeBase) -> Predicate:
        return self.__compare("contains", value)

    def starts_with(self, value: AttributeBase) -> Predicate:
        return self.__compare("starts_with", value)

    def ends_with(self, value: AttributeBase) -> Predicate:
        return self.__compare("ends_with", value)

    def regex(self, value: AttributeBase) -> Predicate:
        return self.__compare("regex", value)

    def is_present(self) -> Predicate:
        return Predicate(self.__handle.presence(True), self.__projection)

    def is_missing(self) -> Predicate:
        return Predicate(self.__handle.presence(False), self.__projection)

    def asc(self, *, missing: str = "reject") -> QueryOrder:
        return QueryOrder(self.__handle.order("ascending", missing), self.__projection)

    def desc(self, *, missing: str = "reject") -> QueryOrder:
        return QueryOrder(self.__handle.order("descending", missing), self.__projection)

    def __compare(self, operator: str, value: AttributeBase | BoundField) -> Predicate:
        if isinstance(value, BoundField):
            if value.attribute_label() != self.__attribute_label:
                raise TypeError(
                    "cross-attribute field comparisons require the explicit *_field method"
                )
            return self.__compare_field(operator, value)
        else:
            _require_projection(_exact_projection(type(value)), self.__projection, "attribute")
            if _model_label(type(value)) != self.__attribute_label:
                raise TypeError("generated field comparison requires its exact attribute wrapper")
            handle = self.__handle.compare_value(operator, _dynamic_value(value))
        return Predicate(handle, self.__projection)

    def __compare_field(self, operator: str, field: BoundField) -> Predicate:
        _require_projection(field.projection_identity(), self.__projection, "bound field")
        return Predicate(
            self.__handle.compare_field(operator, field.native_handle()),
            self.__projection,
        )

    def attribute_label(self) -> str:
        return self.__attribute_label

    def native_handle(self) -> MatchFieldHandle:
        return self.__handle

    def attribute_model(self) -> type[AttributeBase]:
        return self.__attribute_model

    def projection_identity(self) -> PyRuntimeProjection:
        return self.__projection


class _AggregateFactory(_Frozen):
    __slots__ = ()

    def count(self) -> Aggregate:
        return Aggregate("count", None, None, _AGGREGATE_TOKEN)

    def sum(self, field: BoundField) -> Aggregate:
        return self.__field("sum", field)

    def min(self, field: BoundField) -> Aggregate:
        return self.__field("min", field)

    def max(self, field: BoundField) -> Aggregate:
        return self.__field("max", field)

    def mean(self, field: BoundField) -> Aggregate:
        return self.__field("mean", field)

    def median(self, field: BoundField) -> Aggregate:
        return self.__field("median", field)

    def std(self, field: BoundField) -> Aggregate:
        return self.__field("std", field)

    def __field(
        self,
        reducer: Literal["sum", "min", "max", "mean", "median", "std"],
        field: BoundField,
    ) -> Aggregate:
        if type(field) is not BoundField:
            raise TypeError("generated reductions require an exact BoundField")
        return Aggregate(
            reducer,
            field.native_handle(),
            field.projection_identity(),
            _AGGREGATE_TOKEN,
        )


aggregate = _AggregateFactory()


class BoundRole(_Frozen):
    __slots__ = ("__accepted", "__handle", "__projection")
    __accepted: frozenset[str]
    __handle: MatchRoleHandle
    __projection: PyRuntimeProjection

    def __init__(
        self,
        handle: MatchRoleHandle,
        projection: PyRuntimeProjection,
        accepted: frozenset[str],
    ) -> None:
        object.__setattr__(self, "_BoundRole__accepted", accepted)
        object.__setattr__(self, "_BoundRole__handle", handle)
        object.__setattr__(self, "_BoundRole__projection", projection)

    def connects(self, player: BoundVar) -> Predicate:
        _require_projection(player.projection_identity(), self.__projection, "role player")
        if self.__accepted.isdisjoint(player.model_domain_labels()):
            raise TypeError("generated role does not accept this projected player type")
        return Predicate(self.__handle.connects(player.native_handle()), self.__projection)

    def is_(self, player: BoundVar) -> Predicate:
        return self.connects(player)


class BoundVar(_Frozen):
    __slots__ = ("__handle", "__model", "__projection", "__subtypes")
    __handle: MatchBindingHandle
    __model: type[ModelBase]
    __projection: PyRuntimeProjection

    def __init__(
        self,
        handle: MatchBindingHandle,
        model: type[ModelBase],
        projection: PyRuntimeProjection,
        *,
        subtypes: bool,
    ) -> None:
        object.__setattr__(self, "_BoundVar__handle", handle)
        object.__setattr__(self, "_BoundVar__model", model)
        object.__setattr__(self, "_BoundVar__projection", projection)
        object.__setattr__(self, "_BoundVar__subtypes", subtypes)

    @property
    def model(self) -> type[ModelBase]:
        return self.__model

    def field[OwnerT: ModelBase](self, token: FieldToken[OwnerT, AttributeBase]) -> BoundField:
        _require_projection(_exact_projection(token.owner), self.__projection, "field token")
        if token.owner is not self.__model:
            raise TypeError("generated field token owner does not match the bound model")
        identity = token.fact["id"]
        if not _is_object_dict(identity):
            raise TypeError("generated field token identity is invalid")
        owner = identity.get("owner")
        attribute = identity.get("attribute")
        if not _is_object_dict(owner) or not isinstance(attribute, str):
            raise TypeError("generated field token identity is invalid")
        owner_label = owner.get("label")
        if not isinstance(owner_label, str):
            raise TypeError("generated field token owner is invalid")
        return BoundField(
            self.__handle.field_owned_by(owner_label, attribute),
            self.__projection,
            attribute,
            _attribute_model(attribute),
        )

    def role[OwnerT: ModelBase, PlayerT: ModelBase](
        self,
        token: RoleToken[OwnerT, PlayerT, BoundVar],
    ) -> BoundRole:
        _require_projection(_exact_projection(token.owner), self.__projection, "role token")
        if token.owner is not self.__model:
            raise TypeError("generated role token owner does not match the bound model")
        role = token.fact["role"]
        if not _is_object_dict(role):
            raise TypeError("generated role token identity is invalid")
        owner_label = role.get("declaring_relation")
        role_label = role.get("label")
        if not isinstance(owner_label, str) or not isinstance(role_label, str):
            raise TypeError("generated role token identity is invalid")
        accepted_players = token.fact.get("accepted_players")
        if not _is_object_list(accepted_players):
            raise TypeError("generated role token player contract is invalid")
        accepted = frozenset(_type_id_label(player) for player in accepted_players)
        return BoundRole(
            self.__handle.role_owned_by(owner_label, role_label),
            self.__projection,
            accepted,
        )

    def iid(self, iid: str) -> Predicate:
        return Predicate(self.__handle.iid(iid), self.__projection)

    def iid_in(self, iids: Iterable[str]) -> Predicate:
        return Predicate(self.__handle.iid_in(list(iids)), self.__projection)

    def collect(self) -> Collected:
        return Collected(self.__handle.collect(), self.__model, self.__projection)

    def selection_handle(self) -> MatchSelectionHandle:
        return self.__handle.one()

    def native_handle(self) -> MatchBindingHandle:
        return self.__handle

    def projection_identity(self) -> PyRuntimeProjection:
        return self.__projection

    def model_domain_labels(self) -> frozenset[str]:
        if not self.__subtypes:
            return frozenset((_model_label(self.__model),))
        pending = [self.__model]
        labels: set[str] = set()
        while pending:
            candidate = pending.pop()
            if _exact_projection(candidate) is self.__projection:
                labels.add(_model_label(candidate))
            pending.extend(candidate.__subclasses__())
        return frozenset(labels)

    def selected_model(self) -> type[ModelBase]:
        return self.__model

    def is_collection(self) -> bool:
        return False


class SubtypeBoundVar(BoundVar):
    __slots__ = ()


def _require_aggregate_groups(
    candidates: tuple[object, ...],
    projection: PyRuntimeProjection,
    context: str,
) -> BoundVar | BoundField | tuple[BoundField, ...]:
    if not candidates:
        raise TypeError("aggregate grouping requires at least one generated group")
    if len(candidates) == 1 and isinstance(candidates[0], BoundVar):
        candidate = candidates[0]
        _require_projection(candidate.projection_identity(), projection, context)
        return candidate
    fields: list[BoundField] = []
    for candidate in candidates:
        if not isinstance(candidate, BoundField):
            raise TypeError(
                "aggregate grouping requires one BoundVar or one or more generated BoundFields"
            )
        _require_projection(candidate.projection_identity(), projection, f"{context} field")
        fields.append(candidate)
    if len(fields) == 1:
        return fields[0]
    return tuple(fields)


class Collected(_Frozen):
    __slots__ = ("__handle", "__model", "__projection")
    __handle: MatchSelectionHandle
    __model: type[ModelBase]
    __projection: PyRuntimeProjection

    def __init__(
        self,
        handle: MatchSelectionHandle,
        model: type[ModelBase],
        projection: PyRuntimeProjection,
    ) -> None:
        object.__setattr__(self, "_Collected__handle", handle)
        object.__setattr__(self, "_Collected__model", model)
        object.__setattr__(self, "_Collected__projection", projection)

    def distinct(self, enabled: bool = True) -> Collected:
        return Collected(self.__handle.distinct(enabled), self.__model, self.__projection)

    def order_by(self, order: QueryOrder) -> Collected:
        _require_projection(order.projection_identity(), self.__projection, "selection order")
        return Collected(
            self.__handle.order_by(order.native_handle()),
            self.__model,
            self.__projection,
        )

    def selection_handle(self) -> MatchSelectionHandle:
        return self.__handle

    def projection_identity(self) -> PyRuntimeProjection:
        return self.__projection

    def selected_model(self) -> type[ModelBase]:
        return self.__model

    def is_collection(self) -> bool:
        return True


type _Cardinality = Literal["exactly_one", "bounded_many"]
type _Group = BoundVar | BoundField | tuple[BoundField, ...]


class _Terminal[*ArgsT]:
    """One terminal kind's native entry point for each connection kind.

    The arguments after the connection are the same for all four, so a
    terminal is written once and `_execute` picks the entry point.
    """

    __slots__ = ("borrowed", "owned", "prepare", "proxy")

    def __init__(
        self,
        owned: Callable[[MatchQueryHandle, PyRustDatabase, *ArgsT], ValidatedMatchResultHandle],
        borrowed: Callable[
            [MatchQueryHandle, PyRustTransactionContext, *ArgsT], ValidatedMatchResultHandle
        ],
        proxy: Callable[[MatchQueryHandle, ProxyConnection, *ArgsT], ValidatedMatchResultHandle],
        prepare: Callable[
            [MatchQueryHandle, RemoteModelQueryContext, *ArgsT], PendingRemoteModelQuery
        ],
    ) -> None:
        self.owned = owned
        self.borrowed = borrowed
        self.proxy = proxy
        self.prepare = prepare


_FETCH_ROWS: _Terminal[list[MatchOrderHandle], int, int, _Cardinality] = _Terminal(
    MatchQueryHandle.execute_fetch_rows_owned,
    MatchQueryHandle.execute_fetch_rows_borrowed,
    MatchQueryHandle.execute_fetch_rows_proxy,
    query_v2_prepare_remote_model_rows,
)
_PAGE_BY: _Terminal[MatchBindingHandle, list[MatchOrderHandle], int, int, bool] = _Terminal(
    MatchQueryHandle.execute_page_by_owned,
    MatchQueryHandle.execute_page_by_borrowed,
    MatchQueryHandle.execute_page_by_proxy,
    query_v2_prepare_remote_model_page,
)
_COUNT_BY: _Terminal[MatchBindingHandle] = _Terminal(
    MatchQueryHandle.execute_count_by_owned,
    MatchQueryHandle.execute_count_by_borrowed,
    MatchQueryHandle.execute_count_by_proxy,
    query_v2_prepare_remote_model_count,
)
_EXISTS_BY: _Terminal[MatchBindingHandle] = _Terminal(
    MatchQueryHandle.execute_exists_by_owned,
    MatchQueryHandle.execute_exists_by_borrowed,
    MatchQueryHandle.execute_exists_by_proxy,
    query_v2_prepare_remote_model_exists,
)
_REDUCE_BY: _Terminal[
    MatchBindingHandle,
    MatchBindingHandle | None,
    list[_Reducer],
    list[MatchFieldHandle | None],
] = _Terminal(
    MatchQueryHandle.execute_reduce_by_owned,
    MatchQueryHandle.execute_reduce_by_borrowed,
    MatchQueryHandle.execute_reduce_by_proxy,
    query_v2_prepare_remote_model_reduce,
)
_REDUCE_BY_FIELD: _Terminal[
    MatchBindingHandle,
    MatchFieldHandle,
    list[_Reducer],
    list[MatchFieldHandle | None],
] = _Terminal(
    MatchQueryHandle.execute_reduce_by_field_owned,
    MatchQueryHandle.execute_reduce_by_field_borrowed,
    MatchQueryHandle.execute_reduce_by_field_proxy,
    query_v2_prepare_remote_model_reduce_by_field,
)
_REDUCE_BY_FIELDS: _Terminal[
    MatchBindingHandle,
    list[MatchFieldHandle],
    list[_Reducer],
    list[MatchFieldHandle | None],
] = _Terminal(
    MatchQueryHandle.execute_reduce_by_fields_owned,
    MatchQueryHandle.execute_reduce_by_fields_borrowed,
    MatchQueryHandle.execute_reduce_by_fields_proxy,
    query_v2_prepare_remote_model_reduce_by_fields,
)


class ExchangeConnection(_Frozen):
    """A proxy connection whose transport the host performs.

    `exchange` sends one request to the server's query route and returns its
    reply; the host owns routing and authentication. `advertisement` is the
    server's capability advertisement. `answer_limits`, when given, is the
    ceiling every session on this connection runs under; left out, the proxy
    (wire) defaults are.
    """

    __slots__ = ("__advertisement", "__answer_limits", "__exchange")
    __advertisement: bytes
    __answer_limits: QueryExecutionResourceLimits | None
    __exchange: Callable[[bytes], Awaitable[bytes]]

    def __init__(
        self,
        advertisement: bytes,
        exchange: Callable[[bytes], Awaitable[bytes]],
        *,
        answer_limits: QueryExecutionResourceLimits | None = None,
    ) -> None:
        if type(advertisement) is not bytes:
            raise TypeError("generated remote advertisement must be exact bytes")
        if not _is_exchange(exchange):
            raise TypeError("generated remote exchange must be callable")
        if answer_limits is not None and type(answer_limits) is not QueryExecutionResourceLimits:
            raise TypeError("generated answer limits must be QueryExecutionResourceLimits")
        object.__setattr__(self, "_ExchangeConnection__advertisement", advertisement)
        object.__setattr__(self, "_ExchangeConnection__exchange", exchange)
        object.__setattr__(self, "_ExchangeConnection__answer_limits", answer_limits)

    @property
    def advertisement(self) -> bytes:
        return self.__advertisement

    @property
    def exchange(self) -> Callable[[bytes], Awaitable[bytes]]:
        return self.__exchange

    @property
    def answer_limits(self) -> QueryExecutionResourceLimits | None:
        return self.__answer_limits


type QueryConnection = Database | TransactionContext | ProxyConnection | ExchangeConnection


def connect_proxy(
    endpoint: str,
    *,
    tls_mode: Literal["disabled", "native_roots", "custom_root"] = "disabled",
    tls_root_ca: str | PathLike[str] | None = None,
    headers: Sequence[tuple[str, str]] | None = None,
    advertisement: bytes | None = None,
    connection_limits: QueryExecutionResourceLimits | None = None,
    answer_limits: QueryExecutionResourceLimits | None = None,
    cancellation: QueryCancellation | None = None,
) -> ProxyConnection:
    """Open a connection to a TypeBridge server's HTTP routes for this package.

    The server's advertisement is fetched from `endpoint`, or `advertisement`
    is used as given. `connection_limits` bounds that fetch; `answer_limits`
    is the ceiling every session on the connection runs under. Either left
    out takes the proxy (wire) defaults. This call blocks on the fetch.
    """
    return ProxyConnection.connect(
        _installed_query_authority(),
        endpoint,
        tls_mode=tls_mode,
        tls_root_ca=tls_root_ca,
        headers=headers,
        advertisement=advertisement,
        connection_limits=connection_limits,
        answer_limits=answer_limits,
        cancellation=cancellation,
    )


class _ExchangeTarget(_Frozen):
    """An exchange connection bound to one session's resources and cancellation."""

    __slots__ = ("__cancellation", "__context", "__exchange")
    __cancellation: QueryCancellation
    __context: RemoteModelQueryContext
    __exchange: Callable[[bytes], Awaitable[bytes]]

    def __init__(
        self,
        connection: ExchangeConnection,
        resources: QueryExecutionResourceLimits,
        cancellation: QueryCancellation,
    ) -> None:
        context = query_v2_remote_model_context_with_resources(
            _installed_query_authority(),
            connection.advertisement,
            resources,
            cancellation,
            answer_limits=connection.answer_limits,
        )
        object.__setattr__(self, "_ExchangeTarget__context", context)
        object.__setattr__(self, "_ExchangeTarget__exchange", connection.exchange)
        object.__setattr__(self, "_ExchangeTarget__cancellation", cancellation)

    @property
    def context(self) -> RemoteModelQueryContext:
        return self.__context

    async def exchange(self, pending: PendingRemoteModelQuery) -> ValidatedMatchResultHandle:
        request = pending.request_bytes()
        if self.__cancellation.is_cancelled:
            # Claim through the neutral one-shot authority after cancellation
            # so the canonical diagnostic wins and this request is consumed.
            return pending.decode_reply(b"")

        exchange_task = asyncio.ensure_future(self.__exchange(request))
        cancellation_task = asyncio.create_task(_wait_for_query_cancellation(self.__cancellation))
        await asyncio.wait(
            (exchange_task, cancellation_task),
            return_when=asyncio.FIRST_COMPLETED,
        )
        if self.__cancellation.is_cancelled:
            exchange_task.cancel()
            exchange_task.add_done_callback(_consume_remote_exchange_task)
            return pending.decode_reply(b"")

        cancellation_task.cancel()
        cancellation_task.add_done_callback(_consume_remote_exchange_task)
        response = await exchange_task
        if type(response) is not bytes:
            raise TypeError("generated remote query exchange must return exact bytes")
        return await asyncio.to_thread(pending.decode_reply, response)


type _Target = Database | TransactionContext | ProxyConnection | _ExchangeTarget


def _target(
    connection: object,
    resources: QueryExecutionResourceLimits,
    cancellation: QueryCancellation,
) -> _Target:
    if isinstance(connection, ExchangeConnection):
        return _ExchangeTarget(connection, resources, cancellation)
    if isinstance(connection, (Database, TransactionContext, ProxyConnection)):
        return connection
    raise TypeError(
        "generated QuerySession requires a Database, TransactionContext, "
        "ProxyConnection, or ExchangeConnection"
    )


async def _execute[*ArgsT](
    target: _Target,
    query: MatchQueryHandle,
    terminal: _Terminal[*ArgsT],
    *arguments: *ArgsT,
) -> ValidatedMatchResultHandle:
    """Run one terminal on the session's connection.

    This is the only place that branches on the connection kind. Native
    terminals detach the GIL, so a worker thread keeps the event loop free.
    """
    if isinstance(target, _ExchangeTarget):
        return await target.exchange(terminal.prepare(query, target.context, *arguments))
    if isinstance(target, ProxyConnection):
        proxy = target
        return await asyncio.to_thread(lambda: terminal.proxy(query, proxy, *arguments))
    if isinstance(target, TransactionContext):
        transaction = rust_transaction_for(target)
        if transaction is None:
            raise RuntimeError("generated query transaction is not active")
        return await asyncio.to_thread(lambda: terminal.borrowed(query, transaction, *arguments))
    database = target
    return await asyncio.to_thread(
        lambda: terminal.owned(query, rust_database_for(database), *arguments)
    )


class Query(_Frozen):
    __slots__ = (
        "__collected",
        "__connection",
        "__declaration",
        "__handle",
        "__names",
        "__projection",
    )
    __collected: tuple[bool, ...]
    __connection: _Target
    __declaration: type[object] | None
    __handle: MatchQueryHandle
    __names: tuple[str, ...] | None
    __projection: PyRuntimeProjection

    def __init__(
        self,
        handle: MatchQueryHandle,
        projection: PyRuntimeProjection,
        connection: _Target,
        collected: tuple[bool, ...],
        names: tuple[str, ...] | None = None,
        declaration: type[object] | None = None,
    ) -> None:
        object.__setattr__(self, "_Query__handle", handle)
        object.__setattr__(self, "_Query__projection", projection)
        object.__setattr__(self, "_Query__connection", connection)
        object.__setattr__(self, "_Query__collected", collected)
        object.__setattr__(self, "_Query__names", names)
        object.__setattr__(self, "_Query__declaration", declaration)

    def close(self) -> None:
        """Close only this query; persistent ancestors and descendants stay usable."""
        self.__handle.close()

    @property
    def is_closed(self) -> bool:
        return self.__handle.is_closed

    def clone(self) -> Query:
        """Return an independently closable value-equivalent query."""
        return self.__clone(self.__handle.fork())

    def match(self, *bindings: object) -> Query:
        if not bindings:
            raise TypeError("Query.match requires at least one generated binding")
        handle = self.__handle
        for binding in bindings:
            if not isinstance(binding, BoundVar):
                raise TypeError("Query.match requires generated BoundVar values")
            _require_projection(binding.projection_identity(), self.__projection, "query binding")
            handle = handle.add_hidden(binding.native_handle())
        return self.__clone(handle)

    def where(self, *predicates: Predicate) -> Query:
        if not predicates:
            raise TypeError("Query.where requires at least one generated predicate")
        handle = self.__handle
        for predicate in predicates:
            _require_projection(
                predicate.projection_identity(), self.__projection, "query predicate"
            )
            handle = handle.where_predicate(predicate.native_handle())
        return self.__clone(handle)

    def allow_cross_join(
        self,
        left: BoundVar,
        right: BoundVar,
    ) -> Query:
        _require_projection(left.projection_identity(), self.__projection, "cross-join binding")
        _require_projection(right.projection_identity(), self.__projection, "cross-join binding")
        return self.__clone(
            self.__handle.allow_cross_join(left.native_handle(), right.native_handle())
        )

    async def one(self) -> object:
        result = await self.__fetch(0, 1, "exactly_one", ())
        if result.row_count() != 1:
            raise RuntimeError("validated exactly-one result did not contain one row")
        return self.materialize_row(result.row(0))

    async def first(self, *, order_by: Iterable[QueryOrder] = ()) -> object | None:
        result = await self.__fetch(0, 1, "bounded_many", order_by)
        if result.row_count() == 0:
            return None
        return self.materialize_row(result.row(0))

    async def rows(
        self,
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[object]:
        result = await self.__fetch(offset, limit, "bounded_many", order_by)
        return [self.materialize_row(result.row(index)) for index in range(result.row_count())]

    async def page_by(
        self,
        root: BoundVar,
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page:
        self.__require_root(root, "query root")
        if type(include_total) is not bool:
            raise TypeError("include_total must be an exact bool")
        handles = self.order_handles(order_by)
        result = await self.__run(
            _PAGE_BY, root.native_handle(), handles, offset, limit, include_total
        )
        return self.materialize_page(result)

    async def count_by(self, root: BoundVar) -> int:
        self.__require_root(root, "query root")
        return (await self.__run(_COUNT_BY, root.native_handle())).count_value()

    async def exists_by(self, root: BoundVar) -> bool:
        self.__require_root(root, "query root")
        return (await self.__run(_EXISTS_BY, root.native_handle())).exists_value()

    async def aggregate(self, root: BoundVar, *terms: Aggregate) -> tuple[object, ...]:
        result = await self.reduction_result(root, None, terms)
        materialized = self.materialize_reduction(result, len(terms), group=None)
        if not _is_object_tuple(materialized):
            raise RuntimeError("validated aggregate returned an invalid result")
        return materialized

    def group_by(self, root: BoundVar, *groups: BoundVar | BoundField) -> GroupedQuery:
        self.__require_root(root, "aggregate root")
        group = _require_aggregate_groups(groups, self.__projection, "aggregate group")
        fields = (group,) if isinstance(group, BoundField) else group
        if isinstance(fields, tuple):
            # The core refuses too many group fields here, not at aggregate().
            self.__handle.check_group_fields([field.native_handle() for field in fields])
        return GroupedQuery(self.clone(), root, group)

    async def reduction_result(
        self,
        root: BoundVar,
        group: _Group | None,
        terms: tuple[Aggregate, ...],
    ) -> ValidatedMatchResultHandle:
        self.__require_root(root, "aggregate root")
        if isinstance(group, BoundVar):
            self.__require_root(group, "aggregate group")
        elif isinstance(group, BoundField):
            _require_projection(
                group.projection_identity(), self.__projection, "aggregate group field"
            )
        elif isinstance(group, tuple):
            for field in group:
                _require_projection(
                    field.projection_identity(), self.__projection, "aggregate group field"
                )
        reducers, inputs = _prepare_aggregate_terms(terms, self.__projection)
        if isinstance(group, tuple):
            handles = [field.native_handle() for field in group]
            return await self.__run(
                _REDUCE_BY_FIELDS, root.native_handle(), handles, reducers, inputs
            )
        if isinstance(group, BoundField):
            return await self.__run(
                _REDUCE_BY_FIELD, root.native_handle(), group.native_handle(), reducers, inputs
            )
        return await self.__run(
            _REDUCE_BY,
            root.native_handle(),
            None if group is None else group.native_handle(),
            reducers,
            inputs,
        )

    def materialize_reduction(
        self,
        result: ValidatedMatchResultHandle,
        term_count: int,
        *,
        group: _Group | None,
    ) -> object:
        rows: list[object] = []
        for row_index in range(result.reduction_row_count()):
            if result.reduction_value_count(row_index) != term_count:
                raise RuntimeError("validated aggregate result changed its term count")
            values = tuple(
                result.reduction_value(row_index, value_index) for value_index in range(term_count)
            )
            if isinstance(group, BoundVar):
                rows.append(
                    (
                        self.__projection.hydrate_thing(result.reduction_group(row_index)),
                        values,
                    )
                )
            elif isinstance(group, BoundField):
                rows.append(
                    (
                        group.attribute_model()._from_validated_query_value(
                            result.reduction_group_value(row_index)
                        ),
                        values,
                    )
                )
            elif isinstance(group, tuple):
                group_values = result.reduction_group_values(row_index)
                if len(group_values) != len(group):
                    raise RuntimeError("validated aggregate result changed its group arity")
                rows.append(
                    (
                        tuple(
                            field.attribute_model()._from_validated_query_value(value)
                            for field, value in zip(group, group_values, strict=True)
                        ),
                        values,
                    )
                )
            else:
                rows.append(values)
        if group is not None:
            return tuple(rows)
        if len(rows) != 1:
            raise RuntimeError("validated ungrouped aggregate did not return exactly one row")
        return rows[0]

    def __require_root(self, root: BoundVar, context: str) -> None:
        _require_projection(root.projection_identity(), self.__projection, context)

    async def __fetch(
        self,
        offset: int,
        limit: int,
        cardinality: _Cardinality,
        order_by: Iterable[QueryOrder],
    ) -> ValidatedMatchResultHandle:
        handles = self.order_handles(order_by)
        return await self.__run(_FETCH_ROWS, handles, offset, limit, cardinality)

    async def __run[*ArgsT](
        self,
        terminal: _Terminal[*ArgsT],
        *arguments: *ArgsT,
    ) -> ValidatedMatchResultHandle:
        return await _execute(self.__connection, self.__handle, terminal, *arguments)

    def materialize_row(self, row: ValidatedMatchRowHandle) -> object:
        if row.slot_count() != len(self.__collected):
            raise RuntimeError("validated query result shape differs from its generated selection")
        values: list[object] = []
        for index, collected in enumerate(self.__collected):
            slot = row.slot(index)
            if slot.is_collection() is not collected:
                raise RuntimeError(
                    "validated query slot collection shape differs from its selection"
                )
            if collected:
                values.append(
                    tuple(
                        self.__projection.hydrate_thing(slot.thing(item))
                        for item in range(slot.thing_count())
                    )
                )
            else:
                if slot.thing_count() != 1:
                    raise RuntimeError("validated singular query slot has invalid cardinality")
                values.append(self.__projection.hydrate_thing(slot.thing(0)))
        if self.__declaration is None:
            if self.__names is not None or any(
                row.slot(index).name() is not None for index in range(row.slot_count())
            ):
                raise RuntimeError("positional generated query received named result slots")
            return values[0] if len(values) == 1 else tuple(values)
        if self.__names is None:
            raise RuntimeError("generated named query lost its declared field names")
        actual_names = tuple(row.slot(index).name() for index in range(row.slot_count()))
        if actual_names != self.__names:
            raise RuntimeError("generated named result differs from its declaration")
        try:
            result = self.__declaration(**dict(zip(self.__names, values, strict=True)))
        except Exception as error:
            raise RuntimeError("failed to construct generated named query row") from error
        if type(result) is not self.__declaration:
            raise RuntimeError("generated named query constructor substituted its row type")
        return result

    def materialize_page(self, result: ValidatedMatchResultHandle) -> Page:
        return Page(
            (
                self.materialize_row(result.page_entry(index))
                for index in range(result.page_entry_count())
            ),
            offset=result.page_offset(),
            limit=result.page_limit(),
            total=result.page_total(),
        )

    def order_handles(self, order_by: Iterable[object]) -> list[MatchOrderHandle]:
        handles: list[MatchOrderHandle] = []
        for order in order_by:
            if not isinstance(order, QueryOrder):
                raise TypeError("order_by entries must be generated QueryOrder values")
            _require_projection(order.projection_identity(), self.__projection, "query order")
            handles.append(order.native_handle())
        return handles

    def __clone(self, handle: MatchQueryHandle) -> Query:
        return Query(
            handle,
            self.__projection,
            self.__connection,
            self.__collected,
            self.__names,
            self.__declaration,
        )

    def native_handle(self) -> MatchQueryHandle:
        return self.__handle

    def projection_identity(self) -> PyRuntimeProjection:
        return self.__projection


class GroupedQuery(_Frozen):
    __slots__ = ("__group", "__query", "__root")
    __group: _Group
    __query: Query
    __root: BoundVar

    def __init__(
        self,
        query: Query,
        root: BoundVar,
        group: _Group,
    ) -> None:
        object.__setattr__(self, "_GroupedQuery__query", query)
        object.__setattr__(self, "_GroupedQuery__root", root)
        object.__setattr__(self, "_GroupedQuery__group", group)

    def close(self) -> None:
        self.__query.close()

    @property
    def is_closed(self) -> bool:
        return self.__query.is_closed

    def match(self, *bindings: BoundVar) -> GroupedQuery:
        return GroupedQuery(self.__query.match(*bindings), self.__root, self.__group)

    def where(self, *predicates: Predicate) -> GroupedQuery:
        return GroupedQuery(self.__query.where(*predicates), self.__root, self.__group)

    def allow_cross_join(self, left: BoundVar, right: BoundVar) -> GroupedQuery:
        return GroupedQuery(
            self.__query.allow_cross_join(left, right),
            self.__root,
            self.__group,
        )

    async def aggregate(self, *terms: Aggregate) -> tuple[tuple[object, tuple[object, ...]], ...]:
        result = await self.__query.reduction_result(self.__root, self.__group, terms)
        materialized = self.__query.materialize_reduction(result, len(terms), group=self.__group)
        if not _is_grouped_reduction(materialized):
            raise RuntimeError("validated grouped aggregate returned an invalid result")
        return materialized


class _Selection(Protocol):
    def selection_handle(self) -> MatchSelectionHandle: ...
    def projection_identity(self) -> PyRuntimeProjection: ...
    def selected_model(self) -> type[ModelBase]: ...
    def is_collection(self) -> bool: ...


class QuerySession(_Frozen):
    """Author and run queries over one connection.

    The connection is a direct `Database` or `TransactionContext`, a
    `ProxyConnection` from `connect_proxy`, or an `ExchangeConnection`. The
    session, its queries, and their results are the same on every kind;
    terminals are coroutines on all of them.
    """

    __slots__ = ("__connection", "__handle", "__projection")
    __connection: _Target
    __handle: MatchSessionHandle
    __projection: PyRuntimeProjection

    def __init__(
        self,
        connection: QueryConnection,
        *,
        resources: QueryExecutionResourceLimits | None = None,
        cancellation: QueryCancellation | None = None,
        query_limits: QueryLimits | None = None,
    ) -> None:
        if query_limits is not None and type(query_limits) is not QueryLimits:
            raise TypeError("generated query limits must be QueryLimits")
        if resources is None:
            resources = QueryExecutionResourceLimits()
        if cancellation is None:
            cancellation = QueryCancellation()
        if type(resources) is not QueryExecutionResourceLimits:
            raise TypeError("generated query resources must be QueryExecutionResourceLimits")
        if type(cancellation) is not QueryCancellation:
            raise TypeError("generated query cancellation must be QueryCancellation")
        projection = _installed_projection()
        target = _target(connection, resources, cancellation)
        object.__setattr__(self, "_QuerySession__projection", projection)
        object.__setattr__(self, "_QuerySession__connection", target)
        object.__setattr__(
            self,
            "_QuerySession__handle",
            projection.match_session_with_resources(resources, cancellation, query_limits),
        )

    def close(self) -> None:
        """Close this authoring session and invalidate its descendant terminals."""
        self.__handle.close()

    @property
    def is_closed(self) -> bool:
        return self.__handle.is_closed

    def var(self, model: type[ModelBase], *, subtypes: bool = False) -> BoundVar:
        if type(subtypes) is not bool:
            raise TypeError("subtypes must be an exact bool")
        return self.__bind(model, subtypes=subtypes)

    def exact(self, model: type[ModelBase]) -> BoundVar:
        return self.var(model)

    def subtypes(self, model: type[ModelBase]) -> BoundVar:
        return self.var(model, subtypes=True)

    def _function_input[DomainT](
        self, value: object, domain: type[DomainT]
    ) -> FunctionInput[DomainT]:
        if not isinstance(value, AttributeBase):
            raise TypeError("schema-function inputs require projected attribute values")
        del domain
        _require_projection(_exact_projection(type(value)), self.__projection, "function input")
        return FunctionInput(
            self.__handle.function_value(type(value).__type_id__, _dynamic_value(value)),
            self.__projection,
        )

    def _call_function[DomainT](
        self,
        function: FunctionRef[..., object],
        arguments: tuple[object, ...],
        domain: type[DomainT],
    ) -> FunctionCall[DomainT]:
        del domain
        function_id, _signature = function_identity_for_query(function)
        native_arguments: list[MatchFunctionArgumentHandle] = []
        for argument in arguments:
            if isinstance(argument, BoundVar):
                _require_projection(
                    argument.projection_identity(), self.__projection, "function binding"
                )
                native_arguments.append(argument.native_handle().function_argument())
            elif type(argument) is FunctionInput:
                _require_projection(
                    argument.projection_identity(), self.__projection, "function input"
                )
                native_arguments.append(argument.native_argument())
            elif type(argument) is FunctionCall:
                _require_projection(
                    argument.projection_identity(), self.__projection, "function call"
                )
                native_arguments.append(argument.native_argument())
            else:
                raise TypeError("invalid generated schema-function argument")
        handle = self.__handle.function_by_id(function_id).call(native_arguments)
        return FunctionCall(handle, self.__projection)

    def reachable(
        self,
        source: object,
        target: object,
        relation: object,
        role_from: object,
        role_to: object,
        *,
        min_depth: int,
        max_depth: int,
    ) -> Predicate:
        if not isinstance(source, BoundVar) or not isinstance(target, BoundVar):
            raise TypeError("generated reachable endpoints must be BoundVar values")
        if not isinstance(relation, type) or not issubclass(relation, RelationBase):
            raise TypeError("generated reachable relation must be a relation model")
        _require_projection(source.projection_identity(), self.__projection, "reachable source")
        _require_projection(target.projection_identity(), self.__projection, "reachable target")
        _require_projection(_exact_projection(relation), self.__projection, "reachable relation")
        from_role = _reachable_role(
            role_from,
            relation,
            source,
            self.__projection,
            "role_from",
        )
        to_role = _reachable_role(
            role_to,
            relation,
            target,
            self.__projection,
            "role_to",
        )
        return Predicate(
            self.__handle.reachable(
                _model_label(relation),
                from_role,
                to_role,
                source.native_handle(),
                target.native_handle(),
                min_depth,
                max_depth,
            ),
            self.__projection,
        )

    def query(self, *selections: _Selection) -> Query:
        if not selections:
            raise TypeError("QuerySession.query requires at least one generated selection")
        handles: list[MatchSelectionHandle] = []
        collected: list[bool] = []
        for selection in selections:
            if not isinstance(selection, (BoundVar, Collected)):
                raise TypeError("generated selections must be BoundVar or Collected values")
            _require_projection(
                selection.projection_identity(), self.__projection, "query selection"
            )
            handles.append(selection.selection_handle())
            collected.append(isinstance(selection, Collected))
        shape = self.__handle.positional(handles)
        return Query(
            self.__handle.query(shape),
            self.__projection,
            self.__connection,
            tuple(collected),
        )

    def query_as(
        self,
        declaration: type[object],
        /,
        **selections: _Selection,
    ) -> Query:
        declared = _named_declaration(declaration, self.__projection)
        names = tuple(name for name, _, _ in declared)
        if tuple(selections) != names:
            raise TypeError("query_as selections must exactly match declaration fields")
        handles: list[MatchSelectionHandle] = []
        collected: list[bool] = []
        native_declarations: list[tuple[str, str, bool]] = []
        for (name, model, is_collection), selection in zip(
            declared,
            selections.values(),
            strict=True,
        ):
            if not isinstance(selection, (BoundVar, Collected)):
                raise TypeError("query_as values must be generated selections")
            _require_projection(
                selection.projection_identity(),
                self.__projection,
                "named query selection",
            )
            if selection.is_collection() is not is_collection:
                raise TypeError(f"query_as field {name!r} collection shape differs")
            if selection.selected_model() is not model:
                raise TypeError(f"query_as field {name!r} model type differs")
            handles.append(selection.selection_handle())
            collected.append(is_collection)
            native_declarations.append((name, _model_label(model), is_collection))
        shape = self.__handle.named_checked(native_declarations, list(names), handles)
        return Query(
            self.__handle.query(shape),
            self.__projection,
            self.__connection,
            tuple(collected),
            names,
            declaration,
        )

    def __bind(
        self,
        model: type[ModelBase],
        *,
        subtypes: bool,
    ) -> BoundVar:
        _require_projection(_exact_projection(model), self.__projection, "query model")
        if not issubclass(model, (EntityBase, RelationBase)):
            raise TypeError("generated query variables require an entity or relation model")
        label = _model_label(model)
        handle = self.__handle.subtypes(label) if subtypes else self.__handle.exact(label)
        binding = SubtypeBoundVar if subtypes else BoundVar
        return binding(handle, model, self.__projection, subtypes=subtypes)


# Deprecated 2.2 remote-session names, removed in 3.0. Each builds the 2.3
# type it names; none adds behaviour.

RemoteQuery = Query
"""Deprecated: use `Query`; removed in 3.0."""

RemoteGroupedQuery = GroupedQuery
"""Deprecated: use `GroupedQuery`; removed in 3.0."""


class _RemoteQueryLimitsType(type):
    # `RemoteQueryLimits(...)` returns the native limits type, which cannot be
    # subclassed; keep 2.2 `isinstance(limits, RemoteQueryLimits)` checks true.
    def __instancecheck__(cls, instance: object) -> bool:
        return type(instance) is QueryExecutionResourceLimits


class RemoteQueryLimits(metaclass=_RemoteQueryLimitsType):
    """Deprecated: use `QueryExecutionResourceLimits`; removed in 3.0.

    Takes the 2.2 field names and returns a `QueryExecutionResourceLimits`.
    `deadline_ms=None` takes the default timeout.
    """

    __slots__ = ()

    def __new__(
        cls,
        *,
        max_items: int,
        max_bytes: int,
        max_collection_members: int,
        max_graph_nodes: int,
        max_attribute_values: int,
        max_role_players: int,
        deadline_ms: int | None = None,
    ) -> QueryExecutionResourceLimits:
        warnings.warn(
            "RemoteQueryLimits is deprecated and will be removed in 3.0; "
            "use QueryExecutionResourceLimits",
            DeprecationWarning,
            stacklevel=2,
        )
        return QueryExecutionResourceLimits(
            timeout_milliseconds=deadline_ms,
            items=max_items,
            bytes=max_bytes,
            graph_nodes=max_graph_nodes,
            attribute_values=max_attribute_values,
            collection_members=max_collection_members,
            role_players=max_role_players,
        )


class RemoteQuerySession(QuerySession):
    """Deprecated: use `QuerySession(ExchangeConnection(advertisement, exchange),
    resources=limits)`; removed in 3.0."""

    __slots__ = ()

    def __init__(
        self,
        advertisement: bytes,
        exchange: Callable[[bytes], Awaitable[bytes]],
        limits: QueryExecutionResourceLimits,
        *,
        cancellation: QueryCancellation | None = None,
    ) -> None:
        warnings.warn(
            "RemoteQuerySession is deprecated and will be removed in 3.0; use "
            "QuerySession(ExchangeConnection(advertisement, exchange), resources=limits)",
            DeprecationWarning,
            stacklevel=2,
        )
        super().__init__(
            ExchangeConnection(advertisement, exchange),
            resources=limits,
            cancellation=cancellation,
        )


def _require_projection(
    actual: PyRuntimeProjection,
    expected: PyRuntimeProjection,
    kind: str,
) -> None:
    if actual is not expected:
        raise TypeError(f"generated {kind} must belong to the same installed package")


def _reachable_role(
    token: object,
    relation: type[RelationBase],
    endpoint: BoundVar,
    projection: PyRuntimeProjection,
    name: str,
) -> str:
    if not _is_role_token(token) or token.owner is not relation:
        raise TypeError(f"generated reachable {name} must belong to the relation")
    _require_projection(_exact_projection(token.owner), projection, f"reachable {name}")
    role = token.fact.get("role")
    accepted_players = token.fact.get("accepted_players")
    if not _is_object_dict(role) or not _is_object_list(accepted_players):
        raise TypeError(f"generated reachable {name} contract is invalid")
    accepted = frozenset(_type_id_label(player) for player in accepted_players)
    if accepted.isdisjoint(endpoint.model_domain_labels()):
        raise TypeError(f"generated reachable {name} does not accept its endpoint")
    role_label = role.get("label")
    if not isinstance(role_label, str):
        raise TypeError(f"generated reachable {name} identity is invalid")
    return role_label


def _named_declaration(
    declaration: type[object],
    projection: PyRuntimeProjection,
) -> tuple[tuple[str, type[ModelBase], bool], ...]:
    names = _declaration_names(declaration, require_frozen=True)
    try:
        raw_annotations: object = get_type_hints(declaration, include_extras=True)
    except (NameError, TypeError) as error:
        raise TypeError("query_as annotations must resolve exactly") from error
    if not _is_object_dict(raw_annotations) or tuple(raw_annotations) != names:
        raise TypeError("query_as requires one exact annotation per field")
    normalized: list[tuple[str, type[ModelBase], bool]] = []
    for name in names:
        annotation = raw_annotations[name]
        if get_origin(annotation) is tuple:
            raw_arguments: object = get_args(annotation)
            if (
                not _is_object_tuple(raw_arguments)
                or len(raw_arguments) != 2
                or raw_arguments[1] is not Ellipsis
            ):
                raise TypeError(f"query_as field {name!r} has invalid collection type")
            model = raw_arguments[0]
            collection = True
        else:
            model = annotation
            collection = False
        if not isinstance(model, type) or not issubclass(model, (EntityBase, RelationBase)):
            raise TypeError(f"query_as field {name!r} is not a generated model")
        _require_projection(_exact_projection(model), projection, f"query_as field {name!r}")
        normalized.append((name, model, collection))
    return tuple(normalized)


def _declaration_names(
    declaration: type[object],
    *,
    require_frozen: bool,
) -> tuple[str, ...]:
    if is_dataclass(declaration):
        parameters: object = getattr(declaration, "__dataclass_params__", None)
        frozen: object = getattr(parameters, "frozen", None)
        if require_frozen and frozen is not True:
            raise TypeError("query_as dataclasses must be frozen")
        return tuple(field.name for field in fields(declaration))
    raw_names: object = getattr(declaration, "_fields", None)
    if issubclass(declaration, tuple) and _is_string_tuple(raw_names):
        return raw_names
    raise TypeError("query_as requires a frozen dataclass or NamedTuple class")


def _dynamic_value(value: AttributeBase) -> DynamicValue:
    declaration = value.__projection__["declaration"]
    if not _is_object_dict(declaration):
        raise TypeError("generated attribute declaration is invalid")
    value_type = declaration.get("value_type")
    if not isinstance(value_type, str):
        raise TypeError("generated attribute value type is invalid")
    kind = value_type
    scalar = value.value
    if kind == "string" and isinstance(scalar, str):
        return DynamicValue.string(scalar)
    if kind == "long" and type(scalar) is int:
        return DynamicValue.long(scalar)
    if kind == "double" and type(scalar) is float:
        return DynamicValue.double(scalar)
    if kind == "boolean" and type(scalar) is bool:
        return DynamicValue.boolean(scalar)
    if kind == "date" and type(scalar) is date:
        return DynamicValue.date(scalar.isoformat())
    if kind == "datetime" and type(scalar) is datetime and scalar.tzinfo is None:
        return DynamicValue.datetime(scalar.isoformat())
    if kind == "datetime_tz" and type(scalar) is datetime and scalar.tzinfo is not None:
        return DynamicValue.datetime_tz(scalar.isoformat())
    if kind == "decimal" and type(scalar) is Decimal:
        return DynamicValue.decimal(str(scalar))
    if kind == "duration" and type(scalar) is timedelta:
        return DynamicValue.duration(_duration_isoformat(scalar))
    raise TypeError("generated attribute value does not match its projected scalar type")


def _is_object_dict(value: object) -> TypeGuard[dict[str, object]]:
    if not _is_untyped_dict(value):
        return False
    return all(isinstance(key, str) for key in value)


def _is_object_list(value: object) -> TypeGuard[list[object]]:
    return isinstance(value, list)


def _is_object_tuple(value: object) -> TypeGuard[tuple[object, ...]]:
    return isinstance(value, tuple)


def _is_string_tuple(value: object) -> TypeGuard[tuple[str, ...]]:
    return _is_object_tuple(value) and all(isinstance(item, str) for item in value)


def _is_role_token(value: object) -> TypeGuard[RoleToken[ModelBase, ModelBase, BoundVar]]:
    return isinstance(value, RoleToken)


def _is_exchange(
    value: object,
) -> TypeGuard[Callable[[bytes], Awaitable[bytes]]]:
    return callable(value)


async def _wait_for_query_cancellation(cancellation: QueryCancellation) -> None:
    """Yield cooperatively until the shared native cancellation is observed."""
    while not cancellation.is_cancelled:
        await asyncio.sleep(0)


def _consume_remote_exchange_task[ResultT](task: asyncio.Future[ResultT]) -> None:
    """Observe a losing task so cancellation never leaks an event-loop warning."""
    if task.cancelled():
        return
    task.exception()


def _type_id_label(value: object) -> str:
    if not _is_object_dict(value):
        raise TypeError("generated model identity must be an object")
    label = value.get("label")
    if not isinstance(label, str):
        raise TypeError("generated model identity has no string label")
    return label


def _is_untyped_dict(value: object) -> TypeGuard[dict[object, object]]:
    return isinstance(value, dict)


def _duration_isoformat(value: timedelta) -> str:
    total_microseconds = (
        value.days * 86_400_000_000 + value.seconds * 1_000_000 + value.microseconds
    )
    sign = "-" if total_microseconds < 0 else ""
    remaining = abs(total_microseconds)
    days, remaining = divmod(remaining, 86_400_000_000)
    hours, remaining = divmod(remaining, 3_600_000_000)
    minutes, remaining = divmod(remaining, 60_000_000)
    seconds, microseconds = divmod(remaining, 1_000_000)
    second_text = str(seconds)
    if microseconds:
        second_text += f".{microseconds:06d}".rstrip("0")
    return f"{sign}P{days}DT{hours}H{minutes}M{second_text}S"


def _prepare_aggregate_terms(
    terms: tuple[Aggregate, ...],
    projection: PyRuntimeProjection,
) -> tuple[list[_Reducer], list[MatchFieldHandle | None]]:
    reducers: list[_Reducer] = []
    inputs: list[MatchFieldHandle | None] = []
    for term in terms:
        if type(term) is not Aggregate:
            raise TypeError("generated aggregate terms must come from aggregate constructors")
        term_projection = term.projection_identity()
        if term_projection is not None:
            _require_projection(term_projection, projection, "aggregate field")
        reducers.append(term.native_reducer())
        inputs.append(term.native_input())
    return reducers, inputs


def _is_grouped_reduction(
    value: object,
) -> TypeGuard[tuple[tuple[object, tuple[object, ...]], ...]]:
    if not _is_object_tuple(value):
        return False
    for row in value:
        if not _is_object_tuple(row) or len(row) != 2 or not _is_object_tuple(row[1]):
            return False
    return True


__all__ = [
    "Aggregate",
    "BoundField",
    "BoundRole",
    "BoundVar",
    "Collected",
    "ExchangeConnection",
    "FunctionCall",
    "FunctionInput",
    "GroupedQuery",
    "Page",
    "Predicate",
    "ProxyConnection",
    "Query",
    "QueryCancellation",
    "QueryConnection",
    "QueryExecutionResourceLimits",
    "QueryLimits",
    "QueryOrder",
    "QuerySession",
    "RemoteGroupedQuery",
    "RemoteQuery",
    "RemoteQueryLimits",
    "RemoteQuerySession",
    "SubtypeBoundVar",
    "aggregate",
    "connect_proxy",
]
