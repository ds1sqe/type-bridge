#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "usage: $0 <python|node> <empty-output-directory>" >&2
    exit 2
}

[[ $# -eq 2 ]] || usage
binding="$1"
requested_output="$2"

case "$binding" in
    python | node) ;;
    *) usage ;;
esac

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CORE_DIR="$ROOT_DIR/type-bridge-core"
NODE_DIR="$CORE_DIR/crates/node"
SCHEMA="$CORE_DIR/crates/schema-codegen/tests/acceptance/schema.yaml"
output_dir="$(realpath -m "$requested_output")"

if [[ "$output_dir" == "/" || "$output_dir" == "$ROOT_DIR" ]]; then
    echo "Refusing unsafe generated-fixture output: $output_dir" >&2
    exit 2
fi
if [[ -e "$output_dir" ]] && [[ -n "$(find "$output_dir" -mindepth 1 -print -quit)" ]]; then
    echo "Generated-fixture output must be empty: $output_dir" >&2
    exit 2
fi
mkdir -p "$output_dir"

if [[ "$binding" == "python" ]]; then
    cargo run --quiet --manifest-path "$CORE_DIR/Cargo.toml" \
        -p type-bridge-schema-codegen --example emit_python_acceptance -- \
        "$SCHEMA" "$output_dir/generated_v2" "$output_dir/declared-schema.json"
    variant_schema="$output_dir/schema-variant.yaml"
    sed \
        's/member: { card: { min: 0, max: 2 }, doc: membership player }/member: { card: { min: 0, max: 3 }, doc: membership player }/' \
        "$SCHEMA" > "$variant_schema"
    if cmp -s "$SCHEMA" "$variant_schema"; then
        echo "Generated Python variant did not alter the playing fact." >&2
        exit 1
    fi
    cargo run --quiet --manifest-path "$CORE_DIR/Cargo.toml" \
        -p type-bridge-schema-codegen --example emit_python_acceptance -- \
        "$variant_schema" "$output_dir/generated_variant"
    exit 0
fi

cargo run --quiet --manifest-path "$CORE_DIR/Cargo.toml" \
    -p type-bridge-schema-codegen --example emit_typescript_acceptance -- \
    "$SCHEMA" "$output_dir/generated_v2" "$output_dir/declared-schema.json"
cargo run --quiet --manifest-path "$CORE_DIR/Cargo.toml" \
    -p type-bridge-schema-codegen --example emit_typescript_acceptance -- \
    "$SCHEMA" "$output_dir/generated_foreign"

package_scope="$output_dir/node_modules/@type-bridge"
runtime_link="$package_scope/node"
mkdir -p "$package_scope"
ln -s "$NODE_DIR" "$runtime_link"
cleanup_runtime_link() {
    if [[ -L "$runtime_link" ]]; then
        unlink "$runtime_link"
    fi
}
trap cleanup_runtime_link EXIT

"$NODE_DIR/node_modules/.bin/tsc" \
    --project "$output_dir/generated_v2/tsconfig.json"
"$NODE_DIR/node_modules/.bin/tsc" \
    --project "$output_dir/generated_foreign/tsconfig.json"
"$NODE_DIR/node_modules/.bin/tsc" \
    --project "$NODE_DIR/tsconfig.projection-integration.json" \
    --outDir "$output_dir/harness"
cp "$NODE_DIR/tests/projection-integration/package.json" \
    "$output_dir/harness/package.json"

cleanup_runtime_link
trap - EXIT
