"""Role player utilities for relations."""

from collections.abc import Mapping
from typing import TYPE_CHECKING, Any

from type_bridge.crud.formatting import format_value
from type_bridge.crud.types import hydrate_attributes, wrap_attribute_value

if TYPE_CHECKING:
    from type_bridge.models.base import _QueryTypeDBType as TypeDBType
    from type_bridge.models.relation import _QueryRelation as Relation


def build_role_player_match(var_name: str, entity: Any, entity_type_name: str) -> str:
    """Build a match clause for a role player entity.

    Prefers IID-based matching when available (more precise and faster),
    falls back to key attribute matching, and raises a clear error if
    neither is available.

    Used by TypeDBManager for relation CRUD operations.

    Args:
        var_name: The variable name to use (without $)
        entity: The entity instance
        entity_type_name: The TypeDB type name for the entity

    Returns:
        A TypeQL match clause string like "$var_name isa type, iid 0x..."
        or "$var_name isa type, has key_attr value"

    Raises:
        ValueError: If entity has neither _iid nor key attributes
    """
    # Prefer IID-based matching when available (more precise and faster)
    entity_iid = getattr(entity, "_iid", None)
    if entity_iid:
        return f"${var_name} isa {entity_type_name}, iid {entity_iid}"

    # Fall back to key attribute matching
    key_attrs = {
        field_name: attr_info
        for field_name, attr_info in entity.__class__.get_all_attributes().items()
        if attr_info.flags.is_key
    }

    for field_name, attr_info in key_attrs.items():
        value = getattr(entity, field_name)
        if value is not None:
            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            formatted_value = format_value(value)
            return f"${var_name} isa {entity_type_name}, has {attr_name} {formatted_value}"

    # Neither IID nor key attributes available
    raise ValueError(
        f"Role player '{var_name}' ({entity.__class__.__name__}) cannot be identified: "
        f"no _iid set and no @key attributes defined. Either fetch the entity from the "
        f"database first (to populate _iid) or add Flag(Key) to an attribute."
    )


def resolve_entity_class_from_label(
    type_label: str | None,
    allowed_entity_classes: tuple[type["TypeDBType"], ...],
) -> type["TypeDBType"]:
    """Resolve the correct Python entity class from a TypeDB type label.

    This is a thin wrapper around ModelRegistry.resolve() that provides
    polymorphic class resolution with caching.

    Args:
        type_label: The TypeDB type label (from label() function), e.g., "person"
        allowed_entity_classes: Tuple of allowed entity classes for this role

    Returns:
        The matching Python entity class, or the first allowed class as fallback
    """
    from type_bridge.models.registry import _QueryModelRegistry as ModelRegistry

    return ModelRegistry.resolve(type_label, allowed_entity_classes)


def build_role_player_fetch_items(
    role_info: dict[str, tuple[str, Any]],
) -> list[str]:
    """Build fetch items for role players with their IIDs and type labels.

    TypeQL 3.x doesn't allow mixing iid() with .* in the same nested block,
    so we fetch the IID as a separate key alongside the nested attributes.
    We also fetch the type label to correctly identify polymorphic role players.

    Args:
        role_info: Dict mapping role_name -> (role_var, allowed_entity_types)

    Returns:
        List of fetch item strings like:
            '"employee_iid": iid($employee)'
            '"employee_type": label($employee_type)'
            '"employee": { $employee.* }'

    Note: The caller must add type variable bindings to the match clause:
        $employee isa $employee_type;
    """
    fetch_items = []
    for role_name, (role_var, _) in role_info.items():
        # Fetch IID separately (can't be mixed with .* in nested block)
        fetch_items.append(f'"{role_name}_iid": iid({role_var})')
        # Fetch type label for polymorphic role player resolution
        fetch_items.append(f'"{role_name}_type": label({role_var}_type)')
        # Fetch all attributes for the role player
        fetch_items.append(f'"{role_name}": {{\n    {role_var}.*\n  }}')
    return fetch_items


def group_results_by_iid(results: list[dict[str, Any]]) -> dict[str, list[dict[str, Any]]]:
    """Group query results by relation/entity IID.

    TypeDB returns one row per role player combination for multi-cardinality
    roles. This utility groups those rows by IID so they can be merged into
    a single relation instance.

    Args:
        results: List of result dictionaries from TypeDB fetch

    Returns:
        Dictionary mapping IID -> list of result rows with that IID
    """
    grouped: dict[str, list[dict[str, Any]]] = {}
    for result in results:
        iid = result.get("_iid")
        if iid:
            if iid not in grouped:
                grouped[iid] = []
            grouped[iid].append(result)
    return grouped


def _set_iid(instance: Any, iid: str | None) -> None:
    """Set the TypeDB IID on a hydrated instance."""
    if not iid:
        return

    private = getattr(instance, "__pydantic_private__", None)
    if private is not None:
        private["_iid"] = iid
    else:
        object.__setattr__(instance, "_iid", iid)


def _is_relation_class(model_class: type[Any]) -> bool:
    from type_bridge.models.relation import _QueryRelation as Relation

    return issubclass(model_class, Relation)


def extract_relation_attributes(
    model_class: type["Relation"],
    result: dict[str, Any],
) -> dict[str, Any]:
    """Extract relation attributes from a query result.

    Uses the unified wrap_attribute_value() helper for consistent handling
    of multi-value and single-value attributes.

    Args:
        model_class: The relation class to extract attributes for
        result: The result dictionary from TypeDB fetch

    Returns:
        Dictionary mapping field_name -> attribute value (ready for model constructor)
    """
    attrs: dict[str, Any] = {}
    all_attrs = model_class.get_all_attributes()

    for field_name, attr_info in all_attrs.items():
        attr_class = attr_info.typ
        attr_name = attr_class.get_attribute_name()
        if attr_name in result:
            raw_value = result[attr_name]
            # Use unified wrapping helper for consistent behavior
            attrs[field_name] = wrap_attribute_value(
                raw_value, attr_info, use_pydantic_validate=True
            )
        else:
            # For list fields (has_explicit_card), default to empty list
            # For other optional fields, explicitly set to None
            if attr_info.flags.has_explicit_card:
                attrs[field_name] = []
            else:
                attrs[field_name] = None

    return attrs


def extract_role_players_from_results(
    result_group: list[dict[str, Any]],
    role_info: Mapping[str, tuple[str, tuple[type["TypeDBType"], ...]]],
    multi_player_roles: set[str],
) -> dict[str, Any]:
    """Extract and deduplicate role players from grouped query results.

    For multi-player roles, collects all unique players from all rows in the group.
    For single-player roles, takes the first player found.

    Args:
        result_group: List of result rows with the same relation IID
        role_info: Dict mapping role_name -> (role_var, allowed_entity_classes)
        multi_player_roles: Set of role names that allow multiple players

    Returns:
        Dictionary mapping role_name -> player entity or list of player entities
    """
    role_players: dict[str, Any] = {}

    for role_name, (_, allowed_entity_classes) in role_info.items():
        is_multi = role_name in multi_player_roles
        collected_players: list[Any] = []
        seen_player_keys: set[tuple[Any, ...]] = set()

        for result in result_group:
            if role_name in result and isinstance(result[role_name], dict):
                player_data = result[role_name]

                # Get the actual type label from TypeDB (fetched via label())
                type_label = result.get(f"{role_name}_type")

                # Resolve entity class from type label for polymorphic support
                entity_class = resolve_entity_class_from_label(type_label, allowed_entity_classes)

                # Hydrate player entity using shared utility
                player_iid = result.get(f"{role_name}_iid")
                player_attrs, key_values = hydrate_attributes(
                    entity_class, player_data, wrap_values=True
                )

                # Create a role-player instance if the row proves a player exists.
                # Relations used as role players are fetched as nested placeholders:
                # TypeDB returns their IID/type and owned attributes, but not their
                # own role players. Construct them without validation so required
                # roles are not demanded for a partial reference.
                has_player_data = (
                    player_iid is not None
                    or player_attrs == {}
                    or any(v is not None for v in player_attrs.values())
                )
                if has_player_data:
                    player_key = ("iid", player_iid) if player_iid else ("keys", key_values)
                    if player_key not in seen_player_keys:
                        seen_player_keys.add(player_key)
                        if _is_relation_class(entity_class):
                            player_entity = entity_class.model_construct(**player_attrs)
                        else:
                            player_entity = entity_class(**player_attrs)
                        _set_iid(player_entity, player_iid)
                        collected_players.append(player_entity)

        # Store collected players
        if collected_players:
            if is_multi:
                role_players[role_name] = collected_players
            else:
                role_players[role_name] = collected_players[0]

    return role_players
