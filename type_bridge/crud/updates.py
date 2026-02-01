"""Update query building utilities."""

from typing import TYPE_CHECKING, Any

from type_bridge.crud.exceptions import KeyAttributeError
from type_bridge.crud.formatting import format_value, unwrap_attribute
from type_bridge.crud.types import is_multi_value_attribute

if TYPE_CHECKING:
    from type_bridge.models import Entity


def extract_update_values(
    entity_or_relation: Any,
    owned_attrs: dict[str, Any],
    skip_key_attrs: bool = True,
) -> tuple[dict[str, Any], set[str], dict[str, list[Any]]]:
    """Extract attribute updates from an entity or relation instance.

    Separates single-value and multi-value attribute updates, and tracks
    optional attributes that need to be deleted (set to None).

    Args:
        entity_or_relation: The entity or relation instance
        owned_attrs: Dictionary of attribute info (field_name -> attr_info)
        skip_key_attrs: Whether to skip key attributes (True for entities)

    Returns:
        Tuple of (single_value_updates, single_value_deletes, multi_value_updates):
        - single_value_updates: Dict of attr_name -> value for single-value attrs
        - single_value_deletes: Set of attr_names for optional attrs set to None
        - multi_value_updates: Dict of attr_name -> list of values for multi-value attrs
    """
    single_value_updates: dict[str, Any] = {}
    single_value_deletes: set[str] = set()
    multi_value_updates: dict[str, list[Any]] = {}

    for field_name, attr_info in owned_attrs.items():
        # Skip key attributes if requested (used for entity matching)
        if skip_key_attrs and attr_info.flags.is_key:
            continue

        attr_class = attr_info.typ
        attr_name = attr_class.get_attribute_name()
        flags = attr_info.flags

        # Get current value from instance
        current_value = getattr(entity_or_relation, field_name, None)

        # Extract raw values from Attribute instances
        if current_value is not None:
            if isinstance(current_value, list):
                # Multi-value: extract value from each Attribute in list
                current_value = [unwrap_attribute(item) for item in current_value]
            else:
                # Single-value: extract value from Attribute
                current_value = unwrap_attribute(current_value)

        # Determine if multi-value
        if is_multi_value_attribute(flags):
            # Multi-value: store as list (even if empty)
            if current_value is None:
                current_value = []
            multi_value_updates[attr_name] = current_value
        else:
            # Single-value: handle updates and deletions
            if current_value is not None:
                single_value_updates[attr_name] = current_value
            elif flags.card_min == 0:
                # Optional attribute set to None - needs to be deleted
                single_value_deletes.add(attr_name)

    return single_value_updates, single_value_deletes, multi_value_updates


def build_multi_value_match_clauses(
    multi_value_updates: dict[str, list[Any]],
    var_name: str = "$e",
) -> list[str]:
    """Build match clauses for multi-value attribute updates with guards.

    Creates try blocks that match existing attribute values while guarding
    against the values that should be kept (not deleted).

    Args:
        multi_value_updates: Dict of attr_name -> list of values to keep
        var_name: Variable name for the entity/relation (e.g., "$e", "$r")

    Returns:
        List of match clause strings (try blocks with guards)
    """
    match_statements = []
    for attr_name, values in multi_value_updates.items():
        keep_literals = [format_value(v) for v in dict.fromkeys(values)]
        attr_var = f"${attr_name}_{var_name.replace('$', '')}"
        guard_lines = [f"not {{ {attr_var} == {literal}; }};" for literal in keep_literals]
        try_block = "\n".join(
            [
                "try {",
                f"  {var_name} has {attr_name} {attr_var};",
                *[f"  {g}" for g in guard_lines],
                "};",
            ]
        )
        match_statements.append(try_block)
    return match_statements


def build_single_value_match_clauses(
    single_value_updates: dict[str, Any],
    var_name: str = "$e",
) -> list[str]:
    """Build match clauses for single-value attribute updates.

    Creates try blocks to bind existing attribute values for deletion.
    TypeDB 3.x requires delete-then-insert for attribute replacement.

    Args:
        single_value_updates: Dict of attr_name -> value to update
        var_name: Variable name for the entity/relation

    Returns:
        List of match clause strings (try blocks)
    """
    match_statements = []
    for attr_name in single_value_updates:
        match_statements.append(
            f"try {{ {var_name} has {attr_name} $old_{attr_name}_{var_name.replace('$', '')}; }};"
        )
    return match_statements


def build_delete_match_clauses(
    single_value_deletes: set[str],
    var_name: str = "$e",
) -> list[str]:
    """Build match clauses for single-value attribute deletions.

    Creates try blocks to bind existing attribute values for deletion.

    Args:
        single_value_deletes: Set of attr_names to delete
        var_name: Variable name for the entity/relation

    Returns:
        List of match clause strings (try blocks)
    """
    match_statements = []
    for attr_name in single_value_deletes:
        match_statements.append(
            f"try {{ {var_name} has {attr_name} ${attr_name}_{var_name.replace('$', '')}; }};"
        )
    return match_statements


def build_update_delete_clause(
    multi_value_updates: dict[str, list[Any]],
    single_value_updates: dict[str, Any],
    single_value_deletes: set[str],
    var_name: str = "$e",
) -> str:
    """Build the delete clause for update operations.

    Combines deletions for multi-value attrs, single-value updates,
    and explicit deletions.

    Args:
        multi_value_updates: Dict of attr_name -> list of values
        single_value_updates: Dict of attr_name -> value
        single_value_deletes: Set of attr_names to delete
        var_name: Variable name for the entity/relation

    Returns:
        Delete clause string (without "delete" keyword)
    """
    delete_parts = []
    var_suffix = var_name.replace("$", "")

    # Delete for multi-value attributes
    for attr_name in multi_value_updates:
        attr_var = f"${attr_name}_{var_suffix}"
        delete_parts.append(f"try {{ {attr_var} of {var_name}; }};")

    # Delete old values for single-value attributes being updated
    for attr_name in single_value_updates:
        delete_parts.append(f"try {{ $old_{attr_name}_{var_suffix} of {var_name}; }};")

    # Delete for explicit deletions
    for attr_name in single_value_deletes:
        delete_parts.append(f"try {{ ${attr_name}_{var_suffix} of {var_name}; }};")

    return "\n".join(delete_parts)


def build_update_insert_clause(
    multi_value_updates: dict[str, list[Any]],
    single_value_updates: dict[str, Any],
    var_name: str = "$e",
) -> str:
    """Build the insert clause for update operations.

    Creates has statements for all new attribute values.

    Args:
        multi_value_updates: Dict of attr_name -> list of values
        single_value_updates: Dict of attr_name -> value
        var_name: Variable name for the entity/relation

    Returns:
        Insert clause string (without "insert" keyword)
    """
    insert_parts = []

    # Insert multi-value attributes
    for attr_name, values in multi_value_updates.items():
        for value in values:
            formatted_value = format_value(value)
            insert_parts.append(f"{var_name} has {attr_name} {formatted_value};")

    # Insert single-value attributes
    for attr_name, value in single_value_updates.items():
        formatted_value = format_value(value)
        insert_parts.append(f"{var_name} has {attr_name} {formatted_value};")

    return "\n".join(insert_parts)


def build_entity_update_query_parts(
    entity: Any,
    model_class: type["Entity"],
    var_name: str = "$e",
) -> tuple[str, str, str, str]:
    """Build the TypeQL query parts for updating an entity.

    Shared implementation used by both EntityManager and EntityQuery.
    Prefers IID-based matching when available, falls back to key attributes.

    Args:
        entity: The entity instance to update
        model_class: The entity's model class
        var_name: Variable name for the entity in the query

    Returns:
        Tuple of (match_clause, delete_clause, insert_clause, update_clause)

    Raises:
        KeyAttributeError: If entity has no _iid and no key attributes
    """
    owned_attrs = model_class.get_all_attributes()

    # Prefer IID-based matching when available (like relation CRUD)
    entity_iid = getattr(entity, "_iid", None)
    use_iid_matching = entity_iid is not None

    # Fall back to key attributes if no IID
    match_filters: dict[str, Any] = {}
    if not use_iid_matching:
        for field_name, attr_info in owned_attrs.items():
            if attr_info.flags.is_key:
                key_value = getattr(entity, field_name, None)
                if key_value is None:
                    raise KeyAttributeError(
                        entity_type=model_class.__name__,
                        operation="update",
                        field_name=field_name,
                    )
                key_value = unwrap_attribute(key_value)
                attr_name = attr_info.typ.get_attribute_name()
                match_filters[attr_name] = key_value

        if not match_filters:
            raise KeyAttributeError(
                entity_type=model_class.__name__,
                operation="update",
                all_fields=list(owned_attrs.keys()),
            )

    # Extract single/multi-value updates
    single_value_updates, single_value_deletes, multi_value_updates = extract_update_values(
        entity, owned_attrs, skip_key_attrs=True
    )

    # Build Match Clause
    match_statements = []
    entity_match_parts = [f"{var_name} isa {model_class.get_type_name()}"]
    if use_iid_matching:
        # Use IID for precise matching
        entity_match_parts.append(f"iid {entity_iid}")
    else:
        # Use key attributes for matching
        for attr_name, attr_value in match_filters.items():
            formatted_value = format_value(attr_value)
            entity_match_parts.append(f"has {attr_name} {formatted_value}")
    match_statements.append(", ".join(entity_match_parts) + ";")

    # Add match for multi-value attributes with guards
    if multi_value_updates:
        for attr_name, values in multi_value_updates.items():
            keep_literals = [format_value(v) for v in dict.fromkeys(values)]
            attr_var = f"${attr_name}_{var_name.replace('$', '')}"
            guard_lines = [f"not {{ {attr_var} == {literal}; }};" for literal in keep_literals]
            try_block = "\n".join(
                [
                    "try {",
                    f"  {var_name} has {attr_name} {attr_var};",
                    *[f"  {g}" for g in guard_lines],
                    "};",
                ]
            )
            match_statements.append(try_block)

    # Add match for single-value deletes
    if single_value_deletes:
        for attr_name in single_value_deletes:
            attr_var = f"${attr_name}_{var_name.replace('$', '')}"
            match_statements.append(f"try {{ {var_name} has {attr_name} {attr_var}; }};")

    match_clause = "\n".join(match_statements)

    # Build Delete Clause
    delete_parts = []
    if multi_value_updates:
        for attr_name in multi_value_updates:
            attr_var = f"${attr_name}_{var_name.replace('$', '')}"
            delete_parts.append(f"try {{ {attr_var} of {var_name}; }};")

    if single_value_deletes:
        for attr_name in single_value_deletes:
            attr_var = f"${attr_name}_{var_name.replace('$', '')}"
            delete_parts.append(f"try {{ {attr_var} of {var_name}; }};")

    delete_clause = "\n".join(delete_parts)

    # Build Insert Clause (for multi-value attributes)
    insert_parts = []
    for attr_name, values in multi_value_updates.items():
        for value in values:
            formatted_value = format_value(value)
            insert_parts.append(f"{var_name} has {attr_name} {formatted_value};")

    insert_clause = "\n".join(insert_parts)

    # Build Update Clause (for single-value attributes)
    update_parts = []
    if single_value_updates:
        for attr_name, value in single_value_updates.items():
            formatted_value = format_value(value)
            update_parts.append(f"{var_name} has {attr_name} {formatted_value};")

    update_clause = "\n".join(update_parts)

    return match_clause, delete_clause, insert_clause, update_clause
