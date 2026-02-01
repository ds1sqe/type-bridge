# Code Duplication Refactoring Plan

This plan addresses code duplication issues identified in the type_bridge codebase.
Each section has a checklist of locations to update.

## Phase 1: HIGH Severity - Core Utilities

### 1.1 Consolidate `format_value()` to single implementation ✅

Keep `type_bridge/crud/utils.py:format_value()` as the canonical implementation.

- [x] Remove `type_bridge/query.py:_format_value()` (lines 267-297), import from crud/utils
- [x] Remove `type_bridge/models/base.py:TypeDBType._format_value()` (lines 263-295), import from crud/utils
- [x] Update all callers to use the shared function

### 1.2 Add `unwrap_attribute()` utility ✅

Created utility function in `crud/utils.py`:

```python
def unwrap_attribute(value: Any) -> Any:
    """Extract raw value from Attribute instance."""
    return value.value if hasattr(value, "value") else value
```

- [x] Add `unwrap_attribute()` to `type_bridge/crud/utils.py`
- [ ] Replace inline `if hasattr(value, "value"): value = value.value` patterns (50+ occurrences - gradual adoption)

### 1.3 Replace inline multi-value detection with `is_multi_value_attribute()` ✅

- [x] `type_bridge/crud/relation/manager.py` in `_hydrate_entity_from_data()`
- [x] `type_bridge/crud/relation/query.py` in `execute()` player hydration

### 1.4 Extract role player normalization utility ✅

Created shared function:

```python
def normalize_role_players(role_players: dict[str, Any]) -> tuple[dict[str, list[Any]], dict[str, list[str]]]:
    """Normalize role players to always be lists for uniform handling."""
```

- [x] Add to `type_bridge/crud/utils.py`
- [x] `type_bridge/crud/relation/manager.py:insert()`
- [x] `type_bridge/crud/relation/manager.py:put()`
- [x] `type_bridge/models/relation.py:to_insert_query()` (already handles lists inline)

### 1.5 Extract role player match clause building ✅

Created shared utilities:

```python
def build_role_player_match(var_name: str, entity: Any, entity_type_name: str) -> str:
    """Build TypeQL match clause for a role player (IID-preferring, key fallback)."""

def extract_entity_key(entity: Any) -> tuple[str, str, Any] | None:
    """Extract the first key attribute from an entity for matching."""
```

- [x] `type_bridge/crud/relation/manager.py:_build_role_player_match()` - delegates to shared utility
- [ ] `type_bridge/crud/relation/manager.py:delete()` - use shared (complex logic, deferred)
- [ ] `type_bridge/crud/relation/manager.py:delete_many()` - use shared (complex logic, deferred)
- [ ] `type_bridge/crud/relation/query.py:execute()` - use shared (complex logic, deferred)
- [ ] `type_bridge/crud/relation/query.py:_populate_iids()` - use shared (complex logic, deferred)
- [ ] `type_bridge/crud/relation/query.py:delete()` - use shared (complex logic, deferred)
- [ ] `type_bridge/crud/relation/query.py:_build_update_query_parts()` - use shared (complex logic, deferred)

## Phase 2: HIGH Severity - Attribute Handling

### 2.1 Extract key attribute extraction utility ✅

Created `extract_entity_key()` in crud/utils.py. Locations for gradual adoption:

- [ ] `type_bridge/crud/entity/manager.py` (multiple locations)
- [ ] `type_bridge/crud/entity/query.py` (multiple locations)
- [ ] `type_bridge/crud/relation/manager.py` (multiple locations)
- [ ] `type_bridge/crud/relation/query.py` (multiple locations)

### 2.2 Extract attribute hydration utility

Create shared function:

```python
def hydrate_attributes(
    entity_class: type,
    raw_data: dict[str, Any],
    iid: str | None = None
) -> tuple[dict[str, Any], tuple[tuple[str, Any], ...]]:
    """Hydrate attributes from TypeDB fetch result.

    Returns (attrs_dict, key_values_tuple).
    """
```

Locations:

- [ ] `type_bridge/crud/entity/manager.py:_extract_attributes()` - KEEP as base
- [ ] `type_bridge/crud/entity/query.py:execute()` - use shared
- [ ] `type_bridge/crud/relation/manager.py:get()` - use shared
- [ ] `type_bridge/crud/relation/manager.py:get_by_iid()` - use shared
- [ ] `type_bridge/crud/relation/manager.py:_hydrate_entity_from_data()` - consolidate
- [ ] `type_bridge/crud/relation/query.py:execute()` - use shared

## Phase 3: MEDIUM Severity - Query Operations

### 3.1 Consolidate IID/type map building

- [ ] `type_bridge/crud/entity/manager.py:_get_iids_and_types()` - KEEP
- [ ] `type_bridge/crud/entity/query.py:_get_iids_and_types()` - import from manager or extract to utils

### 3.2 Consolidate entity type matching

- [ ] `type_bridge/crud/entity/manager.py:_match_entity_type()` - KEEP
- [ ] `type_bridge/crud/entity/query.py:_match_entity_type()` - import from manager or extract to utils

### 3.3 Consolidate `_populate_iids()` implementations

- [ ] `type_bridge/crud/entity/manager.py:_populate_iids()`
- [ ] `type_bridge/crud/entity/query.py:_populate_iids()`
- [ ] `type_bridge/crud/relation/manager.py:_populate_iids()`
- [ ] `type_bridge/crud/relation/query.py:_populate_iids()`

Strategy: Extract common logic to base utilities, keep entity/relation specific parts.

### 3.4 Consolidate update query building

- [ ] `type_bridge/crud/entity/manager.py:_build_update_query_parts()`
- [ ] `type_bridge/crud/entity/query.py:_build_update_query_parts()`
- [ ] `type_bridge/crud/relation/manager.py:update()` (inline)
- [ ] `type_bridge/crud/relation/query.py:_build_update_query_parts()`

## Phase 4: Verification

- [ ] Run full unit test suite: `uv run pytest tests/unit/`
- [ ] Run full integration test suite: `uv run pytest tests/integration/ -m integration`
- [ ] Run linting: `uv run ruff check .`
- [ ] Run formatting: `uv run ruff format .`
- [ ] Run type checking: `uvx ty check .`

## Summary of New Utilities in crud/utils.py

| Function                                     | Purpose                                    |
| -------------------------------------------- | ------------------------------------------ |
| `format_value(value)`                        | Format Python value for TypeQL (canonical) |
| `unwrap_attribute(value)`                    | Extract raw value from Attribute instance  |
| `is_multi_value_attribute(flags)`            | Check if attribute is multi-value          |
| `normalize_role_players(role_players)`       | Normalize to lists for multi-cardinality   |
| `build_role_player_match(var, entity, type)` | Build match clause for role player         |
| `extract_entity_key(entity)`                 | Extract first key attribute for matching   |
| `resolve_entity_class(base, type_name)`      | Resolve TypeDB type to Python class        |
| `build_metadata_fetch(var)`                  | Build fetch clause for IID/type metadata   |

## Notes

- Each refactoring should be done incrementally with tests run after each change
- Import cycles need to be avoided - utilities go in `crud/utils.py`
- Preserve backward compatibility - method signatures should not change
- Utilities are available for gradual adoption in remaining locations
