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
ACCEPTANCE_DIR="$CORE_DIR/crates/schema-codegen/tests/acceptance"
semantic_profile="${TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE:-typedb-3.12.1/v1}"
output_dir="$(realpath -m "$requested_output")"

case "$semantic_profile" in
    typedb-3.11.5/v1) schema_source="$ACCEPTANCE_DIR/schema-3.11.5.yaml" ;;
    typedb-3.12.1/v1) schema_source="$ACCEPTANCE_DIR/schema.yaml" ;;
    *)
        echo "Unsupported generated-fixture semantic profile: $semantic_profile" >&2
        exit 2
        ;;
esac

if [[ "$output_dir" == "/" || "$output_dir" == "$ROOT_DIR" ]]; then
    echo "Refusing unsafe generated-fixture output: $output_dir" >&2
    exit 2
fi
if [[ -e "$output_dir" ]] && [[ -n "$(find "$output_dir" -mindepth 1 -print -quit)" ]]; then
    echo "Generated-fixture output must be empty: $output_dir" >&2
    exit 2
fi
mkdir -p "$output_dir"

scratch="$(mktemp -d "${TMPDIR:-/tmp}/type-bridge-generated-live.XXXXXX")"
cleanup_scratch() {
    rm -rf -- "$scratch"
}
trap cleanup_scratch EXIT

write_workspace() {
    local workspace="$1"
    local target="$2"
    local output="$3"
    local scope="$4"
    local source="$5"
    local authority="$6"

    mkdir -p "$workspace/schema/fragments" "$workspace/migrations/v2"
    cp "$source" "$workspace/schema/fragments/models.yaml"
    printf '%s\n' \
        'format: typebridge.schema-set/v1' \
        'sources: [fragments/*.yaml]' \
        > "$workspace/schema/schema.yaml"
    printf '%s\n' \
        'format: typebridge.workspace/v1' \
        'schema:' \
        '  root: schema/schema.yaml' \
        '  ownership: exclusive' \
        "  managed-scope: $scope" \
        'compatibility:' \
        "  semantic-profile: $semantic_profile" \
        'migrations:' \
        '  directory: migrations/v2' \
        '  app-label: generatedlive' \
        'bindings:' \
        "  $target:" \
        "    output: generated/$output" \
        > "$workspace/typebridge.yaml"
    if [[ "$authority" == "yes" ]]; then
        printf '%s\n' \
            'artifacts:' \
            '  schema-authority:' \
            '    output: generated/schema-authority.json' \
            >> "$workspace/typebridge.yaml"
    fi
}

generate_workspace() {
    local workspace="$1"
    cargo run --quiet --locked --manifest-path "$CORE_DIR/Cargo.toml" \
        -p type-bridge-cli --bin type-bridge -- \
        --manifest "$workspace/typebridge.yaml" schema generate
}

primary="$scratch/primary"
if [[ "$binding" == "python" ]]; then
    write_workspace \
        "$primary" python generated_v2 generated-python-live "$schema_source" yes
    generate_workspace "$primary"
    cp -R "$primary/generated/generated_v2" "$output_dir/generated_v2"
    cp "$primary/generated/schema-authority.json" "$output_dir/schema-authority.json"

    variant="$scratch/variant"
    variant_schema="$scratch/schema-variant.yaml"
    sed \
        -e 's/member: { card: { min: 0, max: 2 }, doc: membership player }/member: { card: { min: 0, max: 3 }, doc: membership player }/' \
        -e 's/member: { card: { min: 0, max: 2 } }/member: { card: { min: 0, max: 3 } }/' \
        "$schema_source" > "$variant_schema"
    if cmp -s "$schema_source" "$variant_schema"; then
        echo "Generated Python variant did not alter the playing fact." >&2
        exit 1
    fi
    write_workspace \
        "$variant" python generated_variant generated-python-variant "$variant_schema" no
    generate_workspace "$variant"
    cp -R "$variant/generated/generated_variant" "$output_dir/generated_variant"
    exit 0
fi

write_workspace \
    "$primary" typescript generated_v2 generated-projection-live "$schema_source" yes
generate_workspace "$primary"
cp -R "$primary/generated/generated_v2" "$output_dir/generated_v2"
cp "$primary/generated/schema-authority.json" "$output_dir/schema-authority.json"

foreign="$scratch/foreign"
write_workspace \
    "$foreign" typescript generated_foreign generated-node-foreign "$schema_source" no
generate_workspace "$foreign"
cp -R "$foreign/generated/generated_foreign" "$output_dir/generated_foreign"

package_scope="$output_dir/node_modules/@type-bridge"
runtime_link="$package_scope/node"
mkdir -p "$package_scope"
ln -s "$NODE_DIR" "$runtime_link"
cleanup_runtime_link() {
    if [[ -L "$runtime_link" ]]; then
        unlink "$runtime_link"
    fi
}
trap 'cleanup_runtime_link; cleanup_scratch' EXIT

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
cleanup_scratch
trap - EXIT
