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
``build`` as a ``PlanError`` with ``.code``. A handle from another
builder raises ``TypeError`` at once.
"""

from __future__ import annotations

import datetime as _dt
import json
from collections.abc import Callable, Iterable, Mapping, Sequence
from contextvars import ContextVar, Token
from dataclasses import dataclass
from decimal import Decimal
from typing import Any, Final, Literal, Never, NotRequired, Protocol, Self, TypedDict
from weakref import ReferenceType, ref

from type_bridge_core import (
    Count,
    Documents,
    Exists,
    Iid,
    Matched,
    Pending,
    PlanAnswer,
    PlanAuthority,
    PlanError,
    QueryLimits,
    Rows,
    StagedPlan,
    Transaction,
    _Plan,
    _PlanBuilderCore,
)

__all__ = [
    "Condition",
    "Count",
    "Documents",
    "Exists",
    "Expr",
    "FieldToken",
    "Handle",
    "Matched",
    "Pending",
    "Plan",
    "PlanExplanation",
    "PlanError",
    "PlanBuilder",
    "PlanCondition",
    "PlanExpr",
    "PlanInput",
    "PlanInputCell",
    "PlanLiteral",
    "PlanNewThing",
    "PlanThing",
    "PlanTemplate",
    "PlanValue",
    "PlanAnswer",
    "Rows",
    "StagedPlan",
    "StagedResultRef",
    "Transaction",
    "Value",
    "literal",
]

PlanValueType = Literal[
    "iid",
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
    (
        "equal",
        "not_equal",
        "less",
        "less_or_equal",
        "greater",
        "greater_or_equal",
        "contains",
        "starts_with",
        "ends_with",
        "like",
    )
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


class PlanLiteral[T]:
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


type _Scalar = bool | int | float | str | Iid | Decimal | _dt.date | _dt.timedelta | PlanLiteral
type _Operand = _Comparable | PlanThing | _Scalar
type _ExprLike = PlanExpr | _Operand


def _iid_value(value: object) -> Iid:
    if not isinstance(value, Iid):
        raise TypeError("IID input values require Iid, not a plain string")
    return value


def _literal_parts(
    value: _Scalar, value_type: PlanValueType | None
) -> tuple[PlanValueType, object]:
    if value_type == "iid":
        return "string", _iid_value(value)
    if isinstance(value, PlanLiteral):
        if value.value_type == "iid":
            return "string", _iid_value(value.value)
        return value.value_type, value.value
    if value_type is not None:
        return value_type, _host_text(value)
    if isinstance(value, bool):
        return "boolean", value
    if isinstance(value, int):
        return "long", value
    if isinstance(value, float):
        return "double", value
    if isinstance(value, Iid):
        return "string", value
    if isinstance(value, str):
        return "string", value
    if isinstance(value, Decimal):
        return "decimal", _host_text(value)
    if isinstance(value, _dt.datetime):
        return ("datetime" if value.tzinfo is None else "datetime_tz"), value.isoformat()
    if isinstance(value, _dt.date):
        return "date", value.isoformat()
    if isinstance(value, _dt.timedelta):
        return "duration", value
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

    def contains(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "contains", other)

    def starts_with(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "starts_with", other)

    def ends_with(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "ends_with", other)

    def like(self, other: _Operand) -> PlanCondition:
        return PlanCondition(self, "like", other)

    __eq__ = eq  # pyright: ignore[reportAssignmentType] -- DSL comparisons record conditions.
    __ne__ = ne  # pyright: ignore[reportAssignmentType]
    __lt__ = lt
    __le__ = le
    __gt__ = gt
    __ge__ = ge
    __hash__ = object.__hash__

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


class PlanInput[T](_Handle, _Comparable):
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


@dataclass(frozen=True)
class StagedResultRef:
    """One template column reads an earlier selected thing IID."""

    input: PlanInput[Iid]
    source_plan: int
    source_column: int
    expected_types: Sequence[type[Any]]

    def __post_init__(self) -> None:
        if self.input.value_type != "iid":
            raise TypeError("staged IID result references require an IID input column")


class PlanInputCell[T]:
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

    def __bool__(self) -> bool:
        raise TypeError("plan conditions must be passed to where() or require()")


class PlanExpr[T]:
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
    identity = fact.get("declaring_id", fact.get("id")) if isinstance(fact, Mapping) else None
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

    def __init__(self, *, query_limits: QueryLimits | None = None) -> None:
        self._native = _PlanBuilderCore(query_limits)

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

    def by_iid_input(self, thing: PlanThing, column: PlanInput[Iid]) -> PlanBuilder:
        """Restrict a matched thing to an ``input(name, "iid")`` column."""
        column = self._own(column)
        if column.value_type != "iid":
            raise TypeError("IID predicates require an IID input column")
        self._native.by_iid_input(self._own(thing)._index, column._index)
        return self

    def _check_field(self, thing: PlanThing, token: _FieldToken) -> None:
        self._own(thing)
        owner = getattr(token, "owner", None)
        if isinstance(owner, type) and not issubclass(thing.model, owner):
            raise TypeError(
                f"field token of {owner.__name__} does not belong to {thing.model.__name__}"
            )

    def field(self, thing: PlanThing, token: _FieldToken) -> PlanValue:
        """Bind ``thing``'s field value."""
        self._check_field(thing, token)
        return PlanValue(self, self._native.field(thing._index, _owns_json(token)))

    def input(self, name: str, value_type: PlanValueType) -> PlanInput:
        """Declare a required input column named ``name``."""
        return PlanInput(
            self,
            self._native.input(name, "string" if value_type == "iid" else value_type, False),
            value_type,
        )

    def optional_input(self, name: str, value_type: PlanValueType) -> PlanInput:
        """Declare an input column a row may leave absent."""
        return PlanInput(
            self,
            self._native.input(name, "string" if value_type == "iid" else value_type, True),
            value_type,
        )

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

    def fetch(
        self, thing: PlanThing, roles: Iterable[_RoleToken | tuple[_RoleToken, bool]] = ()
    ) -> PlanBuilder:
        """Answer documents for ``thing`` and its listed role players.

        Bare roles are required; ``(role, False)`` permits an absent player.
        Call before ``delete(thing)`` to return a deletion snapshot.
        """
        lowered = [
            (_role_json(role[0]), role[1]) if isinstance(role, tuple) else (_role_json(role), True)
            for role in roles
        ]
        self._native.fetch(self._own(thing)._index, lowered)
        return self

    def count(self) -> PlanBuilder:
        """Answer the number of rows a read plan matches."""
        self._native.count()
        return self

    def exists(self) -> PlanBuilder:
        """Answer whether a read plan matches any row."""
        self._native.exists()
        return self

    def build(self, authority: PlanAuthority) -> Plan:
        """Validate the plan under a schema authority, such as a generated
        package's ``schema_authority()``."""
        plan = Plan(self._native.build(authority))
        plan._builder = self
        plan._authority = authority
        return plan

    def build_staged(
        self,
        authority: PlanAuthority,
        rows: Iterable[Iterable[PlanInputCell]],
        result_refs: Iterable[StagedResultRef] = (),
    ) -> StagedPlan:
        """Finalize templates for ``Transaction.stage_stack``.

        Referenced cells use ``column.absent()``; their values bind inside the
        transaction. Source indexes are relative to that complete stack.
        """
        lowered = [
            [
                (
                    self._own(cell.input)._index,
                    None
                    if cell.value is None
                    else _literal_parts(cell.value, cell.input.value_type)[1],
                )
                for cell in row
            ]
            for row in rows
        ]
        references = [
            {
                "input": self._own(reference.input)._index,
                "source_plan": reference.source_plan,
                "source_column": reference.source_column,
                "expected_types": [
                    json.loads(_type_json(model)) for model in reference.expected_types
                ],
            }
            for reference in result_refs
        ]
        return self._native.build_staged(authority, lowered, json.dumps(references))


class ExplainedType(TypedDict):
    """One possible schema type."""

    kind: Literal["entity", "relation", "attribute"]
    label: str


class ExplainedBinding(TypedDict):
    """Binding identity, provider variable, and possible schema types."""

    id: int
    variable: str
    possible_types: list[ExplainedType]


class ExplainedInput(TypedDict):
    """Input declaration without its transported values."""

    public_name: str
    value_type: PlanValueType
    optional: bool


class ExplainedFingerprint(TypedDict):
    """Canonical plan-shape fingerprint, excluding transported caller values."""

    algorithm: str
    canonicalization: str
    digest: str
    domain: str


class ExplainedOperation(TypedDict):
    """Terminal kind and optional write result kind."""

    kind: Literal["rows", "count", "exists", "write"]
    returns: NotRequired[Literal["rows", "matched"]]


class ExplainedGiven(TypedDict):
    """Transport size without the input values."""

    row_count: int
    canonical_bytes: int


@dataclass(frozen=True)
class PlanExplanation:
    """Exact provider program. Given values are omitted; plan constants remain."""

    fingerprint: ExplainedFingerprint
    format: str
    required_capabilities: list[str]
    operation: ExplainedOperation
    bindings: list[ExplainedBinding]
    inputs: list[ExplainedInput]
    given: ExplainedGiven
    statements: list[str]
    read_statements: int
    write_statements: int


_current_plan: ContextVar[Plan[Any] | None] = ContextVar("type_bridge_plan", default=None)
_MISSING = object()
Condition = PlanCondition
Expr = PlanExpr


def _bad_answer() -> Never:
    raise TypeError("the plan answer does not match its selected result")


def _context_error(code: str, message: str) -> PlanError:
    error = PlanError(f"{code}: {message}")
    error.code = code
    error.message = message
    error.category = "invalid_contract"
    error.path = []
    error.details = {}
    return error


class FieldToken[T](_Comparable):
    """A generated field used relative to the handle receiving ``where``."""

    __slots__ = ()
    owner: type[Any]
    fact: Mapping[str, object]


class Value[T](PlanValue):
    """A typed field binding, with deferred arithmetic and comparisons."""

    __slots__ = ("_site",)

    def __init__(self, owner: PlanBuilder, index: int, site: tuple[int, str]) -> None:
        super().__init__(owner, index)
        self._site = site


class Handle[M](PlanThing):
    """A generated model binding in the current plan context."""

    __slots__ = ("_context", "_values")

    def __init__(self, context: Plan[Any], index: int, model: type[M]) -> None:
        super().__init__(context._builder, index, model)
        self._context = context
        self._values: dict[str, Value[Any]] = {}

    def _check_open(self) -> None:
        if _current_plan.get() is not self._context:
            raise _context_error(
                "plan_context_missing", "use generated plan helpers inside with Plan()"
            )

    def _value[T](self, field: FieldToken[T]) -> Value[T]:
        self._check_open()
        self._owner._check_field(self, field)
        key = _owns_json(field)
        if key not in self._values:
            attribute = json.loads(_owns_json(field))["attribute"]
            index = self._owner._native.field_value(self._index, key)
            self._values[key] = Value(self._owner, index, (self._index, attribute))
        return self._values[key]

    def where(self, *conditions: Condition) -> Handle[M]:
        self._check_open()
        self._context._conditions(conditions, self)
        return self

    def _set(self, fields: Mapping[str, Any]) -> Plan[list[M]]:
        self._check_open()
        for name, value in fields.items():
            if value is _MISSING:
                continue
            token = getattr(self.model, name)
            self._owner._check_field(self, token)
            site = (self._index, json.loads(_owns_json(token))["attribute"])
            operand = self._context._assignment(value, site)
            self._owner.update(self, token, operand)
        return self._context._project(self)

    def unset(self, field: FieldToken[Any]) -> Plan[list[M]]:
        self._check_open()
        self._owner._check_field(self, field)
        self._owner.delete_field(self, field)
        return self._context._project(self)

    def _link(self, players: Mapping[str, Any], *, remove: bool) -> Plan[list[M]]:
        self._check_open()
        for name, player in players.items():
            if player is _MISSING:
                continue
            token = getattr(self.model, name)
            if remove:
                self._owner.delete_player(self, token, player)
            else:
                self._owner._native.link(
                    self._index, _role_json(token), self._owner._own(player)._index
                )
        return self._context._project(self)

    def delete(self) -> Plan[Matched]:
        self._check_open()
        self._owner.delete(self)
        return self._context._result("matched")

    def delete_returning(self) -> Plan[list[M]]:
        self._check_open()
        self._owner.delete(self)
        return self._context._project(self, deleted=True)


class Plan[R](_Plan):
    """One validated plan: ``canonical_bytes()``, ``fingerprint()`` and
    ``writes()``. Stage it with ``Transaction.stage``."""

    def explain(self) -> PlanExplanation:
        """Inspect the statements execution sends, omitting given values."""
        return PlanExplanation(**json.loads(self.explain_json()))

    def __new__(cls, plan: _Plan | None = None, *, query_limits: QueryLimits | None = None) -> Self:
        return super().__new__(cls, plan)

    def __init__(
        self, plan: _Plan | None = None, *, query_limits: QueryLimits | None = None
    ) -> None:
        if plan is not None and query_limits is not None:
            raise TypeError("query_limits belong to a new recording")
        self._builder = PlanBuilder(query_limits=query_limits)
        self._authority: PlanAuthority | None = None
        self._projection: Any = None
        self._results: list[ReferenceType[Plan[Any]]] = []
        self._token: Token[Plan[Any] | None] | None = None
        self._kind = "matched"
        self._thing: Handle[Any] | None = None
        self._deleted = False
        self._context: Plan[Any] = self
        self._finalized = plan is not None
        self._entered = False
        self._template = False
        self._columns: tuple[PlanThing | PlanValue, ...] = ()
        self._generated = plan is None
        if isinstance(plan, Plan):
            self._builder = plan._builder
            self._authority = plan._authority
            self._context = plan._context
            self._kind = plan._kind
            self._thing = plan._thing
            self._deleted = plan._deleted
            self._generated = plan._generated
            self._columns = plan._columns

    def __enter__(self) -> Plan[R]:
        if self._finalized or self._entered:
            raise _context_error("plan_already_built", "a plan context can be entered once")
        self._token = _current_plan.set(self)
        self._entered = True
        return self

    def __exit__(self, *exc: object) -> None:
        try:
            if exc[0] is not None:
                return
            if self._authority is None:
                raise _context_error(
                    "plan_context_missing", "a plan needs a generated model's schema authority"
                )
            held = [result for reference in self._results if (result := reference()) is not None]
            for result in [self, *held]:
                roles = (
                    tuple(
                        (_role_json(role), required)
                        for role, required in result._thing.model.__plan_roles__
                    )
                    if result._thing is not None
                    else ()
                )
                columns = tuple(self._builder._own(column)._index for column in result._columns)
                built = self._builder._native.build_selected(
                    self._authority,
                    result._kind,
                    result._thing._index if result._thing is not None else None,
                    roles,
                    columns,
                    self._projection,
                    result._thing.model.__type_id__ if result._thing is not None else None,
                    [
                        (column._index, column.model.__type_id__)
                        for column in result._columns
                        if isinstance(column, Handle)
                    ],
                )
                result._complete(built, result._decoder(built))
                result._finalized = True
        finally:
            if self._token is not None:
                _current_plan.reset(self._token)
                self._token = None

    @staticmethod
    def _current() -> Plan[Any]:
        plan = _current_plan.get()
        if plan is None:
            raise _context_error(
                "plan_context_missing", "use generated plan helpers inside with Plan()"
            )
        return plan

    def _use_package(self, authority: PlanAuthority, projection: Any) -> None:
        if self._authority is not None and self._authority is not authority:
            raise _context_error(
                "plan_schema_mismatch", "one plan uses one generated schema package"
            )
        self._authority = authority
        self._projection = projection

    def _result(self, kind: str) -> Plan[Any]:
        if self._current() is not self:
            raise _context_error("plan_context_missing", "the result belongs to the open plan")
        result: Plan[Any] = Plan()
        result._context = self
        result._entered = True
        result._kind = kind
        self._results.append(ref(result))
        return result

    def _project[M](self, thing: Handle[M], *, deleted: bool = False) -> Plan[list[M]]:
        result: Plan[list[M]] = self._result("models")
        result._thing = thing
        result._deleted = deleted
        return result

    def _bound_value(self, value: _Scalar, site: tuple[int, str]) -> _Operand:
        if self._template and isinstance(value, PlanLiteral):
            return value
        kind, host = _literal_parts(value, None)
        index = self._builder._native.bind_value(site[0], site[1], kind, host)
        return PlanInput(self._builder, index, kind)

    def _resolve(self, value: Any, handle: Handle[Any] | None) -> Any:
        if isinstance(value, FieldToken):
            if handle is None:
                raise TypeError(
                    "class field tokens need a handle's where(); use bound values in require()"
                )
            return handle._value(value)
        return value

    def _conditions(self, conditions: Iterable[Condition], handle: Handle[Any] | None) -> None:
        for condition in conditions:
            left = self._resolve(condition.left, handle)
            right = self._resolve(condition.right, handle)
            site_value = left if isinstance(left, Value) else right
            if isinstance(site_value, Value):
                if not isinstance(left, PlanThing | PlanValue | PlanInput):
                    left = self._bound_value(left, site_value._site)
                if not isinstance(right, PlanThing | PlanValue | PlanInput):
                    right = self._bound_value(right, site_value._site)
            self._builder.where_(Condition(left, condition.comparator, right))

    def _assignment(self, value: Any, site: tuple[int, str]) -> _Operand:
        if isinstance(value, PlanExpr):

            def resolve(expr: PlanExpr) -> PlanExpr:
                node = expr._node
                if node[0] == "operand":
                    operand = node[1]
                    if not isinstance(operand, PlanThing | PlanValue | PlanInput):
                        operand = self._bound_value(operand, site)
                    return PlanExpr(("operand", operand))
                if node[0] == "arith":
                    return PlanExpr(("arith", node[1], resolve(node[2]), resolve(node[3])))
                return PlanExpr(("call", node[1], tuple(resolve(arg) for arg in node[2])))

            expr = self._builder._expr(resolve(value))
            index = self._builder._native.let_value(site[0], site[1], expr)
            return PlanValue(self._builder, index)
        if isinstance(value, PlanThing | PlanValue | PlanInput):
            return value
        return self._bound_value(value, site)

    def require(self, *conditions: Condition) -> None:
        """Require conditions involving bound handles from this context."""
        if self._current() is not self:
            raise _context_error("plan_context_missing", "require() belongs to the open plan")
        self._conditions(conditions, None)

    def count(self) -> Plan[int]:
        """Select the matched count as an integer."""
        return self._result("count")

    def returning(self, *values: Value[Any] | Handle[Any]) -> Plan[list[tuple[Any, ...]]]:
        """Select model or scalar values as tuples in argument order."""
        result = self._result("rows")
        result._columns = tuple(self._builder._own(value) for value in values)
        return result

    def exists(self) -> Plan[bool]:
        """Select whether any row matches."""
        return self._result("exists")

    def bind(self, rows: Iterable[Iterable[PlanInputCell]]) -> Plan[R]:
        """Bind explicit template columns to a fresh immutable invocation."""
        authority = self._context._authority or self._authority
        if authority is None:
            raise _context_error("plan_not_built", "build the template before binding rows")
        builder = self._context._builder if self._generated else self._builder
        lowered: list[list[tuple[int, object | None]]] = []
        for row in rows:
            cells: list[tuple[int, object | None]] = []
            for cell in row:
                column = builder._own(cell.input)
                value = (
                    None if cell.value is None else _literal_parts(cell.value, column.value_type)[1]
                )
                cells.append((column._index, value))
            lowered.append(cells)
        native = self.bind_rows(authority, lowered, self._decoder if self._generated else None)
        result: Plan[R] = Plan(native)
        result._builder = builder
        result._authority = authority
        result._context = self._context
        result._kind = self._kind
        result._thing = self._thing
        result._deleted = self._deleted
        result._generated = self._generated
        result._columns = self._columns
        return result

    def _decoder(self, built: _Plan) -> Callable[[PlanAnswer], object] | None:
        if self._kind == "rows":
            if any(isinstance(column, Handle) for column in self._columns):
                return None
            return lambda answer: (
                built._scalar_rows(answer) if isinstance(answer, Rows) else _bad_answer()
            )
        if self._kind == "models":
            return None
        if self._kind == "count":
            return lambda answer: answer.count if isinstance(answer, Count) else _bad_answer()
        if self._kind == "exists":
            return lambda answer: answer.exists if isinstance(answer, Exists) else _bad_answer()
        return lambda answer: Matched(answer.count) if isinstance(answer, Count) else answer

    @staticmethod
    def builder() -> PlanBuilder:
        """Start a plan."""
        return PlanBuilder()

    @staticmethod
    def template(*, query_limits: QueryLimits | None = None) -> PlanTemplate:
        """Start an explicit reusable input/row template."""
        return PlanTemplate(query_limits=query_limits)


class PlanTemplate(Plan[Matched]):
    """An explicit-input plan context; use ``literal`` for fixed constants."""

    def __init__(self, *, query_limits: QueryLimits | None = None) -> None:
        super().__init__(query_limits=query_limits)
        self._template = True

    def __enter__(self) -> PlanTemplate:
        super().__enter__()
        return self

    def _require_open(self) -> None:
        if self._finalized:
            raise _context_error("plan_already_built", "bind a built template for each invocation")

    def input(self, name: str, value_type: PlanValueType) -> PlanInput:
        self._require_open()
        return self._builder.input(name, value_type)

    def optional_input(self, name: str, value_type: PlanValueType) -> PlanInput:
        self._require_open()
        return self._builder.optional_input(name, value_type)

    def row(self, cells: Iterable[PlanInputCell]) -> PlanTemplate:
        self._require_open()
        self._builder.row(cells)
        return self


def _match[H: Handle[Any]](
    model: type[Any],
    handle: type[H],
    authority: PlanAuthority,
    key_field: str | None,
    key: Any,
    fields: Mapping[str, Any],
    *,
    subtypes: bool = False,
    iid: Iid | None = None,
) -> H:
    context = Plan._current()
    context._use_package(authority, model.__runtime_projection__)
    builder = context._builder
    thing = builder.subtypes(model) if subtypes else builder.exact(model)
    bound = handle(context, thing._index, model)
    if iid is not None:
        builder._native.by_iid(bound._index, iid)
    if key is not _MISSING:
        if key_field is None:
            raise TypeError("positional matching requires exactly one schema key")
        if key_field in fields:
            raise TypeError("the match key was given twice")
        fields = {key_field: key, **fields}
    for name, value in fields.items():
        token = getattr(model, name)
        field = bound._value(token)
        site = (bound._index, "key") if key is not _MISSING and name == key_field else field._site
        operand = context._assignment(value, site)
        builder.where_(field.eq(operand))
    return bound


def _insert[M](
    model: type[M],
    authority: PlanAuthority,
    fields: Mapping[str, Any],
    players: Mapping[str, Any],
    *,
    put: bool = False,
) -> Plan[list[M]]:
    context = Plan._current()
    context._use_package(authority, getattr(model, "__runtime_projection__"))
    builder = context._builder
    created = builder.put(model) if put else builder.insert(model)
    thing = created.thing()
    for name, value in fields.items():
        if value is _MISSING:
            continue
        token = getattr(model, name)
        site = (thing._index, json.loads(_owns_json(token))["attribute"])
        created.field(token, context._assignment(value, site))
    for name, player in players.items():
        if player is not _MISSING:
            for member in player if isinstance(player, Sequence) else (player,):
                created.player(getattr(model, name), member)
    return context._project(Handle(context, thing._index, model))


def _bulk_insert[M](
    model: type[M],
    authority: PlanAuthority,
    rows: Iterable[Mapping[str, Any]],
    *,
    put: bool = False,
) -> Plan[list[M]]:
    context = Plan._current()
    context._use_package(authority, getattr(model, "__runtime_projection__"))
    builder = context._builder
    created = builder.put(model) if put else builder.insert(model)
    thing = created.thing()
    columns: list[str] = []
    indexes: dict[str, int] = {}
    lowered: list[list[tuple[str, object] | None]] = []
    for row in rows:
        if not isinstance(row, Mapping):
            raise TypeError("bulk rows must be generated Values mappings")
        cells: list[tuple[str, object] | None] = [None] * len(columns)
        for name, value in row.items():
            index = indexes.get(name)
            if index is None:
                index = len(columns)
                indexes[name] = index
                columns.append(_owns_json(getattr(model, name)))
                cells.append(None)
            cells[index] = _literal_parts(value, None)
        lowered.append(cells)
    for cells in lowered:
        cells.extend([None] * (len(columns) - len(cells)))
    builder._native.model_rows(thing._index, columns, lowered)
    return context._project(Handle(context, thing._index, model))
