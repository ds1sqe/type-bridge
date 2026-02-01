"""IID (Internal ID) population utilities for entities and relations."""

from typing import TYPE_CHECKING, Any

from type_bridge.crud.formatting import format_value, unwrap_attribute
from type_bridge.crud.types import extract_entity_key, resolve_entity_class

if TYPE_CHECKING:
    from type_bridge.models import Entity, Relation


# ============================================================================
# Shared IID Utilities
# ============================================================================


def process_iid_type_results(
    results: list[dict[str, Any]],
    key_attrs: dict[str, Any],
    key_attr_names: list[str],
    known_key_values: dict[str, Any],
) -> dict[tuple[tuple[str, Any], ...], tuple[str, str]]:
    """Process fetch results to build IID/type map.

    Shared logic for extracting IID and type information from TypeDB
    fetch results, building a map keyed by key attribute values.

    Args:
        results: List of fetch result dictionaries from TypeDB
        key_attrs: Dictionary of key attributes (field_name -> attr_info)
        key_attr_names: List of TypeDB attribute names for key attributes
        known_key_values: Dictionary of key values already known from filters

    Returns:
        Dictionary mapping key_values_tuple to (iid, type_name) tuple
    """
    iid_type_map: dict[tuple[tuple[str, Any], ...], tuple[str, str]] = {}

    for result in results:
        # Get IID/type from fetch result
        iid = result.get("_iid")
        type_name = result.get("_type")
        if not iid or not type_name:
            continue

        # Build map key from key attributes or known values
        if key_attrs:
            if known_key_values and len(known_key_values) == len(key_attrs):
                # All key values known from filters
                map_key = tuple(sorted(known_key_values.items()))
            else:
                # Extract key values from fetch result
                key_values: list[tuple[str, Any]] = []
                for attr_name in key_attr_names:
                    if attr_name in result:
                        val = result[attr_name]
                        key_values.append((attr_name, val))
                if not key_values:
                    continue
                map_key = tuple(sorted(key_values))
        else:
            # No key attributes, use IID as the map key
            map_key = (("_iid", iid),)

        iid_type_map[map_key] = (iid, type_name)

    return iid_type_map


def get_key_attrs(
    model_class: type["Entity"],
) -> tuple[dict[str, Any], list[str]]:
    """Get key attributes and their TypeDB names from a model class.

    Args:
        model_class: The entity class to extract key attributes from

    Returns:
        Tuple of (key_attrs dict, key_attr_names list)
    """
    owned_attrs = model_class.get_all_attributes()
    key_attrs = {
        field_name: attr_info
        for field_name, attr_info in owned_attrs.items()
        if attr_info.flags.is_key
    }
    key_attr_names = [attr_info.typ.get_attribute_name() for attr_info in key_attrs.values()]
    return key_attrs, key_attr_names


def build_known_key_values(
    key_attrs: dict[str, Any],
    filters: dict[str, Any],
) -> dict[str, Any]:
    """Extract known key values from filters.

    Args:
        key_attrs: Dictionary of key attributes (field_name -> attr_info)
        filters: Filter dictionary (field_name -> value)

    Returns:
        Dictionary of known key values (attr_name -> raw_value)
    """
    known_key_values: dict[str, Any] = {}
    for field_name, attr_info in key_attrs.items():
        attr_name = attr_info.typ.get_attribute_name()
        if field_name in filters:
            filter_value = filters[field_name]
            filter_value = unwrap_attribute(filter_value)
            known_key_values[attr_name] = filter_value
    return known_key_values


def build_iid_type_fetch_clause(
    key_attr_names: list[str],
    known_key_values: dict[str, Any],
    key_attrs: dict[str, Any],
    var: str = "$e",
) -> str:
    """Build fetch clause for IID and type retrieval.

    Args:
        key_attr_names: List of TypeDB attribute names for key attributes
        known_key_values: Dictionary of key values already known from filters
        key_attrs: Dictionary of key attributes
        var: Variable name for the entity (default: "$e")

    Returns:
        Fetch clause string
    """
    fetch_items = [f'"_iid": iid({var})', '"_type": label($t)']

    # Add key attributes to fetch (only if not all known from filters)
    if not (known_key_values and len(known_key_values) == len(key_attrs)):
        for attr_name in key_attr_names:
            fetch_items.append(f'"{attr_name}": {var}.{attr_name}')

    return f"fetch {{\n  {', '.join(fetch_items)}\n}}"


def match_entity_type(
    attrs: dict[str, Any],
    iid_type_map: dict[tuple[tuple[str, Any], ...], tuple[str, str]],
    model_class: type["Entity"],
    owned_attrs: dict[str, Any] | None = None,
) -> tuple[type["Entity"], str | None]:
    """Match entity attributes to IID/type and resolve the correct class.

    Uses key attributes to look up the corresponding IID/type from the map
    (in-memory, no database query), then resolves the actual Python class
    for polymorphic instantiation.

    Args:
        attrs: Extracted attributes for the entity
        iid_type_map: Map from key_values_tuple to (iid, type_name)
        model_class: The base model class for resolution
        owned_attrs: Optional attribute metadata. If None, fetched from model_class

    Returns:
        Tuple of (resolved_class, iid) where resolved_class is the
        concrete subclass if found, otherwise model_class
    """
    # If no type info available, use model_class
    if not iid_type_map:
        return model_class, None

    # Get owned_attrs if not provided
    attrs_dict: dict[str, Any] = (
        owned_attrs if owned_attrs is not None else model_class.get_all_attributes()
    )

    # Get key attributes for matching
    key_attrs = {
        field_name: attr_info
        for field_name, attr_info in attrs_dict.items()
        if attr_info.flags.is_key
    }

    if not key_attrs:
        # No key attributes - can't match reliably, use first available
        if iid_type_map:
            iid, type_name = next(iter(iid_type_map.values()))
            resolved_class = resolve_entity_class(model_class, type_name)
            return resolved_class, iid
        return model_class, None

    # Build key signature from attrs for in-memory lookup
    key_values: list[tuple[str, Any]] = []
    for field_name, attr_info in key_attrs.items():
        value = attrs.get(field_name)
        if value is not None:
            value = unwrap_attribute(value)
            attr_name = attr_info.typ.get_attribute_name()
            key_values.append((attr_name, value))

    if not key_values:
        return model_class, None

    # Look up in the map using key values (no database query!)
    map_key = tuple(sorted(key_values))
    if map_key in iid_type_map:
        iid, type_name = iid_type_map[map_key]
        resolved_class = resolve_entity_class(model_class, type_name)
        return resolved_class, iid

    return model_class, None


def modify_match_for_type_binding(match_str: str, type_name: str) -> str:
    """Modify match clause to bind exact type for label() retrieval.

    Changes: "$e isa person" to "$e isa! $t; $t sub person"
    This allows using label($t) to get the exact type name.

    Args:
        match_str: Original match clause
        type_name: TypeDB type name

    Returns:
        Modified match clause with type binding
    """
    match_str = match_str.replace(f"$e isa {type_name}", "$e isa! $t")
    return f"{match_str}; $t sub {type_name}"


# ============================================================================
# Entity IID Population Utilities
# ============================================================================


def build_entity_iid_query(
    entities: list[Any],
    model_class: type["Entity"],
    key_attrs: dict[str, Any],
) -> tuple[str, list[str]] | None:
    """Build batched IID lookup query for entities.

    Constructs a TypeQL fetch query using OR clauses to match all entities
    by their key attributes in a single database round-trip.

    Args:
        entities: List of entity instances to look up IIDs for
        model_class: The entity class (must have get_type_name())
        key_attrs: Dictionary of key attributes (field_name -> attr_info)

    Returns:
        Tuple of (query_str, key_attr_names) if valid query can be built,
        otherwise None (e.g., if no entities or no key attributes)
    """
    if not entities or not key_attrs:
        return None

    type_name = model_class.get_type_name()
    key_attr_names = [attr_info.typ.get_attribute_name() for attr_info in key_attrs.values()]

    # Build batched disjunctive query for all entities
    or_clauses = []
    for entity in entities:
        match_parts = [f"$e isa {type_name}"]
        for field_name, attr_info in key_attrs.items():
            value = getattr(entity, field_name, None)
            if value is not None:
                value = unwrap_attribute(value)
                attr_name = attr_info.typ.get_attribute_name()
                formatted_value = format_value(value)
                match_parts.append(f"has {attr_name} {formatted_value}")
        or_clauses.append(f"{{ {', '.join(match_parts)}; }}")

    if not or_clauses:
        return None

    # Build fetch clause: iid and key attributes (for matching)
    fetch_items = ['"_iid": iid($e)']
    for attr_name in key_attr_names:
        fetch_items.append(f'"{attr_name}": $e.{attr_name}')
    fetch_clause = f"fetch {{\n  {', '.join(fetch_items)}\n}}"

    query_str = f"match\n{' or '.join(or_clauses)};\n{fetch_clause};"
    return query_str, key_attr_names


def build_entity_iid_map(
    results: list[dict[str, Any]],
    key_attr_names: list[str],
) -> dict[tuple[tuple[str, Any], ...], str]:
    """Build IID map from query results.

    Processes fetch query results to create a lookup map from key attribute
    values to IIDs for fast in-memory matching.

    Args:
        results: List of fetch result dictionaries from TypeDB
        key_attr_names: List of TypeDB attribute names for key attributes

    Returns:
        Dictionary mapping key_values_tuple to IID string
    """
    iid_map: dict[tuple[tuple[str, Any], ...], str] = {}

    for result in results:
        iid = result.get("_iid")
        if not iid:
            continue

        # Extract key attribute values
        key_values: list[tuple[str, Any]] = []
        for attr_name in key_attr_names:
            if attr_name in result:
                key_values.append((attr_name, result[attr_name]))
        if key_values:
            iid_map[tuple(sorted(key_values))] = iid

    return iid_map


def assign_entity_iids(
    entities: list[Any],
    iid_map: dict[tuple[tuple[str, Any], ...], str],
    key_attrs: dict[str, Any],
) -> None:
    """Assign IIDs to entities using the IID map.

    Performs in-memory lookup to match entities to their IIDs by key
    attribute values, then sets the _iid field on each entity.

    Args:
        entities: List of entity instances to assign IIDs to
        iid_map: Dictionary mapping key_values_tuple to IID string
        key_attrs: Dictionary of key attributes (field_name -> attr_info)
    """
    for entity in entities:
        key_values: list[tuple[str, Any]] = []
        for field_name, attr_info in key_attrs.items():
            value = getattr(entity, field_name, None)
            if value is not None:
                value = unwrap_attribute(value)
                attr_name = attr_info.typ.get_attribute_name()
                key_values.append((attr_name, value))

        if key_values:
            map_key = tuple(sorted(key_values))
            if map_key in iid_map:
                object.__setattr__(entity, "_iid", iid_map[map_key])


# ============================================================================
# Relation IID Population Utilities
# ============================================================================


def build_relation_iid_query(
    relations: list[Any],
    model_class: type["Relation"],
    roles: dict[str, Any],
) -> (
    tuple[str, list[str], dict[str, tuple[str, str]], list[dict[str, tuple[str, Any, Any]]]] | None
):
    """Build batched IID lookup query for relations.

    Constructs a TypeQL select query using OR clauses to match all relations
    by their role players in a single database round-trip.

    Args:
        relations: List of relation instances to look up IIDs for
        model_class: The relation class (must have get_type_name())
        roles: Dictionary of roles from model_class._roles

    Returns:
        Tuple of (query_str, role_names, role_key_info, relation_key_data) if valid,
        otherwise None.
        - query_str: The TypeQL select query
        - role_names: List of role field names
        - role_key_info: Dict mapping role_name -> (key_var_name, attr_name)
        - relation_key_data: List of per-relation role key info for correlation
    """
    if not relations:
        return None

    role_names = list(roles.keys())
    type_name = model_class.get_type_name()

    # Track which roles have key attributes and their attribute names
    role_key_info: dict[str, tuple[str, str]] = {}
    relation_key_data: list[dict[str, tuple[str, Any, Any]]] = []

    or_clauses = []
    for relation in relations:
        role_parts = []
        match_statements = []
        per_relation_key_info: dict[str, tuple[str, Any, Any]] = {}

        for role_name, role in roles.items():
            entity = getattr(relation, role_name, None)
            if entity is None:
                continue

            # Validate entity is a TypeDBType instance
            entity_class = entity.__class__
            if not hasattr(entity_class, "get_all_attributes"):
                continue

            role_var = f"${role_name}"
            role_parts.append(f"{role.role_name}: {role_var}")

            key_info = extract_entity_key(entity)
            if key_info:
                _, attr_name, raw_value = key_info
                formatted_value = format_value(raw_value)
                # Use a variable for the key attribute to capture its value
                key_var = f"${role_name}_key"
                match_statements.append(
                    f"{role_var} has {attr_name} {key_var}; {key_var} == {formatted_value}"
                )
                # Track the key variable for this role
                if role_name not in role_key_info:
                    role_key_info[role_name] = (f"{role_name}_key", attr_name)
                per_relation_key_info[role_name] = (attr_name, raw_value, entity)

        if not role_parts:
            continue

        roles_str = ", ".join(role_parts)
        relation_match = f"$r isa {type_name} ({roles_str})"
        clause_parts = [relation_match] + match_statements
        or_clauses.append(f"{{ {'; '.join(clause_parts)}; }}")
        relation_key_data.append(per_relation_key_info)

    if not or_clauses:
        return None

    # Build select variables - shared across all branches
    select_vars = ["$r"] + [f"${role_name}" for role_name in role_names]
    for key_var_name, _ in role_key_info.values():
        select_vars.append(f"${key_var_name}")

    query_str = f"match\n{' or '.join(or_clauses)};\nselect {', '.join(select_vars)};"
    return query_str, role_names, role_key_info, relation_key_data


def build_relation_result_map(
    results: list[dict[str, Any]],
    role_key_info: dict[str, tuple[str, str]],
) -> dict[tuple[tuple[str, Any], ...], dict[str, Any]]:
    """Build a lookup map from key attribute values to results.

    Processes select query results to create a lookup map for correlating
    relations to their query results.

    Args:
        results: List of select result dictionaries from TypeDB
        role_key_info: Dict mapping role_name -> (key_var_name, attr_name)

    Returns:
        Dictionary mapping key_values_tuple to result dict
    """
    result_map: dict[tuple[tuple[str, Any], ...], dict[str, Any]] = {}

    for result in results:
        key_parts: list[tuple[str, Any]] = []
        for role_name, (key_var_name, _) in role_key_info.items():
            if key_var_name in result:
                key_val = result[key_var_name]
                # Extract value from concept data dict
                if isinstance(key_val, dict):
                    key_val = key_val.get("value", key_val.get("result"))
                key_parts.append((role_name, key_val))
        if key_parts:
            result_map[tuple(sorted(key_parts))] = result

    return result_map


def assign_relation_iids(
    relations: list[Any],
    result_map: dict[tuple[tuple[str, Any], ...], dict[str, Any]],
    role_key_info: dict[str, tuple[str, str]],
    role_names: list[str],
) -> None:
    """Assign IIDs to relations and their role players.

    Performs in-memory lookup to match relations to their query results,
    then sets the _iid field on relations and their role player entities.

    Args:
        relations: List of relation instances to assign IIDs to
        result_map: Dictionary mapping key_values_tuple to result dict
        role_key_info: Dict mapping role_name -> (key_var_name, attr_name)
        role_names: List of role field names
    """
    for relation in relations:
        # Build the key for this relation from its role players' key attributes
        relation_key_parts: list[tuple[str, Any]] = []
        role_var_to_entity: dict[str, Any] = {}

        for role_name in role_names:
            entity = getattr(relation, role_name, None)
            if entity is not None:
                role_var_to_entity[role_name] = entity

                # Get the key attribute value for this role player
                if role_name in role_key_info:
                    key_info = extract_entity_key(entity)
                    if key_info:
                        _, _, raw_value = key_info
                        relation_key_parts.append((role_name, raw_value))

        # Look up the result by key values
        relation_key = tuple(sorted(relation_key_parts))
        matched_result = result_map.get(relation_key)

        if matched_result:
            # Extract relation IID
            if "r" in matched_result and isinstance(matched_result["r"], dict):
                relation_iid = matched_result["r"].get("_iid")
                if relation_iid:
                    object.__setattr__(relation, "_iid", relation_iid)

            # Extract role player IIDs
            for role_name, entity in role_var_to_entity.items():
                if role_name in matched_result and isinstance(matched_result[role_name], dict):
                    player_iid = matched_result[role_name].get("_iid")
                    if player_iid:
                        object.__setattr__(entity, "_iid", player_iid)
