#!/usr/bin/env bash
# Local source-tree CI checks. Release-artifact acceptance is workflow-only:
# this script neither builds/installs Python wheels nor claims publication parity.
# Run from repo root: ./scripts/check.sh [rust|python|node|all]
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# PyO3 0.23 predates CPython 3.14, while this extension deliberately targets
# abi3-py312. Keep source builds usable on both declared interpreter lines,
# including Python-only checks whose initial `uv run` may rebuild the core.
export PYO3_USE_ABI3_FORWARD_COMPATIBILITY="${PYO3_USE_ABI3_FORWARD_COMPATIBILITY:-1}"

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

pass=0
fail=0
failures=()

run_step() {
    local name="$1"
    shift
    printf "${CYAN}▶ %s${RESET}\n" "$name"
    if "$@"; then
        printf "${GREEN}  ✓ %s${RESET}\n\n" "$name"
        pass=$((pass + 1))
    else
        printf "${RED}  ✗ %s${RESET}\n\n" "$name"
        fail=$((fail + 1))
        failures+=("$name")
    fi
}

# ── Rust checks (matching ci.yml rust-check job) ────────────────────────────
run_rust() {
    printf "${BOLD}━━━ Rust ━━━${RESET}\n\n"

    run_step "cargo check --all-targets" \
        cargo check --manifest-path type-bridge-core/Cargo.toml --all-targets

    run_step "cargo test --all-targets" \
        cargo test --manifest-path type-bridge-core/Cargo.toml --all-targets

    run_step "contract alternate serde_json backend conformance" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-contract --features serde-backend-conformance

    run_step "released validation-rule wire without feature unification" \
        env CARGO_TARGET_DIR=type-bridge-core/target/rule-wire-standalone \
        cargo test --locked \
        --manifest-path type-bridge-core/crates/core/tests/fixtures/rule-wire-standalone/Cargo.toml

    run_step "schema-codegen Rust projection acceptance" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-schema-codegen --test rust_acceptance

    run_step "generated Rust projection acceptance on MSRV 1.88" \
        cargo +1.88.0 test --locked \
        --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-schema-codegen --test rust_acceptance \
        generated_rust_crate_compiles_rejects_invalid_types_and_runs -- --exact

    run_step "cargo clippy --all-targets -- -D warnings" \
        cargo clippy --manifest-path type-bridge-core/Cargo.toml --all-targets -- -D warnings
}

# ── Python checks (matching ci.yml lint + typecheck + test-unit jobs) ────────
run_python() {
    printf "${BOLD}━━━ Python ━━━${RESET}\n\n"

    run_step "ruff check ." \
        uv run ruff check .

    run_step "ruff format --check ." \
        uv run ruff format --check .

    run_step "pyright type_bridge/" \
        uv run pyright type_bridge/

    run_step "pyright tests/" \
        uv run pyright tests/

    run_step "typed Query negative Pyright contract" \
        uv run python tests/contracts/typed_query/python/check_negative.py

    run_step "owner-aware negative Pyright contract" \
        uv run python tests/unit/typed_query/check_negative.py

    run_step "typed Query API negative Pyright contract" \
        uv run python tests/unit/typed_query/check_query_negative.py

    run_step "schema-codegen Python projection acceptance" \
        uv run python type-bridge-core/crates/schema-codegen/tests/acceptance/check.py

    run_step "pytest tests/unit/" \
        uv run pytest tests/unit/ -x --tb=short -q
}

# ── Node checks (matching ci.yml node-check job) ────────────────────────────
run_node() {
    printf "${BOLD}━━━ Node ━━━${RESET}\n\n"

    # npm run executes each script with the node crate as its working directory,
    # so the scripts' relative paths (e.g. ../../../tmp/node-unit) resolve.
    pushd type-bridge-core/crates/node >/dev/null

    run_step "npm ci"                npm ci
    run_step "npm run build"         npm run build
    run_step "npm run typecheck"     npm run typecheck
    run_step "npm run typecheck:query-contract" npm run typecheck:query-contract
    run_step "npm run scope:probe"    npm run scope:probe
    run_step "schema-codegen TypeScript projection acceptance" \
        node ../schema-codegen/tests/typescript_acceptance/check.mjs
    run_step "npm run test:unit"     npm run test:unit
    run_step "npm run test:dts"      npm run test:dts
    run_step "npm run dts:parity"    npm run dts:parity
    run_step "npm run smoke:package" npm run smoke:package
    run_step "npm run smoke:legacy-package" npm run smoke:legacy-package
    run_step "npm run test:contract-adapter" npm run test:contract-adapter

    popd >/dev/null
}

# ── Dispatch ─────────────────────────────────────────────────────────────────
target="${1:-all}"
case "$target" in
    rust)   run_rust   ;;
    python) run_python ;;
    node)   run_node   ;;
    all)    run_rust; run_python; run_node ;;
    *)
        echo "Usage: $0 [rust|python|node|all]"
        exit 1
        ;;
esac

# ── Summary ──────────────────────────────────────────────────────────────────
printf "${BOLD}━━━ Summary ━━━${RESET}\n"
printf "${GREEN}  ✓ %d passed${RESET}\n" "$pass"
if ((fail > 0)); then
    printf "${RED}  ✗ %d failed:${RESET}\n" "$fail"
    for f in "${failures[@]}"; do
        printf "${RED}    - %s${RESET}\n" "$f"
    done
    exit 1
fi
