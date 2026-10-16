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
#   ./test.sh --typedb all     run the live tier on every supported TypeDB line, then the
#                              two-server profile-upgrade lane
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
typedb_request=""
pytest_args=()

usage() {
    cat <<'EOF'
Usage: ./test.sh [--no-integration] [--proxy] [--tls] [--no-isolated]
                 [--typedb <3.11.5|3.12.3|3.13.6|all>] [-- <pytest args>]

  --no-integration  Run only the offline tiers (Rust, Python unit, Node unit/dts,
                    and the internal C foundation).
  --proxy           Additionally run the proxy integration suite (-m proxy).
  --tls             Additionally run dedicated TLS transport tests. In isolated
                    mode this starts a test-only TLS endpoint in front of TypeDB.
  --no-isolated     Use an already-running TypeDB (USE_DOCKER=false) instead of
                    managing a container. Default is isolated: test.sh owns a TypeDB.
  --typedb LINE     Run the live tier on one supported TypeDB line (default 3.13.6),
                    or on each line with `all`; `all` then runs the two-server
                    3.12.3 -> 3.13.6 profile-upgrade lane. The offline tiers run once.
                    Each line runs on its own isolated server and semantic profile.

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
        --typedb)
            if [[ $# -lt 2 ]]; then
                printf "${RED}--typedb needs a TypeDB line or 'all'${RESET}\n\n" >&2
                usage
                exit 1
            fi
            shift
            typedb_request="$1" ;;
        --typedb=*)       typedb_request="${1#--typedb=}" ;;
        --)               shift; pytest_args=("$@"); break ;;
        -h|--help)        usage; exit 0 ;;
        *) printf "${RED}unknown flag: %s${RESET}\n\n" "$1" >&2; usage; exit 1 ;;
    esac
    shift
done

for runner_owned_sdk_variable in \
    TYPE_BRIDGE_SDK_REPORT \
    TYPE_BRIDGE_SDK_QUERIES_REPORT \
    TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT \
    TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS \
    TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE \
    TYPE_BRIDGE_SDK_QUERIES_VALIDATED_OBSERVATIONS \
    TYPE_BRIDGE_SDK_QUERIES_VALIDATOR_PYTHON \
    TYPE_BRIDGE_PROJECTED_LIVE_REPORT \
    TYPE_BRIDGE_PROJECTED_LIVE_DATABASE \
    TYPE_BRIDGE_PROJECTED_PYTHON_PACKAGE_ROOT \
    TYPE_BRIDGE_PROJECTED_NODE_PACKAGE_ROOT \
    TYPE_BRIDGE_PROJECTED_REPOSITORY_ROOT \
    ACCEPTANCE_TARGET_DIR; do
    if [[ ${!runner_owned_sdk_variable+x} == x ]]; then
        printf "${RED}%s is runner-owned; unset it before invoking test.sh.${RESET}\n" \
            "$runner_owned_sdk_variable" >&2
        exit 2
    fi
done
unset runner_owned_sdk_variable

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

# Caller pins for the managed server and its lane, captured once so each lane
# starts from them instead of from the previous lane's discovered values.
CALLER_TYPEDB_IMAGE="${TYPEDB_IMAGE:-}"
CALLER_TYPEDB_PORT="${TYPEDB_PORT:-}"
CALLER_TYPEDB_HTTP_PORT="${TYPEDB_HTTP_PORT:-}"
CALLER_TYPEDB_ADDRESS="$TYPEDB_ADDRESS"
CALLER_ACCEPTANCE_PROFILE="${TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE:-}"

# ── TypeDB lanes ─────────────────────────────────────────────────────────────
# Each supported TypeDB line and the semantic profile its live tier runs under.
# 3.13.6 is the default line.
TYPEDB_LANES=(3.11.5 3.12.3 3.13.6)
declare -A LANE_PROFILES=(
    [3.11.5]=typedb-3.11.5/v1
    [3.12.3]=typedb-3.12.1/v1
    [3.13.6]=typedb-3.13.6/v1
)
default_lane="3.13.6"

# ── Step runner (mirrors scripts/check.sh) ───────────────────────────────────
pass=0
fail=0
failures=()

# Keep test binaries and compiler descendants inside the same deadline.
run_with_timeout() {
    timeout --kill-after=5s "$@"
}

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
        failures+=("${lane_label:+[$lane_label] }$name")
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

write_sdk_summary() {
    local summary="$1"
    local validator_python="$2"
    local staged
    shift 2
    if [[ "$summary" != /* || -e "$summary" ]]; then
        printf "${RED}Sdk summary must be an absent absolute path: %s${RESET}\n" \
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

log_sdk_evidence_sha256() {
    local validator_python="$1"
    local summary="${!#}"
    shift
    validate_canonical_json_files "$validator_python" "$@" || return
    if ! "$validator_python" -c '
import json
import sys
from pathlib import Path

summary = json.loads(Path(sys.argv[1]).read_bytes())
pending = summary.get("pending_manifest_promotions")
if not isinstance(pending, list):
    raise SystemExit("queries summary has no pending_manifest_promotions list")
print(
    "queries pending_manifest_promotions="
    + json.dumps(pending, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
)
' "$summary"; then
        return 1
    fi
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
started_projects=()
sdk_report_dir=""
lane_label=""
lane_fail_start=0
lane_summaries=()
preserve_sdk_evidence="${TYPE_BRIDGE_PRESERVE_SDK_EVIDENCE:-0}"
if [[ "$preserve_sdk_evidence" != 0 && "$preserve_sdk_evidence" != 1 ]]; then
    printf "${RED}TYPE_BRIDGE_PRESERVE_SDK_EVIDENCE must be 0 or 1.${RESET}\n" >&2
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

# Print the exact TypeDB server a semantic profile pairs with, from the SDK
# conformance version matrix. Fails closed on a profile outside the matrix.
profile_server_version() {
    python3 -I -c 'import sys; sys.path.insert(0, sys.argv[1]); import sdk_profile_matrix as m; print(m.select_lane(sys.argv[2]).live_server_version)' \
        "$ROOT/scripts/ci" "$1"
}

# Resolve the lane a semantic profile names: its line in the lane table, else
# the server the SDK profile matrix pairs it with.
lane_for_profile() {
    local server
    for server in "${TYPEDB_LANES[@]}"; do
        if [[ "${LANE_PROFILES[$server]}" == "$1" ]]; then
            printf '%s' "$server"
            return 0
        fi
    done
    profile_server_version "$1"
}

# The compose project of one managed server. A single lane keeps the worktree
# project; multi-server runs suffix it so each server is its own project.
lane_compose_project() {
    local suffix="$1"
    if [[ -z "$suffix" ]]; then
        compose_project
    else
        printf '%s-%s' "$(compose_project)" "${suffix//./-}"
    fi
}

# Discover an engine-assigned host port. 'compose port' can return empty
# immediately after 'up -d' while the port mapping propagates, so we retry up
# to 3 times with a 1-second gap. Docker may print one line per address family
# (IPv4 + IPv6); the port number is the same on both, so the last line works.
discover_port() {
    local proj="$1"
    local service="$2"
    local container_port="$3"
    local out=""
    for _ in {1..3}; do
        out="$($compose -f docker-compose.yml -p "$proj" port "$service" "$container_port" 2>/dev/null || true)"
        [[ -n "$out" ]] && break
        sleep 1
    done
    printf '%s' "$out" | tail -1 | sed 's/.*://'
}

# start_typedb <server> <project> <with-tls>: start one isolated TypeDB of that
# line as its own compose project, wait until it serves gRPC and HTTP, and set
# started_port / started_http_port (and the TLS endpoint when <with-tls> is 1).
# An explicit caller image or port remains authoritative.
start_typedb() {
    local lane_server="$1"
    local proj="$2"
    local with_tls="$3"
    local services=(typedb)
    local typedb_image="${CALLER_TYPEDB_IMAGE:-typedb/typedb:${lane_server}}"
    if [[ "$with_tls" == 1 ]]; then
        services+=(typedb-tls)
    fi

    printf "${BOLD}━━━ TypeDB %s (isolated, project %s) ━━━${RESET}\n\n" "$lane_server" "$proj"
    started_projects+=("$proj")
    env \
        TYPEDB_IMAGE="$typedb_image" \
        TYPEDB_PORT="${CALLER_TYPEDB_PORT:-0}" \
        TYPEDB_HTTP_PORT="${CALLER_TYPEDB_HTTP_PORT:-0}" \
        TYPEDB_TLS_PORT="${CALLER_TYPEDB_TLS_PORT:-0}" \
        TYPEDB_TLS_HTTP_PORT="${CALLER_TYPEDB_TLS_HTTP_PORT:-0}" \
        $compose -f docker-compose.yml -p "$proj" up -d "${services[@]}"

    started_port="${CALLER_TYPEDB_PORT:-$(discover_port "$proj" typedb 1729)}"
    started_http_port="${CALLER_TYPEDB_HTTP_PORT:-$(discover_port "$proj" typedb 8000)}"
    if [[ -z "$started_port" || -z "$started_http_port" ]]; then
        printf "${RED}Could not discover TypeDB gRPC and HTTP host ports after up -d${RESET}\n" >&2
        exit 1
    fi

    printf "${BOLD}━━━ TypeDB (project %s, port %s) ━━━${RESET}\n\n" "$proj" "$started_port"

    local typedb_ready=0
    for _ in {1..45}; do
        if timeout 2 bash -c "</dev/tcp/127.0.0.1/${started_port}" 2>/dev/null \
            && timeout 3 python3 -c '
import sys
import urllib.request

with urllib.request.urlopen(
    f"http://127.0.0.1:{sys.argv[1]}/v1/version",
    timeout=2,
) as response:
    if not response.read(1):
        raise SystemExit(1)
' "$started_http_port" >/dev/null 2>&1; then
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
        "$started_port" "$started_http_port"

    if [[ "$with_tls" == 1 ]]; then
        TYPEDB_TLS_PORT="${CALLER_TYPEDB_TLS_PORT:-$(discover_port "$proj" typedb-tls 1729)}"
        TYPEDB_TLS_HTTP_PORT="${CALLER_TYPEDB_TLS_HTTP_PORT:-$(discover_port "$proj" typedb-tls 8000)}"
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
    local proj="$1"
    local remaining=()
    local started
    printf "\n${BOLD}━━━ Tearing down TypeDB (project %s) ━━━${RESET}\n" "$proj"
    $compose -f docker-compose.yml -p "$proj" down -v || true
    for started in "${started_projects[@]}"; do
        [[ "$started" != "$proj" ]] && remaining+=("$started")
    done
    started_projects=("${remaining[@]}")
}

# Tear down every server still up, also when a lane exits early.
stop_all_typedb() {
    local proj
    for proj in "${started_projects[@]}"; do
        stop_typedb "$proj"
    done
}
trap stop_all_typedb EXIT

# Resolve the lanes this run covers.
typedb_lanes=()
if [[ -n "$typedb_request" ]]; then
    if [[ "$typedb_request" == all ]]; then
        typedb_lanes=("${TYPEDB_LANES[@]}")
    elif [[ ${LANE_PROFILES[$typedb_request]+x} == x ]]; then
        typedb_lanes=("$typedb_request")
    else
        printf "${RED}--typedb must be one of %s or all, got %s${RESET}\n\n" \
            "${TYPEDB_LANES[*]}" "$typedb_request" >&2
        usage
        exit 1
    fi
    if [[ "$isolated" != 1 ]]; then
        printf "${RED}--typedb selects the managed server; with --no-isolated the running server is the lane.${RESET}\n" >&2
        exit 2
    fi
    # The lane fixes its image and profile; a caller pin would contradict it.
    # Under `all`, pinned ports or addresses would also be shared by servers.
    lane_pins=()
    [[ -n "$CALLER_TYPEDB_IMAGE" ]] && lane_pins+=(TYPEDB_IMAGE)
    [[ -n "$CALLER_ACCEPTANCE_PROFILE" ]] && lane_pins+=(TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE)
    [[ -n "${TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE:-}" ]] && lane_pins+=(TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE)
    if ((${#typedb_lanes[@]} > 1)); then
        [[ -n "$CALLER_TYPEDB_PORT" ]] && lane_pins+=(TYPEDB_PORT)
        [[ -n "$CALLER_TYPEDB_HTTP_PORT" ]] && lane_pins+=(TYPEDB_HTTP_PORT)
        [[ -n "$CALLER_TYPEDB_ADDRESS" ]] && lane_pins+=(TYPEDB_ADDRESS)
        [[ -n "$CALLER_TYPEDB_TLS_PORT" ]] && lane_pins+=(TYPEDB_TLS_PORT)
        [[ -n "$CALLER_TYPEDB_TLS_HTTP_PORT" ]] && lane_pins+=(TYPEDB_TLS_HTTP_PORT)
        [[ -n "$CALLER_TYPEDB_TLS_ADDRESS" ]] && lane_pins+=(TYPEDB_TLS_ADDRESS)
    fi
    if ((${#lane_pins[@]} > 0)); then
        printf "${RED}%s pin one server or profile; unset them with --typedb %s.${RESET}\n" \
            "${lane_pins[*]}" "$typedb_request" >&2
        exit 2
    fi
    unset lane_pins
elif [[ "$isolated" == 1 && -n "$CALLER_ACCEPTANCE_PROFILE" ]]; then
    # Without --typedb, a requested semantic profile still picks its line.
    profile_lane="$(lane_for_profile "$CALLER_ACCEPTANCE_PROFILE")"
    typedb_lanes=("$profile_lane")
    unset profile_lane
elif [[ "$isolated" == 1 ]]; then
    typedb_lanes=("$default_lane")
else
    # --no-isolated: the running server decides the lane after detection.
    typedb_lanes=("")
fi

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
run_step "C typed-query, diagnostic, function, remote ABI, and parity corpus" \
    cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
    -p type-bridge-c \
    --test query_abi \
    --test query_diagnostic_abi \
    --test query_function_abi \
    --test query_remote_abi \
    --test typed_query_corpus
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

# ── Integration tiers (one run per TypeDB lane) ──────────────────────────────
# Connected migrations need a migration profile, which TypeDB 3.11 lacks.
run_live_migration_steps() {
    printf "${BOLD}━━━ CLI workspace lifecycle (integration) ━━━${RESET}\n\n"
    for cli_live_test in \
        empty_workspace_to_replayed_history_live \
        documented_examples_initial_constraints_apply_and_verify_live \
        verify_never_creates_databases_live \
        adopt_legacy_history_then_evolve_live \
        shipped_python_converter_to_native_adoption_live; do
        if [[ "$cli_live_test" == documented_examples_initial_constraints_apply_and_verify_live \
            && "$lane_profile" != typedb-3.13.6/v1 ]]; then
            printf "${CYAN}Skipping documented examples on profile %s: examples require typedb-3.13.6/v1.${RESET}\n\n" "$lane_profile"
            continue
        fi
        run_step "cargo test -p type-bridge-cli --test e2e_workspace_live $cli_live_test" \
            run_with_timeout 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPE_BRIDGE_TEST_PYTHON="$ROOT/.venv/bin/python" \
                "${migration_profile_env[@]}" \
            bash scripts/ci/run_exact_ignored_rust_test.sh "$cli_live_test" \
                --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-cli --test e2e_workspace_live
    done

    if [[ "$lane_profile" == typedb-3.13.6/v1 ]]; then
        run_step "native ordered schema formatter exports and removes collection modes" \
            run_with_timeout 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            bash scripts/ci/run_exact_ignored_rust_test.sh \
                lowering::tests::native_ordered_formatter_exports_and_removes_schema_live \
                --manifest-path type-bridge-core/Cargo.toml --locked \
                -p type-bridge-schema-migration --lib
    fi

    printf "${BOLD}━━━ Connected migration recovery (integration) ━━━${RESET}\n\n"
    run_step "connected rollback and reapply lifecycle" \
        run_with_timeout 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE="type_bridge_local_rollback_${$}" \
            "${migration_profile_env[@]}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            runner_rolls_back_the_applied_head_and_reapplies_on_3_12_3 \
            --manifest-path type-bridge-core/Cargo.toml --locked \
            -p type-bridge-schema-migration-typedb --test live_runner
    run_step "interrupted-plan fenced recovery lifecycle" \
        run_with_timeout 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE="type_bridge_local_recovery_${$}" \
            "${migration_profile_env[@]}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            control_schema_and_fenced_lease_round_trip_on_3_12_3 \
            --manifest-path type-bridge-core/Cargo.toml --locked \
            -p type-bridge-schema-migration-typedb --test live_store
}

# run_live_lane <server>: run every live step against one TypeDB line: a fresh
# isolated server of that line, or the running server under --no-isolated
# (empty <server>). Lane state is global so the step helpers can read it.
run_live_lane() {
    local requested_server="$1"
    local lane_project=""
    local lane_pass_start="$pass"
    lane_fail_start="$fail"
    lane_label="$requested_server"
    sdk_report_dir=""
    TYPEDB_PORT="$CALLER_TYPEDB_PORT"
    TYPEDB_HTTP_PORT="$CALLER_TYPEDB_HTTP_PORT"
    TYPEDB_ADDRESS="$CALLER_TYPEDB_ADDRESS"
    if [[ "$isolated" == 1 ]]; then
        if ((${#typedb_lanes[@]} > 1)); then
            lane_project="$(lane_compose_project "typedb-$requested_server")"
        else
            lane_project="$(lane_compose_project "")"
        fi
        start_typedb "$requested_server" "$lane_project" "$tls"
        TYPEDB_PORT="$started_port"
        TYPEDB_HTTP_PORT="$started_http_port"
    fi

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
    lane_label="$typedb_server_version"
    if [[ "$isolated" == 1 && -z "$CALLER_TYPEDB_IMAGE" \
        && "$typedb_server_version" != "$requested_server" ]]; then
        printf "${RED}The TypeDB %s lane detected server %s.${RESET}\n" \
            "$requested_server" "$typedb_server_version" >&2
        exit 1
    fi

    # Generated live fixtures admit exactly the server their semantic profile
    # names, so pair the fixture profile with the detected server.
    lane_profile="${CALLER_ACCEPTANCE_PROFILE:-${LANE_PROFILES[$typedb_server_version]:-}}"
    if [[ -n "$lane_profile" ]]; then
        export TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE="$lane_profile"
    else
        unset TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE
    fi
    # Steps bound to the SDK conformance contracts (report fan-in, projected
    # exact-live parity, C live, TLS) run where the profile is in the SDK
    # version matrix and pairs with this server: 3.12.3 and 3.13.6.
    lane_in_sdk_matrix=0
    if [[ -n "$lane_profile" ]] \
        && lane_matrix_server="$(profile_server_version "$lane_profile" 2>/dev/null)" \
        && [[ "$lane_matrix_server" == "$typedb_server_version" ]]; then
        lane_in_sdk_matrix=1
    fi
    # TypeDB 3.11 has no migration profile: connected migrations refuse it.
    lane_migrations=1
    if [[ "$typedb_server_version" == 3.11.* ]]; then
        lane_migrations=0
    fi
    # Live workspace tests author their manifests on
    # TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE; keep it on the lane's profile.
    migration_profile_env=()
    if [[ -n "${TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE:-}" ]]; then
        migration_profile_env=(
            "TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE=${TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE:-$TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE}"
        )
    fi

    sdk_rust_env=()
    sdk_python_env=()
    sdk_node_env=()
    sdk_c_env=()
    # Forwarded pytest arguments may change collection and exclude the sole
    # Python report producer. Keep targeted Python runs useful instead of
    # allocating a fan-in that can never become complete; the unfiltered full
    # suite remains the local conformance gate.
    if [[ "$lane_in_sdk_matrix" == 1 && ${#pytest_args[@]} -eq 0 ]]; then
        sdk_report_dir="$(
            mktemp -d "${TMPDIR:-/tmp}/typebridge-sdk.XXXXXXXXXX"
        )"
        sdk_report_dir="$(cd "$sdk_report_dir" && pwd -P)"
        mkdir -p "$sdk_report_dir/queries"
        sdk_validator_python="$(
            uv run python -c 'import os, sys; print(os.path.realpath(sys.executable))'
        )"
        if [[ "$sdk_validator_python" != /* \
            || ! -x "$sdk_validator_python" ]]; then
            printf "${RED}Sdk validator Python must be an absolute executable path: %s${RESET}\n" \
                "$sdk_validator_python" >&2
            exit 2
        fi
        sdk_python_nonce="$(
            "$sdk_validator_python" -c 'import secrets; print(secrets.token_hex(32))'
        )"
        sdk_node_nonce="$(
            "$sdk_validator_python" -c 'import secrets; print(secrets.token_hex(32))'
        )"
        sdk_rust_nonce="$(
            "$sdk_validator_python" -c 'import secrets; print(secrets.token_hex(32))'
        )"
        sdk_c_nonce="$(
            "$sdk_validator_python" -c 'import secrets; print(secrets.token_hex(32))'
        )"
        declare -A sdk_nonce_set=()
        for sdk_nonce in \
            "$sdk_python_nonce" \
            "$sdk_node_nonce" \
            "$sdk_rust_nonce" \
            "$sdk_c_nonce"; do
            if [[ ! "$sdk_nonce" =~ ^[0-9a-f]{64}$ \
                || ${sdk_nonce_set[$sdk_nonce]+x} == x ]]; then
                printf "${RED}SDK queries binding nonces must be distinct lowercase 64-hex values.${RESET}\n" >&2
                exit 2
            fi
            sdk_nonce_set[$sdk_nonce]=1
        done
        unset sdk_nonce sdk_nonce_set

        sdk_python_direct_fragment="$sdk_report_dir/queries/python-direct-proof.json"
        sdk_python_remote_fragment="$sdk_report_dir/queries/python-remote-proof.json"
        sdk_node_direct_fragment="$sdk_report_dir/queries/node-direct-proof.json"
        sdk_node_remote_fragment="$sdk_report_dir/queries/node-remote-proof.json"
        sdk_rust_fragment="$sdk_report_dir/queries/rust-proof.json"
        sdk_c_fragment="$sdk_report_dir/queries/c-proof.json"
        sdk_queries_summary="$sdk_report_dir/queries-comparison.json"
        sdk_rust_env=(
            "TYPE_BRIDGE_SDK_QUERIES_REPORT=$sdk_report_dir/queries/rust.json"
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS=$sdk_rust_fragment"
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE=$sdk_rust_nonce"
            "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE=$lane_profile"
        )
        sdk_python_env=(
            "TYPE_BRIDGE_SDK_QUERIES_REPORT=$sdk_report_dir/queries/python.json"
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS=$sdk_python_direct_fragment:$sdk_python_remote_fragment"
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE=$sdk_python_nonce"
        )
        sdk_node_env=(
            "TYPE_BRIDGE_SDK_QUERIES_REPORT=$sdk_report_dir/queries/node.json"
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS=$sdk_node_direct_fragment:$sdk_node_remote_fragment"
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE=$sdk_node_nonce"
            "TYPE_BRIDGE_SDK_QUERIES_VALIDATOR_PYTHON=$sdk_validator_python"
            "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE=$lane_profile"
        )
        sdk_c_env=(
            "TYPE_BRIDGE_SDK_QUERIES_REPORT=$sdk_report_dir/queries/c.json"
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS=$sdk_c_fragment"
            "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE=$sdk_c_nonce"
        )
        printf "${CYAN}Sdk reports: %s${RESET}\n\n" "$sdk_report_dir"
    elif [[ "$lane_in_sdk_matrix" == 1 ]]; then
        printf "${CYAN}Sdk report fan-in skipped because forwarded pytest arguments may change collection.${RESET}\n\n"
    fi

    if [[ "$lane_in_sdk_matrix" == 1 ]]; then
        printf "${BOLD}━━━ Projected exact-live parity (integration) ━━━${RESET}\n\n"
        run_step "four-binding exact-TypeDB-$typedb_server_version Projected live fan-in" \
            env TYPE_BRIDGE_PROJECTED_LIVE_ADDRESS="$TYPEDB_ADDRESS" \
                TYPE_BRIDGE_PROJECTED_LIVE_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            uv run python scripts/ci/run_generated_live.py projected
    else
        printf "${CYAN}Projected exact-live fan-in requires a profile in the SDK matrix; skipping TypeDB %s.${RESET}\n\n" \
            "$typedb_server_version"
    fi

    printf "${BOLD}━━━ Rust (integration) ━━━${RESET}\n\n"
    run_step "runtime and staged transaction options/reuse live checks" \
        run_with_timeout 10m \
        env TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS="$TYPEDB_ADDRESS" \
            TYPE_BRIDGE_TRANSACTION_LIVE_VERSION="$typedb_server_version" \
            TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPEDB_USERNAME="${TYPEDB_USERNAME-admin}" \
            TYPEDB_PASSWORD="${TYPEDB_PASSWORD-password}" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-typedb-runtime -p type-bridge-orm \
            --test transaction_options_live --test staged_transaction_live

    if [[ "$typedb_server_version" == "3.13.6" ]]; then
        run_step "staged query-plan/v3 programs resolve at commit (TypeDB 3.13.6)" \
            run_with_timeout 10m \
            env TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS="$TYPEDB_ADDRESS" \
                TYPE_BRIDGE_TRANSACTION_LIVE_VERSION="$typedb_server_version" \
                TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPEDB_USERNAME="${TYPEDB_USERNAME-admin}" \
                TYPEDB_PASSWORD="${TYPEDB_PASSWORD-password}" \
            cargo test --manifest-path type-bridge-core/Cargo.toml --locked \
                -p type-bridge-orm --test plan_execute_live
        run_step "concurrency guide TypeQL snippets (TypeDB 3.13.6)" \
            run_with_timeout 5m \
            env TYPE_BRIDGE_CONCURRENCY_LIVE_ADDRESS="$TYPEDB_ADDRESS" \
                TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPEDB_USERNAME="${TYPEDB_USERNAME-admin}" \
                TYPEDB_PASSWORD="${TYPEDB_PASSWORD-password}" \
            bash scripts/ci/run_exact_ignored_rust_test.sh \
                concurrency_guide_snippets_run_on_3_13_6 \
                --manifest-path type-bridge-core/Cargo.toml --locked \
                -p type-bridge-typedb-runtime --test concurrency_documentation_live
    fi

    run_step "cargo test -p type-bridge-orm --features integration-tests --test integration" \
        run_with_timeout 15m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
        -p type-bridge-orm --features integration-tests --test integration -- \
            --nocapture --test-threads=1

    printf "${BOLD}━━━ Production V2 server (integration) ━━━${RESET}\n\n"
    run_step "type-bridge-server V1 + V2 live smoke" \
        run_with_timeout 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            production_binary_serves_v1_health_and_v2_query \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-server --features v2-query \
            --test v2_query_integration_tests

    run_step "type-bridge-server fetch write commit failure regression" \
        run_with_timeout 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            fetch_write_commit_failure_reaches_driver_and_v1_query \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-server --features v2-query \
            --test v2_query_integration_tests

    run_step "type-bridge-server classified commit certainty regression" \
        run_with_timeout 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            typedb::client::tests::integration_classified_commit_isolation_conflict \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-server --features v2-query \
            --lib

    # Connects without an HTTP port, so discovery takes the band-8 gRPC
    # fallback and must reach the lane server's own band.
    run_step "runtime gRPC-only fallback discovery" \
        run_with_timeout 10m \
        env TYPEDB_GRPC_FALLBACK_ADDRESS="$TYPEDB_ADDRESS" \
            TYPEDB_GRPC_FALLBACK_VERSION="$typedb_server_version" \
        cargo test --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-typedb-runtime --test grpc_fallback_live

    printf "${BOLD}━━━ Generated Rust projection (integration) ━━━${RESET}\n\n"
    if [[ -n "$sdk_report_dir" ]]; then
        run_step "emit Rust queries deterministic proof fragment" \
            env TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT="$sdk_rust_fragment" \
                TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE="$sdk_rust_nonce" \
            cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge --lib \
                remote::tests::sdk_queries_rust_deterministic_proof_fragment \
                -- --exact
    fi
    run_step "generated Rust application parity" \
        run_with_timeout 10m \
        env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE="type_bridge_rust_projection_live_${$}" \
            ACCEPTANCE_TARGET_DIR="$ROOT/type-bridge-core/target/tmp_projection_live_target" \
            "${sdk_rust_env[@]}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            generated_rust_projection_round_trips_exact_live_models \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-schema-codegen --test rust_projection_live

    if [[ "$lane_in_sdk_matrix" == 1 ]]; then
        printf "${BOLD}━━━ C runtime transactions (integration) ━━━${RESET}\n\n"
        run_step "compiled C17 runtime and transaction lifecycle" \
            run_with_timeout 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER=1 \
                TYPE_BRIDGE_C_INTG_DATABASE="type_bridge_c_runtime_live_${$}" \
            bash scripts/ci/run_exact_ignored_rust_test.sh \
                live_c17_consumer_exercises_profile_server_transaction_lifecycle \
                --manifest-path type-bridge-core/Cargo.toml --locked \
                -p type-bridge-c --test schema_package_abi

        printf "${BOLD}━━━ Generated C entity and relation CRUD (integration) ━━━${RESET}\n\n"
        if [[ -n "$sdk_report_dir" ]]; then
            run_step "emit C queries deterministic proof fragment" \
                env TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT="$sdk_c_fragment" \
                    TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE="$sdk_c_nonce" \
                cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
                    -p type-bridge-c --lib \
                    query::tests::sdk_queries_c_deterministic_proof_fragment \
                    -- --exact
        fi
        run_step "compiled generated C17 Person and Membership CRUD lifecycle" \
            run_with_timeout 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
                TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE="type_bridge_c_projection_live_${$}" \
                ACCEPTANCE_TARGET_DIR="$ROOT/type-bridge-core/target/tmp_c_projection_live_target" \
                "${sdk_c_env[@]}" \
            bash scripts/ci/run_exact_ignored_rust_test.sh \
                live_c17_generated_person_and_membership_crud_round_trips_on_profile_server \
                --manifest-path type-bridge-core/Cargo.toml --locked \
                -p type-bridge-schema-codegen --test c_projection_live
    else
        printf "${CYAN}C runtime and generated C CRUD live smoke need a profile in the SDK matrix; skipping TypeDB %s.${RESET}\n\n" \
            "$typedb_server_version"
    fi

    if [[ "$lane_migrations" == 1 ]]; then
        run_live_migration_steps
    else
        printf "${CYAN}Connected migrations refuse TypeDB %s (no migration profile); skipping the CLI workspace and recovery lifecycles.${RESET}\n\n" \
            "$typedb_server_version"
        printf "${BOLD}━━━ Unsupported-server migration refusal (integration) ━━━${RESET}\n\n"
        run_step "unsupported migration apply creates no databases" \
            run_with_timeout 10m \
            env TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            bash scripts/ci/run_exact_ignored_rust_test.sh \
                unsupported_server_apply_creates_neither_database_live \
                --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-cli --test e2e_workspace_live
    fi

    printf "${BOLD}━━━ Python (integration) ━━━${RESET}\n\n"
    if [[ -n "$sdk_report_dir" ]]; then
        run_step "emit Python direct-cancellation queries proof fragment" \
            env TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT="$sdk_python_direct_fragment" \
                TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE="$sdk_python_nonce" \
            cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-core --lib \
                match_runtime::tests::python_direct_cancellation_fragment_is_measured_from_owned_execution \
                -- --exact
        run_step "emit Python generated-remote queries proof fragment" \
            env TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT="$sdk_python_remote_fragment" \
                TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE="$sdk_python_nonce" \
            uv run python \
                type-bridge-core/crates/schema-codegen/tests/acceptance/check.py
    fi
    run_step "pytest -m integration" \
        run_with_timeout 20m \
        env USE_DOCKER=false TYPEDB_ADDRESS="$TYPEDB_ADDRESS" TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            "${sdk_python_env[@]}" \
        uv run pytest -m integration --tb=short "${pytest_args[@]}"

    printf "${BOLD}━━━ Node (integration) ━━━${RESET}\n\n"
    # The Node suite defaults TYPEDB_ADDRESS to :1730; pass the live endpoint explicitly.
    native="$(ls "$NODE_DIR"/type_bridge_node.*.node 2>/dev/null | head -1 || true)"
    if [[ -n "$native" ]]; then
        native="$ROOT/$native"
    fi
    run_step "npm run test:integration" \
        run_with_timeout 15m \
        bash -c "cd '$NODE_DIR' && TYPE_BRIDGE_NODE_NATIVE_PATH='$native' \
            USE_DOCKER=false TYPEDB_ADDRESS='$TYPEDB_ADDRESS' TYPEDB_HTTP_PORT='$TYPEDB_HTTP_PORT' \
            TYPEDB_VERSION='$typedb_server_version' \
            npm run test:integration"
    if [[ -n "$sdk_report_dir" ]]; then
        run_step "emit Node direct-cancellation queries proof fragment" \
            env TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT="$sdk_node_direct_fragment" \
                TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE="$sdk_node_nonce" \
            cargo test --locked --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-node --lib \
                match_runtime::tests::node_direct_cancellation_fragment_is_measured_from_owned_execution \
                -- --exact
        run_step "emit Node generated-remote queries proof fragment" \
            env TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT="$sdk_node_remote_fragment" \
                TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE="$sdk_node_nonce" \
            node type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/check.mjs
    fi
    run_step "npm run test:projection-integration" \
        run_with_timeout 15m \
        env TYPE_BRIDGE_NODE_NATIVE_PATH="$native" \
            USE_DOCKER=false TYPEDB_ADDRESS="$TYPEDB_ADDRESS" \
            TYPEDB_HTTP_PORT="$TYPEDB_HTTP_PORT" \
            TYPEDB_VERSION="$typedb_server_version" \
            TYPE_BRIDGE_NODE_INTG_DATABASE="type_bridge_projection_live_${$}" \
            "${sdk_node_env[@]}" \
        npm --prefix "$NODE_DIR" run test:projection-integration

    if [[ -n "$sdk_report_dir" ]]; then
        run_step "compare generated SDK queries reports" \
            write_sdk_summary \
                "$sdk_queries_summary" \
                "$sdk_validator_python" \
                "$sdk_validator_python" \
                scripts/ci/sdk_queries_compare.py \
                "$sdk_report_dir/queries/python.json" \
                "$sdk_report_dir/queries/node.json" \
                "$sdk_report_dir/queries/rust.json" \
                "$sdk_report_dir/queries/c.json"
        run_step "validate and log sdk report and summary SHA-256 evidence" \
            log_sdk_evidence_sha256 \
                "$sdk_validator_python" \
                "$sdk_report_dir/queries/python.json" \
                "$sdk_report_dir/queries/node.json" \
                "$sdk_report_dir/queries/rust.json" \
                "$sdk_report_dir/queries/c.json" \
                "$sdk_queries_summary"
    fi

    if [[ "$tls" == 1 ]]; then
        run_lane_tls
    fi
    finish_sdk_evidence
    lane_summaries+=("TypeDB $lane_label: $((pass - lane_pass_start)) passed, $((fail - lane_fail_start)) failed")
    if [[ -n "$lane_project" ]]; then
        stop_typedb "$lane_project"
    fi
    lane_label=""
}

# Remove one lane's accepted sdk evidence, or keep it for diagnosis when the
# lane failed or preservation was requested.
finish_sdk_evidence() {
    if [[ -n "$sdk_report_dir" ]]; then
        if ((fail == lane_fail_start)) && [[ "$preserve_sdk_evidence" == 0 ]]; then
            rm -f -- \
                "$sdk_report_dir/queries/python.json" \
                "$sdk_report_dir/queries/node.json" \
                "$sdk_report_dir/queries/rust.json" \
                "$sdk_report_dir/queries/c.json" \
                "$sdk_report_dir/queries/python-direct-proof.json" \
                "$sdk_report_dir/queries/python-remote-proof.json" \
                "$sdk_report_dir/queries/node-direct-proof.json" \
                "$sdk_report_dir/queries/node-remote-proof.json" \
                "$sdk_report_dir/queries/rust-proof.json" \
                "$sdk_report_dir/queries/c-proof.json" \
                "$sdk_report_dir/queries-comparison.json"
            if ! rmdir -- "$sdk_report_dir/queries"; then
                printf "${RED}Could not remove accepted queries report directory: %s${RESET}\n\n" \
                    "$sdk_report_dir/queries" >&2
                fail=$((fail + 1))
                failures+=("remove accepted queries report directory")
            fi
            if rmdir -- "$sdk_report_dir"; then
                printf "${GREEN}Removed accepted sdk reports: %s${RESET}\n\n" \
                    "$sdk_report_dir"
            else
                printf "${RED}Could not remove accepted sdk report directory: %s${RESET}\n\n" \
                    "$sdk_report_dir" >&2
                fail=$((fail + 1))
                failures+=("remove accepted sdk report directory")
            fi
        elif ((fail > lane_fail_start)); then
            printf "${CYAN}Preserved sdk reports for diagnosis: %s${RESET}\n\n" \
                "$sdk_report_dir"
        else
            printf "${CYAN}Preserved accepted sdk evidence by request: %s${RESET}\n\n" \
                "$sdk_report_dir"
        fi
    fi
}

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
    # The TLS endpoint fronts the lane's server: the one its semantic profile
    # pairs with, else the detected server when no profile is paired.
    local lane_server="$typedb_server_version"
    if [[ -n "${TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE:-}" ]]; then
        lane_server="$(profile_server_version "$TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE")"
    fi

    if [[ "$required_topology" == 1 ]]; then
        run_step "TLS runtime HTTP + gRPC lifecycle" \
            run_with_timeout 10m \
            env TYPEDB_TLS_ADDRESS="$tls_address" \
                TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
                TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
                SSL_CERT_FILE="$fixture_root_ca" \
                TYPE_BRIDGE_TLS_LIVE_REQUIRED=1 \
                TYPE_BRIDGE_TLS_NATIVE_ROOTS=1 \
                TYPE_BRIDGE_TLS_EXPECTED_SERVER_VERSION="$lane_server" \
                TYPE_BRIDGE_TLS_EXPECTED_DRIVER_BAND=9 \
                TYPE_BRIDGE_TLS_EXPECTED_DRIVER_VERSION=3.13.6 \
            cargo test --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-typedb-runtime --test tls_live \
                -- --nocapture --test-threads=1

        run_step "TLS generated Rust application parity" \
            run_with_timeout 10m \
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

        run_step "TLS ordered generated Python + Node manager parity" \
            run_with_timeout 20m \
            env TYPEDB_TLS_ADDRESS="$tls_address" \
                TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
                TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
                RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}" \
            uv run python scripts/ci/run_generated_live.py manager-tls
    else
        printf "${CYAN}External TLS runtime proof is custom-root only; native-root and exact-topology assertions require the isolated lane.${RESET}\n\n"
        run_step "TLS runtime HTTP + gRPC lifecycle (external custom-root)" \
            run_with_timeout 10m \
            env TYPEDB_TLS_ADDRESS="$tls_address" \
                TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
                TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            cargo test --manifest-path type-bridge-core/Cargo.toml \
                -p type-bridge-typedb-runtime --test tls_live \
                -- --nocapture --test-threads=1
    fi

    run_step "TLS CLI workspace migration apply + verify" \
        run_with_timeout 10m \
        env TYPEDB_TLS_ADDRESS="$tls_address" \
            TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            "${migration_profile_env[@]}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            tls_workspace_apply_and_verify_live \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-cli --test e2e_workspace_live

    run_step "TLS Python local query + HTTPS remote envelope" \
        run_with_timeout 10m \
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
        run_with_timeout 10m \
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
        run_with_timeout 10m \
        env TYPE_BRIDGE_NODE_NATIVE_PATH="$node_native" \
            TYPEDB_TLS_ADDRESS="$tls_address" \
            TYPEDB_TLS_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            SMOKE_TLS_CERT="$fixture_server_cert" \
            SMOKE_TLS_KEY="$fixture_server_key" \
            NODE_EXTRA_CA_CERTS="$fixture_root_ca" \
        node "$NODE_DIR/scripts/run-integration.js" \
            tests/integration/queries/query-v2-smoke.test.js

    run_step "TLS generated Node application parity" \
        run_with_timeout 10m \
        env TYPE_BRIDGE_NODE_NATIVE_PATH="$node_native" \
            TYPEDB_ADDRESS="$tls_address" \
            TYPEDB_VERSION="$lane_server" \
            TYPEDB_HTTP_PORT="$tls_http_port" \
            TYPEDB_TLS_ROOT_CA="$tls_root_ca" \
            SMOKE_TLS_CERT="$fixture_server_cert" \
            SMOKE_TLS_KEY="$fixture_server_key" \
            NODE_EXTRA_CA_CERTS="$fixture_root_ca" \
            TYPE_BRIDGE_NODE_INTG_DATABASE="type_bridge_projection_tls_${$}" \
        npm --prefix "$NODE_DIR" run test:projection-integration
}

run_lane_tls() {
    printf "${BOLD}━━━ TLS transport (opt-in) ━━━${RESET}\n\n"
    if [[ "$lane_in_sdk_matrix" != 1 ]]; then
        # The TLS lane asserts the band-9 driver identity and applies a
        # workspace migration; TypeDB 3.11 is served by band 8 and refuses
        # migrations.
        printf "${CYAN}Skipping TLS transport on TypeDB %s: it needs a profile in the SDK matrix.${RESET}\n\n" \
            "$typedb_server_version"
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
}

# ── Two-server profile-upgrade lane (--typedb all) ───────────────────────────
# A history applied on 3.12.3 moves to 3.13.6 through a profile bridge. Both
# servers run at once as separate compose projects; the tests move databases
# with the TypeDB driver's export and import.
run_upgrade_lane() {
    local from_project to_project from_port from_http_port to_port to_http_port
    printf "${BOLD}━━━ Profile upgrade 3.12.3 -> 3.13.6 (two servers) ━━━${RESET}\n\n"
    local lane_pass_start="$pass"
    lane_fail_start="$fail"
    lane_label="upgrade"
    from_project="$(lane_compose_project upgrade-3.12.3)"
    to_project="$(lane_compose_project upgrade-3.13.6)"
    start_typedb 3.12.3 "$from_project" 0
    from_port="$started_port"
    from_http_port="$started_http_port"
    start_typedb 3.13.6 "$to_project" 0
    to_port="$started_port"
    to_http_port="$started_http_port"

    local upgrade_env=(
        "TYPE_BRIDGE_MIGRATION_UPGRADE_FROM=localhost:$from_port"
        "TYPE_BRIDGE_MIGRATION_UPGRADE_FROM_HTTP_PORT=$from_http_port"
        "TYPE_BRIDGE_MIGRATION_UPGRADE_TO=localhost:$to_port"
        "TYPE_BRIDGE_MIGRATION_UPGRADE_TO_HTTP_PORT=$to_http_port"
    )
    run_step "connected history crosses a profile bridge and replays on 3.13.6" \
        run_with_timeout 10m \
        env "${upgrade_env[@]}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            runner_upgrades_a_3_12_history_to_3_13_at_a_profile_bridge_and_replays_it \
            --manifest-path type-bridge-core/Cargo.toml --locked \
            -p type-bridge-schema-migration-typedb --test live_profile_upgrade
    run_step "CLI upgrade-profile moves a 3.12 history to 3.13" \
        run_with_timeout 10m \
        env "${upgrade_env[@]}" \
        bash scripts/ci/run_exact_ignored_rust_test.sh \
            upgrade_profile_moves_a_3_12_history_to_3_13_live \
            --manifest-path type-bridge-core/Cargo.toml \
            -p type-bridge-cli --test e2e_workspace_live

    lane_summaries+=("Profile upgrade 3.12.3 -> 3.13.6: $((pass - lane_pass_start)) passed, $((fail - lane_fail_start)) failed")
    stop_typedb "$from_project"
    stop_typedb "$to_project"
    lane_label=""
}

if [[ "$integration" == 1 ]]; then
    if [[ "$isolated" == 1 ]]; then
        detect_compose
    fi
    for typedb_lane in "${typedb_lanes[@]}"; do
        run_live_lane "$typedb_lane"
    done
    unset typedb_lane
    if [[ "$typedb_request" == all ]]; then
        run_upgrade_lane
    fi
elif [[ "$tls" == 1 ]]; then
    printf "${CYAN}Skipping TLS transport: --no-integration disables live tiers.${RESET}\n\n"
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
for lane_summary in "${lane_summaries[@]}"; do
    printf "  %s\n" "$lane_summary"
done
printf "${GREEN}  ✓ %d passed${RESET}\n" "$pass"
if ((fail > 0)); then
    printf "${RED}  ✗ %d failed:${RESET}\n" "$fail"
    for f in "${failures[@]}"; do
        printf "${RED}    - %s${RESET}\n" "$f"
    done
    exit 1
fi
