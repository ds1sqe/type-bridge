# Project Overview

**type-bridge** is a Python ORM for TypeDB. It provides Pythonic entity,
relation, attribute, schema, query, and CRUD APIs over TypeDB's TypeQL model.

The root package is Python-first. Native/shared binding work lives under
`type-bridge-core/`; follow the local README or active plan for that subtree.

## Python Version

This project requires **Python 3.13+**. The package metadata currently allows
`>=3.13,<3.15`; `.python-version` pins local development to `3.13`.

## Quick Start

```bash
# Install dependencies, including dev tools from pyproject.toml
uv sync --extra dev

# Run the default test set; pyproject excludes integration/proxy/benchmark tests
uv run pytest

# Run integration tests; fixtures manage TypeDB through Docker or Podman
./test-integration.sh

# Code quality
uv run ruff check .
uv run ruff format --check .
uv run pyright type_bridge/
uv run pyright tests/

# CI-shaped Python check
./scripts/check.sh python
```

For Podman, run integration tests with `CONTAINER_TOOL=podman
./test-integration.sh`. Use `CONTAINER_TOOL=docker` to force Docker.

## Documentation

| Scope | Link |
| --- | --- |
| Documentation site | <https://ds1sqe.github.io/type-bridge/> |
| User quick start | [README.md](README.md) |
| Development setup | [docs/development/setup.md](docs/development/setup.md) |
| Testing guide | [docs/development/testing.md](docs/development/testing.md) |
| TypeDB notes | [docs/development/typedb.md](docs/development/typedb.md) |
| Abstract types | [docs/development/abstract-types.md](docs/development/abstract-types.md) |
| Internals | [docs/development/internals.md](docs/development/internals.md) |
| API guide | [docs/guide/index.md](docs/guide/index.md) |
| Attributes | [docs/guide/attributes.md](docs/guide/attributes.md) |
| Entities | [docs/guide/entities.md](docs/guide/entities.md) |
| Relations | [docs/guide/relations.md](docs/guide/relations.md) |
| Cardinality and flags | [docs/guide/cardinality.md](docs/guide/cardinality.md) |
| CRUD | [docs/guide/crud.md](docs/guide/crud.md) |
| Queries | [docs/guide/queries.md](docs/guide/queries.md) |
| Schema | [docs/guide/schema.md](docs/guide/schema.md) |
| Generator | [docs/guide/generator.md](docs/guide/generator.md) |
| Validation | [docs/guide/validation.md](docs/guide/validation.md) |
| Basic examples | [examples/basic/](examples/basic/) |
| Unit tests | [tests/unit/README.md](tests/unit/README.md) |
| Integration tests | [tests/integration/README.md](tests/integration/README.md) |

Browse the live source tree with `fd`, `rg`, or `ls` instead of maintaining a
directory snapshot here.

## Project-Specific Notes

- Keep temporary scripts, reports, generated probes, and verification notes in
  `tmp/`; it is ignored by git.
- Integration tests require TypeDB 3.x. The top-level integration script
  auto-detects Podman first, then Docker, unless `CONTAINER_TOOL` is set.
- `AGENTS.md` is intentionally a symlink to this file. Update `CLAUDE.md`, not
  both files.
