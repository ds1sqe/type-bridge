#!/usr/bin/env bash
# Helper script to run integration tests with Docker or Podman
# Auto-detects container tool (prefers podman, falls back to docker)
# Override with: CONTAINER_TOOL=docker ./test-integration.sh

set -e

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Auto-detect container tool if not set
if [ -z "$CONTAINER_TOOL" ]; then
    if command -v podman &> /dev/null; then
        export CONTAINER_TOOL=podman
    elif command -v docker &> /dev/null; then
        export CONTAINER_TOOL=docker
    fi
fi

echo -e "${GREEN}Running integration tests with ${CONTAINER_TOOL:-auto-detect}...${NC}"

# Run pytest with integration marker
# Container management is handled by conftest.py
uv run pytest -m integration "$@"

echo -e "${GREEN}Integration tests completed!${NC}"
