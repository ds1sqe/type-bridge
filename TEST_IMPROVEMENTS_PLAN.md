# Test Infrastructure Improvements Plan

This plan outlines improvements to type-bridge's test infrastructure, combining infrastructure patterns from the autok project with functional coverage gaps identified through systematic analysis.

---

## Current State

- **1,132 total tests** (768 unit, 364 integration)
- Single `tests/integration/conftest.py` with all fixtures
- Manual test data creation
- Basic Docker container lifecycle management
- No test performance tracking

### Key Problem

Tests are **happy-path focused** and don't catch bugs in:

- Parser → Renderer → Use pipeline (data parsed but never used)
- Constraint enforcement at runtime
- Edge cases in CRUD operations
- Complex query patterns

Evidence: Recent bugs found in generator where `plays_cardinalities`, `docstrings`, and `annotations` were parsed but never rendered - none caught by tests.

### Priority Summary

| Phase | Section                    | Purpose                                | Est. Tests |
| ----- | -------------------------- | -------------------------------------- | ---------- |
| **1** | 2,4,5,6. Infrastructure    | Simplify before adding tests           | -          |
| **2** | 9. Generator Pipeline E2E  | Parser/renderer disconnects            | ~10        |
| **2** | 10. Constraint Enforcement | Constraints not enforced               | ~15        |
| **2** | 11. CRUD Edge Cases        | Update/delete failures, optional attrs | ~10        |
| **3** | 8. Complex Queries         | Query builder bugs                     | ~10        |
| **3** | 13. Complex Relations      | Relation CRUD failures                 | ~10        |
| **3** | 12. TypeDB 3.8 Built-ins   | Missing feature support                | ~6         |
| **4** | 1,3,7. Perf & Organization | Optional improvements                  | -          |

---

## Proposed Improvements

### 1. Test Duration Tracking (Phase 4 - Optional)

**Goal:** Track test performance over time to detect regressions and identify optimization targets.

**Implementation:**

- Create `tests/duration_db.py` - SQLite-based duration storage
- Add pytest hooks in `conftest.py` to record durations
- Provide query methods: `get_slowest_tests()`, `get_test_history()`
- Optional output via `LOG_TEST_DURATIONS=1` environment variable

**Files to create:**

- `tests/duration_db.py`

**Files to modify:**

- `tests/conftest.py` - Add `pytest_runtest_makereport` hook

**Reference:** `/Users/luca/code/autok/auto-k-server/src/tests/duration_db.py`

---

### 2. Test Utilities Directory (Phase 1)

**Goal:** Move reusable test helpers out of conftest.py into focused modules.

**New structure:**

```text
tests/
├── utils/
│   ├── __init__.py
│   ├── schema_helpers.py    # Schema setup utilities
│   ├── data_builders.py     # Entity/relation builders
│   ├── assertions.py        # TypeDB-specific assertions
│   ├── typedb_lifecycle.py  # Container and DB management
│   └── fixtures.py          # Shared fixture definitions
```

**Migration tasks:**

1. Extract `suppress_stderr()` → `utils/typedb_lifecycle.py`
2. Extract Docker/container logic → `utils/typedb_lifecycle.py`
3. Create schema builders → `utils/schema_helpers.py`
4. Create entity/relation factories → `utils/data_builders.py`

**Reference:** `/Users/luca/code/autok/auto-k-server/src/tests/utils/`

---

### 3. Resource Audit Trails (Phase 4 - Optional)

**Goal:** Track which test created each temporary database for easier debugging of leaks.

**Implementation:**

- Add `_db_origins: dict[str, str]` mapping db_name → test nodeid
- Monkeypatch database creation to register origins
- Log leaked databases with their originating tests on session end
- Add `INTENTIONAL_TYPEDB_LEAK=1` flag for debugging

**Files to create:**

- `tests/utils/typedb_lifecycle.py`

**Files to modify:**

- `tests/integration/conftest.py`

**Reference:** `/Users/luca/code/autok/auto-k-server/src/tests/utils/typedb_temp_db.py`

---

### 4. Class-Scoped Fixtures (Phase 1)

**Goal:** Reduce integration test time by reusing expensive schema setup across test classes.

**New fixtures:**

```python
@pytest.fixture(scope="class")
def db_with_schema_class(typedb_driver):
    """Class-scoped database with schema - reused across all tests in class."""
    ...

@pytest.fixture(scope="class")
def test_client_class(db_with_schema_class):
    """Class-scoped test client for expensive setup scenarios."""
    ...
```

**Use case:** Test classes that run many assertions against the same schema setup.

**Files to modify:**

- `tests/integration/conftest.py`

**Reference:** `/Users/luca/code/autok/auto-k-server/src/tests/conftest.py` (lines 180-220)

---

### 5. Data Factories with polyfactory (Phase 1)

**Goal:** Reduce boilerplate in test data creation with automatic factory generation.

**Implementation:**

```python
# tests/factories.py
from polyfactory.factories.pydantic_factory import ModelFactory
from type_bridge import Entity

class PersonFactory(ModelFactory[Person]):
    __model__ = Person
    name = Use(lambda: Name(f"Person-{uuid4().hex[:8]}"))
    email = Use(lambda: Email(f"test-{uuid4().hex[:8]}@example.com"))
```

**Benefits:**

- Auto-generate valid test data
- Override specific fields as needed
- Consistent naming patterns

**Dependencies to add:**

```toml
# pyproject.toml
[dependency-groups]
dev = [
    # ... existing
    "polyfactory>=2.13.0",
]
```

**Files to create:**

- `tests/factories.py`

**Reference:** `/Users/luca/code/autok/auto-k-server/src/tests/factories.py`

---

### 6. Custom Domain Assertions (Phase 1)

**Goal:** Create TypeDB-specific assertion helpers for cleaner test code.

**Implementation:**

```python
# tests/utils/assertions.py

def assert_entity_exists(db: Database, entity_type: type[Entity], **attrs) -> Entity:
    """Assert an entity with given attributes exists and return it."""
    manager = entity_type.manager(db)
    result = manager.filter(**attrs).first()
    assert result is not None, f"Expected {entity_type.__name__} with {attrs} to exist"
    return result

def assert_entity_count(db: Database, entity_type: type[Entity], expected: int) -> None:
    """Assert the count of entities matches expected."""
    manager = entity_type.manager(db)
    actual = manager.count()
    assert actual == expected, f"Expected {expected} {entity_type.__name__}, got {actual}"

def assert_relation_exists(db: Database, relation_type: type[Relation], **role_players) -> Relation:
    """Assert a relation with given role players exists."""
    ...
```

**Files to create:**

- `tests/utils/assertions.py`

---

### 7. Specialized Integration Conftest Files (Phase 4 - Optional)

**Goal:** Split integration conftest.py into domain-specific files for cleaner organization.

**New structure:**

```text
tests/integration/
├── conftest.py              # Base fixtures (container, driver)
├── crud/
│   └── conftest.py          # CRUD-specific fixtures
├── schema/
│   └── conftest.py          # Schema-specific fixtures
├── generator/
│   └── conftest.py          # Generator-specific fixtures
└── queries/
    └── conftest.py          # Query-specific fixtures
```

**Reference:** `/Users/luca/code/autok/auto-k-server/src/tests/integration/auth/conftest.py`

---

### 8. Complex Query Scenarios (Phase 3)

**Goal:** Address gaps in complex query scenarios, cross-entity filtering, and graph traversal.

**New Test Scenarios to Implement:**

1. **Deep Traversal (Chain of Command)**
   - **Description:** Verify 3-hop relation traversal.
   - **Query:** "Find the `CEO` of the company that employs the `User` who authored a specific `Comment`."
   - **Why:** Tests variable generation, nesting depth, and scope management.

2. **Dual-Constraint Relations**
   - **Description:** Filter a relation based on properties of _two different_ role players simultaneously.
   - **Query:** "Find an `Assignment` where the `Project` is 'Critical' AND the `Employee` has 'Junior' status."
   - **Why:** Tests complex `AND` logic within a `match` block involving multiple variables and ensures correct TypeQL generation for multi-player constraints.

3. **Self-Referential Graph (Cycles)**
   - **Description:** Test graph cycles and recursive structures.
   - **Query:** "Find a `Person` who is friends with someone they also work with." (Two different relations between the same two entities).
   - **Why:** Verifies that the query builder handles cyclic dependencies and variable reuse correctly without infinite recursion or invalid variable shadowing.

4. **Cross-Entity Filtering**
   - **Description:** Filter entities based on the attributes of related entities.
   - **Query:** "Find a `Trace` where the `origin` is a `Document` with `status='final'` AND the `destination` is a `Folder` created before 2024."
   - **Why:** Ensures that filters correctly propagate across relation boundaries.

5. **Complex Aggregation Filters**
   - **Description:** Use aggregation results as filters for other parts of the query.
   - **Query:** "Find all `Department` entities where the average salary of related `Employee` entities is greater than $100k."
   - **Why:** Validates that aggregation sub-queries are correctly formed and their results can be used in parent query logic.

6. **Polymorphic Queries with Shared Attributes**
   - **Description:** Query an abstract supertype filtering by an attribute shared by only some subtypes.
   - **Query:** "Find all `Artifact` entities (abstract) where `priority` is 'High' (attribute present in `UserStory` and `Bug`, but not `DesignDoc`)."
   - **Why:** Tests correct handling of polymorphic queries where attributes might not exist on all potential subtypes.

7. **Variable Collision Stress Test**
   - **Description:** Explicitly test queries that generate many variables.
   - **Query:** A constructed query that forces the generation of `$x`, `$y`, `$z`, `$x_1`, `$y_1` etc.
   - **Why:** Ensures that the variable name generator handles collisions robustly and doesn't produce invalid TypeQL with duplicate variable definitions.

**Files to create:**

- `tests/integration/queries/test_complex_graph.py`

---

### 9. Generator Pipeline End-to-End Tests (Phase 2)

**Goal:** Test the full pipeline: Parse TQL → Generate Python → Import → Create instances → Use with real DB.

**Problem:** Current tests verify generated code _imports_ but never _uses_ it. This allowed bugs where data was parsed but not rendered (plays_cardinalities, docstrings, annotations).

**New Test Scenarios:**

1. **Generate → Insert → Query Cycle**
   - Generate models from bookstore.tql fixture
   - Import generated classes
   - Create instances with all attribute types
   - Insert into real TypeDB
   - Query back and verify attributes match

2. **Role Cardinality Enforcement**
   - Generate relation with `@card(2..2)` on role
   - Create relation instance with list of 2 players
   - Verify DB operation succeeds
   - Attempt with 1 player → should fail or raise

3. **Constraint Propagation**
   - Generate entity with `@key`, `@unique`, `@regex`, `@range` constraints
   - Insert valid data → succeeds
   - Insert constraint-violating data → fails appropriately

4. **Annotation Rendering Verification**
   - Generate from schema with `## @api(public)` annotation
   - Verify annotation appears in generated code (as comment or metadata)
   - Access annotation at runtime if exposed

**Files to create:**

- `tests/integration/generator/test_generator_use_pipeline.py`

---

### 10. Constraint Enforcement Tests (Phase 2)

**Goal:** Verify TypeDB constraints are enforced during CRUD operations.

**Problem:** Constraints (@key, @unique, @card, @range, @regex, @values) are synced to DB but never tested for enforcement.

**New Test Scenarios:**

1. **@key Enforcement**
   - Insert entity with key attribute → succeeds
   - Insert second entity with same key → fails with clear error
   - Insert entity without key attribute → fails

2. **@unique Enforcement**
   - Insert entity with unique attribute → succeeds
   - Insert second entity with same unique value → fails
   - Update entity to duplicate unique value → fails

3. **@card Enforcement on Attributes**
   - Entity with `@card(1..3)` on attribute
   - Insert with 0 values → fails (min=1)
   - Insert with 4 values → fails (max=3)
   - Insert with 2 values → succeeds

4. **@card Enforcement on Roles**
   - Relation with `@card(2..2)` on role
   - Create with 1 player → fails
   - Create with 3 players → fails
   - Create with 2 players → succeeds

5. **@range Enforcement**
   - Attribute with `@range(0..100)`
   - Insert value 50 → succeeds
   - Insert value -1 → fails
   - Insert value 101 → fails

6. **@regex Enforcement**
   - Attribute with `@regex("^[A-Z]{3}$")`
   - Insert "ABC" → succeeds
   - Insert "abc" → fails
   - Insert "ABCD" → fails

7. **@values Enforcement**
   - Attribute with `@values("active", "inactive")`
   - Insert "active" → succeeds
   - Insert "pending" → fails

**Files to create:**

- `tests/integration/schema/test_constraint_enforcement.py`

---

### 11. CRUD Edge Cases (Phase 2)

**Goal:** Test CRUD operations beyond happy path.

**New Test Scenarios:**

1. **Update Edge Cases**
   - Update optional attribute from None to value
   - Update optional attribute from value to None (remove)
   - Update key attribute → should fail
   - Update with wrong type → type validation error

2. **Delete Edge Cases**
   - Delete non-existent entity → no-op, no error
   - Delete entity with relations → cascade or constraint error
   - Bulk delete with filter

3. **Optional Attribute Lifecycle**
   - Insert without optional attr → query returns None
   - Update to add optional attr → query returns value
   - Update to remove optional attr → query returns None again

4. **Multi-Value Attribute Operations**
   - Insert with empty list (min=0) → succeeds
   - Append to existing list via update
   - Remove from list via update
   - Replace entire list

**Files to create:**

- `tests/integration/crud/test_crud_edge_cases.py`

---

### 12. TypeDB 3.8+ Built-in Functions (Phase 3)

**Goal:** Test new TypeDB 3.8.0 built-in functions work correctly.

**New Test Scenarios:**

1. **iid() Function**
   - Query entity, get \_iid attribute
   - Use iid() in query, verify matches \_iid
   - Filter by iid() result

2. **label() Function**
   - Query polymorphic type (abstract parent)
   - Use label() to get concrete type name
   - Verify label matches Python class type

3. **Unicode Identifiers**
   - Create schema with unicode type names
   - Generate Python models
   - CRUD operations work correctly

**Files to create:**

- `tests/integration/queries/test_typedb_38_builtins.py`

---

### 13. Complex Relation Scenarios (Phase 3)

**Goal:** Test relation patterns beyond simple 2-role cases.

**New Test Scenarios:**

1. **Self-Referential Relations**
   - Entity plays same role twice (e.g., friendship where both are Person)
   - Verify insert, query, update, delete

2. **3+ Role Relations**
   - Relation with 3 distinct roles
   - All CRUD operations work

3. **Polymorphic Role Players**
   - Role accepts abstract type
   - Insert with different concrete subtypes
   - Query returns correct concrete types

4. **Relation with Multi-Value Role**
   - Role with @card(2..) allowing list of players
   - Insert with list
   - Query returns list
   - Update list (add/remove players)

5. **Relation Updates**
   - Update relation to change one role player
   - Verify old player detached, new player attached

**Files to create:**

- `tests/integration/crud/test_complex_relations.py`

---

## Implementation Order

### Phase 1: Infrastructure (Do First)

Simplify before adding more tests.

- [ ] **2. Test Utilities Directory**
  - `tests/utils/` structure
  - Extract helpers from conftest.py

- [ ] **5. Data Factories**
  - polyfactory integration
  - Entity/relation builders

- [ ] **6. Custom Assertions**
  - `assert_entity_exists()`, `assert_relation_exists()`

- [ ] **4. Class-Scoped Fixtures**
  - Reuse expensive schema setup

### Phase 2: Critical Coverage Gaps (Do Second)

These catch bugs that are hiding in plain sight. Use the new infrastructure.

- [ ] **9. Generator Pipeline E2E** - `test_generator_use_pipeline.py`
  - Generate → Import → Insert → Query → Verify
  - Catches parser/renderer disconnects like we just found

- [ ] **10. Constraint Enforcement** - `test_constraint_enforcement.py`
  - @key, @unique, @card, @range, @regex, @values
  - Ensures constraints aren't just synced but enforced

- [ ] **11. CRUD Edge Cases** - `test_crud_edge_cases.py`
  - Optional attribute lifecycle
  - Update/delete edge cases
  - Multi-value attribute operations

### Phase 3: Complex Patterns (Do Third)

These catch bugs in non-trivial usage patterns.

- [ ] **8. Complex Query Scenarios** - `test_complex_graph.py`
  - Deep traversal, dual-constraint, self-referential
  - Cross-entity filtering, polymorphic queries

- [ ] **13. Complex Relations** - `test_complex_relations.py`
  - Self-referential, 3+ roles, polymorphic players
  - Multi-value roles, relation updates

- [ ] **12. TypeDB 3.8+ Built-ins** - `test_typedb_38_builtins.py`
  - iid(), label() functions
  - Unicode identifiers

### Phase 4: Performance & Organization (Optional)

- [ ] **1. Test Duration Tracking**
  - `tests/duration_db.py`
  - pytest hooks for recording

- [ ] **3. Resource Audit Trails**
  - Track DB origins for leak debugging

- [ ] **7. Split Integration Conftest**
  - Domain-specific conftest files

---

## Files Summary

**New test files (functional coverage):**

- `tests/integration/generator/test_generator_use_pipeline.py` - E2E generator tests
- `tests/integration/schema/test_constraint_enforcement.py` - Constraint validation
- `tests/integration/crud/test_crud_edge_cases.py` - CRUD edge cases
- `tests/integration/crud/test_complex_relations.py` - Complex relation patterns
- `tests/integration/queries/test_complex_graph.py` - Complex query scenarios
- `tests/integration/queries/test_typedb_38_builtins.py` - TypeDB 3.8+ features

**New infrastructure files:**

- `tests/utils/__init__.py`
- `tests/utils/typedb_lifecycle.py`
- `tests/utils/schema_helpers.py`
- `tests/utils/data_builders.py`
- `tests/utils/assertions.py`
- `tests/duration_db.py`
- `tests/factories.py`

**Modified files:**

- `tests/conftest.py` - Add duration hooks
- `tests/integration/conftest.py` - Refactor to use utils
- `pyproject.toml` - Add polyfactory dependency

---

## Reference Projects

- **autok test setup:** `/Users/luca/code/autok/auto-k-server/src/tests/`
- **Key files to review:**
  - `duration_db.py` - Performance tracking
  - `utils/typedb_temp_db.py` - Resource lifecycle
  - `conftest.py` - Multi-scope fixture patterns
  - `factories.py` - polyfactory usage
