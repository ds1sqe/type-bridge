#!/usr/bin/env bash
# Local CI check — mirrors .github/workflows/ci.yml
# Run from repo root: ./scripts/check.sh [rust|python|all]
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

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

    # PyO3 0.23.5 max is Python 3.13; local may be newer
    export PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1

    run_step "cargo check --all-targets" \
        cargo check --manifest-path type-bridge-core/Cargo.toml --all-targets

    run_step "cargo test --all-targets" \
        cargo test --manifest-path type-bridge-core/Cargo.toml --all-targets

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
    run_step "npm run test:unit"     npm run test:unit
    run_step "npm run test:dts"      npm run test:dts
    run_step "npm run smoke:package" npm run smoke:package

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
