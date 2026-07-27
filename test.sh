#!/usr/bin/env bash
# Full source-tree test suite — Rust + Python + Node, unit + integration.
#
# Reproduces the CI unit/integration tiers locally. Exact wheel and npm release
# artifact acceptance remains workflow-only; this script does not build or
# install publication artifacts. Behaviour is flag-controlled:
#
#   ./test.sh                  full source-tree test, isolated (default): brings up TypeDB,
#                              runs every tier, tears the container down on exit
#   ./test.sh --no-integration unit/offline tiers only (no TypeDB, no container)
#   ./test.sh --proxy          additionally run the -m proxy suite (proxy stack)
#   ./test.sh --tls            additionally run the isolated TLS transport lane
#   ./test.sh --no-isolated    use an already-running TypeDB instead of managing one
#
# Flags compose, e.g. `./test.sh --tls --no-isolated`. Args after `--` are forwarded to the
# pytest invocations (e.g. `./test.sh -- -k some_test`).
#
# Replaces the retired test-integration.sh / test-integration-dind.sh /
# test-proxy-integration.sh: the isolated default is the container-managed run, --proxy is
# the proxy suite, --no-isolated is the use-a-running-server path.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
BOLD='\033[1m'
RESET='\033[0m'

# ── Compose project helpers ───────────────────────────────────────────────────
# Derive a compose project name from a directory path.  The rule:
#   1. Take the basename of the path.
#   2. Lowercase it.
#   3. Replace every run of characters outside [a-z0-9] with a single '-'.
#   4. Strip any leading or trailing '-'.
# Prefixed with "tb-" so the name is always non-empty and human-recognisable.
# The Python counterpart in tests/utils/typedb_lifecycle.py implements the
# same rule byte-for-byte; the parity test pins that agreement.
compose_project_for() {
    local path="$1"
    local base
    base="$(basename "$path")"
    # lowercase
    base="${base,,}"
    # collapse runs of non-[a-z0-9] to '-'
    base="$(printf '%s' "$base" | sed 's/[^a-z0-9]\+/-/g')"
    # strip leading/trailing '-'
    base="${base#-}"
    base="${base%-}"
    printf 'tb-%s' "$base"
}

compose_project() {
    compose_project_for "$ROOT"
}

# ── Flags ────────────────────────────────────────────────────────────────────
integration=1
proxy=0
tls=0
isolated=1
pytest_args=()

usage() {
    cat <<'EOF'
Usage: ./test.sh [--no-integration] [--proxy] [--tls] [--no-isolated] [-- <pytest args>]

  --no-integration  Run only the offline tiers (Rust, Python unit, Node unit/dts).
  --proxy           Additionally run the proxy integration suite (-m proxy).
  --tls             Additionally run dedicated TLS transport tests. In isolated
                    mode this starts a test-only TLS endpoint in front of TypeDB.
  --no-isolated     Use an already-running TypeDB (USE_DOCKER=false) instead of
                    managing a container. Default is isolated: test.sh owns a TypeDB.

Override assigned ports with TYPEDB_PORT / TYPEDB_HTTP_PORT.  By default, isolated
mode lets the engine pick free ports per worktree; the derived project name is
tb-<worktree-basename>. For external `--no-isolated --tls`, set
TYPEDB_TLS_ADDRESS, TYPEDB_TLS_HTTP_PORT, and TYPEDB_TLS_ROOT_CA. CI alone uses
port 1729 with USE_DOCKER=false.
EOF
}

# Hidden early-exit for the parity unit test.  Not listed in usage().
if [[ "${1:-}" == "--print-project" ]]; then
    compose_project_for "${2:-$ROOT}"
    exit 0
fi

while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-integration) integration=0 ;;
        --proxy)          proxy=1 ;;
        --tls)            tls=1 ;;
        --no-isolated)    isolated=0 ;;
        --)               shift; pytest_args=("$@"); break ;;
        -h|--help)        usage; exit 0 ;;
        *) printf "${RED}unknown flag: %s${RESET}\n\n" "$1" >&2; usage; exit 1 ;;
    esac
    shift
done

NODE_DIR=type-bridge-core/crates/node
# TYPEDB_PORT and TYPEDB_HTTP_PORT are intentionally NOT defaulted here.
# Isolated mode discovers the engine-assigned port after 'up -d' and sets them.
# Explicit caller-provided values are preserved (see start_typedb).
# Not exported: TYPEDB_ADDRESS is passed inline only to the integration tiers. Exporting it
# globally leaks into the offline `cargo test` tier, where the server config test honours a
# TYPEDB_ADDRESS override and fails when it disagrees with the config-file value.
TYPEDB_ADDRESS="${TYPEDB_ADDRESS:-}"

# TLS live inputs are captured and removed from the ambient environment. This
# keeps every ordinary/offline tier on the plaintext matrix; only the dedicated
# TLS step receives them explicitly. TYPEDB_TLS_PORT is an isolated compose
# port override, while the other three names are the public live-test contract.
CALLER_TYPEDB_TLS_ADDRESS="${TYPEDB_TLS_ADDRESS:-}"
CALLER_TYPEDB_TLS_HTTP_PORT="${TYPEDB_TLS_HTTP_PORT:-}"
CALLER_TYPEDB_TLS_ROOT_CA="${TYPEDB_TLS_ROOT_CA:-}"
CALLER_TYPEDB_TLS_PORT="${TYPEDB_TLS_PORT:-}"
unset TYPEDB_TLS_ADDRESS TYPEDB_TLS_HTTP_PORT TYPEDB_TLS_ROOT_CA TYPEDB_TLS_PORT

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
# One shared TypeDB serves the Rust, Python, and Node integration tiers — the CI shape,
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
    local proj
    local services=(typedb)
    local typedb_image="${TYPEDB_IMAGE:-typedb/typedb:3.12.1}"
    proj="$(compose_project)"
    if [[ "$tls" == 1 ]]; then
        services+=(typedb-tls)
        # Workspace migration execution is pinned to the shipped semantic
        # profile's exact server, while the ordinary lane retains its prior
        # image. An explicit caller override remains authoritative.
        typedb_image="${TYPEDB_IMAGE:-typedb/typedb:3.12.1}"
    fi

    printf "${BOLD}━━━ TypeDB (isolated, project %s) ━━━${RESET}\n\n" "$proj"
    env \
        TYPEDB_IMAGE="$typedb_image" \
        TYPEDB_TLS_PORT="${CALLER_TYPEDB_TLS_PORT:-0}" \
        TYPEDB_TLS_HTTP_PORT="${CALLER_TYPEDB_TLS_HTTP_PORT:-0}" \
        $compose -f docker-compose.yml -p "$proj" up -d "${services[@]}"
    typedb_started=1

    # Discover the engine-assigned host ports for the two TypeDB container ports.
    # 'compose port' can return empty immediately after 'up -d' while the port
    # mapping propagates, so we retry up to 3 times with a 1-second gap.
    # Docker may print one line per address family (IPv4 + IPv6); taking the last
    # line is deterministic because the IPv6 line always follows the IPv4 line when
    # both appear, and the port number is the same on both — last line always works.
    _discover_port() {
        local service="$1"
        local container_port="$2"
        local out=""
        for _ in {1..3}; do
            out="$($compose -f docker-compose.yml -p "$proj" port "$service" "$container_port" 2>/dev/null || true)"
            [[ -n "$out" ]] && break
            sleep 1
        done
        printf '%s' "$out" | tail -1 | sed 's/.*://'
    }

    # Only override when the caller did NOT explicitly set the port.
    if [[ -z "${TYPEDB_PORT:-}" ]]; then
        TYPEDB_PORT="$(_discover_port typedb 1729)"
    fi
    if [[ -z "${TYPEDB_HTTP_PORT:-}" ]]; then
        TYPEDB_HTTP_PORT="$(_discover_port typedb 8000)"
    fi

    if [[ -z "$TYPEDB_PORT" ]]; then
        printf "${RED}Could not discover TypeDB host port after up -d${RESET}\n" >&2
        exit 1
    fi

    printf "${BOLD}━━━ TypeDB (project %s, port %s) ━━━${RESET}\n\n" "$proj" "$TYPEDB_PORT"

    local typedb_ready=0
    for _ in {1..45}; do
        if timeout 2 bash -c "</dev/tcp/127.0.0.1/${TYPEDB_PORT}" 2>/dev/null; then
            typedb_ready=1
            break
        fi
        sleep 2
    done
    if [[ "$typedb_ready" != 1 ]]; then
        printf "${RED}TypeDB did not open port ${TYPEDB_PORT} in time${RESET}\n" >&2
        exit 1
    fi
    printf "${GREEN}TypeDB ready on ${TYPEDB_PORT}${RESET}\n\n"

    if [[ "$tls" == 1 ]]; then
        TYPEDB_TLS_PORT="${CALLER_TYPEDB_TLS_PORT:-$(_discover_port typedb-tls 1729)}"
        TYPEDB_TLS_HTTP_PORT="${CALLER_TYPEDB_TLS_HTTP_PORT:-$(_discover_port typedb-tls 8000)}"
        if [[ -z "$TYPEDB_TLS_PORT" || -z "$TYPEDB_TLS_HTTP_PORT" ]]; then
            printf "${RED}Could not discover isolated TLS endpoint ports${RESET}\n" >&2
            exit 1
        fi
        TYPEDB_TLS_ADDRESS="${CALLER_TYPEDB_TLS_ADDRESS:-127.0.0.1:${TYPEDB_TLS_PORT}}"
        TYPEDB_TLS_ROOT_CA="${CALLER_TYPEDB_TLS_ROOT_CA:-$ROOT/tests/fixtures/tls/root-ca.pem}"

        if ! command -v openssl >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1; then
            printf "${RED}The isolated TLS lane requires openssl and curl.${RESET}\n" >&2
            exit 1
        fi

        local tls_ready=0
        local fixture_root_ca="$ROOT/tests/fixtures/tls/root-ca.pem"
        for _ in {1..45}; do
            if timeout 3 openssl s_client \
                -connect "127.0.0.1:${TYPEDB_TLS_PORT}" \
                -servername localhost \
                -verify_hostname localhost \
                -verify_return_error \
                -CAfile "$fixture_root_ca" \
                -alpn h2 </dev/null >/dev/null 2>&1 \
                && curl --fail --silent --max-time 3 \
                    --cacert "$fixture_root_ca" \
                    --resolve "localhost:${TYPEDB_TLS_HTTP_PORT}:127.0.0.1" \
                    "https://localhost:${TYPEDB_TLS_HTTP_PORT}/v1/version" \
                    >/dev/null 2>&1; then
                tls_ready=1
                break
            fi
            sleep 2
        done
        if [[ "$tls_ready" != 1 ]]; then
            printf "${RED}Isolated TLS endpoints did not become ready in time${RESET}\n" >&2
            $compose -f docker-compose.yml -p "$proj" logs typedb-tls >&2 || true
            exit 1
        fi
        printf "${GREEN}TypeDB TLS ready on gRPC %s and HTTP %s${RESET}\n\n" \
            "$TYPEDB_TLS_PORT" "$TYPEDB_TLS_HTTP_PORT"
    fi
}

stop_typedb() {
    if [[ "$typedb_started" == 1 ]]; then
        local proj
        proj="$(compose_project)"
        printf "\n${BOLD}━━━ Tearing down TypeDB ━━━${RESET}\n"
        $compose -f docker-compose.yml -p "$proj" down -v || true
    fi
}
trap stop_typedb EXIT

# ── Offline tiers (always) ───────────────────────────────────────────────────
printf "${BOLD}━━━ Rust ━━━${RESET}\n\n"
export PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1
run_step "cargo test --all-targets" \
    cargo test --manifest-path type-bridge-core/Cargo.toml --all-targets

run_step "released validation-rule wire without feature unification" \
    env CARGO_TARGET_DIR=type-bridge-core/target/rule-wire-standalone \
    cargo test --locked \
    --manifest-path type-bridge-core/crates/core/tests/fixtures/rule-wire-standalone/Cargo.toml

printf "${BOLD}━━━ Python (unit) ━━━${RESET}\n\n"
run_step "pytest tests/unit/" \
    uv run pytest tests/unit/ --tb=short -q

printf "${BOLD}━━━ Node (build + offline) ━━━${RESET}\n\n"
run_step "npm ci"            bash -c "cd '$NODE_DIR' && npm ci"
run_step "npm run build"     bash -c "cd '$NODE_DIR' && npm run build"
run_step "npm run scope:probe" bash -c "cd '$NODE_DIR' && npm run scope:probe"
run_step "npm run test:unit" bash -c "cd '$NODE_DIR' && npm run test:unit"
run_step "npm run test:dts"  bash -c "cd '$NODE_DIR' && npm run test:dts"
run_step "npm run dts:parity" bash -c "cd '$NODE_DIR' && npm run dts:parity"

# ── Integration tiers ────────────────────────────────────────────────────────
if [[ "$integration" == 1 ]]; then
    [[ "$isolated" == 1 ]] && start_typedb

    # After start_typedb, TYPEDB_PORT is either caller-provided or discovered.
    # For --no-isolated, fall back to the conventional default.
    TYPEDB_PORT="${TYPEDB_PORT:-1730}"
    TYPEDB_HTTP_PORT="${TYPEDB_HTTP_PORT:-8000}"
    TYPEDB_ADDRESS="${TYPEDB_ADDRESS:-localhost:${TYPEDB_PORT}}"

    printf "${BOLD}━━━ Rust (integration) ━━━${RESET}\n\n"
    run_step "cargo test -p type-bridge-orm --features integration-tests --test integration" \
        timeout --foreground 15m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-orm --features integration-tests --test integration -- --nocapture

    printf "${BOLD}━━━ Production V2 server (integration) ━━━${RESET}\n\n"
    run_step "type-bridge-server V1 + V2 live smoke" \
        timeout --foreground 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-server --features v2-query \
            --test v2_query_integration_tests \
            production_binary_serves_v1_health_and_v2_query \
            -- --ignored --exact --nocapture

    printf "${BOLD}━━━ CLI workspace lifecycle (integration) ━━━${RESET}\n\n"
    for cli_live_test in \
        empty_workspace_to_replayed_history_live \
        verify_never_creates_databases_live \
        adopt_legacy_history_then_evolve_live \
        shipped_python_converter_to_native_adoption_live; do
        run_step "cargo test -p type-bridge-cli --test e2e_workspace_live $cli_live_test" \
            timeout --foreground 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPE_BRIDGE_TEST_PYTHON="$ROOT/.venv/bin/python" \
            cargo test --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-cli --test e2e_workspace_live \
                "$cli_live_test" -- --ignored --exact --nocapture
    done

    printf "${BOLD}━━━ Python (integration) ━━━${RESET}\n\n"
    run_step "pytest -m integration" \
        timeout --foreground 20m \
        env USE_DOCKER=false TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        uv run pytest -m integration --tb=short "${pytest_args[@]}"

    # The parity suite mixes live-TypeDB tests with deliberately unmarked
    # offline ones (descriptor snapshots, generator parity); the marker
    # override mirrors CI's cross-language-parity job so the offline tests
    # don't fall through both the unit and `-m integration` selections.
    printf "${BOLD}━━━ Python (cross-language parity) ━━━${RESET}\n\n"
    run_step "pytest tests/integration/parity" \
        timeout --foreground 10m \
        env USE_DOCKER=false TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        uv run pytest tests/integration/parity -m "integration or not integration" \
        --tb=short "${pytest_args[@]}"

    printf "${BOLD}━━━ Node (integration) ━━━${RESET}\n\n"
    # The Node suites default TYPEDB_ADDRESS to :1730; we pass it explicitly. test:integration
    # chains test:typed-integration, so this one command covers both Node integration suites.
    native="$(ls "$NODE_DIR"/type_bridge_node.*.node 2>/dev/null | head -1 || true)"
    run_step "npm run test:integration" \
        timeout --foreground 15m \
        bash -c "cd '$NODE_DIR' && TYPE_BRIDGE_NODE_NATIVE_PATH='${native:+$PWD/$native}' \
            USE_DOCKER=false TYPEDB_ADDRESS='$TYPEDB_ADDRESS' TYPEDB_HTTP_PORT='$TYPEDB_HTTP_PORT' \
            npm run test:integration"
fi

# ── TLS transport tier (opt-in) ──────────────────────────────────────────────
# The ordinary integration matrix above remains plaintext. Live TLS variables
# are passed only to these dedicated tests, never exported process-wide.
run_tls_transport_steps() {
    local tls_address="$1"
    local tls_http_port="$2"
    local tls_root_ca="$3"
    local required_topology="${4:-0}"
    local fixture_root_ca="$ROOT/tests/fixtures/tls/root-ca.pem"
    local fixture_server_cert="$ROOT/tests/fixtures/tls/server-cert.pem"
    local fixture_server_key="$ROOT/tests/fixtures/tls/server-key.pem"
    local node_native
    node_native="$(ls "$NODE_DIR"/type_bridge_node.*.node 2>/dev/null | head -1 || true)"
    if [[ -n "$node_native" ]]; then
        node_native="$ROOT/$node_native"
    fi

    if [[ "$required_topology" == 1 ]]; then
        run_step "TLS runtime HTTP + gRPC lifecycle" \
            timeout --foreground 10m \
            env TYPEDB_TLS_ADDRESS="$tls_address" \
                TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
                TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
                SSL_CERT_FILE="$fixture_root_ca" \
                TYPE_BRIDGE_TLS_LIVE_REQUIRED=1 \
                TYPE_BRIDGE_TLS_NATIVE_ROOTS=1 \
                TYPE_BRIDGE_TLS_EXPECTED_SERVER_VERSION=3.12.1 \
                TYPE_BRIDGE_TLS_EXPECTED_DRIVER_BAND=9 \
                TYPE_BRIDGE_TLS_EXPECTED_DRIVER_VERSION=3.12.1 \
            cargo test --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-typedb-runtime --test tls_live \
                -- --nocapture --test-threads=1
    else
        printf "${CYAN}External TLS runtime proof is custom-root only; native-root and exact-topology assertions require the isolated 3.12.1 lane.${RESET}\n\n"
        run_step "TLS runtime HTTP + gRPC lifecycle (external custom-root)" \
            timeout --foreground 10m \
            env TYPEDB_TLS_ADDRESS="$tls_address" \
                TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
                TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            cargo test --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-typedb-runtime --test tls_live \
                -- --nocapture --test-threads=1
    fi

    run_step "TLS CLI workspace migration apply + verify" \
        timeout --foreground 10m \
        env TYPEDB_TLS_ADDRESS="$tls_address" \
            TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-cli --test e2e_workspace_live \
            tls_workspace_apply_and_verify_live -- --ignored --exact --nocapture

    run_step "TLS Python local query + HTTPS remote envelope" \
        timeout --foreground 10m \
        env TYPEDB_TLS_ADDRESS="$tls_address" \
            TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            SMOKE_TLS_CERT="$fixture_server_cert" \
            SMOKE_TLS_KEY="$fixture_server_key" \
            SMOKE_TLS_ROOT_CA="$fixture_root_ca" \
        uv run pytest \
            tests/integration/queries/test_query_v2_binding_smoke.py::test_prepared_plan_executes_locally_and_remotely \
            -m integration --tb=short -q

    run_step "TLS Python and packed Node remote model parity" \
        timeout --foreground 10m \
        env TYPEDB_TLS_ADDRESS="$tls_address" \
            TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            SMOKE_TLS_CERT="$fixture_server_cert" \
            SMOKE_TLS_KEY="$fixture_server_key" \
            SMOKE_TLS_ROOT_CA="$fixture_root_ca" \
        uv run pytest \
            tests/integration/queries/test_remote_query_session_parity.py::test_public_remote_query_session_matches_direct_subtype_hydration \
            -m integration --tb=short -q

    run_step "TLS Node local query + HTTPS remote envelope" \
        timeout --foreground 10m \
        env TYPE_BRIDGE_NODE_NATIVE_PATH="$node_native" \
            TYPEDB_TLS_ADDRESS="$tls_address" \
            TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            SMOKE_TLS_CERT="$fixture_server_cert" \
            SMOKE_TLS_KEY="$fixture_server_key" \
            NODE_EXTRA_CA_CERTS="$fixture_root_ca" \
        node --test \
            "$NODE_DIR/tests/integration/queries/query-v2-smoke.test.ts"

    run_step "TLS Node remote model subtype hydration" \
        timeout --foreground 10m \
        env TYPE_BRIDGE_NODE_NATIVE_PATH="$node_native" \
            TYPEDB_TLS_ADDRESS="$tls_address" \
            TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            SMOKE_TLS_CERT="$fixture_server_cert" \
            SMOKE_TLS_KEY="$fixture_server_key" \
            NODE_EXTRA_CA_CERTS="$fixture_root_ca" \
        node --test \
            "$NODE_DIR/tests/integration/queries/typed-remote-query-parity.test.ts"
}

if [[ "$tls" == 1 ]]; then
    printf "${BOLD}━━━ TLS transport (opt-in) ━━━${RESET}\n\n"
    if [[ "$integration" != 1 ]]; then
        printf "${CYAN}Skipping TLS transport: --no-integration disables live tiers.${RESET}\n\n"
    elif [[ "$isolated" != 1 ]]; then
        if [[ -z "$CALLER_TYPEDB_TLS_ADDRESS" \
            || -z "$CALLER_TYPEDB_TLS_HTTP_PORT" \
            || -z "$CALLER_TYPEDB_TLS_ROOT_CA" ]]; then
            printf "${CYAN}Skipping external TLS transport: set TYPEDB_TLS_ADDRESS, TYPEDB_TLS_HTTP_PORT, and TYPEDB_TLS_ROOT_CA.${RESET}\n\n"
        else
            run_tls_transport_steps \
                "$CALLER_TYPEDB_TLS_ADDRESS" \
                "$CALLER_TYPEDB_TLS_HTTP_PORT" \
                "$CALLER_TYPEDB_TLS_ROOT_CA" \
                0
        fi
    else
        run_tls_transport_steps \
            "$TYPEDB_TLS_ADDRESS" \
            "$TYPEDB_TLS_HTTP_PORT" \
            "$TYPEDB_TLS_ROOT_CA" \
            1
    fi
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
