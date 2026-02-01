"""Shared utilities for CRUD operations.

This module re-exports functions from focused submodules for backward compatibility.
New code should import directly from the submodules:

- type_bridge.crud.formatting: format_value, unwrap_attribute
- type_bridge.crud.patterns: build_entity_match_pattern, build_relation_match_pattern, normalize_role_players
- type_bridge.crud.types: is_multi_value_attribute, resolve_entity_class, hydrate_attributes, etc.
- type_bridge.crud.iid: IID population utilities for entities and relations
- type_bridge.crud.role_players: Role player utilities
- type_bridge.crud.updates: Update query building utilities
"""

# Re-export from formatting
from type_bridge.crud.formatting import format_value, unwrap_attribute

# Re-export from iid
from type_bridge.crud.iid import (
    assign_entity_iids,
    assign_relation_iids,
    build_entity_iid_map,
    build_entity_iid_query,
    build_iid_type_fetch_clause,
    build_known_key_values,
    build_relation_iid_query,
    build_relation_result_map,
    get_key_attrs,
    match_entity_type,
    modify_match_for_type_binding,
    process_iid_type_results,
)

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
    resolve_entity_class,
)

# Re-export from updates
from type_bridge.crud.updates import (
    build_delete_match_clauses,
    build_entity_update_query_parts,
    build_multi_value_match_clauses,
    build_single_value_match_clauses,
    build_update_delete_clause,
    build_update_insert_clause,
    extract_update_values,
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
    "resolve_entity_class",
    "build_metadata_fetch",
    "hydrate_attributes",
    "extract_entity_key",
    # iid
    "process_iid_type_results",
    "get_key_attrs",
    "build_known_key_values",
    "build_iid_type_fetch_clause",
    "match_entity_type",
    "modify_match_for_type_binding",
    "build_entity_iid_query",
    "build_entity_iid_map",
    "assign_entity_iids",
    "build_relation_iid_query",
    "build_relation_result_map",
    "assign_relation_iids",
    # role_players
    "build_role_player_match",
    "resolve_entity_class_from_label",
    "build_role_player_fetch_items",
    "group_results_by_iid",
    "extract_relation_attributes",
    "extract_role_players_from_results",
    # updates
    "extract_update_values",
    "build_multi_value_match_clauses",
    "build_single_value_match_clauses",
    "build_delete_match_clauses",
    "build_update_delete_clause",
    "build_update_insert_clause",
    "build_entity_update_query_parts",
]
