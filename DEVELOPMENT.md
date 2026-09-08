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

The generated C package and `type-bridge-c` crate are a separate internal
foundation under development. They are neither a supported SDK nor a release
distribution until #110 completes the application contract.

## Requirements

- Python 3.12–3.14; `.python-version` pins the local default to 3.13
- [uv](https://docs.astral.sh/uv/) for Python and workspace dependencies
- Rust 1.88+ for the public SDK and Rust workspace
- Node 18+ for the Node package; the primary development matrix uses Node 20
- CMake 3.20+ plus C17 and C++17 compilers when changing the internal C
  schema-package foundation; Unix checks also require `pkg-config`
- TypeDB 3.x for integration tests
- Podman or Docker for the default isolated live suite

## Set up the source tree

```bash
uv sync --extra dev --extra docs
```

The patched PyO3 0.29 binding supports the declared CPython 3.12–3.14 source
matrix directly and preserves the abi3-py312 wheel baseline. No forward-
compatibility override is required. The module retains its GIL requirement.

## Repository map

| Path | Responsibility |
| --- | --- |
| `type_bridge/` | Python connection/query facade and read-only archive recovery APIs |
| `type-bridge-core/crates/` | Rust contracts, engines, ORM, bindings, CLI, and server |
| `type-bridge-core/crates/node/` | N-API boundary and TypeScript package |
| `type-bridge-core/crates/rust/` | Public generated-model Rust client |
| `type-bridge-core/crates/c/` | Private C projected-value and provider-lifecycle ABI foundation |
| `docs/` | MkDocs source, guides, maintainer contracts, and site assets |
| `examples/` | Split-YAML workspace and generated-package application examples |
| `tests/` | Python unit, integration, compatibility, contract, and parity tests |
| `scripts/` | Source-tree checks, generated files, and focused live runners |

Browse the live tree with `fd`, `rg --files`, or `ls`; do not maintain a
duplicated directory snapshot here.

## Architecture invariants

- Rust is the only semantic engine for V2 behavior.
- Python and Node bindings marshal typed values and expose language-native
  facades; they do not reimplement schema, query, migration, or ORM rules.
- The generated C package and native C ABI are an internal foundation under
  development. ABI 1.4 verifies flat and chunked schema-package evidence and provides opaque
  projected values/models plus synchronous runtime, exact-3.12.3 database,
  policy-aware read/write transaction, and pre-dispatch cancellation handles,
  together with generated nominal exact single-entity and single-relation
  CRUD/count, homogeneous atomic mutation batches, and closed typed role-player
  unions. The same internal boundary includes a
  generated nominal typed-query facade, reductions, schema-function calls, and
  caller-owned remote transport over Rust-owned query semantics. Ordered C-v3
  packages also compose those frozen query entries into generated nominal
  field-token manager filters with database and borrowed-read terminals; this
  adds no native exports. Its chunked
  package resources and streaming create builder keep generated objects within
  the hosted C11 portability floors. Ordered C-v3 packages select this successor
  surface, but exact TypeDB 3.12.3 live acceptance leaves ordered attributes and
  ordered role-player lists empty and makes no list-instance evidence claim. C
  is not yet a supported SDK or release artifact.
- Generated files are projections of canonical schema authority and must not be
  edited by hand.
- Separately retained V1 query surfaces stay available unless an exact future
  inventory schedules their removal; they are not schema authority.
- Split-YAML is the only active schema/model authoring authority. Python,
  TypeScript/Node, and Rust applications consume generated projections.
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

Rust scope checks require cargo-audit 0.22.2. The shared CI/release gate
`bash scripts/ci/check_dependency_security.sh` audits both maintained lockfiles
against a freshly fetched database without target/severity filters.
Vulnerabilities, unsoundness, yanked crates and audit errors block acceptance.
The retained TypeDB transport graph requires rustls-pemfile 2.2.0 through
tonic 0.12.3. RUSTSEC-2025-0134 reports it unmaintained, not vulnerable; that
informational finding remains visible without an advisory ignore.

Facade builds validate metadata in the exact digest-verified PyPI publisher
image with networking disabled before cross-registry publication. PyPI uses
the pinned metadata-2.5-compatible publisher with Trusted Publishing and
attestations enabled. Historical recovery identities remain unchanged.

```bash
# Default offline Python tests
uv run pytest

# Full source-tree suite; starts and removes an isolated TypeDB by default
./test.sh

# Offline-only Rust + Python + Node + internal C-foundation tiers
./test.sh --no-integration

# Scope-level CI mirrors
./scripts/check.sh rust
./scripts/check.sh python
./scripts/check.sh node
./scripts/check.sh c
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

The 2.0.2 facade publisher recovery is separately selected with
`release_channel=notice-recovery` in `release.yml` on `release/2.0.2-notice`.
It defaults to `recovery_mode=verify`; publishing requires the exact successful
same-control verification run in `notice_verify_run_id`. The committed
`.github/release/v2.0.2-recovery.json` binds the original tag, partial stable
run, every job/step, archive identity and payload hash. New publisher controls
do not change the original artifact source. Recovery never rebuilds artifacts
or republishes Cargo, npm, native-core PyPI or GHCR; it signs an explicit
promotion predicate and retains PyPI Trusted Publishing and attestations.
The subsequent GitHub notice/assets must be independently verified before
making the draft public. The old v2.0.0 recovery remains separately frozen.

GitHub-only notice finalization uses `release_channel=notice-finalize` and
`notice_finalize_mode=verify`, then `draft`, then `publish`. Both mutating
stages require the same-control verification run in
`notice_finalize_verify_run_id`. Its separately pinned finalization ledger
requires the successful facade recovery, all 13 exact assets and full notice
body. Draft assets are downloaded and verified before publication. This path
uses existing workflow release-writing permissions and cannot republish any
package or container; it does not require changing local credential scopes.

Python facade builds also run a network-disabled metadata check in the exact
digest-verified publisher image before entering cross-registry publication.

The separately selected C 2.2.0 path uses `c-release.yml`: read-only verification
consumes successful same-source master CI artifacts and runs complete V1–V6
and FULL-C acceptance. Protected promotion consumes that exact verification
run, signs the unchanged bytes and adds only absent or identical assets to
the ordinary GitHub draft. See the [C release procedure](docs/development/c-release.md)
for the selected Ubuntu 24.04 shared-runtime matrix, provenance, partial-upload
recovery and independent public acceptance required before claiming support.

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
