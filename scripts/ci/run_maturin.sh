#!/usr/bin/env bash
# Keep source-tree acceptance builds on the validated build backend.
set -euo pipefail
if ! command -v uv >/dev/null || ! command -v uvx >/dev/null; then
    printf 'required tool is unavailable: uv (including uvx)\n' >&2
    exit 1
fi
exec uvx --from maturin==1.14.1 maturin "$@"
