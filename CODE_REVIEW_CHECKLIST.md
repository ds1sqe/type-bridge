# Code Review Checklist: type-bridge

This document outlines the remaining architectural and technical debt issues identified during the code review.

## 1. Architecture & Design Patterns

- [ ] **Schema Scanner Side-Effects**: `SchemaScanner.scan_attributes` modifies `cls.__annotations__` in-place. While intended for Pydantic v2, this side-effect makes the scanning process non-idempotent and potentially breaks other introspection tools or multiple scanning passes.

## 2. Query Engine & CRUD Solidity

- [ ] **Missing Recursive Deep Lookups**: `parse_role_lookup_filters` only supports a single level of nesting (`role__attr__lookup`). It cannot traverse the graph (e.g., `role__sub_role__attr`), which limits the ORM's power compared to raw TypeQL.
- [x] **Inconsistent Exception Handling**: ~~Some methods still swallow exceptions.~~ **FIXED**: All hydration paths now raise `HydrationError` including `get_by_iid`.

## 3. Performance & Resource Management

- [ ] **Query Compiler Caching**: AST-to-TypeQL compilation happens on every call. For standard CRUD operations, many queries are structurally identical. Caching compiled strings would provide a performance boost.
- [ ] **Large Result Set Memory Usage**: `TypeDBManager` still loads entire result sets into memory lists (`instances = []`) rather than providing an iterator or generator. This could lead to memory exhaustion for very large queries.

## 4. Test Suite Completeness

- [x] **Recursive Depth & Cycle Detection**: ~~Test deep recursive relations and cyclic graphs.~~ **DONE**: Added tests for self-referential relations, cyclic structures (A→B→A), self-loops, and deep hierarchies (50+ levels). All pass without stack overflow or infinite loops.
- [x] **Attribute Validation Overhead**: ~~Profile `_pydantic_validate` for bottlenecks.~~ **PROFILED**: Validation is extremely fast (8M attrs/sec, 246k entities/sec). `_pydantic_validate` adds ~78% overhead vs direct instantiation but remains at 4M+/sec - not a bottleneck.
