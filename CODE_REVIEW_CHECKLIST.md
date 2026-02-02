# Code Review Checklist: type-bridge

This document outlines the findings of a ruthless code review focused on architecture, solidity, bug resistance, and test completeness.

## 1. Architecture & Design Patterns

- [ ] **Fragmentation of Hydration Logic**: Object hydration (DB result -> Python instance) is dangerously fragmented across multiple modules:
  - `Entity.from_dict` / `_wrap_attribute_value` (models/entity.py)
  - `hydrate_attributes` (crud/types.py)
  - `extract_role_players_from_results` / `extract_relation_attributes` (crud/role_players.py)
  - _Risk_: Inconsistent handling of multi-value attributes, Attribute wrapping, and type coercion. A single "Hydrator" service is needed.
- [x] **Polymorphic Resolution Redundancy**: ~~Three separate mechanisms exist to resolve concrete classes from TypeDB labels.~~ **FIXED**: Removed dead code `resolve_entity_class()` from `crud/types.py` (never called). Simplified `resolve_entity_class_from_label()` in `crud/role_players.py` to directly search allowed classes and their subclasses. `ModelRegistry.get()` remains for general type lookup. See `crud/types.py` and `crud/role_players.py:61-98`.
- [ ] **Schema Scanner Side-Effects**: `SchemaScanner.scan_attributes` modifies `cls.__annotations__` in-place. While intended for Pydantic v2, this side-effect makes the scanning process non-idempotent and potentially breaks other introspection tools.
- [ ] **Inheritance Chain Complexity**: The logic in `SchemaScanner` for traversing MRO to find `base=True` parents is opaque. Verify it handles deep inheritance (e.g., `A(base=True) -> B(base=True) -> C(base=False)`) correctly.

## 2. Code Duplication & Structural Issues

- [x] **`to_ast` Implementation Duplication**: ~~`Entity.to_ast` and `Relation.to_ast` share significant logic for attribute serialization and TypeQL literal mapping.~~ **FIXED**: Extracted `_build_attribute_statements()` into `TypeDBType` base class. Both `Entity.to_ast` and `Relation.to_ast` delegate to this shared helper. See `models/base.py:478-529`.
- [x] **Identification Logic Redundancy**: ~~`EntityStrategy.identify()` and `Entity.get_match_pattern()` duplicated IID/key-attribute identification logic.~~ **FIXED**: Extracted `_build_identification_constraints()` into `Entity`. `EntityStrategy.identify()` now delegates to this method. See `models/entity.py:207-258`.
- [ ] **TypeQL Annotation Builders**: Both `AttributeFlags.to_typeql_annotations` and manual string formatting (in `Attribute` and `TypeDBType`) are used to generate `@abstract`, `@key`, `@card`, etc.
- [ ] **Variable Scoping Fragility**: `generate_attr_var` in `expressions/utils.py` uses a simple `prefix_attrname` scheme. This is prone to collisions in complex queries with similarly named variables (e.g., `$rel` and `$rel_item`).

## 3. Query Engine & CRUD Solidity

- [ ] **Missing Recursive Deep Lookups**: `parse_role_lookup_filters` only supports a single level of nesting (`role__attr__lookup`).
  - _Critical Failure_: It cannot traverse the graph (e.g., `role__other_role__attr`). This severely limits the power of the ORM compared to raw TypeQL.
- [x] **IID Retrieval Roundtrip**: ~~`TypeDBManager.insert` and `put` perform a separate fetch query after the write.~~ **FIXED**: Added `_execute_insert_with_iid()` helper that combines insert/put + fetch in a single query using TypeDB 3.x pipelining. Both `insert()` and `put()` now retrieve IID in one roundtrip. See `crud/typedb_manager.py:197-233`.
- [ ] **Inconsistent Exception Handling**: `TypeDBManager` catches generic `Exception` during hydration and merely logs it, potentially returning corrupted or incomplete datasets to the user.

## 4. Test Suite Completeness & Performance

- [ ] **Recursive Depth & Cycle Detection**: Test behavior with deep recursive relations (50+ levels) and cyclic graphs (A -> B -> A). Hydration logic must be verified for stack overflow or infinite loops.
- [ ] **Query Compiler Caching**: AST-to-TypeQL compilation happens on every call. Structural query caching could significantly improve performance for repetitive CRUD operations.
- [x] **Batching Efficiency**: ~~`insert_many`, `update_many`, etc., are currently just loops.~~ **FIXED** for most cases:
  - `insert_many`: Batches all entity inserts into a single query (N→1 roundtrips). See `_batch_insert_entities()`.
  - `put_many`: Uses optimistic batch with fallback on key constraint violations. Best case: N→1; worst case: N+1.
  - `delete_many`: Batches deletes using disjunctive OR-pattern matching by IID (N→2 roundtrips). See `_batch_delete_by_iid()`.
  - `update_many`: Still uses individual operations due to complexity (different attribute changes per entity).
  - Relations still use individual operations due to match clause complexity.
    See `crud/typedb_manager.py:911-1175`.
- [ ] **Large Result Set Iteration**: Verify that `ConnectionExecutor` handles result sets larger than the driver's default page size (100-1000) correctly without memory spikes.

## 5. Pydantic v2 Integration

- [ ] **Revalidation Recursion**: `TypeDBType._wrap_raw_values` uses `mode="wrap"`. Ensure this doesn't trigger infinite recursion in nested/recursive models when `revalidate_instances="always"` is active.
- [ ] **Attribute Validation Overhead**: `Attribute._pydantic_validate` is called for every single attribute value. Profile this for large-scale data ingestion.
