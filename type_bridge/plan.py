"""Typed ``query-plan/v3`` plans and staged transactions.

``PlanBuilder`` writes one plan from generated model, field and role tokens
and forwards every call to the shared Rust builder, so a Python plan has the
same canonical bytes as the same plan built in Rust, TypeScript or C::

    plan = Plan.builder()
    item = plan.exact(Item)
    stock = plan.field(item, Item.stock)
    plan.where_(stock.gt(0))
    plan.update(item, Item.stock, plan.let_("next_stock", stock - 1))
    built = plan.build(my_package.schema_authority())

    with db.staged_transaction() as tx:
        answer = tx.stage(built)
    assert answer.result() == Matched(1)

Names follow the Rust builder; ``where_`` and ``let_`` avoid the Python
keywords. Calls never fail on content: the first invalid call is raised by
``build`` as a ``QueryV2Error`` with ``.code``. A handle from another
builder raises ``TypeError`` at once.
"""

from __future__ import annotations

import datetime as _dt
import json
from collections.abc import Iterable, Mapping
from decimal import Decimal
from typing import Any, Final, Literal, Protocol

from type_bridge_core import (
    Count,
    Exists,
    Matched,
    Pending,
    PlanAnswer,
    QueryV2Authority,
    Rows,
    Transaction,
    _Plan,
    _PlanBuilderCore,
)

__all__ = [
    "Count",
    "Exists",
    "Matched",
    "Pending",
    "Plan",
    "PlanBuilder",
    "PlanCondition",
    "PlanExpr",
    "PlanInput",
    "PlanInputCell",
    "PlanLiteral",
    "PlanNewThing",
    "PlanThing",
    "PlanValue",
    "PlanAnswer",
    "Rows",
    "Transaction",
    "literal",
]

PlanValueType = Literal[
    "string",
    "long",
    "double",
    "boolean",
    "date",
    "datetime",
    "datetime_tz",
    "decimal",
    "duration",
]

_COMPARATORS: Final = frozenset(
    ("equal", "not_equal", "less", "less_or_equal", "greater", "greater_or_equal")
)


class _FieldToken(Protocol):
    """A generated field token, such as ``Item.stock``."""

    @property
    def owner(self) -> type[Any]: ...
    @property
    def fact(self) -> Mapping[str, object]: ...


class _RoleToken(Protocol):
    """A generated role token, such as ``Purchase.buyer``."""

    @property
    def owner(self) -> type[Any]: ...
    @property
    def fact(self) -> Mapping[str, object]: ...


class PlanLiteral:
    """A literal with an explicit value type, for values Python cannot type.

    Plain ``bool``, ``int``, ``float``, ``str``, ``Decimal``, ``date`` and
    ``datetime`` values are typed automatically; use ``literal("duration",
    "P1D")`` for the rest.
    """

    __slots__ = ("value", "value_type")

    def __init__(self, value_type: PlanValueType, value: object) -> None:
        self.value_type: PlanValueType = value_type
        self.value = value

    def __repr__(self) -> str:
        return f"literal({self.value_type!r}, {self.value!r})"


def literal(value_type: PlanValueType, value: object) -> PlanLiteral:
    """Type ``value`` as ``value_type`` in a plan."""
    return PlanLiteral(value_type, value)


type _Scalar = bool | int | float | str | Decimal | _dt.date | PlanLiteral
type _Operand = _Comparable | PlanThing | _Scalar
type _ExprLike = PlanExpr | _Operand


def _literal_parts(value: _Scalar, value_type: PlanValueType | None) -> tuple[str, object]:
    if isinstance(value, PlanLiteral):
        return value.value_type, value.value
    if value_type is not None:
        return value_type, _host_text(value)
    if isinstance(value, bool):
        return "boolean", value
    if isinstance(value, int):
        return "long", value
    if isinstance(value, float):
        return "double", value
    if isinstance(value, str):
        return "string", value
    if isinstance(value, Decimal):
        return "decimal", _host_text(value)
    if isinstance(value, _dt.datetime):
        return ("datetime" if value.tzinfo is None else "datetime_tz"), value.isoformat()
    if isinstance(value, _dt.date):
        return "date", value.isoformat()
    raise TypeError(f"plan literals must be typed values, not {type(value).__name__}")


def _host_text(value: object) -> object:
    if isinstance(value, Decimal):
        return format(value, "f")
    if isinstance(value, _dt.date):
        return value.isoformat()
    return value


class _Handle:
    __slots__ = ("_index", "_owner")

    def __init__(self, owner: PlanBuilder, index: int) -> None:
        self._owner = owner
        self._index = index


class _Comparable:
    """Comparisons and arithmetic shared by values and inputs."""

    __slots__ = ()

    def eq(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "equal", other)

    def ne(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "not_equal", other)

    def lt(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "less", other)

    def le(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "less_or_equal", other)

    def gt(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "greater", other)

    def ge(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "greater_or_equal", other)

    def __add__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("add", self, other)

    def __sub__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("sub", self, other)

    def __mul__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("mul", self, other)

    def __truediv__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("div", self, other)

    def __mod__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("mod", self, other)


class PlanThing(_Handle):
    """A matched, inserted or put thing of ``model``."""

    __slots__ = ("model",)

    def __init__(self, owner: PlanBuilder, index: int, model: type[Any]) -> None:
        super().__init__(owner, index)
        self.model = model


class PlanValue(_Handle, _Comparable):
    """A value binding: a field of a thing or a ``let_`` result."""

    __slots__ = ()


class PlanInput(_Handle, _Comparable):
    """A typed input column; rows give it values."""

    __slots__ = ("value_type",)

    def __init__(self, owner: PlanBuilder, index: int, value_type: PlanValueType) -> None:
        super().__init__(owner, index)
        self.value_type: PlanValueType = value_type

    def value(self, value: _Scalar) -> PlanInputCell:
        """This column's value in one row."""
        return PlanInputCell(self, value)

    def absent(self) -> PlanInputCell:
        """This optional column has no value in one row."""
        return PlanInputCell(self, None)


class PlanInputCell:
    """One cell of an input row; see ``PlanInput.value``."""

    __slots__ = ("input", "value")

    def __init__(self, input: PlanInput, value: _Scalar | None) -> None:
        self.input = input
        self.value = value


class PlanCondition:
    """``left comparator right``; pass it to ``PlanBuilder.where_``."""

    __slots__ = ("comparator", "left", "right")

    def __init__(self, left: _Operand, comparator: str, right: _Operand) -> None:
        if comparator not in _COMPARATORS:
            raise ValueError(f"unknown plan comparator {comparator!r}")
        self.left = left
        self.comparator = comparator
        self.right = right


class PlanExpr:
    """An arithmetic or builtin expression; bind it with ``let_``."""

    __slots__ = ("_node",)

    def __init__(self, node: tuple[Any, ...]) -> None:
        self._node = node

    @staticmethod
    def _of(value: _ExprLike) -> PlanExpr:
        return value if isinstance(value, PlanExpr) else PlanExpr(("operand", value))

    @staticmethod
    def _arith(op: str, lhs: _ExprLike, rhs: _ExprLike) -> PlanExpr:
        return PlanExpr(("arith", op, PlanExpr._of(lhs), PlanExpr._of(rhs)))

    @staticmethod
    def _call(func: str, *args: _ExprLike) -> PlanExpr:
        return PlanExpr(("call", func, tuple(PlanExpr._of(arg) for arg in args)))

    def __add__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("add", self, other)

    def __sub__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("sub", self, other)

    def __mul__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("mul", self, other)

    def __truediv__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("div", self, other)

    def __mod__(self, other: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("mod", self, other)

    @staticmethod
    def pow(base: _ExprLike, exponent: _ExprLike) -> PlanExpr:
        return PlanExpr._arith("pow", base, exponent)

    @staticmethod
    def abs(value: _ExprLike) -> PlanExpr:
        return PlanExpr._call("abs", value)

    @staticmethod
    def ceil(value: _ExprLike) -> PlanExpr:
        return PlanExpr._call("ceil", value)

    @staticmethod
    def floor(value: _ExprLike) -> PlanExpr:
        return PlanExpr._call("floor", value)

    @staticmethod
    def round(value: _ExprLike) -> PlanExpr:
        return PlanExpr._call("round", value)

    @staticmethod
    def min(left: _ExprLike, right: _ExprLike) -> PlanExpr:
        return PlanExpr._call("min", left, right)

    @staticmethod
    def max(left: _ExprLike, right: _ExprLike) -> PlanExpr:
        return PlanExpr._call("max", left, right)

    @staticmethod
    def length(value: _ExprLike) -> PlanExpr:
        return PlanExpr._call("length", value)


class PlanNewThing:
    """An inserted or put thing; give it fields and players."""

    __slots__ = ("_builder", "_thing")

    def __init__(self, builder: PlanBuilder, thing: PlanThing) -> None:
        self._builder = builder
        self._thing = thing

    def field(self, token: _FieldToken, value: _Operand) -> PlanNewThing:
        """Give the thing a field value."""
        builder = self._builder
        builder._native.thing_field(
            self._thing._index, _owns_json(token), builder._operand(value, None)
        )
        return self

    def player(self, token: _RoleToken, player: PlanThing) -> PlanNewThing:
        """Give the relation a role player."""
        builder = self._builder
        builder._native.thing_player(
            self._thing._index, _role_json(token), builder._own(player)._index
        )
        return self

    def thing(self) -> PlanThing:
        """The thing's handle, for later stages and ``returning``."""
        return self._thing


def _type_json(model: type[Any]) -> str:
    identity = getattr(model, "__type_id__", None)
    if not isinstance(identity, str):
        raise TypeError("plan models must be generated model classes with __type_id__")
    return identity


def _owns_json(token: _FieldToken) -> str:
    fact = getattr(token, "fact", None)
    identity = fact.get("id") if isinstance(fact, Mapping) else None
    if not isinstance(identity, Mapping):
        raise TypeError("plan fields must be generated field tokens such as Item.stock")
    return json.dumps(identity)


def _role_json(token: _RoleToken) -> str:
    fact = getattr(token, "fact", None)
    role = fact.get("role") if isinstance(fact, Mapping) else None
    if not isinstance(role, Mapping):
        raise TypeError("plan roles must be generated role tokens such as Purchase.buyer")
    return json.dumps(role)


class PlanBuilder:
    """One plan under construction; see the module documentation.

    Handles are dense in creation order. Matches, fields, conditions and
    ``let_`` bindings join the one leading match in call order; writes
    follow in call order, and consecutive writes of one kind share a stage.
    """

    __slots__ = ("_native",)

    def __init__(self) -> None:
        self._native = _PlanBuilderCore()

    def _own[H: _Handle](self, handle: H) -> H:
        if not isinstance(handle, _Handle) or handle._owner is not self:
            raise TypeError("a plan handle does not belong to this builder")
        return handle

    def _operand(
        self, value: _Operand, value_type: PlanValueType | None
    ) -> tuple[Literal["binding", "input", "literal"], int]:
        if isinstance(value, PlanThing | PlanValue):
            return "binding", self._own(value)._index
        if isinstance(value, PlanInput):
            return "input", self._own(value)._index
        if isinstance(value, _Handle | _Comparable):
            raise TypeError("a plan handle does not belong to this builder")
        kind, host = _literal_parts(value, value_type)
        return "literal", self._native.literal(kind, host)

    def _expr(self, expr: PlanExpr) -> int:
        node = expr._node
        if node[0] == "operand":
            return self._native.expr_operand(self._operand(node[1], None))
        if node[0] == "arith":
            return self._native.expr_arith(node[1], self._expr(node[2]), self._expr(node[3]))
        return self._native.expr_call(node[1], [self._expr(arg) for arg in node[2]])

    def exact(self, model: type[Any]) -> PlanThing:
        """Match things of exactly ``model``."""
        return PlanThing(self, self._native.exact(_type_json(model)), model)

    def subtypes(self, model: type[Any]) -> PlanThing:
        """Match things of ``model`` or any subtype."""
        return PlanThing(self, self._native.subtypes(_type_json(model)), model)

    def field(self, thing: PlanThing, token: _FieldToken) -> PlanValue:
        """Bind ``thing``'s field value."""
        self._own(thing)
        owner = getattr(token, "owner", None)
        if isinstance(owner, type) and not issubclass(thing.model, owner):
            raise TypeError(
                f"field token of {owner.__name__} does not belong to {thing.model.__name__}"
            )
        return PlanValue(self, self._native.field(thing._index, _owns_json(token)))

    def input(self, name: str, value_type: PlanValueType) -> PlanInput:
        """Declare a required input column named ``name``."""
        return PlanInput(self, self._native.input(name, value_type, False), value_type)

    def optional_input(self, name: str, value_type: PlanValueType) -> PlanInput:
        """Declare an input column a row may leave absent."""
        return PlanInput(self, self._native.input(name, value_type, True), value_type)

    def where_(self, condition: PlanCondition) -> PlanBuilder:
        """Require ``condition`` of every matched row."""
        typed = (
            condition.left.value_type
            if isinstance(condition.left, PlanInput)
            else condition.right.value_type
            if isinstance(condition.right, PlanInput)
            else None
        )
        left = self._operand(condition.left, typed)
        right = self._operand(condition.right, typed)
        self._native.compare(left, condition.comparator, right)
        return self

    def links(self, relation: PlanThing, role: _RoleToken, player: PlanThing) -> PlanBuilder:
        """Require ``relation`` to link ``player`` in ``role``."""
        self._native.links(self._own(relation)._index, _role_json(role), self._own(player)._index)
        return self

    def let_(self, name: str, expr: _ExprLike) -> PlanValue:
        """Bind ``name`` to ``expr``, evaluated for each matched row."""
        return PlanValue(self, self._native.let_(name, self._expr(PlanExpr._of(expr))))

    def update(self, owner: PlanThing, field: _FieldToken, value: _Operand) -> PlanBuilder:
        """Replace ``owner``'s field value with ``value``."""
        self._native.update(self._own(owner)._index, _owns_json(field), self._operand(value, None))
        return self

    def update_player(
        self, relation: PlanThing, role: _RoleToken, player: PlanThing
    ) -> PlanBuilder:
        """Make ``player`` the role player of ``relation``."""
        self._native.update_player(
            self._own(relation)._index, _role_json(role), self._own(player)._index
        )
        return self

    def insert(self, model: type[Any]) -> PlanNewThing:
        """Create a thing of ``model``."""
        index = self._native.insert(_type_json(model))
        return PlanNewThing(self, PlanThing(self, index, model))

    def put(self, model: type[Any]) -> PlanNewThing:
        """Match a thing with the given fields and players, or create it."""
        index = self._native.put(_type_json(model))
        return PlanNewThing(self, PlanThing(self, index, model))

    def delete(self, thing: PlanThing) -> PlanBuilder:
        """Delete ``thing``."""
        self._native.delete(self._own(thing)._index)
        return self

    def delete_field(self, owner: PlanThing, field: _FieldToken) -> PlanBuilder:
        """Delete ``owner``'s field, keeping the thing."""
        self._native.delete_field(self._own(owner)._index, _owns_json(field))
        return self

    def delete_player(
        self, relation: PlanThing, role: _RoleToken, player: PlanThing
    ) -> PlanBuilder:
        """Remove ``player`` from ``relation``."""
        self._native.delete_player(
            self._own(relation)._index, _role_json(role), self._own(player)._index
        )
        return self

    def row(self, cells: Iterable[PlanInputCell]) -> PlanBuilder:
        """Add one input row; a column left out is absent."""
        lowered: list[tuple[int, int | None]] = []
        for cell in cells:
            column = self._own(cell.input)
            value = None if cell.value is None else self._operand(cell.value, column.value_type)[1]
            lowered.append((column._index, value))
        self._native.row(lowered)
        return self

    def returning(self, columns: Iterable[PlanThing | PlanValue]) -> PlanBuilder:
        """Answer rows of these bindings, in order."""
        self._native.returning([self._own(column)._index for column in columns])
        return self

    def count(self) -> PlanBuilder:
        """Answer the number of rows a read plan matches."""
        self._native.count()
        return self

    def exists(self) -> PlanBuilder:
        """Answer whether a read plan matches any row."""
        self._native.exists()
        return self

    def build(self, authority: QueryV2Authority) -> Plan:
        """Validate the plan under a schema authority, such as a generated
        package's ``schema_authority()``."""
        return Plan(self._native.build(authority))


class Plan(_Plan):
    """One validated plan: ``canonical_bytes()``, ``fingerprint()`` and
    ``writes()``. Stage it with ``Transaction.stage``."""

    @staticmethod
    def builder() -> PlanBuilder:
        """Start a plan."""
        return PlanBuilder()
