#!/usr/bin/env bash
# Run integration tests inside a pinned Python runner that talks to Docker-in-Docker.

set -euo pipefail

cleanup() {
    docker compose -f docker-compose.dind.yml down -v
}
trap cleanup EXIT

docker compose -f docker-compose.dind.yml run --rm integration "$@"
