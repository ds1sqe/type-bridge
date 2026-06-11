#!/usr/bin/env bash
# Full test suite — Rust + Python + Node, unit + integration.
#
# Reproduces the CI test jobs locally. Behaviour is flag-controlled:
#
#   ./test.sh                  full test, isolated (default): brings up its own TypeDB,
#                              runs every tier, tears the container down on exit
#   ./test.sh --no-integration unit/offline tiers only (no TypeDB, no container)
#   ./test.sh --proxy          additionally run the -m proxy suite (proxy stack)
#   ./test.sh --no-isolated    use an already-running TypeDB instead of managing one
#
# Flags compose, e.g. `./test.sh --proxy --no-isolated`. Args after `--` are forwarded to
# the pytest invocations (e.g. `./test.sh -- -k some_test`).
#
# Replaces the retired test-integration.sh / test-integration-dind.sh /
# test-proxy-integration.sh: the isolated default is the container-managed run, --proxy is
# the proxy suite, --no-isolated is the use-a-running-server path.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

# ── Flags ────────────────────────────────────────────────────────────────────
integration=1
proxy=0
isolated=1
pytest_args=()

usage() {
    cat <<'EOF'
Usage: ./test.sh [--no-integration] [--proxy] [--no-isolated] [-- <pytest args>]

  --no-integration  Run only the offline tiers (Rust, Python unit, Node unit/dts).
  --proxy           Additionally run the proxy integration suite (-m proxy).
  --no-isolated     Use an already-running TypeDB (USE_DOCKER=false) instead of
                    managing a container. Default is isolated: test.sh owns a TypeDB.

The local TypeDB convention is port 1730 (docker-compose.yml); override with TYPEDB_PORT
or TYPEDB_ADDRESS. CI alone uses 1729.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-integration) integration=0 ;;
        --proxy)          proxy=1 ;;
        --no-isolated)    isolated=0 ;;
        --)               shift; pytest_args=("$@"); break ;;
        -h|--help)        usage; exit 0 ;;
        *) printf "${RED}unknown flag: %s${RESET}\n\n" "$1" >&2; usage; exit 1 ;;
    esac
    shift
done

NODE_DIR=type-bridge-core/crates/node
TYPEDB_PORT="${TYPEDB_PORT:-1730}"
# Not exported: TYPEDB_ADDRESS is passed inline only to the integration tiers. Exporting it
# globally leaks into the offline `cargo test` tier, where the server config test honours a
# TYPEDB_ADDRESS override and fails when it disagrees with the config-file value.
TYPEDB_ADDRESS="${TYPEDB_ADDRESS:-localhost:${TYPEDB_PORT}}"

# ── Step runner (mirrors scripts/check.sh) ───────────────────────────────────
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

# ── TypeDB container lifecycle (isolated integration only) ───────────────────
# One shared TypeDB serves both the Python and Node integration tiers — the CI shape,
# reproduced locally. The proxy tier (--proxy) owns its own stack via proxy_lifecycle.py.
compose=""
typedb_started=0

detect_compose() {
    if [[ -n "${CONTAINER_TOOL:-}" ]]; then
        compose="$CONTAINER_TOOL compose"
    elif command -v podman >/dev/null 2>&1; then
        compose="podman compose"
    elif command -v docker >/dev/null 2>&1; then
        compose="docker compose"
    else
        printf "${RED}No container tool (podman/docker) found for isolated mode.${RESET}\n" >&2
        printf "Start a TypeDB yourself and re-run with --no-isolated.\n" >&2
        exit 1
    fi
}

start_typedb() {
    detect_compose
    printf "${BOLD}━━━ TypeDB (isolated, port %s) ━━━${RESET}\n\n" "$TYPEDB_PORT"
    TYPEDB_PORT="$TYPEDB_PORT" $compose -f docker-compose.yml up -d typedb
    typedb_started=1
    for _ in {1..45}; do
        if timeout 2 bash -c "</dev/tcp/127.0.0.1/${TYPEDB_PORT}" 2>/dev/null; then
            printf "${GREEN}TypeDB ready on ${TYPEDB_PORT}${RESET}\n\n"
            return 0
        fi
        sleep 2
    done
    printf "${RED}TypeDB did not open port ${TYPEDB_PORT} in time${RESET}\n" >&2
    exit 1
}

stop_typedb() {
    if [[ "$typedb_started" == 1 ]]; then
        printf "\n${BOLD}━━━ Tearing down TypeDB ━━━${RESET}\n"
        TYPEDB_PORT="$TYPEDB_PORT" $compose -f docker-compose.yml down -v || true
    fi
}
trap stop_typedb EXIT

# ── Offline tiers (always) ───────────────────────────────────────────────────
printf "${BOLD}━━━ Rust ━━━${RESET}\n\n"
export PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1
run_step "cargo test --all-targets" \
    cargo test --manifest-path type-bridge-core/Cargo.toml --all-targets

printf "${BOLD}━━━ Python (unit) ━━━${RESET}\n\n"
run_step "pytest tests/unit/" \
    uv run pytest tests/unit/ --tb=short -q

printf "${BOLD}━━━ Node (build + offline) ━━━${RESET}\n\n"
run_step "npm ci"            bash -c "cd '$NODE_DIR' && npm ci"
run_step "npm run build"     bash -c "cd '$NODE_DIR' && npm run build"
run_step "npm run test:unit" bash -c "cd '$NODE_DIR' && npm run test:unit"
run_step "npm run test:dts"  bash -c "cd '$NODE_DIR' && npm run test:dts"

# ── Integration tiers ────────────────────────────────────────────────────────
if [[ "$integration" == 1 ]]; then
    [[ "$isolated" == 1 ]] && start_typedb

    TYPEDB_HTTP_PORT="${TYPEDB_HTTP_PORT:-8000}"

    printf "${BOLD}━━━ Python (integration) ━━━${RESET}\n\n"
    run_step "pytest -m integration" \
        env USE_DOCKER=false TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        uv run pytest -m integration --tb=short "${pytest_args[@]}"

    # The parity suite mixes live-TypeDB tests with deliberately unmarked
    # offline ones (descriptor snapshots, generator parity); the marker
    # override mirrors CI's cross-language-parity job so the offline tests
    # don't fall through both the unit and `-m integration` selections.
    printf "${BOLD}━━━ Python (cross-language parity) ━━━${RESET}\n\n"
    run_step "pytest tests/integration/parity" \
        env USE_DOCKER=false TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        uv run pytest tests/integration/parity -m "integration or not integration" \
        --tb=short "${pytest_args[@]}"

    printf "${BOLD}━━━ Node (integration) ━━━${RESET}\n\n"
    # The Node suites default TYPEDB_ADDRESS to :1730; we pass it explicitly. test:integration
    # chains test:typed-integration, so this one command covers both Node integration suites.
    native="$(ls "$NODE_DIR"/type_bridge_node.*.node 2>/dev/null | head -1 || true)"
    run_step "npm run test:integration" \
        bash -c "cd '$NODE_DIR' && TYPE_BRIDGE_NODE_NATIVE_PATH='${native:+$PWD/$native}' \
            USE_DOCKER=false TYPEDB_ADDRESS='$TYPEDB_ADDRESS' TYPEDB_HTTP_PORT='$TYPEDB_HTTP_PORT' \
            npm run test:integration"
fi

# ── Proxy tier (opt-in; owns its own stack via proxy_lifecycle.py) ───────────
if [[ "$proxy" == 1 ]]; then
    printf "${BOLD}━━━ Python (proxy) ━━━${RESET}\n\n"
    if [[ "$isolated" == 1 ]]; then
        # Let proxy_lifecycle bring up docker-compose.proxy.yml (USE_DOCKER unset → true).
        run_step "pytest -m proxy" \
            uv run pytest -m proxy --tb=short "${pytest_args[@]}"
    else
        run_step "pytest -m proxy" \
            env USE_DOCKER=false uv run pytest -m proxy --tb=short "${pytest_args[@]}"
    fi
fi

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
