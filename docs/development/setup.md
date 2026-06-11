# Development Guide

This guide covers development setup, commands, and code quality standards for TypeBridge.

## Table of Contents

- [Package Management](#package-management)
- [Docker Setup](#docker-setup)
- [Running Examples](#running-examples)
- [Code Quality Standards](#code-quality-standards)
- [Temporary Files Policy](#temporary-files-policy)

## Package Management

### Installing Dependencies

```bash
# Install all dependencies including dev tools
uv sync --extra dev

# Or install in editable mode with pip
uv pip install -e ".[dev]"
```

### Project Dependencies

The project requires:
- `type-bridge-core>=1.4.5`: Rust runtime for ORM connectivity and query execution
- `pydantic>=2.12.4`: For validation and type coercion
- `isodate==0.7.2`: For Duration type support (ISO 8601)
- `jinja2>=3.1.0`: Template engine for code generation
- `typer>=0.15.0`: CLI framework for generator and migration tools

`typedb-driver` is not a default runtime dependency. It is available through
the `dev` extra (pinned to `~=3.11.5`) and the `typedb-driver` extra for
integration tests and direct driver calls. The ORM's embedded runtime
dispatches automatically across the full TypeDB 3.8–3.11 window; this pin
is for the installed Python driver used by tests and direct driver APIs. See
the [compatibility table](typedb.md#server-and-driver-compatibility) for the
full support window.

Development dependencies include:
- `pytest`: Testing framework
- `ruff`: Fast Python linter and formatter
- `pyright`: Static type checker
- `pytest-order`: For ordered integration tests

## Docker Setup

### Integration Tests with TypeDB

Integration tests run against a TypeDB server in the supported window (see [compatibility table](typedb.md#server-and-driver-compatibility)). The project includes Docker/Podman configuration for automated setup.

**Requirements:**
- Docker or Podman with Compose support installed
- Port 1729 available (TypeDB server)

**`./test.sh` manages the TypeDB container automatically** (isolated by default). Simply run:

```bash
./test.sh                      # Full suite, isolated: starts TypeDB, runs tests, tears down
./test.sh --no-integration     # Offline tiers only (no container)
./test.sh -- -v                # Forward extra args (e.g. -v) to pytest
```

### Manual Docker Control

```bash
# Start TypeDB container
docker compose up -d

# View TypeDB logs
docker compose logs typedb

# Stop TypeDB container
docker compose down
```

### Skip Docker (Use Existing Server)

If you have a TypeDB server already running:

```bash
# Start your TypeDB 3.x server
typedb server

# Run integration tests without Docker
USE_DOCKER=false uv run pytest -m integration
USE_DOCKER=false uv run pytest -m integration -v  # Verbose
```

## Running Examples

TypeBridge includes comprehensive examples organized by complexity:

### Basic CRUD Examples (Start Here!)

```bash
uv run python examples/basic/crud_01_define.py  # Schema definition and basic usage
uv run python examples/basic/crud_02_insert.py  # Bulk insertion
uv run python examples/basic/crud_03_read.py    # Fetching API: get(), filter(), all()
uv run python examples/basic/crud_04_update.py  # Update API for single and multi-value attrs
```

### Advanced Examples

```bash
uv run python examples/advanced/schema_01_manager.py     # Schema operations
uv run python examples/advanced/schema_02_comparison.py  # Schema diff and comparison
uv run python examples/advanced/schema_03_conflict.py    # Conflict detection
uv run python examples/advanced/pydantic_features.py     # Pydantic integration
uv run python examples/advanced/type_safety.py           # Literal types for type safety
uv run python examples/advanced/string_representation.py # Custom __str__ and __repr__
```

## Code Quality Standards

TypeBridge maintains high code quality standards with zero tolerance for technical debt.

### Linting and Type Checking

All code must pass these checks without errors or warnings:

```bash
# Ruff - Python linter and formatter (must pass with 0 errors)
uv run ruff check .          # Check for style issues
uv run ruff format .         # Auto-format code

# Pyright - Static type checker (must pass with 0 errors, 0 warnings)
uv run pyright type_bridge/  # Check core library
uv run pyright examples/     # Check examples
uv run pyright tests/        # Check tests (note: intentional validation errors are OK)
```

### Code Quality Requirements

1. **No linter suppressions**: Do not use `# noqa`, `# type: ignore`, or similar comments
   - Exception: Tests intentionally checking validation failures may show type warnings

2. **Modern Python syntax**:
   - Use PEP 604 (`X | Y`) instead of `Union[X, Y]`
   - Use PEP 695 type parameters (`class Foo[T]:`) when possible
   - Use `X | None` instead of `Optional[X]`

   ```python
   # ✅ Modern (Python 3.12+)
   age: int | str | None

   # ❌ Deprecated
   from typing import Union, Optional
   age: Optional[Union[int, str]]
   ```

3. **Consistent ModelAttrInfo usage**:
   - Always use `attr_info.typ` and `attr_info.flags`
   - Never use dict-style access like `attr_info["type"]`

   ```python
   # ✅ CORRECT
   owned_attrs = Entity.get_owned_attributes()
   for field_name, attr_info in owned_attrs.items():
       attr_class = attr_info.typ
       flags = attr_info.flags

   # ❌ WRONG - Never use dict-style access
   attr_class = attr_info["type"]   # Will fail!
   flags = attr_info["flags"]       # Will fail!
   ```

4. **Import organization**: Imports must be sorted and organized (ruff handles this automatically)

### Testing Requirements

All tests must pass:

```bash
# Unit tests (default)
uv run pytest                              # All 425 unit tests

# Integration tests (./test.sh manages a TypeDB by default)
./test.sh --no-isolated                   # Python+Node integration against a running TypeDB
USE_DOCKER=false uv run pytest -m integration  # Python integration only

# All tests
uv run pytest -m ""                       # All Python tests
./test.sh                                 # Full suite (Rust + Python + Node), isolated
./scripts/check.sh                        # CI-shaped checks (rust|python|node|all)
```

When adding new features:
- Add corresponding tests in `tests/`
- Ensure examples in `examples/` demonstrate the feature
- Update documentation
- Run all quality checks before committing

### Pre-commit Checklist

Before committing changes, ensure:

1. ✅ All tests pass (`uv run pytest -m ""`)
2. ✅ Ruff passes (`uv run ruff check .`)
3. ✅ Code is formatted (`uv run ruff format .`)
4. ✅ Pyright passes with 0 errors (`uv run pyright type_bridge/`)
5. ✅ Examples run successfully
6. ✅ Documentation is updated

Quick command to run all checks:

```bash
./scripts/check.sh  # Runs the CI-shaped checks (rust|python|node|all)
```

## Temporary Files Policy

When creating temporary test scripts, reports, or analysis files during development/debugging:

- **Create them in the `tmp/` directory** (already in .gitignore)
- **DO NOT create temporary files in the project root**
- Examples: test scripts, debug reports, analysis documents, verification files
- Exception: Permanent documentation that should be committed belongs in the root or docs/

```bash
# ✅ CORRECT
tmp/test_script.py
tmp/debug_report.md
tmp/analysis.txt

# ❌ WRONG
test_script.py          # Don't put in root
debug_report.md         # Don't put in root
```

## Development Workflow

### Typical Development Cycle

1. **Create a feature branch**:
   ```bash
   git checkout -b feature/your-feature-name
   ```

2. **Make your changes**:
   - Edit code in `type_bridge/`
   - Add tests in `tests/unit/` or `tests/integration/`
   - Add examples in `examples/` if applicable

3. **Run tests locally**:
   ```bash
   # Quick: unit tests only
   uv run pytest

   # Full: Rust + Python + Node, unit + integration (isolated TypeDB)
   ./test.sh
   ```

4. **Check code quality**:
   ```bash
   ./scripts/check.sh
   ```

5. **Run examples to verify**:
   ```bash
   uv run python examples/basic/crud_01_define.py
   ```

6. **Update documentation**:
   - Update relevant docs in `docs/`
   - Update CHANGELOG.md
   - Update README.md if needed

7. **Commit and push**:
   ```bash
   git add .
   git commit -m "feat: your feature description"
   git push origin feature/your-feature-name
   ```

### Debugging Tips

**Using Python Debugger:**

```python
# Add breakpoint in code
breakpoint()

# Run test with pdb
uv run pytest tests/unit/test_something.py -s
```

**Verbose Test Output:**

```bash
# Show print statements and full output
uv run pytest -v -s

# Show captured logs
uv run pytest --log-cli-level=DEBUG
```

**TypeDB Query Debugging:**

Enable query logging in your test/example:

```python
import logging
logging.basicConfig(level=logging.DEBUG)
```

## Logging

TypeBridge uses Python's standard `logging` module for comprehensive logging throughout the library. This follows the best practice of letting library users configure logging as they prefer.

### Logger Hierarchy

TypeBridge loggers follow Python's module hierarchy:

```
type_bridge                              # Root logger
├── type_bridge.session                  # Connection, Database, Transaction
├── type_bridge.schema.manager           # SchemaManager operations
├── type_bridge.schema.migration         # Migration operations
├── type_bridge.crud.entity.manager      # Entity CRUD
├── type_bridge.crud.entity.query        # Entity queries
├── type_bridge.crud.entity.group_by     # Entity aggregations
├── type_bridge.crud.relation.manager    # Relation CRUD
├── type_bridge.crud.relation.query      # Relation queries
├── type_bridge.crud.relation.group_by   # Relation aggregations
├── type_bridge.query                    # Query builder
├── type_bridge.generator                # Code generation
│   ├── type_bridge.generator.parser
│   └── type_bridge.generator.render.*
├── type_bridge.models.entity            # Entity model
├── type_bridge.models.relation          # Relation model
└── type_bridge.validation               # Validation
```

### Enabling Logging

Since TypeBridge is a library, it doesn't configure logging by default. Enable logging in your application:

```python
import logging

# Enable all TypeBridge logging at DEBUG level
logging.basicConfig(level=logging.DEBUG)
logging.getLogger("type_bridge").setLevel(logging.DEBUG)

# Or enable specific modules
logging.getLogger("type_bridge.session").setLevel(logging.DEBUG)      # Connection/transaction logs
logging.getLogger("type_bridge.crud").setLevel(logging.DEBUG)         # CRUD operations
logging.getLogger("type_bridge.generator").setLevel(logging.INFO)     # Code generator
```

### Log Levels

| Level   | Use Case                                      | Example                                    |
|---------|-----------------------------------------------|-------------------------------------------|
| DEBUG   | Query text, detailed internal operations      | `Executing: match $e isa person; fetch...` |
| INFO    | Significant events, operation completion      | `Inserted 5 entities`                      |
| WARNING | Recoverable issues, validation warnings       | `Reserved word used as entity name`        |
| ERROR   | Failures (with exc_info where appropriate)    | `Connection failed: ...`                   |

### Example: Debugging Queries

```python
import logging

# Set up detailed logging for CRUD operations
logging.basicConfig(
    level=logging.DEBUG,
    format='%(name)s - %(levelname)s - %(message)s'
)

# Enable only CRUD logging
logging.getLogger("type_bridge.crud").setLevel(logging.DEBUG)
logging.getLogger("type_bridge.session").setLevel(logging.DEBUG)

# Your code - will now show query details
persons = Person.manager(db).filter(age=Age(30)).execute()
```

### Logging in Tests

```bash
# Show TypeBridge logs during tests
uv run pytest --log-cli-level=DEBUG

# Show logs only for specific modules
uv run pytest --log-cli-level=DEBUG -k "test_entity_insert"
```

For more details on logging configuration, see [docs/guide/logging.md](../guide/logging.md).

## Environment Setup

### Python Version

This project requires **Python 3.13+**. Check your Python version:

```bash
python --version  # Should show 3.13 or higher
```

If you need to install Python 3.13, use:

```bash
# Using pyenv (recommended)
pyenv install 3.13
pyenv local 3.13

# Or download from python.org
```

### Virtual Environment

The project uses `uv` for package management, which handles virtual environments automatically. If you need to manually activate:

```bash
# uv creates .venv automatically
source .venv/bin/activate  # Linux/macOS
.venv\Scripts\activate     # Windows
```

### IDE Configuration

**VS Code:**

Install recommended extensions:
- Python (Microsoft)
- Pylance (Microsoft)
- Ruff (Astral Software)

Configure settings.json:

```json
{
  "python.defaultInterpreterPath": ".venv/bin/python",
  "python.linting.enabled": true,
  "python.linting.ruffEnabled": true,
  "python.formatting.provider": "none",
  "[python]": {
    "editor.defaultFormatter": "charliermarsh.ruff",
    "editor.formatOnSave": true,
    "editor.codeActionsOnSave": {
      "source.fixAll": true,
      "source.organizeImports": true
    }
  }
}
```

**PyCharm:**

1. Set Python interpreter to `.venv/bin/python`
2. Enable Pyright as external tool
3. Configure Ruff as external formatter
4. Enable "Reformat code" on save

## Continuous Integration

The project uses GitHub Actions for CI. See `.github/workflows/ci.yml` for the full configuration.

CI runs:
- Linting (Ruff)
- Type checking (Pyright)
- Unit tests
- Integration tests (with Docker)

All checks must pass before merging PRs.

---

For testing specifics, see [testing.md](testing.md).

For TypeDB integration details, see [typedb.md](typedb.md).

For internal architecture, see [internals.md](internals.md).
