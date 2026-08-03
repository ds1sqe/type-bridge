# TypeBridge development

## Product boundary

TypeBridge is a multi-language TypeDB application toolkit, not only a Python
ORM. One Rust semantic engine owns schema, query, migration, validation,
generation, ORM, and provider behavior. The repository exposes that engine
through:

- the `type-bridge` Python package;
- the `@type-bridge/node` TypeScript/Node package;
- the crates.io-distributed generated Rust SDK;
- the `type-bridge` workspace and migration CLI;
- the `type-bridge-server` container.

Keep those distribution identities distinct while preserving their shared
contracts.

## Requirements

- Python 3.12–3.14; `.python-version` pins the local default to 3.13
- [uv](https://docs.astral.sh/uv/) for Python and workspace dependencies
- Rust 1.88+ for the public SDK and Rust workspace
- Node 18+ for the Node package; the primary development matrix uses Node 20
- TypeDB 3.x for integration tests
- Podman or Docker for the default isolated live suite

## Set up the source tree

```bash
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 uv sync --extra dev --extra docs
```

The PyO3 compatibility variable is required for current CPython 3.14 source
builds and harmless on 3.12–3.13. Published abi3 wheels do not need it.

## Repository map

| Path | Responsibility |
| --- | --- |
| `type_bridge/` | Python facade, Pydantic models, compatibility APIs |
| `type-bridge-core/crates/` | Rust contracts, engines, ORM, bindings, CLI, and server |
| `type-bridge-core/crates/node/` | N-API boundary and TypeScript package |
| `type-bridge-core/crates/rust/` | Public generated-model Rust client |
| `docs/` | MkDocs source, guides, maintainer contracts, and site assets |
| `examples/` | Executable Python examples |
| `tests/` | Python unit, integration, compatibility, contract, and parity tests |
| `scripts/` | Source-tree checks, generated files, and focused live runners |

Browse the live tree with `fd`, `rg --files`, or `ls`; do not maintain a
duplicated directory snapshot here.

## Architecture invariants

- Rust is the only semantic engine for V2 behavior.
- Python and Node bindings marshal typed values and expose language-native
  facades; they do not reimplement schema, query, migration, or ORM rules.
- Generated files are projections of canonical schema authority and must not be
  edited by hand.
- Existing V1 compatibility surfaces stay available unless the exact
  deprecation inventory schedules their removal.
- Rust releases starting with 2.0.1 resolve a complete, version-locked crates.io
  graph; the historical 2.0.0 SDK resolves from its exact release Git revision.
- Release-specific compatibility, trust, resource-limit, and security
  boundaries are contracts, not illustrative prose.

See [Internals](docs/development/internals.md),
[Rust backend](docs/development/rust-backend.md), and the
[unified typed-query contract](docs/development/typed-query-contract.md) before
changing a shared boundary.

## Test and check

Use the smallest focused check while iterating, then the scope-level check
before handoff.

```bash
# Default offline Python tests
uv run pytest

# Full source-tree suite; starts and removes an isolated TypeDB by default
./test.sh

# Offline-only Rust + Python + Node tiers
./test.sh --no-integration

# Scope-level CI mirrors
./scripts/check.sh rust
./scripts/check.sh python
./scripts/check.sh node
./scripts/check.sh all

# Python quality checks
uv run ruff check .
uv run ruff format --check .
uv run pyright type_bridge/
uv run pyright tests/
```

Use `CONTAINER_TOOL=podman ./test.sh` or
`CONTAINER_TOOL=docker ./test.sh` to choose an engine. Use
`./test.sh --no-isolated` only when intentionally targeting an existing
TypeDB.

Exact wheel, npm tarball, native-platform, multi-platform container, and
publication acceptance remains workflow-only. Local source checks do not
replace those gates. See [Testing](docs/development/testing.md) for suite
selection and environment variables.

## Documentation system

The site uses MkDocs Material:

- `mkdocs.yml` owns navigation, theme, Markdown extensions, and plugins.
- `docs/` contains authored pages and repository-owned assets.
- `scripts/gen_ref_pages.py` generates the Python API reference from
  `type_bridge/` docstrings and copies `CHANGELOG.md` into the site.
- `.github/workflows/docs.yml` performs the strict build and publishes the
  default branch to GitHub Pages.

When changing public behavior:

1. Update the relevant task guide and generated-reference docstring.
2. Keep long-lived page paths stable where practical.
3. Put every maintained page in `nav` or explicitly exclude non-site content.
4. Keep the root `README.md` at no more than 200 lines; route detail into docs.
5. Update project descriptions when positioning changes across README, MkDocs,
   and package metadata.
6. Build strictly:

   ```bash
   uv run --extra docs mkdocs build --strict
   ```

The public site is <https://ds1sqe.github.io/type-bridge/>.

## Change conventions

- Follow existing ownership boundaries and extend the correct shared API
  instead of adding facade-local workarounds.
- Add public API documentation where behavior or compatibility depends on it.
- Add inline comments only for non-obvious reasons.
- Use modern Python 3.12+ typing and project-specific Rust error types.
- Land tests with behavior changes.
- Keep temporary probes, generated reports, and verification notes in `tmp/`;
  it is ignored by Git.
- Preserve unrelated user changes in a dirty worktree.
- Do not stage, commit, push, publish, dispatch workflows, or mutate GitHub
  unless the current task authorizes it.

For deeper setup, container, IDE, and debugging guidance, see
[Development setup](docs/development/setup.md).
