#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
MANIFEST="$ROOT/examples/typebridge.yaml"
NODE_CRATE="$ROOT/type-bridge-core/crates/node"
TYPESCRIPT_OUTPUT="$ROOT/examples/generated/typescript"
RUST_OUTPUT="$ROOT/examples/generated/rust"

cd "$ROOT"

cargo run --quiet \
    --manifest-path type-bridge-core/Cargo.toml \
    --package type-bridge-cli -- \
    --manifest "$MANIFEST" schema check
cargo run --quiet \
    --manifest-path type-bridge-core/Cargo.toml \
    --package type-bridge-cli -- \
    --manifest "$MANIFEST" schema generate

if rg -n \
    'from type_bridge(\.(attribute|fields|generator|models)| import .*(Attribute|Entity|Relation|Role|TypeDBType))' \
    examples --glob '*.py'; then
    printf 'Generated-only examples import a removed handwritten authoring API.\n' >&2
    exit 1
fi

PYTHONPATH="$ROOT/examples/generated/python" \
    uv run python -m compileall -q examples/basic examples/advanced
uv run pyright examples/basic examples/advanced
PYTHONPATH="$ROOT/examples/generated/python" \
    uv run python examples/advanced/features_01_generated_validation.py
PYTHONPATH="$ROOT/examples/generated/python" \
    uv run python examples/advanced/features_02_type_safety.py
PYTHONPATH="$ROOT/examples/generated/python" \
    uv run python examples/advanced/features_03_string_repr.py

if [[ ! -x "$NODE_CRATE/node_modules/.bin/tsc" ]]; then
    printf 'Node dependencies are missing; run npm ci in %s first.\n' "$NODE_CRATE" >&2
    exit 1
fi
mkdir -p "$TYPESCRIPT_OUTPUT/node_modules/@type-bridge"
if [[ ! -e "$TYPESCRIPT_OUTPUT/node_modules/@type-bridge/node" ]]; then
    ln -s "$NODE_CRATE" "$TYPESCRIPT_OUTPUT/node_modules/@type-bridge/node"
fi
rg -q '"@type-bridge/node": "\^2\.1\.0"' "$TYPESCRIPT_OUTPUT/package.json"
"$NODE_CRATE/node_modules/.bin/tsc" \
    --project "$TYPESCRIPT_OUTPUT/tsconfig.json" --noEmit

cargo check --quiet --offline \
    --manifest-path "$RUST_OUTPUT/Cargo.toml" \
    --config "patch.crates-io.type-bridge.path=\"$ROOT/type-bridge-core/crates/rust\""
