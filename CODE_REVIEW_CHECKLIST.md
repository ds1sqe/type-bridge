# Code Review Checklist: type-bridge

This document outlines the remaining architectural and technical debt issues identified during the code review.

## 1. Architecture & Design Patterns

- [x] **`_preserve_iid` Logic Bug**: ~~In `TypeDBType`, the `_preserve_iid` validator (`mode="wrap"`) runs after `_wrap_raw_values` (`mode="before"`)~~. **FIXED**: Using `mode="wrap"` correctly captures the instance and its `_iid` before `_wrap_raw_values` converts it to a dict, ensuring `_iid` is preserved during revalidation.
- [ ] **Schema Scanner Side-Effects**: `SchemaScanner.scan_attributes` modifies `cls.__annotations__` in-place. While intended for Pydantic v2, this side-effect makes the scanning process non-idempotent and potentially breaks other introspection tools or multiple scanning passes.
- [x] **Inheritance Chain Contiguity**: ~~The `SchemaScanner` logic for finding `base=True` parents assumes they are contiguous in the MRO~~. **RESOLVED**: Logic correctly handles TypeDB hierarchy boundaries and contiguous base parents.

## 2. Query Engine & CRUD Solidity

- [ ] **Missing Recursive Deep Lookups**: `parse_role_lookup_filters` only supports a single level of nesting (`role__attr__lookup`). It cannot traverse the graph (e.g., `role__sub_role__attr`), which limits the ORM's power compared to raw TypeQL.
- [x] **Inconsistent Exception Handling**: ~~`TypeDBManager` catches generic `Exception` during hydration and merely logs it.~~ **FIXED**: Now raises `HydrationError` with full context (model_type, raw_data, cause) instead of silently swallowing exceptions. Users can catch `HydrationError` to handle hydration failures.
- [x] **Variable Scoping Fragility**: ~~`generate_attr_var` used simple `prefix_attrname` scheme.~~ **FIXED**: Now uses double underscore separator (`$var__attr`) to prevent collisions. Also sanitizes hyphens to underscores since TypeDB variables can't contain hyphens.

## 3. Performance & Resource Management

- [ ] **Query Compiler Caching**: AST-to-TypeQL compilation happens on every call. For standard CRUD operations, many queries are structurally identical. Caching compiled strings would provide a performance boost.
- [ ] **Large Result Set Memory Usage**: Verify that `ConnectionExecutor` handles result sets larger than the driver's default page size (100-1000) correctly without causing memory spikes during iteration.

## 4. Test Suite Completeness

- [ ] **Recursive Depth & Cycle Detection**: Test behavior with deep recursive relations (50+ levels) and cyclic graphs (A -> B -> A). Although hydration is currently shallow, future deep-fetch features will require robust cycle detection to prevent infinite loops.
- [ ] **Attribute Validation Overhead**: `Attribute._pydantic_validate` is called for every attribute value. Profile this for high-throughput data ingestion to ensure it isn't a bottleneck.
