"""Match pattern building utilities for TypeQL queries."""

from typing import TYPE_CHECKING, Any, Literal

from type_bridge.query.ast import (
    EntityPattern,
    HasConstraint,
    LiteralValue,
    RelationPattern,
    RolePlayer,
)
from type_bridge.query.compiler import QueryCompiler

if TYPE_CHECKING:
    from type_bridge.models.entity import _QueryEntity as Entity
    from type_bridge.models.relation import _QueryRelation as Relation


def _get_literal_type(
    value: Any,
) -> Literal["string", "long", "double", "boolean", "datetime", "date"]:
    """Determine the LiteralValue type for a Python value."""
    if isinstance(value, bool):
        return "boolean"
    elif isinstance(value, int):
        return "long"
    elif isinstance(value, float):
        return "double"
    elif isinstance(value, str):
        return "string"
    else:
        return "string"


def build_entity_match_ast(
    model_class: type["Entity"],
    var: str = "$e",
    filters: dict[str, Any] | None = None,
) -> EntityPattern:
    """Build an AST EntityPattern for an entity.

    Args:
        model_class: The entity model class
        var: Variable name to use (default: "$e")
        filters: Attribute filters as field_name -> value

    Returns:
        EntityPattern AST node
    """
    constraints = []

    if filters:
        owned_attrs = model_class.get_all_attributes()
        for field_name, field_value in filters.items():
            if field_name in owned_attrs:
                attr_info = owned_attrs[field_name]
                attr_name = attr_info.typ.get_attribute_name()
                literal_type = _get_literal_type(field_value)
                constraints.append(
                    HasConstraint(
                        attr_name=attr_name,
                        value=LiteralValue(value=field_value, value_type=literal_type),
                    )
                )

    return EntityPattern(
        variable=var,
        type_name=model_class.get_type_name(),
        constraints=constraints,
    )


def build_entity_match_pattern(
    model_class: type["Entity"],
    var: str = "$e",
    filters: dict[str, Any] | None = None,
) -> str:
    """Build a TypeQL match pattern for an entity.

    Args:
        model_class: The entity model class
        var: Variable name to use (default: "$e")
        filters: Attribute filters as field_name -> value

    Returns:
        TypeQL match pattern string like "$e isa person, has name \"Alice\""

    Examples:
        >>> build_entity_match_pattern(Person, filters={"name": "Alice"})
        '$e isa person, has name "Alice"'
    """
    pattern = build_entity_match_ast(model_class, var, filters)
    return QueryCompiler().compile(pattern)


def build_relation_match_ast(
    model_class: type["Relation"],
    var: str = "$r",
    role_players: dict[str, str] | None = None,
) -> RelationPattern:
    """Build an AST RelationPattern for a relation.

    Args:
        model_class: The relation model class
        var: Variable name to use (default: "$r")
        role_players: Dict mapping role names to player variable names

    Returns:
        RelationPattern AST node

    Raises:
        ValueError: If a role name is not defined in the model
    """
    role_player_nodes = []

    if role_players:
        defined_roles = model_class._roles
        for role_name, player_var in role_players.items():
            if role_name not in defined_roles:
                raise ValueError(
                    f"Unknown role '{role_name}' for relation {model_class.__name__}. "
                    f"Available roles: {list(defined_roles.keys())}"
                )
            role_player_nodes.append(RolePlayer(role=role_name, player_var=player_var))

    return RelationPattern(
        variable=var,
        type_name=model_class.get_type_name(),
        role_players=role_player_nodes,
    )


def build_relation_match_pattern(
    model_class: type["Relation"],
    var: str = "$r",
    role_players: dict[str, str] | None = None,
) -> str:
    """Build a TypeQL match pattern for a relation.

    Args:
        model_class: The relation model class
        var: Variable name to use (default: "$r")
        role_players: Dict mapping role names to player variable names

    Returns:
        TypeQL match pattern string

    Raises:
        ValueError: If a role name is not defined in the model

    Examples:
        >>> build_relation_match_pattern(Employment, role_players={"employee": "$p"})
        '$r isa employment, (employee: $p)'
    """
    pattern = build_relation_match_ast(model_class, var, role_players)
    return QueryCompiler().compile(pattern)


def normalize_role_players(
    role_players: dict[str, Any],
) -> tuple[dict[str, list[Any]], dict[str, list[str]]]:
    """Normalize role players to always be lists for uniform handling.

    Handles both single entities and lists of entities (for multi-cardinality roles).
    Also generates unique variable names for each player in the match clause.

    Args:
        role_players: Dict mapping role_name -> entity or list of entities

    Returns:
        Tuple of:
        - normalized_players: Dict mapping role_name -> list of entities
        - var_mapping: Dict mapping role_name -> list of variable names

    Examples:
        >>> normalize_role_players({"employee": alice, "employer": company})
        ({"employee": [alice], "employer": [company]},
         {"employee": ["employee"], "employer": ["employer"]})

        >>> normalize_role_players({"member": [alice, bob]})
        ({"member": [alice, bob]},
         {"member": ["member_0", "member_1"]})
    """
    normalized_players: dict[str, list[Any]] = {}
    var_mapping: dict[str, list[str]] = {}

    for role_name, entity_or_list in role_players.items():
        # Normalize to list
        entities = entity_or_list if isinstance(entity_or_list, list) else [entity_or_list]
        normalized_players[role_name] = entities

        # Generate variable names
        var_names = []
        for i in range(len(entities)):
            var_name = f"{role_name}_{i}" if len(entities) > 1 else role_name
            var_names.append(var_name)
        var_mapping[role_name] = var_names

    return normalized_players, var_mapping
