# Code Review Checklist: type-bridge

This document outlines the remaining architectural and technical debt issues identified during the code review.

## 1. Architecture & Design Patterns

- [ ] **`_preserve_iid` Logic Bug**: In `TypeDBType`, the `_preserve_iid` validator (`mode="wrap"`) runs after `_wrap_raw_values` (`mode="before"`). Since `_wrap_raw_values` converts model instances to `dict` during revalidation, the `isinstance(values, cls)` check in `_preserve_iid` fails, leading to the loss of `_iid` during instance revalidation.
- [ ] **Schema Scanner Side-Effects**: `SchemaScanner.scan_attributes` modifies `cls.__annotations__` in-place. While intended for Pydantic v2, this side-effect makes the scanning process non-idempotent and potentially breaks other introspection tools or multiple scanning passes.
- [ ] **Inheritance Chain Contiguity**: The `SchemaScanner` logic for finding `base=True` parents assumes they are contiguous in the MRO. Verify if non-TypeDB base classes or complex mixins can break this traversal.

## 2. Query Engine & CRUD Solidity

- [ ] **Missing Recursive Deep Lookups**: `parse_role_lookup_filters` only supports a single level of nesting (`role__attr__lookup`). It cannot traverse the graph (e.g., `role__sub_role__attr`), which limits the ORM's power compared to raw TypeQL.
- [ ] **Inconsistent Exception Handling**: `TypeDBManager` catches generic `Exception` during hydration and merely logs it. This can result in the library returning incomplete or corrupted result sets to the user without raising an error.
- [x] **Variable Scoping Fragility**: ~~`generate_attr_var` used simple `prefix_attrname` scheme.~~ **FIXED**: Now uses double underscore separator (`$var__attr`) to prevent collisions like `$actor_name + status` vs `$actor + name_status`. Also sanitizes hyphens to underscores since TypeDB variables can't contain hyphens.

## 3. Performance & Resource Management

- [ ] **Query Compiler Caching**: AST-to-TypeQL compilation happens on every call. For standard CRUD operations, many queries are structurally identical. Caching compiled strings would provide a performance boost.
- [ ] **Large Result Set Memory Usage**: Verify that `ConnectionExecutor` handles result sets larger than the driver's default page size (100-1000) correctly without causing memory spikes during iteration.

## 4. Test Suite Completeness

- [ ] **Recursive Depth & Cycle Detection**: Test behavior with deep recursive relations (50+ levels) and cyclic graphs (A -> B -> A). Although hydration is currently shallow, future deep-fetch features will require robust cycle detection to prevent infinite loops.
- [ ] **Attribute Validation Overhead**: `Attribute._pydantic_validate` is called for every attribute value. Profile this for high-throughput data ingestion to ensure it isn't a bottleneck.
