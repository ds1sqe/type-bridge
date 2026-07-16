#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPOSE_FILE="$ROOT_DIR/docker-compose.yml"
PROJECT="type-bridge-rust-projection-${$}"

if [[ -n "${CONTAINER_TOOL:-}" ]]; then
    container_tool="$CONTAINER_TOOL"
elif command -v podman >/dev/null 2>&1 && podman info >/dev/null 2>&1; then
    container_tool="podman"
elif command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    container_tool="docker"
else
    echo "No usable Podman or Docker runtime was found." >&2
    exit 1
fi

if ! command -v "$container_tool" >/dev/null 2>&1; then
    echo "Container runtime '$container_tool' was not found." >&2
    exit 1
fi

compose=("$container_tool" compose -p "$PROJECT" -f "$COMPOSE_FILE")
"${compose[@]}" version >/dev/null

export TYPEDB_IMAGE="typedb/typedb:3.12.1"
export TYPEDB_PORT=0
export TYPEDB_HTTP_PORT=0

cleanup() {
    "${compose[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

"${compose[@]}" up -d typedb
typedb_port="$(${compose[@]} port typedb 1729)"
typedb_http_port="$(${compose[@]} port typedb 8000)"
typedb_port="${typedb_port##*:}"
typedb_http_port="${typedb_http_port##*:}"
typedb_port="${typedb_port//$'\r'/}"
typedb_http_port="${typedb_http_port//$'\r'/}"

ready=false
for _ in {1..60}; do
    if (exec 3<>"/dev/tcp/127.0.0.1/$typedb_port") 2>/dev/null \
        && (exec 4<>"/dev/tcp/127.0.0.1/$typedb_http_port") 2>/dev/null; then
        exec 3>&-
        exec 4>&-
        ready=true
        break
    fi
    sleep 1
done
if [[ "$ready" != true ]]; then
    "${compose[@]}" logs typedb >&2 || true
    echo "TypeDB 3.12.1 did not become ready." >&2
    exit 1
fi

TYPEDB_ADDRESS="127.0.0.1:$typedb_port" \
TYPEDB_HTTP_PORT="$typedb_http_port" \
TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE="type_bridge_rust_projection_live_${$}" \
cargo test --manifest-path "$ROOT_DIR/type-bridge-core/Cargo.toml" \
    -p type-bridge-schema-codegen --test rust_projection_live \
    -- --ignored --nocapture
