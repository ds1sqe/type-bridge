"""Cross-type and narrowed attribute lookup.

Provides :func:`has_lookup` which builds and executes a TypeQL query that
finds all entity **or** relation instances owning a given attribute,
optionally filtered by value or expression and optionally narrowed to a
concrete (or abstract base) type.

Two query shapes are emitted depending on whether ``type_name`` is set:

Cross-type form (``type_name=None``)::

    # All entities with Name = "Alice"
    match entity $e; $x isa $e, has Name "Alice";
    fetch { "_iid": iid($x), "_type": label($e), "attributes": { $x.* } };

    # All relations with Name attribute (any value)
    match relation $r; $x isa $r, has Name $n;
    fetch { "_iid": iid($x), "_type": label($r), "attributes": { $x.* } };

Narrowed form (``type_name="some_type"``)::

    # All instances of some_type (and its subtypes) with Name = "Alice".
    # Uses isa! + sub so that label($t) recovers the concrete subtype.
    # `label($x)` is illegal because $x is an Object variable.
    match $t sub some_type; $x isa! $t, has Name "Alice";
    fetch { "_iid": iid($x), "_type": label($t), "attributes": { $x.* } };

Hydration:

* Entity results are hydrated from the wildcard ``$x.*`` payload directly
  (single query, no follow-up).
* Relation results are re-fetched via
  ``concrete_class.manager(connection).get(_iid=iid)`` so that role players
  are populated. The relation hydration path is therefore N+1 in the number
  of returned relations; this is accepted because relation result sets from
  attribute lookups are typically small.
"""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any, Literal

from type_bridge.crud.formatting import format_value
from type_bridge.expressions.base import Expression
from type_bridge.query.compiler import QueryCompiler

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
    """Build the TypeQL query string for a cross-type or narrowed attribute lookup.

    This is the single source of truth for query construction. Tests should
    call this directly rather than mirroring the logic.

    Args:
        attr_class: The attribute type to search for.
        value: Optional filter — see :func:`has_lookup` for accepted shapes.
        kind: ``"entity"`` or ``"relation"`` — selects the TypeDB kind keyword.
            Only consulted on the cross-type path; the narrowed path is
            kind-agnostic because ``$t sub <type_name>`` already constrains the
            match to the right kind via the type name.
        type_name: Optional TypeDB type name to narrow the match to.
            When ``None``, the query matches across all types of *kind* and
            uses ``label($e)`` / ``label($r)`` to recover each result's
            concrete type. When set, the query emits
            ``$t sub <type_name>; $x isa! $t`` and uses ``label($t)`` to
            recover the most-specific concrete subtype. ``isa!`` is required
            so ``$t`` binds to the exact type of ``$x`` (not a supertype),
            and ``$t sub <type_name>`` is reflexive on the parent in TypeDB,
            so it includes ``<type_name>`` itself plus any of its subtypes.
            ``label($x)`` is illegal in TypeDB 3 because ``$x`` is an Object
            variable, not a Type variable — hence the type-variable dance.

    Returns:
        A complete TypeQL query string (match clause + fetch clause).
    """
    attr_name = attr_class.get_attribute_name()

    match_parts: list[str] = []

    if type_name is None:
        # Cross-type: bind a kind variable so label() returns the concrete subtype.
        # Form:  match {kind} $kv; $x isa $kv, ...
        kind_var = "$e" if kind == "entity" else "$r"
        match_parts.append(f"{kind} {kind_var}")
        isa_anchor = kind_var
        label_var = kind_var
    else:
        # Narrowed: bind a type variable to the most-specific subtype of
        # ``type_name`` so ``label($t)`` returns the concrete subtype label.
        # Form:  match $t sub {type_name}; $x isa! $t, ...
        # ``isa!`` ensures $t is bound to the *exact* type of $x (not a
        # supertype), and ``$t sub {type_name}`` constrains $t to {type_name}
        # or any of its subtypes (TypeDB ``sub`` is reflexive on the parent).
        # We need this dance because ``label($x)`` is illegal — $x is an
        # Object variable, not a Type variable.
        #
        # NOTE: ``kind`` is intentionally unused here. ``$t sub <type_name>``
        # is kind-agnostic — the type name itself already constrains the
        # match to the right kind. Callers passing a mismatched kind +
        # type_name (e.g. ``kind="entity"`` with a relation type name) will
        # get an empty result set, not an error. The only public caller is
        # ``TypeDBType.has``, which always derives both from the same ``cls``.
        match_parts.append(f"$t sub {type_name}")
        isa_anchor = "$t"
        label_var = "$t"

    isa_op = "isa!" if type_name is not None else "isa"

    if value is None:
        # All instances with this attribute (any value)
        match_parts.append(f"$x {isa_op} {isa_anchor}, has {attr_name} $n")
    elif isinstance(value, Expression):
        # Let the expression generate its own HasPattern + comparison.
        # We only emit `$x isa <anchor>` here — no manual `has` clause,
        # so we avoid a duplicate binding with the expression's variable.
        match_parts.append(f"$x {isa_op} {isa_anchor}")
        compiler = QueryCompiler()
        for pattern in value.to_ast("$x"):
            match_parts.append(compiler.compile(pattern))
    else:
        # Exact match (raw value or Attribute instance)
        formatted = format_value(value)
        match_parts.append(f"$x {isa_op} {isa_anchor}, has {attr_name} {formatted}")

    match_clause = "match " + ";\n".join(match_parts) + ";"
    fetch_clause = (
        'fetch { "_iid": iid($x), "_type": label(' + label_var + '), "attributes": { $x.* } };'
    )
    return match_clause + "\n" + fetch_clause


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
    from typedb.driver import TransactionType

    executor = ConnectionExecutor(connection)
    results = executor.execute(query, TransactionType.READ)

    # Hydrate (relations route through manager.get to recover role players)
    return _hydrate_results(results, ModelRegistry, connection=connection)


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
