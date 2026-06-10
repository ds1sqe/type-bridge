"""Cross-type and narrowed attribute lookup.

Python owns public-call lowering and result hydration. Rust owns TypeQL
construction and execution.
"""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any, Literal

from type_bridge.expressions.base import Expression

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.models.base import TypeDBType
    from type_bridge.session import Connection

logger = logging.getLogger(__name__)


def _build_has_query(
    attr_class: type[Attribute],
    value: Any | None = None,
    *,
    kind: Literal["entity", "relation"],
    type_name: str | None = None,
) -> str:
    """Build has-lookup TypeQL through the Rust ORM query builder."""
    core = _rust_core()
    expression = _has_lookup_expression(attr_class, value)
    return core.build_has_lookup_query(
        kind,
        attr_class.get_attribute_name(),
        expression,
        type_name,
    )


def has_lookup(
    connection: Connection,
    attr_class: type[Attribute],
    value: Any | None = None,
    *,
    kind: Literal["entity", "relation"],
    type_name: str | None = None,
) -> list[TypeDBType]:
    """Find all instances of *kind* that own *attr_class*, with optional filter.

    Args:
        connection: Database, Transaction, or TransactionContext.
        attr_class: The attribute type to search for (e.g. ``Name``).
        value: Optional filter — may be:
            - ``None``  → return all instances that own the attribute
            - A raw Python value or :class:`Attribute` instance → exact match
            - An :class:`Expression` (e.g. ``Name.gt(Name("B"))``) → comparison
        kind: ``"entity"`` or ``"relation"`` — selects the TypeDB kind keyword.
        type_name: Optional concrete TypeDB type name to narrow the match to.
            When set, results are restricted to that type and its subtypes.

    Returns:
        Hydrated model instances (mixed concrete types).
    """
    from type_bridge.models.registry import ModelRegistry
    from type_bridge.session import ConnectionExecutor

    query = _build_has_query(attr_class, value, kind=kind, type_name=type_name)

    # Execute
    from type_bridge.typedb_driver import TransactionType

    executor = ConnectionExecutor(connection)
    results = executor.execute(query, TransactionType.READ)

    # Hydrate (relations route through manager.get to recover role players)
    return _hydrate_results(results, ModelRegistry, connection=connection)


def _rust_core() -> Any:
    from type_bridge._rust_runtime import rust_core

    return rust_core()


def _has_lookup_expression(attr_class: type[Attribute], value: Any | None) -> Any | None:
    if value is None:
        return None
    if isinstance(value, Expression):
        return _lower_has_expression(attr_class, value)

    wrapped = value if isinstance(value, attr_class) else attr_class(value)
    return _lower_has_expression(attr_class, attr_class.eq(wrapped))


def _lower_has_expression(attr_class: type[Attribute], expression: Expression) -> Any:
    from type_bridge.crud.rust_manager import _dynamic_value, _raw_attr_value
    from type_bridge.expressions import (
        AttributeExistsExpr,
        BooleanExpr,
        ComparisonExpr,
        IidExpr,
        StringExpr,
    )

    core = _rust_core()

    if isinstance(expression, core.DynamicExpr):
        return expression

    if isinstance(expression, ComparisonExpr):
        _validate_has_expression_attr(attr_class, expression.attr_type)
        value = _dynamic_value(_raw_attr_value(expression.value), expression.attr_type)
        attr_name = expression.attr_type.get_attribute_name()
        match expression.operator:
            case "==":
                return core.DynamicExpr.eq(attr_name, value)
            case "!=":
                return core.DynamicExpr.neq(attr_name, value)
            case ">":
                return core.DynamicExpr.gt(attr_name, value)
            case ">=":
                return core.DynamicExpr.gte(attr_name, value)
            case "<":
                return core.DynamicExpr.lt(attr_name, value)
            case "<=":
                return core.DynamicExpr.lte(attr_name, value)
        raise ValueError(f"Unsupported comparison operator {expression.operator!r}")

    if isinstance(expression, StringExpr):
        _validate_has_expression_attr(attr_class, expression.attr_type)
        attr_name = expression.attr_type.get_attribute_name()
        pattern = str(_raw_attr_value(expression.pattern))
        if expression.operation == "contains":
            return core.DynamicExpr.contains(attr_name, pattern)
        if expression.operation in {"like", "regex"}:
            return core.DynamicExpr.like(attr_name, pattern)
        raise ValueError(f"Unsupported string operation {expression.operation!r}")

    if isinstance(expression, AttributeExistsExpr):
        _validate_has_expression_attr(attr_class, expression.attr_type)
        attr_name = expression.attr_type.get_attribute_name()
        if expression.present:
            return core.DynamicExpr.is_not_null(attr_name)
        return core.DynamicExpr.is_null(attr_name)

    if isinstance(expression, IidExpr):
        return core.DynamicExpr.iid(expression.iid)

    if isinstance(expression, BooleanExpr):
        expressions = [_lower_has_expression(attr_class, item) for item in expression.operands]
        if expression.operation == "and":
            return core.DynamicExpr.and_(expressions)
        if expression.operation == "or":
            return core.DynamicExpr.or_(expressions)
        if expression.operation == "not":
            return core.DynamicExpr.not_(expressions[0])
        raise ValueError(f"Unsupported boolean operation {expression.operation!r}")

    raise TypeError(f"Unsupported has lookup expression {type(expression).__name__}")


def _validate_has_expression_attr(
    attr_class: type[Attribute],
    expression_attr_class: type[Attribute],
) -> None:
    if expression_attr_class.get_attribute_name() != attr_class.get_attribute_name():
        raise ValueError(
            "has lookup expression attribute must match the searched attribute "
            f"{attr_class.get_attribute_name()!r}"
        )


def _hydrate_entity(concrete_class: type, attrs: dict[str, Any]) -> Any:
    """Hydrate an entity instance from a wildcard ``$x.*`` payload.

    Entities can use the wildcard fetch payload directly because they have
    no role players. This is the single-query fast path.
    """
    return concrete_class.from_dict(attrs, strict=False)


def _hydrate_relation_via_manager(
    concrete_class: type,
    iid: str,
    connection: Any,
) -> Any | None:
    """Hydrate a relation by re-fetching it through its manager.

    Relations need their role players populated, which the wildcard ``$x.*``
    payload does not provide. The existing relation manager already extracts
    role players via ``crud/role_players.py``, so we delegate to it via
    ``manager.get(_iid=iid)``.

    Returns ``None`` if the relation no longer exists (e.g. deleted between
    the initial query and the follow-up fetch).
    """
    manager = concrete_class.manager(connection)
    fetched = manager.get(_iid=iid)
    if not fetched:
        return None
    return fetched[0]


def _hydrate_results(
    results: list[dict[str, Any]],
    registry: type,
    *,
    connection: Any,
) -> list[Any]:
    """Deserialize raw query results into typed model instances.

    Entities are hydrated from the wildcard ``$x.*`` payload (single query).
    Relations are re-fetched via ``concrete_class.manager(connection).get(
    _iid=iid)`` so that role players are populated. The relation path is
    therefore N+1 in the number of returned relations; this is accepted
    because relation result sets from attribute lookups are typically small.

    Args:
        results: Raw query result rows from the wildcard fetch.
        registry: The :class:`ModelRegistry` (or compatible) used to resolve
            type labels to concrete Python classes.
        connection: Connection used for the relation follow-up fetches.
    """
    from type_bridge.crud.exceptions import HydrationError
    from type_bridge.models.relation import Relation

    instances: list[Any] = []
    for result in results:
        type_label: str | None = None
        try:
            iid = result.pop("_iid", None)
            if isinstance(iid, dict) and "value" in iid:
                iid = iid["value"]

            type_label = result.pop("_type", None)
            attrs = result.pop("attributes", result)

            if not type_label:
                logger.warning("Skipping result with no _type label")
                continue

            concrete_class = registry.get(type_label)
            if concrete_class is None:
                logger.warning("Skipping unregistered type '%s'", type_label)
                continue

            if issubclass(concrete_class, Relation):
                if not iid or not isinstance(iid, str):
                    logger.warning(
                        "Skipping relation '%s' with missing or non-string IID — "
                        "cannot fetch role players",
                        type_label,
                    )
                    continue
                instance = _hydrate_relation_via_manager(concrete_class, iid, connection)
                if instance is None:
                    logger.warning(
                        "Relation '%s' with iid=%s vanished between fetch and hydration",
                        type_label,
                        iid,
                    )
                    continue
            else:
                instance = _hydrate_entity(concrete_class, attrs)
                if iid:
                    object.__setattr__(instance, "_iid", iid)

            instances.append(instance)
        except Exception as e:
            raise HydrationError(
                model_type=type_label or "unknown",
                raw_data=result,
                cause=e,
            ) from e

    return instances
