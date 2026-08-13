#!/usr/bin/env bash
# Full source-tree test suite — Rust + Python + Node + the internal C
# foundation, unit + integration.
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

  --no-integration  Run only the offline tiers (Rust, Python unit, Node unit/dts,
                    and the internal C foundation).
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

for runner_owned_workforce_variable in \
    TYPE_BRIDGE_WORKFORCE_REPORT \
    TYPE_BRIDGE_WORKFORCE_REPORT_V2 \
    TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT \
    TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS \
    TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE \
    TYPE_BRIDGE_WORKFORCE_V2_VALIDATED_OBSERVATIONS \
    TYPE_BRIDGE_WORKFORCE_V2_VALIDATOR_PYTHON; do
    if [[ ${!runner_owned_workforce_variable+x} == x ]]; then
        printf "${RED}%s is runner-owned; unset it before invoking test.sh.${RESET}\n" \
            "$runner_owned_workforce_variable" >&2
        exit 2
    fi
done
unset runner_owned_workforce_variable

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

validate_canonical_json_files() {
    local validator_python="$1"
    shift
    "$validator_python" -c '
import json
import sys
from pathlib import Path

for raw_path in sys.argv[1:]:
    path = Path(raw_path)
    raw = path.read_bytes()
    if not raw:
        raise SystemExit(f"empty JSON evidence: {path}")
    value = json.loads(raw)
    canonical = (
        json.dumps(
            value,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        + "\n"
    ).encode()
    if raw != canonical:
        raise SystemExit(f"noncanonical JSON evidence: {path}")
' "$@"
}

write_workforce_summary() {
    local summary="$1"
    local validator_python="$2"
    local staged
    shift 2
    if [[ "$summary" != /* || -e "$summary" ]]; then
        printf "${RED}Workforce summary must be an absent absolute path: %s${RESET}\n" \
            "$summary" >&2
        return 1
    fi
    staged="$(mktemp "${summary}.tmp.XXXXXX")" || return 1
    if ! "$@" > "$staged"; then
        unlink -- "$staged"
        return 1
    fi
    if ! validate_canonical_json_files "$validator_python" "$staged"; then
        unlink -- "$staged"
        return 1
    fi
    if ! ln -- "$staged" "$summary"; then
        unlink -- "$staged"
        return 1
    fi
    unlink -- "$staged"
}

log_workforce_evidence_sha256() {
    local validator_python="$1"
    local v2_summary="$2"
    shift 2
    validate_canonical_json_files "$validator_python" "$@"
    "$validator_python" -c '
import json
import sys
from pathlib import Path

summary = json.loads(Path(sys.argv[1]).read_bytes())
pending = summary.get("pending_manifest_promotions")
if not isinstance(pending, list):
    raise SystemExit("workforce-v2 summary has no pending_manifest_promotions list")
print(
    "workforce-v2 pending_manifest_promotions="
    + json.dumps(pending, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
)
' "$v2_summary"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -- "$@"
    else
        shasum -a 256 -- "$@"
    fi
}

# ── TypeDB container lifecycle (isolated integration only) ───────────────────
# One shared TypeDB serves the Rust, Python, and Node integration tiers — the CI shape,
# reproduced locally. The proxy tier (--proxy) owns its own stack via proxy_lifecycle.py.
compose=""
typedb_started=0
workforce_report_dir=""
preserve_workforce_evidence="${TYPE_BRIDGE_PRESERVE_WORKFORCE_EVIDENCE:-0}"
if [[ "$preserve_workforce_evidence" != 0 && "$preserve_workforce_evidence" != 1 ]]; then
    printf "${RED}TYPE_BRIDGE_PRESERVE_WORKFORCE_EVIDENCE must be 0 or 1.${RESET}\n" >&2
    exit 2
fi

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

    if [[ -z "$TYPEDB_PORT" || -z "$TYPEDB_HTTP_PORT" ]]; then
        printf "${RED}Could not discover TypeDB gRPC and HTTP host ports after up -d${RESET}\n" >&2
        exit 1
    fi

    printf "${BOLD}━━━ TypeDB (project %s, port %s) ━━━${RESET}\n\n" "$proj" "$TYPEDB_PORT"

    local typedb_ready=0
    for _ in {1..45}; do
        if timeout 2 bash -c "</dev/tcp/127.0.0.1/${TYPEDB_PORT}" 2>/dev/null \
            && timeout 3 python3 -c '
import sys
import urllib.request

with urllib.request.urlopen(
    f"http://127.0.0.1:{sys.argv[1]}/v1/version",
    timeout=2,
) as response:
    if not response.read(1):
        raise SystemExit(1)
' "$TYPEDB_HTTP_PORT" >/dev/null 2>&1; then
            typedb_ready=1
            break
        fi
        sleep 2
    done
    if [[ "$typedb_ready" != 1 ]]; then
        printf "${RED}TypeDB gRPC and HTTP endpoints did not become ready in time${RESET}\n" >&2
        exit 1
    fi
    printf "${GREEN}TypeDB ready on gRPC %s and HTTP %s${RESET}\n\n" \
        "$TYPEDB_PORT" "$TYPEDB_HTTP_PORT"

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

run_step "generated Rust projection acceptance on MSRV 1.88" \
    cargo +1.88.0 test --locked \
    --manifest-path type-bridge-core/Cargo.toml \
    -p type-bridge-schema-codegen --test rust_acceptance \
    generated_rust_crate_compiles_rejects_invalid_types_and_runs -- --exact

printf "${BOLD}━━━ C foundation (offline, internal) ━━━${RESET}\n\n"
run_step "generated C package and strict installed-compiler checks" \
    cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
    -p type-bridge-schema-codegen --test c_emitter
run_step "generated C entity and relation CRUD strict C17 consumer" \
    cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
    -p type-bridge-schema-codegen --test c_projection_live \
    exact_live_consumer_is_strict_c17_against_the_shared_generated_schema -- --exact
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
run_step "build the C ABI shared library" \
    cargo build --locked --manifest-path type-bridge-core/Cargo.toml \
    -p type-bridge-c --lib
run_step "C schema-package ABI and standalone consumer" \
    env TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER=1 \
    cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
    -p type-bridge-c --test schema_package_abi

printf "${BOLD}━━━ Python (unit) ━━━${RESET}\n\n"
run_step "pytest tests/unit/" \
    uv run pytest tests/unit/ --tb=short -q

printf "${BOLD}━━━ Node (build + offline) ━━━${RESET}\n\n"
run_step "npm ci"            bash -c "cd '$NODE_DIR' && npm ci"
run_step "npm run build"     bash -c "cd '$NODE_DIR' && npm run build"
run_step "ordered four-binding generated package compiler smoke" \
    cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
    -p type-bridge-schema-codegen --test ordered_collections \
    ordered_generated_packages_pass_all_four_language_compilers \
    -- --exact --ignored
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

    typedb_server_version="$(
        uv run python -c \
            'import sys; from type_bridge.typedb_driver import server_version; print(server_version(sys.argv[1], http_port=int(sys.argv[2])))' \
            "$TYPEDB_ADDRESS" "$TYPEDB_HTTP_PORT"
    )"
    printf "${CYAN}Detected TypeDB %s${RESET}\n\n" "$typedb_server_version"

    workforce_rust_env=()
    workforce_python_env=()
    workforce_node_env=()
    workforce_c_env=()
    # Forwarded pytest arguments may change collection and exclude the sole
    # Python report producer. Keep targeted Python runs useful instead of
    # allocating a fan-in that can never become complete; the unfiltered full
    # suite remains the local conformance gate.
    if [[ "$typedb_server_version" == "3.12.1" && ${#pytest_args[@]} -eq 0 ]]; then
        workforce_report_dir="$(
            mktemp -d "${TMPDIR:-/tmp}/typebridge-workforce.XXXXXXXXXX"
        )"
        workforce_report_dir="$(cd "$workforce_report_dir" && pwd -P)"
        mkdir -p "$workforce_report_dir/v2"
        workforce_validator_python="$(
            uv run python -c 'import os, sys; print(os.path.realpath(sys.executable))'
        )"
        if [[ "$workforce_validator_python" != /* \
            || ! -x "$workforce_validator_python" ]]; then
            printf "${RED}Workforce validator Python must be an absolute executable path: %s${RESET}\n" \
                "$workforce_validator_python" >&2
            exit 2
        fi
        workforce_python_nonce="$(
            "$workforce_validator_python" -c 'import secrets; print(secrets.token_hex(32))'
        )"
        workforce_node_nonce="$(
            "$workforce_validator_python" -c 'import secrets; print(secrets.token_hex(32))'
        )"
        workforce_rust_nonce="$(
            "$workforce_validator_python" -c 'import secrets; print(secrets.token_hex(32))'
        )"
        workforce_c_nonce="$(
            "$workforce_validator_python" -c 'import secrets; print(secrets.token_hex(32))'
        )"
        declare -A workforce_nonce_set=()
        for workforce_nonce in \
            "$workforce_python_nonce" \
            "$workforce_node_nonce" \
            "$workforce_rust_nonce" \
            "$workforce_c_nonce"; do
            if [[ ! "$workforce_nonce" =~ ^[0-9a-f]{64}$ \
                || ${workforce_nonce_set[$workforce_nonce]+x} == x ]]; then
                printf "${RED}Workforce-v2 binding nonces must be distinct lowercase 64-hex values.${RESET}\n" >&2
                exit 2
            fi
            workforce_nonce_set[$workforce_nonce]=1
        done
        unset workforce_nonce workforce_nonce_set

        workforce_python_direct_fragment="$workforce_report_dir/v2/python-direct-proof.json"
        workforce_python_remote_fragment="$workforce_report_dir/v2/python-remote-proof.json"
        workforce_node_direct_fragment="$workforce_report_dir/v2/node-direct-proof.json"
        workforce_node_remote_fragment="$workforce_report_dir/v2/node-remote-proof.json"
        workforce_rust_fragment="$workforce_report_dir/v2/rust-proof.json"
        workforce_c_fragment="$workforce_report_dir/v2/c-proof.json"
        workforce_v1_summary="$workforce_report_dir/summary-v1.json"
        workforce_v2_summary="$workforce_report_dir/summary-v2.json"
        workforce_rust_env=(
            "TYPE_BRIDGE_WORKFORCE_REPORT=$workforce_report_dir/rust.json"
            "TYPE_BRIDGE_WORKFORCE_REPORT_V2=$workforce_report_dir/v2/rust.json"
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS=$workforce_rust_fragment"
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE=$workforce_rust_nonce"
            "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE=typedb-3.12.1/v1"
        )
        workforce_python_env=(
            "TYPE_BRIDGE_WORKFORCE_REPORT=$workforce_report_dir/python.json"
            "TYPE_BRIDGE_WORKFORCE_REPORT_V2=$workforce_report_dir/v2/python.json"
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS=$workforce_python_direct_fragment:$workforce_python_remote_fragment"
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE=$workforce_python_nonce"
        )
        workforce_node_env=(
            "TYPE_BRIDGE_WORKFORCE_REPORT=$workforce_report_dir/node.json"
            "TYPE_BRIDGE_WORKFORCE_REPORT_V2=$workforce_report_dir/v2/node.json"
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS=$workforce_node_direct_fragment:$workforce_node_remote_fragment"
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE=$workforce_node_nonce"
            "TYPE_BRIDGE_WORKFORCE_V2_VALIDATOR_PYTHON=$workforce_validator_python"
            "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE=typedb-3.12.1/v1"
        )
        workforce_c_env=(
            "TYPE_BRIDGE_WORKFORCE_REPORT_V2=$workforce_report_dir/v2/c.json"
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS=$workforce_c_fragment"
            "TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE=$workforce_c_nonce"
        )
        printf "${CYAN}Workforce reports: %s${RESET}\n\n" "$workforce_report_dir"
    elif [[ "$typedb_server_version" == "3.12.1" ]]; then
        printf "${CYAN}Workforce report fan-in skipped because forwarded pytest arguments may change collection.${RESET}\n\n"
    fi

    printf "${BOLD}━━━ Rust (integration) ━━━${RESET}\n\n"
    run_step "cargo test -p type-bridge-orm --features integration-tests --test integration" \
        timeout --foreground 15m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-orm --features integration-tests --test integration -- \
            --nocapture --test-threads=1

    printf "${BOLD}━━━ Production V2 server (integration) ━━━${RESET}\n\n"
    run_step "type-bridge-server V1 + V2 live smoke" \
        timeout --foreground 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            production_binary_serves_v1_health_and_v2_query \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-server --features v2-query \
            --test v2_query_integration_tests

    printf "${BOLD}━━━ Generated Rust projection (integration) ━━━${RESET}\n\n"
    if [[ -n "$workforce_report_dir" ]]; then
        run_step "emit Rust workforce-v2 deterministic proof fragment" \
            env TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT="$workforce_rust_fragment" \
                TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE="$workforce_rust_nonce" \
            cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge --lib \
                remote::tests::workforce_v2_rust_deterministic_proof_fragment \
                -- --exact
    fi
    run_step "generated Rust application parity" \
        timeout --foreground 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE="type_bridge_rust_projection_live_${$}" \
            ACCEPTANCE_TARGET_DIR="$ROOT/type-bridge-core/target/tmp_projection_live_target" \
            "${workforce_rust_env[@]}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            generated_rust_projection_round_trips_exact_live_models \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-schema-codegen --test rust_projection_live

    if [[ "$typedb_server_version" == "3.12.1" ]]; then
        printf "${BOLD}━━━ C runtime transactions (integration) ━━━${RESET}\n\n"
        run_step "compiled C17 runtime and transaction lifecycle" \
            timeout --foreground 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER=1 \
                TYPE_BRIDGE_C_INTG_DATABASE="type_bridge_c_runtime_live_${$}" \
            bash scripts/ci/run_exact_ignored_rust_test.sh \
                live_c17_consumer_exercises_exact_3_12_1_transaction_lifecycle \
                --manifest-path type-bridge-core/Cargo.toml --locked \
                -p type-bridge-c --test schema_package_abi

        printf "${BOLD}━━━ Generated C entity and relation CRUD (integration) ━━━${RESET}\n\n"
        if [[ -n "$workforce_report_dir" ]]; then
            run_step "emit C workforce-v2 deterministic proof fragment" \
                env TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT="$workforce_c_fragment" \
                    TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE="$workforce_c_nonce" \
                cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
                    -p type-bridge-c --lib \
                    query::tests::workforce_v2_c_deterministic_proof_fragment \
                    -- --exact
        fi
        run_step "compiled generated C17 Person and Membership CRUD lifecycle" \
            timeout --foreground 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE="type_bridge_c_projection_live_${$}" \
                ACCEPTANCE_TARGET_DIR="$ROOT/type-bridge-core/target/tmp_c_projection_live_target" \
                "${workforce_c_env[@]}" \
            bash scripts/ci/run_exact_ignored_rust_test.sh \
                live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_1 \
                --manifest-path type-bridge-core/Cargo.toml --locked \
                -p type-bridge-schema-codegen --test c_projection_live
    else
        printf "${CYAN}Generated C entity and relation CRUD live smoke is intentionally limited to exact TypeDB 3.12.1; skipping %s.${RESET}\n\n" \
            "$typedb_server_version"
    fi

    printf "${BOLD}━━━ CLI workspace lifecycle (integration) ━━━${RESET}\n\n"
    for cli_live_test in \
        empty_workspace_to_replayed_history_live \
        documented_examples_initial_constraints_apply_and_verify_live \
        verify_never_creates_databases_live \
        adopt_legacy_history_then_evolve_live \
        shipped_python_converter_to_native_adoption_live; do
        run_step "cargo test -p type-bridge-cli --test e2e_workspace_live $cli_live_test" \
            timeout --foreground 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPE_BRIDGE_TEST_PYTHON="$ROOT/.venv/bin/python" \
            bash scripts/ci/run_exact_ignored_rust_test.sh "$cli_live_test" \
                --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-cli --test e2e_workspace_live
    done

    printf "${BOLD}━━━ Connected migration recovery (integration) ━━━${RESET}\n\n"
    run_step "connected rollback and reapply lifecycle" \
        timeout --foreground 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE="type_bridge_local_rollback_${$}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            runner_rolls_back_the_applied_head_and_reapplies_on_3_12_1 \
            --manifest-path type-bridge-core/Cargo.toml --locked \
            -p type-bridge-schema-migration-typedb --test live_runner
    run_step "interrupted-plan fenced recovery lifecycle" \
        timeout --foreground 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE="type_bridge_local_recovery_${$}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            control_schema_and_fenced_lease_round_trip_on_3_12_1 \
            --manifest-path type-bridge-core/Cargo.toml --locked \
            -p type-bridge-schema-migration-typedb --test live_store

    printf "${BOLD}━━━ Python (integration) ━━━${RESET}\n\n"
    if [[ -n "$workforce_report_dir" ]]; then
        run_step "emit Python direct-cancellation workforce-v2 proof fragment" \
            env TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT="$workforce_python_direct_fragment" \
                TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE="$workforce_python_nonce" \
            cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-core --lib \
                match_runtime::tests::python_direct_cancellation_fragment_is_measured_from_owned_execution \
                -- --exact
        run_step "emit Python generated-remote workforce-v2 proof fragment" \
            env TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT="$workforce_python_remote_fragment" \
                TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE="$workforce_python_nonce" \
            uv run python \
                type-bridge-core/crates/schema-codegen/tests/acceptance/check.py
    fi
    run_step "pytest -m integration" \
        timeout --foreground 20m \
        env USE_DOCKER=false TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            "${workforce_python_env[@]}" \
        uv run pytest -m integration --tb=short "${pytest_args[@]}"

    printf "${BOLD}━━━ Node (integration) ━━━${RESET}\n\n"
    # The Node suite defaults TYPEDB_ADDRESS to :1730; pass the live endpoint explicitly.
    native="$(ls "$NODE_DIR"/type_bridge_node.*.node 2>/dev/null | head -1 || true)"
    if [[ -n "$native" ]]; then
        native="$ROOT/$native"
    fi
    run_step "npm run test:integration" \
        timeout --foreground 15m \
        bash -c "cd '$NODE_DIR' && TYPE_BRIDGE_NODE_NATIVE_PATH='$native' \
            USE_DOCKER=false TYPEDB_ADDRESS='$TYPEDB_ADDRESS' TYPEDB_HTTP_PORT='$TYPEDB_HTTP_PORT' \
            TYPEDB_VERSION='$typedb_server_version' \
            npm run test:integration"
    if [[ -n "$workforce_report_dir" ]]; then
        run_step "emit Node direct-cancellation workforce-v2 proof fragment" \
            env TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT="$workforce_node_direct_fragment" \
                TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE="$workforce_node_nonce" \
            cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-node --lib \
                match_runtime::tests::node_direct_cancellation_fragment_is_measured_from_owned_execution \
                -- --exact
        run_step "emit Node generated-remote workforce-v2 proof fragment" \
            env TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT="$workforce_node_remote_fragment" \
                TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE="$workforce_node_nonce" \
            node type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/check.mjs
    fi
    run_step "npm run test:projection-integration" \
        timeout --foreground 15m \
        env TYPE_BRIDGE_NODE_NATIVE_PATH="$native" \
            USE_DOCKER=false TYPEDB_ADDRESS="$TYPEDB_ADDRESS" \
            TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPEDB_VERSION="$typedb_server_version" \
            TYPE_BRIDGE_NODE_INTG_DATABASE="type_bridge_projection_live_${$}" \
            "${workforce_node_env[@]}" \
        npm --prefix "$NODE_DIR" run test:projection-integration

    if [[ -n "$workforce_report_dir" ]]; then
        run_step "compare generated SDK workforce reports" \
            write_workforce_summary \
                "$workforce_v1_summary" \
                "$workforce_validator_python" \
                uv run python scripts/ci/compare_workforce_conformance.py \
                "$workforce_report_dir/python.json" \
                "$workforce_report_dir/node.json" \
                "$workforce_report_dir/rust.json"
        run_step "compare generated SDK workforce-v2 reports" \
            write_workforce_summary \
                "$workforce_v2_summary" \
                "$workforce_validator_python" \
                "$workforce_validator_python" \
                scripts/ci/compare_workforce_conformance_v2.py \
                "$workforce_report_dir/v2/python.json" \
                "$workforce_report_dir/v2/node.json" \
                "$workforce_report_dir/v2/rust.json" \
                "$workforce_report_dir/v2/c.json"
        run_step "validate and log workforce report and summary SHA-256 evidence" \
            log_workforce_evidence_sha256 \
                "$workforce_validator_python" \
                "$workforce_v2_summary" \
                "$workforce_report_dir/python.json" \
                "$workforce_report_dir/node.json" \
                "$workforce_report_dir/rust.json" \
                "$workforce_v1_summary" \
                "$workforce_report_dir/v2/python.json" \
                "$workforce_report_dir/v2/node.json" \
                "$workforce_report_dir/v2/rust.json" \
                "$workforce_report_dir/v2/c.json" \
                "$workforce_v2_summary"
    fi
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

        run_step "TLS generated Rust application parity" \
            timeout --foreground 10m \
            env TYPEDB_ADDRESS="$tls_address" \
                TYPEDB_HTTP_PORT="$tls_http_port" \
                TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
                SSL_CERT_FILE="$fixture_root_ca" \
                TYPE_BRIDGE_RUST_PROJECTION_TLS=1 \
                TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE="type_bridge_rust_projection_tls_${$}" \
                ACCEPTANCE_TARGET_DIR="$ROOT/type-bridge-core/target/tmp_projection_live_target" \
            bash scripts/ci/run_exact_ignored_rust_test.sh \
                generated_rust_projection_round_trips_exact_live_models \
                --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-schema-codegen --test rust_projection_live
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
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            tls_workspace_apply_and_verify_live \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-cli --test e2e_workspace_live

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

    run_step "TLS generated Python application parity" \
        timeout --foreground 10m \
        env USE_DOCKER=false \
            TYPEDB_ADDRESS="$tls_address" \
            TYPEDB_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            SMOKE_TLS_CERT="$fixture_server_cert" \
            SMOKE_TLS_KEY="$fixture_server_key" \
            SMOKE_TLS_ROOT_CA="$fixture_root_ca" \
        uv run pytest \
            tests/integration/schema/test_generated_projection_live.py::test_generated_package_preserves_application_operation_outcomes_live \
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

    run_step "TLS generated Node application parity" \
        timeout --foreground 10m \
        env TYPE_BRIDGE_NODE_NATIVE_PATH="$node_native" \
            TYPEDB_ADDRESS="$tls_address" \
            TYPEDB_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            SMOKE_TLS_CERT="$fixture_server_cert" \
            SMOKE_TLS_KEY="$fixture_server_key" \
            NODE_EXTRA_CA_CERTS="$fixture_root_ca" \
            TYPE_BRIDGE_NODE_INTG_DATABASE="type_bridge_projection_tls_${$}" \
        npm --prefix "$NODE_DIR" run test:projection-integration
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
if [[ -n "$workforce_report_dir" ]]; then
    if ((fail == 0)) && [[ "$preserve_workforce_evidence" == 0 ]]; then
        rm -f -- \
            "$workforce_report_dir/python.json" \
            "$workforce_report_dir/node.json" \
            "$workforce_report_dir/rust.json" \
            "$workforce_report_dir/v2/python.json" \
            "$workforce_report_dir/v2/node.json" \
            "$workforce_report_dir/v2/rust.json" \
            "$workforce_report_dir/v2/c.json" \
            "$workforce_report_dir/v2/python-direct-proof.json" \
            "$workforce_report_dir/v2/python-remote-proof.json" \
            "$workforce_report_dir/v2/node-direct-proof.json" \
            "$workforce_report_dir/v2/node-remote-proof.json" \
            "$workforce_report_dir/v2/rust-proof.json" \
            "$workforce_report_dir/v2/c-proof.json" \
            "$workforce_report_dir/summary-v1.json" \
            "$workforce_report_dir/summary-v2.json"
        if ! rmdir -- "$workforce_report_dir/v2"; then
            printf "${RED}Could not remove accepted workforce-v2 report directory: %s${RESET}\n\n" \
                "$workforce_report_dir/v2" >&2
            fail=$((fail + 1))
            failures+=("remove accepted workforce-v2 report directory")
        fi
        if rmdir -- "$workforce_report_dir"; then
            printf "${GREEN}Removed accepted workforce reports: %s${RESET}\n\n" \
                "$workforce_report_dir"
        else
            printf "${RED}Could not remove accepted workforce report directory: %s${RESET}\n\n" \
                "$workforce_report_dir" >&2
            fail=$((fail + 1))
            failures+=("remove accepted workforce report directory")
        fi
    elif ((fail > 0)); then
        printf "${CYAN}Preserved workforce reports for diagnosis: %s${RESET}\n\n" \
            "$workforce_report_dir"
    else
        printf "${CYAN}Preserved accepted workforce evidence by request: %s${RESET}\n\n" \
            "$workforce_report_dir"
    fi
fi

printf "${BOLD}━━━ Summary ━━━${RESET}\n"
printf "${GREEN}  ✓ %d passed${RESET}\n" "$pass"
if ((fail > 0)); then
    printf "${RED}  ✗ %d failed:${RESET}\n" "$fail"
    for f in "${failures[@]}"; do
        printf "${RED}    - %s${RESET}\n" "$f"
    done
    exit 1
fi
