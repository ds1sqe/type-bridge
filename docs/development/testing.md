# Testing Guide

TypeBridge uses a comprehensive two-tier testing approach with **100% test pass rate (1117/1117 tests)**.

## Table of Contents

- [Testing Strategy](#testing-strategy)
- [Unit Tests](#unit-tests)
- [Integration Tests](#integration-tests)
- [Test Execution Patterns](#test-execution-patterns)
- [Writing Tests](#writing-tests)

## Testing Strategy

TypeBridge employs a two-tier testing approach that balances speed, isolation, and real-world validation:

### Unit Tests (Default)
- **Fast**: Run in ~0.3 seconds without external dependencies
- **Isolated**: Test individual components in isolation
- **No TypeDB required**: Use mocks and in-memory validation
- **Run by default**: `pytest` runs unit tests only
- **768 tests total**: Organized by functionality (40 test files)

### Integration Tests
- **Sequential**: Use `@pytest.mark.order()` for predictable execution order
- **Real database**: Require running TypeDB 3.x server
- **End-to-end**: Test complete workflows from schema to queries
- **Explicit execution**: Must use `pytest -m integration`
- **349 tests total**: Full CRUD, schema, and query coverage (56 test files)

## Unit Tests

### Overview

Unit tests are located in `tests/unit/` with organized subdirectories:

```
tests/unit/
├── attributes/               # 10 test files - Attribute type tests
│   ├── test_boolean.py       # Boolean attribute type
│   ├── test_date.py          # Date attribute type
│   ├── test_datetime_tz.py   # DateTimeTZ attribute type
│   ├── test_decimal.py       # Decimal attribute type
│   ├── test_double.py        # Double attribute type
│   ├── test_duration.py      # Duration attribute type (ISO 8601)
│   ├── test_formatting.py    # Mixed attribute formatting
│   ├── test_integer.py       # Integer attribute type
│   ├── test_multivalue_escaping.py # Multi-value string escaping
│   └── test_string.py        # String attribute type
├── core/                     # 4 test files - Core functionality tests
│   ├── test_entity_dict.py   # Entity dict serialization
│   ├── test_inheritance.py   # Inheritance and type hierarchies
│   ├── test_inherited_attribute_filter.py # Inherited attribute filtering
│   └── test_multi_role_players.py # Multi-role player tests
├── crud/                     # 5 test files - CRUD operation unit tests
│   ├── test_format_value.py  # Value formatting for TypeQL
│   ├── test_lookup_parser.py # Lookup filter parsing
│   ├── test_negative_cases.py # Negative test cases
│   ├── test_role_lookup_parser.py # Role lookup parsing
│   └── test_update_queries.py # Update query generation
├── exceptions/               # 1 test file - Exception handling
│   └── test_exceptions.py    # Exception tests
├── expressions/              # 2 test files - Query expression API tests
│   ├── test_field_refs.py    # Field reference creation
│   └── test_role_player_expr.py # Role player expressions
├── fields/                   # 1 test file - Field descriptor tests
│   └── test_field_descriptor.py # Field descriptor functionality
├── flags/                    # 4 test files - Flag system tests
│   ├── test_attribute_typename_case.py # Attribute name formatting
│   ├── test_base_flag.py     # Base flag for schema exclusion
│   ├── test_deprecated_type_name.py # Deprecated type name handling
│   └── test_typename_case.py # Entity/Relation type name formatting
├── generator/                # 3 test files - Code generator tests
│   ├── test_generator.py     # Generator functionality
│   ├── test_naming.py        # Naming conventions
│   └── test_parser.py        # TypeQL parser tests
├── query/                    # 1 test file - Query builder tests
│   └── test_query_builder.py # Query builder functionality
├── session/                  # 1 test file - Session unit tests
│   └── test_session_unit.py  # Session functionality
├── type-check-except/        # 6 test files - Type checking validation
│   ├── test_basic.py         # Basic validation errors
│   ├── test_cardinality.py   # Cardinality validation
│   ├── test_list_default.py  # List default validation
│   ├── test_multi_role_players_invalid.py # Invalid multi-role players
│   ├── test_pydantic.py      # Pydantic validation
│   └── test_update_api.py    # Update API validation
└── validation/               # 2 test files - Validation tests
    ├── test_duplicate_attributes.py # Duplicate attribute detection
    └── test_reserved_words.py # Reserved word validation
```

### Running Unit Tests

```bash
# Run all unit tests (default)
uv run pytest                              # All 768 unit tests (~1s)
uv run pytest -v                           # With verbose output

# Run specific test category
uv run pytest tests/unit/core/             # Core tests
uv run pytest tests/unit/attributes/       # Attribute tests
uv run pytest tests/unit/flags/            # Flag tests
uv run pytest tests/unit/expressions/      # Expression tests
uv run pytest tests/unit/validation/       # Validation tests

# Run specific test file
uv run pytest tests/unit/attributes/test_integer.py -v
uv run pytest tests/unit/attributes/test_string.py -v

# Run specific test function
uv run pytest tests/unit/core/test_basic.py::test_entity_creation -v

# With coverage report
uv run pytest --cov=type_bridge --cov-report=html
```

### Unit Test Coverage

**Core API:**
- Entity/Relation creation
- Schema generation
- Inheritance and type hierarchies

**Attribute types (all 9 types):**
- Boolean, Date, DateTime, DateTimeTZ, Decimal, Double, Duration, Integer, String
- Value validation and type coercion
- Mixed formatting tests for query generation

**Flag system:**
- Base flags for schema exclusion
- Cardinality constraints (Card API)
- Type name formatting (snake_case, kebab-case, etc.)

**Expression API:**
- Field references and access
- Comparison operators (gt, lt, eq, etc.)
- String operations (contains, like, regex)
- Aggregation functions (avg, sum, min, max)

**Validation:**
- Pydantic integration
- Keyword and reserved word validation
- Type checking
- Schema validation (duplicate attribute type detection)

**String Escaping:**
- Multi-value attribute escaping (quotes, backslashes, Unicode)
- Edge cases: empty strings, single quotes, mixed escaping
- TypeQL string literal formatting

## Integration Tests

### Overview

Integration tests are located in `tests/integration/` with organized subdirectories:

```
tests/integration/
├── crud/                         # 35 test files total
│   ├── attributes/               # 11 test files - Attribute type operations
│   │   ├── test_boolean.py       # Boolean attribute CRUD
│   │   ├── test_date.py          # Date attribute CRUD
│   │   ├── test_datetime.py      # DateTime attribute CRUD
│   │   ├── test_datetimetz.py    # DateTimeTZ attribute CRUD
│   │   ├── test_decimal.py       # Decimal attribute CRUD
│   │   ├── test_double.py        # Double attribute CRUD
│   │   ├── test_duration.py      # Duration attribute CRUD
│   │   ├── test_integer.py       # Integer attribute CRUD
│   │   ├── test_multivalue_escaping.py # String escaping edge cases
│   │   ├── test_multi_value.py   # Multi-value attributes
│   │   └── test_string.py        # String attribute CRUD
│   ├── entities/                 # 8 test files - Entity CRUD operations
│   │   ├── test_bulk_operations.py # Bulk insert/update/delete
│   │   ├── test_chainable.py     # Chainable query operations
│   │   ├── test_delete.py        # Entity deletion
│   │   ├── test_fetch.py         # Entity fetching
│   │   ├── test_insert.py        # Entity insertion
│   │   ├── test_integer_key_bug.py # Integer key bug fix
│   │   ├── test_put.py           # Entity PUT operations
│   │   └── test_update.py        # Entity updates
│   ├── interop/                  # 3 test files - Cross-type operations
│   │   ├── test_mixed_entity.py  # Mixed entity operations
│   │   ├── test_mixed_queries.py # Complex mixed queries
│   │   └── test_type_combinations.py # Type combination tests
│   ├── relations/                # 9 test files - Relation CRUD operations
│   │   ├── test_abstract_roles.py # Abstract entity types in roles
│   │   ├── test_chainable.py     # Chainable query operations
│   │   ├── test_delete.py        # Relation deletion
│   │   ├── test_fetch.py         # Relation fetching
│   │   ├── test_filter.py        # Relation filtering
│   │   ├── test_insert.py        # Relation insertion
│   │   ├── test_multi_role.py    # Multi-player role relations
│   │   ├── test_put.py           # Relation PUT operations
│   │   └── test_update.py        # Relation updates
│   ├── test_filter_validation.py # Filter validation tests
│   ├── test_inherited_attribute_filter.py # Inherited attribute filtering
│   ├── test_issue_47.py          # Issue #47 regression test
│   └── test_update_advanced.py   # Advanced update operations
├── generator/                    # 1 test file - Code generator tests
│   └── test_generate_and_import.py # Generate and import test
├── queries/                      # 8 test files - Query builder tests
│   ├── test_expressions.py       # Query expressions
│   ├── test_filters.py           # Filter operations
│   ├── test_lookup_filters.py    # Lookup filter tests
│   ├── test_match.py             # Match clause tests
│   ├── test_pagination.py        # Limit, offset, sort
│   ├── test_role_lookup_filters.py # Role lookup filters
│   ├── test_role_multi_lookup_filters.py # Multi-role lookup filters
│   └── test_role_players.py      # Role player queries
├── schema/                       # 9 test files - Schema operations
│   ├── test_attributes.py        # Attribute schema tests
│   ├── test_cardinality.py       # Cardinality constraints
│   ├── test_conflict.py          # Conflict detection
│   ├── test_creation.py          # Schema creation
│   ├── test_diff.py              # Schema diff tests
│   ├── test_inheritance.py       # Inheritance hierarchies
│   ├── test_migration.py         # Schema migrations
│   ├── test_relations.py         # Relation schema tests
│   └── test_types.py             # Type schema tests
├── session/                      # 2 test files - Session tests
│   ├── test_session_lifecycle.py # Session lifecycle
│   └── test_transaction_context.py # TransactionContext operations
└── validation/                   # 1 test file - Validation tests
    └── test_reserved_words_integration.py # Reserved words integration
```

### Running Integration Tests

Integration tests require a running TypeDB 3.x server.

**Option 1: Isolated (Recommended - Automatic)**

```bash
# ./test.sh manages a TypeDB container automatically:
# - Started before the integration tiers with an engine-assigned host port
# - Each worktree gets its own compose project (tb-<worktree-basename>)
# - Torn down on exit (even on failure)
./test.sh                                 # Full source-tree suite, isolated
./test.sh -- -v                           # Forward -v to the pytest tiers
```

By default isolated mode uses engine-assigned ports, so two worktrees can run
their suites concurrently without colliding.  To pin a fixed port (useful for
debugging or connecting an external tool):

```bash
TYPEDB_PORT=1730 ./test.sh
TYPEDB_PORT=1730 TYPEDB_HTTP_PORT=8000 ./test.sh
```

To inspect a running isolated stack:

```bash
# Find the project name for the current worktree
./test.sh --print-project   # e.g. tb-v1-5-0

# List containers for that project
docker compose -p tb-v1-5-0 ps
```

**Option 2: Use Existing TypeDB Server**

```bash
# 1. Start TypeDB 3.x server manually
typedb server

# 2a. Full source-tree suite against the server (no container management)
./test.sh --no-isolated

# 2b. Python integration only (skip Docker)
USE_DOCKER=false uv run pytest -m integration
USE_DOCKER=false uv run pytest -m integration -v  # Verbose
```

**Run specific integration test categories:**

```bash
# Entity CRUD tests
uv run pytest tests/integration/crud/entities/ -v

# Relation CRUD tests
uv run pytest tests/integration/crud/relations/ -v

# Query expression tests
uv run pytest tests/integration/queries/ -v

# Schema operation tests
uv run pytest tests/integration/schema/ -v

# Specific test file
uv run pytest tests/integration/schema/test_conflict.py -v
uv run pytest tests/integration/queries/test_pagination.py -v
uv run pytest tests/integration/queries/test_expressions.py -v
uv run pytest tests/integration/crud/relations/test_abstract_roles.py -v
uv run pytest tests/integration/crud/relations/test_multi_role.py -v
```

**Run the generated Rust consumer E2E suite:**

```bash
./scripts/run-rust-projection-live.sh
```

This isolated TypeDB 3.12.1 lane generates the application-owned schema crate
and tests it as an external consumer of the public `type-bridge` crate. Its
focused tests cover schema binding and generated tokens, entity CRUD and scalar
domains, inheritance, relation/query/remote behavior, and transaction
commit/rollback/drop. The provider setup also verifies that representative
`@doc` annotations survive a define-and-export round trip.

### Integration Test Coverage

**Schema operations:**
- Schema creation and synchronization
- Conflict detection
- Inheritance hierarchies
- Schema migrations

**CRUD operations for all 9 attribute types:**
- Insert (single and bulk)
- Fetch (get, filter, all, first)
- Update (single-value and multi-value attributes)
- Delete

**Complex queries:**
- Query expressions (comparisons, string operations)
- Boolean logic (AND, OR, NOT)
- Aggregations (avg, sum, min, max, median, std)
- Group-by queries
- Pagination (limit, offset, sort)
- Filtering with role players

**Relations:**
- Abstract entity types in role definitions
- Multi-player roles (`Role.multi()`)
- Role player queries
- Relation inheritance

**TypeDB 3.x specific features:**
- Proper `isa` syntax (not `sub`)
- Offset before limit clause ordering
- Explicit sorting for pagination

**Transaction management:**
- READ, WRITE, SCHEMA transaction types
- Proper transaction lifecycle
- Database creation and cleanup

### Docker Setup for Integration Tests

**Requirements:**
- Docker or Podman with Compose installed
- A free host port (engine-assigned by default; set `TYPEDB_PORT` to pin one)

**Configuration:**

The project includes `docker-compose.yml` configured for the supported TypeDB version
(see [compatibility table](typedb.md#server-and-driver-compatibility)).
By default the host port is engine-assigned (0), so two worktrees can run
isolated stacks concurrently.  The compose project name is derived from the
worktree directory (`tb-<worktree-basename>`):

```yaml
services:
  typedb:
    image: ${TYPEDB_IMAGE:-typedb/typedb:3.11.5}
    ports:
      - "${TYPEDB_PORT:-0}:1729"
      - "${TYPEDB_HTTP_PORT:-0}:8000"
```

**Manual Docker control:**

```bash
# Start TypeDB container (engine assigns a free host port)
docker compose -p tb-v1-5-0 up -d

# Find the assigned host port
docker compose -p tb-v1-5-0 port typedb 1729

# View TypeDB logs
docker compose -p tb-v1-5-0 logs typedb

# Stop TypeDB container
docker compose -p tb-v1-5-0 down

# Remove volumes (clean slate)
docker compose -p tb-v1-5-0 down -v
```

## Test Execution Patterns

### Running All Tests

```bash
# Unit tests only (default, fast)
uv run pytest                              # Python unit tests

# Python integration only (requires TypeDB)
USE_DOCKER=false uv run pytest -m integration   # against a running TypeDB
./test.sh --no-isolated -- -m integration       # or via test.sh

# All source-tree tests (Rust + Python + Node, unit + integration)
./test.sh                                  # Isolated; manages TypeDB
```

The local entry points intentionally stop at source-tree gates. They do not
build or install Python release artifacts and do not claim publication parity.
CI and release jobs consume the exact built wheels and npm tarball; the release
workflow makes those consumers prerequisites for registry publication.

### Selective Test Execution

```bash
# By marker
uv run pytest -m unit           # Only unit tests
uv run pytest -m integration    # Only integration tests

# By keyword
uv run pytest -k "test_entity"  # All tests matching "test_entity"
uv run pytest -k "crud"         # All CRUD-related tests

# By path
uv run pytest tests/unit/                    # All unit tests
uv run pytest tests/integration/crud/        # All CRUD integration tests

# Specific test
uv run pytest tests/unit/core/test_basic.py::test_entity_creation
```

### Test Output Options

```bash
# Verbose output
uv run pytest -v

# Show print statements
uv run pytest -s

# Show captured logs
uv run pytest --log-cli-level=DEBUG

# Stop on first failure
uv run pytest -x

# Run last failed tests
uv run pytest --lf

# Run failed tests first
uv run pytest --ff
```

### Parallel Execution

```bash
# Install pytest-xdist
uv pip install pytest-xdist

# Run tests in parallel (unit tests only)
uv run pytest -n auto  # Auto-detect CPU count
uv run pytest -n 4     # Use 4 workers

# Note: Integration tests use @pytest.mark.order() and should run sequentially
```

## Writing Tests

### Unit Test Template

```python
"""Unit tests for [feature name]."""

import pytest
from type_bridge import Entity, TypeFlags, String, Flag, Key


class Name(String):
    pass


class TestFeature:
    """Test [feature description]."""

    def test_basic_functionality(self):
        """Test basic functionality."""
        # Arrange
        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name = Flag(Key)

        # Act
        person = Person(name=Name("Alice"))

        # Assert
        assert person.name.value == "Alice"
        assert Person.get_type_name() == "person"

    def test_edge_case(self):
        """Test edge case behavior."""
        # Test implementation
        pass

    def test_error_handling(self):
        """Test error handling."""
        with pytest.raises(ValueError, match="Expected error message"):
            # Code that should raise ValueError
            pass
```

### Integration Test Template

```python
"""Integration tests for [feature name]."""

import pytest
from type_bridge import Database, Entity, TypeFlags, String, Flag, Key, SchemaManager


class Name(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)


@pytest.mark.integration
@pytest.mark.order(1)
class TestFeatureIntegration:
    """Integration tests for [feature description]."""

    @pytest.fixture(autouse=True)
    def setup(self, db: Database):
        """Setup schema for tests."""
        schema_manager = SchemaManager(db)
        schema_manager.register(Person)
        schema_manager.sync_schema()

    def test_end_to_end_workflow(self, db: Database):
        """Test complete workflow with database."""
        # Create manager
        manager = Person.manager(db)

        # Insert
        alice = Person(name=Name("Alice"))
        manager.insert(alice)

        # Fetch
        persons = manager.all()
        assert len(persons) == 1
        assert persons[0].name.value == "Alice"

        # Update
        persons[0].name = Name("Alice Smith")
        manager.update(persons[0])

        # Verify
        updated = manager.get(name="Alice Smith")
        assert len(updated) == 1
```

### Test Best Practices

1. **Use descriptive test names**:
   ```python
   # ✅ Good
   def test_entity_with_optional_field_allows_none():
       pass

   # ❌ Bad
   def test_entity():
       pass
   ```

2. **Follow Arrange-Act-Assert pattern**:
   ```python
   def test_something():
       # Arrange: Set up test data
       person = Person(name=Name("Alice"))

       # Act: Perform the operation
       result = person.to_schema_definition()

       # Assert: Verify the result
       assert "person" in result
   ```

3. **One assertion per test** (when possible):
   ```python
   # ✅ Good
   def test_entity_type_name():
       assert Person.get_type_name() == "person"

   def test_entity_has_attributes():
       assert len(Person.get_owned_attributes()) > 0

   # ❌ Less ideal
   def test_entity():
       assert Person.get_type_name() == "person"
       assert len(Person.get_owned_attributes()) > 0
   ```

4. **Use fixtures for common setup**:
   ```python
   @pytest.fixture
   def person():
       return Person(name=Name("Alice"))

   def test_with_fixture(person):
       assert person.name.value == "Alice"
   ```

5. **Test edge cases and error conditions**:
   ```python
   def test_empty_string():
       pass

   def test_none_value():
       pass

   def test_invalid_type_raises_error():
       with pytest.raises(TypeError):
           # Invalid operation
           pass
   ```

### Test Organization

- **One test file per module**: `test_<module_name>.py`
- **Group related tests in classes**: Use `TestClassName` for grouping
- **Use markers**: `@pytest.mark.unit`, `@pytest.mark.integration`
- **Order integration tests**: Use `@pytest.mark.order(N)` for sequential tests

### Running Tests During Development

Quick test commands while developing:

```bash
# Test current file
uv run pytest tests/unit/core/test_basic.py -v

# Test with auto-rerun on file changes (requires pytest-watch)
uv run ptw tests/unit/

# Test with coverage
uv run pytest --cov=type_bridge tests/unit/

# Generate coverage HTML report
uv run pytest --cov=type_bridge --cov-report=html tests/unit/
open htmlcov/index.html  # View coverage report
```

---

For development setup, see [setup.md](setup.md).

For TypeDB-specific testing considerations, see [typedb.md](typedb.md).
