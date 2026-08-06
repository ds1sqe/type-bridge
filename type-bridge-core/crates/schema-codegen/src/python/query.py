"""Projection-owned query references emitted with a generated Python package."""

from __future__ import annotations

import json
from collections.abc import Awaitable, Callable, Iterable, Iterator, Sequence
from dataclasses import fields, is_dataclass
from datetime import date, datetime, timedelta
from decimal import Decimal
from typing import Literal, Protocol, TypeGuard, get_args, get_origin, get_type_hints, overload

from type_bridge_core import (
    DynamicValue,
    MatchBindingHandle,
    MatchFieldHandle,
    MatchOrderHandle,
    MatchPredicateHandle,
    MatchQueryHandle,
    MatchRoleHandle,
    MatchSelectionHandle,
    MatchSessionHandle,
    PendingRemoteModelQuery,
    PyRuntimeProjection,
    RemoteModelQueryContext,
    ValidatedMatchResultHandle,
    ValidatedMatchRowHandle,
    query_v2_prepare_remote_model_count,
    query_v2_prepare_remote_model_exists,
    query_v2_prepare_remote_model_page,
    query_v2_prepare_remote_model_reduce,
    query_v2_prepare_remote_model_reduce_by_field,
    query_v2_prepare_remote_model_reduce_by_fields,
    query_v2_prepare_remote_model_rows,
    query_v2_remote_model_context,
)

from type_bridge._rust_runtime import rust_database_for, rust_transaction_for
from type_bridge.query_v2 import QueryV2Authority
from type_bridge.session import Database, TransactionContext

from ._runtime import (
    AttributeBase,
    EntityBase,
    FieldToken,
    ModelBase,
    RelationBase,
    RoleToken,
    attribute_model_for_query_label,
)


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


def install_projection(projection: PyRuntimeProjection) -> None:
    global _package_projection
    if _package_projection is not None:
        raise RuntimeError("generated query projection is already installed")
    _package_projection = projection


def _installed_projection() -> PyRuntimeProjection:
    if _package_projection is None:
        raise RuntimeError("generated package runtime projection is not installed")
    return _package_projection


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

    def neq(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("not_equal", value)

    def lt(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("less_than", value)

    def lte(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("less_than_or_equal", value)

    def gt(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("greater_than", value)

    def gte(self, value: AttributeBase | BoundField) -> Predicate:
        return self.__compare("greater_than_or_equal", value)

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
            _require_projection(value.projection_identity(), self.__projection, "bound field")
            if value.attribute_label() != self.__attribute_label:
                raise TypeError("generated field comparisons require the same attribute type")
            handle = self.__handle.compare_field(operator, value.native_handle())
        else:
            _require_projection(_exact_projection(type(value)), self.__projection, "attribute")
            if _model_label(type(value)) != self.__attribute_label:
                raise TypeError("generated field comparison requires its exact attribute wrapper")
            handle = self.__handle.compare_value(operator, _dynamic_value(value))
        return Predicate(handle, self.__projection)

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
    if len(candidates) > 16:
        raise ValueError("aggregate grouping supports at most sixteen fields")
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
    __connection: Database | TransactionContext | None
    __declaration: type[object] | None
    __handle: MatchQueryHandle
    __names: tuple[str, ...] | None
    __projection: PyRuntimeProjection

    def __init__(
        self,
        handle: MatchQueryHandle,
        projection: PyRuntimeProjection,
        connection: Database | TransactionContext | None,
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

    def one(self) -> object:
        result = self.__fetch(0, 1, "exactly_one", ())
        return self.__materialize_row(result.row(0))

    def first(self, *, order_by: Iterable[QueryOrder] = ()) -> object | None:
        result = self.__fetch(0, 1, "bounded_many", order_by)
        if result.row_count() == 0:
            return None
        return self.__materialize_row(result.row(0))

    def rows(
        self,
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[object]:
        result = self.__fetch(offset, limit, "bounded_many", order_by)
        return [self.__materialize_row(result.row(index)) for index in range(result.row_count())]

    def page_by(
        self,
        root: BoundVar,
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page:
        _require_projection(root.projection_identity(), self.__projection, "query root")
        if type(include_total) is not bool:
            raise TypeError("include_total must be an exact bool")
        offset, limit = _window(offset, limit)
        handles = self.__order_handles(order_by)
        transaction = rust_transaction_for(self.__direct_connection())
        if transaction is not None:
            result = self.__handle.execute_page_by_borrowed(
                transaction,
                root.native_handle(),
                handles,
                offset,
                limit,
                include_total,
            )
        else:
            result = self.__handle.execute_page_by_owned(
                rust_database_for(self.__direct_connection()),
                root.native_handle(),
                handles,
                offset,
                limit,
                include_total,
            )
        return self.__materialize_page(result)

    def count_by(self, root: BoundVar) -> int:
        result = self.__root_result(root, count=True)
        return result.count_value()

    def exists_by(self, root: BoundVar) -> bool:
        result = self.__root_result(root, count=False)
        return result.exists_value()

    def aggregate(self, root: BoundVar, *terms: Aggregate) -> tuple[object, ...]:
        result = self.reduction_result(root, None, terms)
        materialized = self.__materialize_reduction(result, len(terms), group=None)
        if not _is_object_tuple(materialized):
            raise RuntimeError("validated aggregate returned an invalid result")
        return materialized

    def group_by(self, root: BoundVar, *groups: BoundVar | BoundField) -> GroupedQuery:
        self.__require_root(root, "aggregate root")
        return GroupedQuery(
            self,
            root,
            _require_aggregate_groups(groups, self.__projection, "aggregate group"),
        )

    def __root_result(
        self,
        root: BoundVar,
        *,
        count: bool,
    ) -> ValidatedMatchResultHandle:
        _require_projection(root.projection_identity(), self.__projection, "query root")
        connection = self.__direct_connection()
        transaction = rust_transaction_for(connection)
        if count:
            if transaction is not None:
                return self.__handle.execute_count_by_borrowed(transaction, root.native_handle())
            return self.__handle.execute_count_by_owned(
                rust_database_for(connection), root.native_handle()
            )
        if transaction is not None:
            return self.__handle.execute_exists_by_borrowed(transaction, root.native_handle())
        return self.__handle.execute_exists_by_owned(
            rust_database_for(connection), root.native_handle()
        )

    def reduction_result(
        self,
        root: BoundVar,
        group: BoundVar | BoundField | tuple[BoundField, ...] | None,
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
        connection = self.__direct_connection()
        transaction = rust_transaction_for(connection)
        if isinstance(group, tuple):
            handles = [field.native_handle() for field in group]
            if transaction is not None:
                return self.__handle.execute_reduce_by_fields_borrowed(
                    transaction,
                    root.native_handle(),
                    handles,
                    reducers,
                    inputs,
                )
            return self.__handle.execute_reduce_by_fields_owned(
                rust_database_for(connection),
                root.native_handle(),
                handles,
                reducers,
                inputs,
            )
        if isinstance(group, BoundField):
            if transaction is not None:
                return self.__handle.execute_reduce_by_field_borrowed(
                    transaction,
                    root.native_handle(),
                    group.native_handle(),
                    reducers,
                    inputs,
                )
            return self.__handle.execute_reduce_by_field_owned(
                rust_database_for(connection),
                root.native_handle(),
                group.native_handle(),
                reducers,
                inputs,
            )
        if transaction is not None:
            return self.__handle.execute_reduce_by_borrowed(
                transaction,
                root.native_handle(),
                None if group is None else group.native_handle(),
                reducers,
                inputs,
            )
        return self.__handle.execute_reduce_by_owned(
            rust_database_for(connection),
            root.native_handle(),
            None if group is None else group.native_handle(),
            reducers,
            inputs,
        )

    def __materialize_reduction(
        self,
        result: ValidatedMatchResultHandle,
        term_count: int,
        *,
        group: BoundVar | BoundField | tuple[BoundField, ...] | None,
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

    def __fetch(
        self,
        offset: int,
        limit: int,
        cardinality: str,
        order_by: Iterable[QueryOrder],
    ) -> ValidatedMatchResultHandle:
        offset, limit = _window(offset, limit)
        handles = self.__order_handles(order_by)
        connection = self.__direct_connection()
        transaction = rust_transaction_for(connection)
        if transaction is not None:
            return self.__handle.execute_fetch_rows_borrowed(
                transaction,
                handles,
                offset,
                limit,
                cardinality,
            )
        return self.__handle.execute_fetch_rows_owned(
            rust_database_for(connection),
            handles,
            offset,
            limit,
            cardinality,
        )

    def __materialize_row(self, row: ValidatedMatchRowHandle) -> object:
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

    def __materialize_page(self, result: ValidatedMatchResultHandle) -> Page:
        return Page(
            (
                self.__materialize_row(result.page_entry(index))
                for index in range(result.page_entry_count())
            ),
            offset=result.page_offset(),
            limit=result.page_limit(),
            total=result.page_total(),
        )

    def __order_handles(self, order_by: Iterable[object]) -> list[MatchOrderHandle]:
        handles: list[MatchOrderHandle] = []
        for order in order_by:
            if not isinstance(order, QueryOrder):
                raise TypeError("order_by entries must be generated QueryOrder values")
            _require_projection(order.projection_identity(), self.__projection, "query order")
            handles.append(order.native_handle())
        return handles

    def __direct_connection(self) -> Database | TransactionContext:
        if self.__connection is None:
            raise RuntimeError("direct generated query requires a Database or TransactionContext")
        return self.__connection

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

    def materialize_row(self, row: ValidatedMatchRowHandle) -> object:
        return self.__materialize_row(row)

    def materialize_page(self, result: ValidatedMatchResultHandle) -> Page:
        return self.__materialize_page(result)

    def materialize_reduction(
        self,
        result: ValidatedMatchResultHandle,
        term_count: int,
        *,
        group: BoundVar | BoundField | tuple[BoundField, ...] | None,
    ) -> object:
        return self.__materialize_reduction(result, term_count, group=group)

    def order_handles(self, order_by: Iterable[object]) -> list[MatchOrderHandle]:
        return self.__order_handles(order_by)


class GroupedQuery(_Frozen):
    __slots__ = ("__group", "__query", "__root")
    __group: BoundVar | BoundField | tuple[BoundField, ...]
    __query: Query
    __root: BoundVar

    def __init__(
        self,
        query: Query,
        root: BoundVar,
        group: BoundVar | BoundField | tuple[BoundField, ...],
    ) -> None:
        object.__setattr__(self, "_GroupedQuery__query", query)
        object.__setattr__(self, "_GroupedQuery__root", root)
        object.__setattr__(self, "_GroupedQuery__group", group)

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

    def aggregate(self, *terms: Aggregate) -> tuple[tuple[object, tuple[object, ...]], ...]:
        result = self.__query.reduction_result(self.__root, self.__group, terms)
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
    __slots__ = ("__connection", "__handle", "__projection")
    __connection: Database | TransactionContext | None
    __handle: MatchSessionHandle
    __projection: PyRuntimeProjection

    def __init__(
        self,
        projection: PyRuntimeProjection,
        connection: Database | TransactionContext,
    ) -> None:
        if not _is_connection(connection):
            raise TypeError("generated QuerySession requires a Database or TransactionContext")
        object.__setattr__(self, "_QuerySession__projection", projection)
        object.__setattr__(self, "_QuerySession__connection", connection)
        object.__setattr__(self, "_QuerySession__handle", projection.match_session())

    @classmethod
    def remote_factory(cls, projection: PyRuntimeProjection) -> QuerySession:
        session = cls.__new__(cls)
        object.__setattr__(session, "_QuerySession__projection", projection)
        object.__setattr__(session, "_QuerySession__connection", None)
        object.__setattr__(session, "_QuerySession__handle", projection.match_session())
        return session

    def var(self, model: type[ModelBase], *, subtypes: bool = False) -> BoundVar:
        if type(subtypes) is not bool:
            raise TypeError("subtypes must be an exact bool")
        return self.__bind(model, subtypes=subtypes)

    def exact(self, model: type[ModelBase]) -> BoundVar:
        return self.var(model)

    def subtypes(self, model: type[ModelBase]) -> BoundVar:
        return self.var(model, subtypes=True)

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
                _depth(min_depth, "min_depth"),
                _depth(max_depth, "max_depth"),
            ),
            self.__projection,
        )

    def query(self, *selections: _Selection) -> Query:
        if not selections:
            raise TypeError("QuerySession.query requires at least one generated selection")
        if len(selections) > 16:
            raise ValueError("QuerySession.query supports at most sixteen generated selections")
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


class RemoteQueryLimits(_Frozen):
    __slots__ = (
        "__deadline_ms",
        "__max_attribute_values",
        "__max_bytes",
        "__max_collection_members",
        "__max_graph_nodes",
        "__max_items",
        "__max_role_players",
    )

    def __init__(
        self,
        *,
        max_items: int,
        max_bytes: int,
        max_collection_members: int,
        max_graph_nodes: int,
        max_attribute_values: int,
        max_role_players: int,
        deadline_ms: int | None = None,
    ) -> None:
        object.__setattr__(self, "_RemoteQueryLimits__max_items", max_items)
        object.__setattr__(self, "_RemoteQueryLimits__max_bytes", max_bytes)
        object.__setattr__(
            self,
            "_RemoteQueryLimits__max_collection_members",
            max_collection_members,
        )
        object.__setattr__(self, "_RemoteQueryLimits__max_graph_nodes", max_graph_nodes)
        object.__setattr__(
            self,
            "_RemoteQueryLimits__max_attribute_values",
            max_attribute_values,
        )
        object.__setattr__(self, "_RemoteQueryLimits__max_role_players", max_role_players)
        object.__setattr__(self, "_RemoteQueryLimits__deadline_ms", deadline_ms)

    @property
    def max_items(self) -> int:
        return self.__max_items

    @property
    def max_bytes(self) -> int:
        return self.__max_bytes

    @property
    def max_collection_members(self) -> int:
        return self.__max_collection_members

    @property
    def max_graph_nodes(self) -> int:
        return self.__max_graph_nodes

    @property
    def max_attribute_values(self) -> int:
        return self.__max_attribute_values

    @property
    def max_role_players(self) -> int:
        return self.__max_role_players

    @property
    def deadline_ms(self) -> int | None:
        return self.__deadline_ms


class RemoteQuery(_Frozen):
    __slots__ = ("__context", "__exchange", "__query")
    __context: RemoteModelQueryContext
    __exchange: Callable[[bytes], Awaitable[bytes]]
    __query: Query

    def __init__(
        self,
        query: Query,
        context: RemoteModelQueryContext,
        exchange: Callable[[bytes], Awaitable[bytes]],
    ) -> None:
        object.__setattr__(self, "_RemoteQuery__query", query)
        object.__setattr__(self, "_RemoteQuery__context", context)
        object.__setattr__(self, "_RemoteQuery__exchange", exchange)

    def match(self, *bindings: BoundVar) -> RemoteQuery:
        return RemoteQuery(self.__query.match(*bindings), self.__context, self.__exchange)

    def where(self, *predicates: Predicate) -> RemoteQuery:
        return RemoteQuery(self.__query.where(*predicates), self.__context, self.__exchange)

    def allow_cross_join(self, left: BoundVar, right: BoundVar) -> RemoteQuery:
        return RemoteQuery(
            self.__query.allow_cross_join(left, right),
            self.__context,
            self.__exchange,
        )

    async def one(self) -> object:
        pending = query_v2_prepare_remote_model_rows(
            self.__query.native_handle(),
            self.__context,
            [],
            0,
            1,
            "exactly_one",
        )
        result = await self.__execute(pending)
        if result.row_count() != 1:
            raise RuntimeError("validated remote exactly-one result did not contain one row")
        return self.__query.materialize_row(result.row(0))

    async def first(self, *, order_by: Iterable[QueryOrder] = ()) -> object | None:
        pending = query_v2_prepare_remote_model_rows(
            self.__query.native_handle(),
            self.__context,
            self.__query.order_handles(order_by),
            0,
            1,
            "bounded_many",
        )
        result = await self.__execute(pending)
        if result.row_count() == 0:
            return None
        return self.__query.materialize_row(result.row(0))

    async def rows(
        self,
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
    ) -> list[object]:
        offset, limit = _window(offset, limit)
        pending = query_v2_prepare_remote_model_rows(
            self.__query.native_handle(),
            self.__context,
            self.__query.order_handles(order_by),
            offset,
            limit,
            "bounded_many",
        )
        result = await self.__execute(pending)
        return [
            self.__query.materialize_row(result.row(index)) for index in range(result.row_count())
        ]

    async def page_by(
        self,
        root: BoundVar,
        *,
        limit: int,
        offset: int = 0,
        order_by: Iterable[QueryOrder] = (),
        include_total: bool = False,
    ) -> Page:
        _require_projection(
            root.projection_identity(),
            self.__query.projection_identity(),
            "remote query root",
        )
        if type(include_total) is not bool:
            raise TypeError("include_total must be an exact bool")
        offset, limit = _window(offset, limit)
        pending = query_v2_prepare_remote_model_page(
            self.__query.native_handle(),
            self.__context,
            root.native_handle(),
            self.__query.order_handles(order_by),
            offset,
            limit,
            include_total,
        )
        return self.__query.materialize_page(await self.__execute(pending))

    async def count_by(self, root: BoundVar) -> int:
        self.__require_root(root)
        pending = query_v2_prepare_remote_model_count(
            self.__query.native_handle(),
            self.__context,
            root.native_handle(),
        )
        return (await self.__execute(pending)).count_value()

    async def exists_by(self, root: BoundVar) -> bool:
        self.__require_root(root)
        pending = query_v2_prepare_remote_model_exists(
            self.__query.native_handle(),
            self.__context,
            root.native_handle(),
        )
        return (await self.__execute(pending)).exists_value()

    async def aggregate(self, root: BoundVar, *terms: Aggregate) -> tuple[object, ...]:
        result = await self.reduction_result(root, None, terms)
        materialized = self.__query.materialize_reduction(result, len(terms), group=None)
        if not _is_object_tuple(materialized):
            raise RuntimeError("validated remote aggregate returned an invalid result")
        return materialized

    def group_by(self, root: BoundVar, *groups: BoundVar | BoundField) -> RemoteGroupedQuery:
        self.__require_root(root, "remote aggregate root")
        return RemoteGroupedQuery(
            self,
            root,
            _require_aggregate_groups(
                groups,
                self.__query.projection_identity(),
                "remote aggregate group",
            ),
        )

    def __require_root(self, root: BoundVar, context: str = "remote query root") -> None:
        _require_projection(
            root.projection_identity(),
            self.__query.projection_identity(),
            context,
        )

    async def reduction_result(
        self,
        root: BoundVar,
        group: BoundVar | BoundField | tuple[BoundField, ...] | None,
        terms: tuple[Aggregate, ...],
    ) -> ValidatedMatchResultHandle:
        self.__require_root(root, "remote aggregate root")
        if isinstance(group, BoundVar):
            self.__require_root(group, "remote aggregate group")
        elif isinstance(group, BoundField):
            _require_projection(
                group.projection_identity(),
                self.__query.projection_identity(),
                "remote aggregate group field",
            )
        elif isinstance(group, tuple):
            for field in group:
                _require_projection(
                    field.projection_identity(),
                    self.__query.projection_identity(),
                    "remote aggregate group field",
                )
        reducers, inputs = _prepare_aggregate_terms(
            terms,
            self.__query.projection_identity(),
        )
        if isinstance(group, tuple):
            pending = query_v2_prepare_remote_model_reduce_by_fields(
                self.__query.native_handle(),
                self.__context,
                root.native_handle(),
                [field.native_handle() for field in group],
                reducers,
                inputs,
            )
        elif isinstance(group, BoundField):
            pending = query_v2_prepare_remote_model_reduce_by_field(
                self.__query.native_handle(),
                self.__context,
                root.native_handle(),
                group.native_handle(),
                reducers,
                inputs,
            )
        else:
            pending = query_v2_prepare_remote_model_reduce(
                self.__query.native_handle(),
                self.__context,
                root.native_handle(),
                None if group is None else group.native_handle(),
                reducers,
                inputs,
            )
        return await self.__execute(pending)

    def materialize_reduction(
        self,
        result: ValidatedMatchResultHandle,
        term_count: int,
        *,
        group: BoundVar | BoundField | tuple[BoundField, ...] | None,
    ) -> object:
        return self.__query.materialize_reduction(result, term_count, group=group)

    async def __execute(
        self,
        pending: PendingRemoteModelQuery,
    ) -> ValidatedMatchResultHandle:
        response = await self.__exchange(pending.request_bytes())
        if type(response) is not bytes:
            raise TypeError("generated remote query exchange must return exact bytes")
        return pending.decode_reply(response)


class RemoteGroupedQuery(_Frozen):
    __slots__ = ("__group", "__query", "__root")
    __group: BoundVar | BoundField | tuple[BoundField, ...]
    __query: RemoteQuery
    __root: BoundVar

    def __init__(
        self,
        query: RemoteQuery,
        root: BoundVar,
        group: BoundVar | BoundField | tuple[BoundField, ...],
    ) -> None:
        object.__setattr__(self, "_RemoteGroupedQuery__query", query)
        object.__setattr__(self, "_RemoteGroupedQuery__root", root)
        object.__setattr__(self, "_RemoteGroupedQuery__group", group)

    def match(self, *bindings: BoundVar) -> RemoteGroupedQuery:
        return RemoteGroupedQuery(self.__query.match(*bindings), self.__root, self.__group)

    def where(self, *predicates: Predicate) -> RemoteGroupedQuery:
        return RemoteGroupedQuery(self.__query.where(*predicates), self.__root, self.__group)

    def allow_cross_join(self, left: BoundVar, right: BoundVar) -> RemoteGroupedQuery:
        return RemoteGroupedQuery(
            self.__query.allow_cross_join(left, right),
            self.__root,
            self.__group,
        )

    async def aggregate(
        self,
        *terms: Aggregate,
    ) -> tuple[tuple[object, tuple[object, ...]], ...]:
        result = await self.__query.reduction_result(self.__root, self.__group, terms)
        materialized = self.__query.materialize_reduction(
            result,
            len(terms),
            group=self.__group,
        )
        if not _is_grouped_reduction(materialized):
            raise RuntimeError("validated remote grouped aggregate returned an invalid result")
        return materialized


class RemoteQuerySession(_Frozen):
    __slots__ = ("__context", "__direct", "__exchange")
    __context: RemoteModelQueryContext
    __direct: QuerySession
    __exchange: Callable[[bytes], Awaitable[bytes]]

    def __init__(
        self,
        authority: object,
        advertisement: object,
        exchange: object,
        limits: object,
    ) -> None:
        if not isinstance(authority, QueryV2Authority):
            raise TypeError("generated RemoteQuerySession requires a QueryV2Authority")
        if type(advertisement) is not bytes:
            raise TypeError("generated remote advertisement must be exact bytes")
        if not _is_exchange(exchange):
            raise TypeError("generated remote exchange must be callable")
        if type(limits) is not RemoteQueryLimits:
            raise TypeError("generated remote limits must be RemoteQueryLimits")
        context = query_v2_remote_model_context(
            authority,
            advertisement,
            limits.max_items,
            limits.max_bytes,
            limits.max_collection_members,
            limits.max_graph_nodes,
            limits.max_attribute_values,
            limits.max_role_players,
            limits.deadline_ms,
        )
        object.__setattr__(self, "_RemoteQuerySession__context", context)
        object.__setattr__(self, "_RemoteQuerySession__exchange", exchange)
        object.__setattr__(
            self,
            "_RemoteQuerySession__direct",
            QuerySession.remote_factory(_installed_projection()),
        )

    def var(self, model: type[ModelBase], *, subtypes: bool = False) -> BoundVar:
        return self.__direct.var(model, subtypes=subtypes)

    def exact(self, model: type[ModelBase]) -> BoundVar:
        return self.__direct.exact(model)

    def subtypes(self, model: type[ModelBase]) -> BoundVar:
        return self.__direct.subtypes(model)

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
        return self.__direct.reachable(
            source,
            target,
            relation,
            role_from,
            role_to,
            min_depth=min_depth,
            max_depth=max_depth,
        )

    def query(self, *selections: _Selection) -> RemoteQuery:
        return RemoteQuery(
            self.__direct.query(*selections),
            self.__context,
            self.__exchange,
        )

    def query_as(
        self,
        declaration: type[object],
        /,
        **selections: _Selection,
    ) -> RemoteQuery:
        return RemoteQuery(
            self.__direct.query_as(declaration, **selections),
            self.__context,
            self.__exchange,
        )


def _require_projection(
    actual: PyRuntimeProjection,
    expected: PyRuntimeProjection,
    kind: str,
) -> None:
    if actual is not expected:
        raise TypeError(f"generated {kind} must belong to the same installed package")


def _depth(value: int, name: str) -> int:
    if type(value) is not int:
        raise TypeError(f"{name} must be an exact int")
    if not 0 <= value <= 255:
        raise ValueError(f"{name} must be between zero and 255")
    return value


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


def _type_id_label(value: object) -> str:
    if not _is_object_dict(value):
        raise TypeError("generated model identity must be an object")
    label = value.get("label")
    if not isinstance(label, str):
        raise TypeError("generated model identity has no string label")
    return label


def _is_untyped_dict(value: object) -> TypeGuard[dict[object, object]]:
    return isinstance(value, dict)


def _is_connection(value: object) -> TypeGuard[Database | TransactionContext]:
    return isinstance(value, (Database, TransactionContext))


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


def _window(offset: int, limit: int) -> tuple[int, int]:
    if type(offset) is not int or offset < 0:
        raise ValueError("generated query offset must be a non-negative integer")
    if type(limit) is not int or limit < 1:
        raise ValueError("generated query limit must be a positive integer")
    return offset, limit


def _prepare_aggregate_terms(
    terms: tuple[Aggregate, ...],
    projection: PyRuntimeProjection,
) -> tuple[list[_Reducer], list[MatchFieldHandle | None]]:
    if not 1 <= len(terms) <= 16:
        raise ValueError("generated aggregate requires between one and sixteen terms")
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
    "Predicate",
    "Query",
    "QueryOrder",
    "QuerySession",
    "GroupedQuery",
    "Page",
    "RemoteGroupedQuery",
    "RemoteQuery",
    "RemoteQueryLimits",
    "RemoteQuerySession",
    "SubtypeBoundVar",
    "aggregate",
]
