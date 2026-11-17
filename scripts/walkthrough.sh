#!/usr/bin/env bash
# Run all four generated SDK examples against an existing TypeDB.
set -euo pipefail
cd "$(dirname "$0")/.."
export TMPDIR="${TMPDIR:-$HOME/.cache/type-bridge-walkthrough}"
mkdir -p "$TMPDIR"
uv run --extra dev python examples/walkthrough/run.py \
  --output "${TYPE_BRIDGE_WALKTHROUGH_OUTPUT:-tmp/walkthrough-$(date -u +%Y%m%dT%H%M%S)-$$}" "$@"
