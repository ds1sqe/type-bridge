"""Shared utilities for CRUD operations.

This module re-exports functions from focused submodules for convenience.
You can also import directly from the submodules:

- type_bridge.crud.formatting: format_value, unwrap_attribute
- type_bridge.crud.patterns: build_entity_match_pattern, build_relation_match_pattern, normalize_role_players
- type_bridge.crud.types: is_multi_value_attribute, hydrate_attributes, etc.
- type_bridge.crud.role_players: Role player utilities
"""

# Re-export from formatting
from type_bridge.crud.formatting import format_value, unwrap_attribute

# Re-export from patterns
from type_bridge.crud.patterns import (
    build_entity_match_pattern,
    build_relation_match_pattern,
    normalize_role_players,
)

# Re-export from role_players
from type_bridge.crud.role_players import (
    build_role_player_fetch_items,
    build_role_player_match,
    extract_relation_attributes,
    extract_role_players_from_results,
    group_results_by_iid,
    resolve_entity_class_from_label,
)

# Re-export from types
from type_bridge.crud.types import (
    build_metadata_fetch,
    extract_entity_key,
    hydrate_attributes,
    is_multi_value_attribute,
)

__all__ = [
    # formatting
    "format_value",
    "unwrap_attribute",
    # patterns
    "build_entity_match_pattern",
    "build_relation_match_pattern",
    "normalize_role_players",
    # types
    "is_multi_value_attribute",
    "build_metadata_fetch",
    "hydrate_attributes",
    "extract_entity_key",
    # role_players
    "build_role_player_match",
    "resolve_entity_class_from_label",
    "build_role_player_fetch_items",
    "group_results_by_iid",
    "extract_relation_attributes",
    "extract_role_players_from_results",
]
