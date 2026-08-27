#!/usr/bin/env bash
# Local source-tree CI checks. Release-artifact acceptance is workflow-only:
# this script neither builds/installs Python wheels nor claims publication parity.
# Run from repo root: ./scripts/check.sh [rust|python|node|c|phase2-parity|phase2-live|all]
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

    run_step "first-party public Cargo docs" \
        python scripts/ci/validate_cargo_rustdoc.py

    run_step "first-party public Cargo docs on MSRV 1.88" \
        python scripts/ci/validate_cargo_rustdoc.py --toolchain 1.88.0

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
    run_step "ordered four-binding generated package compiler smoke" \
        cargo test --locked --manifest-path ../../Cargo.toml \
        -p type-bridge-schema-codegen --test ordered_collections \
        ordered_generated_packages_pass_all_four_language_compilers \
        -- --exact --ignored
    run_step "npm run typecheck"     npm run typecheck
    run_step "npm run typecheck:projection-integration" npm run typecheck:projection-integration
    run_step "npm run scope:probe"    npm run scope:probe
    run_step "schema-codegen TypeScript projection acceptance" \
        node ../schema-codegen/tests/typescript_acceptance/check.mjs
    run_step "npm run test:unit"     npm run test:unit
    run_step "npm run test:dts"      npm run test:dts
    run_step "npm run dts:parity"    npm run dts:parity
    run_step "npm run smoke:package" npm run smoke:package
    run_step "npm run test:contract-adapter" npm run test:contract-adapter

    popd >/dev/null
}

# ── C foundation checks (matching ci.yml rust-check C steps) ──────────────
run_c() {
    printf "${BOLD}━━━ C foundation (internal) ━━━${RESET}\n\n"

    run_step "frozen Plan 08 C distribution contract" \
        python scripts/ci/validate_c_distribution_contract.py

    run_step "standalone CLI candidate and hostile archive contracts" \
        uv run pytest tests/unit/compat/test_standalone_cli_candidate.py -q

    run_step "C runtime/generated-package candidate and hostile archive contracts" \
        uv run pytest tests/unit/compat/test_c_package_candidates.py -q

    run_step "C candidate supply-chain and hostile evidence contracts" \
        uv run pytest tests/unit/compat/test_c_distribution_security.py -q

    local c_shared_target c_shared_library
    c_shared_target="$(mktemp -d)"
    if [[ "$(uname -s)" == "Darwin" ]]; then
        c_shared_library="$c_shared_target/debug/libtype_bridge_c.dylib"
    else
        c_shared_library="$c_shared_target/debug/libtype_bridge_c.so"
    fi
    trap 'rm -rf -- "$c_shared_target"' EXIT

    run_step "generated C package and strict installed-compiler checks" \
        cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-schema-codegen --test c_emitter

    run_step "generated ABI 1.4 successor C17 and C++17 consumers" \
        cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-schema-codegen --test c_projection_live \
        phase4_successor_consumers_compile_as_strict_c17_and_cpp17 -- --exact

    run_step "C runtime, transaction, and cancellation ABI" \
        cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-c --lib --test execution_abi

    run_step "C typed-query, diagnostic, function, and remote ABI" \
        cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-c \
        --test query_abi \
        --test query_diagnostic_abi \
        --test query_function_abi \
        --test query_remote_abi

    run_step "build the isolated C ABI shared library" \
        env CARGO_TARGET_DIR="$c_shared_target" \
        cargo build --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-c --lib

    run_step "C ABI 1.4 additive header, export, and package ledger" \
        env TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER=1 \
        TYPE_BRIDGE_C_SHARED_LIBRARY="$c_shared_library" \
        cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-c --test abi_1_4

    run_step "C provider-free Phase-2 parity producer" \
        env TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER=1 \
        TYPE_BRIDGE_C_SHARED_LIBRARY="$c_shared_library" \
        cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-c --test phase2_projection_parity

    run_step "C schema-package ABI and standalone consumer" \
        env TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER=1 \
        TYPE_BRIDGE_C_SHARED_LIBRARY="$c_shared_library" \
        cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-c --test schema_package_abi

    run_step "C foundation on MSRV 1.88" \
        cargo +1.88.0 check --locked --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-c --all-targets

    rm -rf -- "$c_shared_target"
    trap - EXIT
}

run_generated_examples() {
    printf "${BOLD}━━━ Generated examples ━━━${RESET}\n\n"
    run_step "generated-only example workspace" \
        scripts/ci/validate_generated_examples.sh
}

run_phase2_parity() {
    printf "${BOLD}━━━ Provider-free Phase-2 projection parity ━━━${RESET}\n\n"
    run_step "four-binding provider-free Phase-2 parity fan-in" \
        uv run python scripts/ci/run_phase2_projection_parity.py
}

run_phase2_live() {
    printf "${BOLD}━━━ Exact-TypeDB-3.12.3 Phase-2 live parity ━━━${RESET}\n\n"
    run_step "four-binding exact-TypeDB-3.12.3 Phase-2 live fan-in" \
        uv run python scripts/ci/run_phase2_projection_live.py
}

# ── Dispatch ─────────────────────────────────────────────────────────────────
target="${1:-all}"
case "$target" in
    rust)   run_rust   ;;
    python) run_python ;;
    node)   run_node   ;;
    c)      run_c      ;;
    phase2-parity) run_phase2_parity ;;
    phase2-live) run_phase2_live ;;
    all)    run_rust; run_python; run_node; run_c; run_phase2_parity; run_generated_examples ;;
    *)
        echo "Usage: $0 [rust|python|node|c|phase2-parity|phase2-live|all]"
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
