#!/usr/bin/env bash
# Preflight or publish the complete crates.io release graph in dependency order.
set -euo pipefail

usage() {
  echo "usage: release_crates_graph.sh (--preflight|--publish) [CORE_WORKSPACE]" >&2
  exit 2
}

[[ $# -ge 1 && $# -le 2 ]] || usage
mode="$1"
case "$mode" in
  --preflight | --publish) ;;
  *) usage ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/../.." && pwd)"
core_workspace="${2:-$repo_root/type-bridge-core}"
[[ -d "$core_workspace" && ! -L "$core_workspace" ]] || {
  echo "Cargo workspace is missing or not a directory: $core_workspace" >&2
  exit 1
}
core_workspace="$(cd -- "$core_workspace" && pwd)"
helper="$script_dir/publish_crate_idempotently.sh"
[[ -f "$helper" && ! -L "$helper" ]] || {
  echo "Cargo publication helper is missing or not a regular file: $helper" >&2
  exit 1
}

preexisting_crates=(
  type-bridge-typedb-protocol-b8
  type-bridge-typedb-driver-b8
)
release_crates=(
  type-bridge-contract
  type-bridge-core-lib
  type-bridge-schema
  type-bridge-query
  type-bridge-schema-migration
  type-bridge-toml-transpiler
  type-bridge-schema-compat
  type-bridge-schema-codegen
  type-bridge-orm-derive
  type-bridge-typedb-protocol-b8
  type-bridge-typedb-driver-b8
  type-bridge-typedb-runtime
  type-bridge-orm
  type-bridge-migration
  type-bridge-schema-migration-typedb
  type-bridge-workspace
  type-bridge-cli
  type-bridge
)
publish_crates=(
  type-bridge-contract
  type-bridge-core-lib
  type-bridge-schema
  type-bridge-query
  type-bridge-schema-migration
  type-bridge-toml-transpiler
  type-bridge-schema-compat
  type-bridge-schema-codegen
  type-bridge-orm-derive
  type-bridge-typedb-runtime
  type-bridge-orm
  type-bridge-migration
  type-bridge-schema-migration-typedb
  type-bridge-workspace
  type-bridge-cli
  type-bridge
)

cd -- "$core_workspace"

if [[ "$mode" == "--publish" ]]; then
  for crate in "${publish_crates[@]}"; do
    bash "$helper" "$crate"
  done
  exit 0
fi

for crate in "${preexisting_crates[@]}"; do
  bash "$helper" --verify-preexisting "$crate"
done

patches=(
  --config 'patch.crates-io.type-bridge-contract.path="crates/contract"'
  --config 'patch.crates-io.type-bridge-core-lib.path="crates/core"'
  --config 'patch.crates-io.type-bridge-schema.path="crates/schema"'
  --config 'patch.crates-io.type-bridge-query.path="crates/query"'
  --config 'patch.crates-io.type-bridge-schema-migration.path="crates/schema-migration"'
  --config 'patch.crates-io.type-bridge-toml-transpiler.path="crates/toml-transpiler"'
  --config 'patch.crates-io.type-bridge-schema-compat.path="crates/schema-compat"'
  --config 'patch.crates-io.type-bridge-schema-codegen.path="crates/schema-codegen"'
  --config 'patch.crates-io.type-bridge-orm-derive.path="crates/orm-derive"'
  --config 'patch.crates-io.type-bridge-typedb-protocol-b8.path="vendor/typedb-protocol-b8"'
  --config 'patch.crates-io.type-bridge-typedb-driver-b8.path="vendor/typedb-driver-b8"'
  --config 'patch.crates-io.type-bridge-typedb-runtime.path="crates/typedb-runtime"'
  --config 'patch.crates-io.type-bridge-orm.path="crates/orm"'
  --config 'patch.crates-io.type-bridge-migration.path="crates/migration"'
  --config 'patch.crates-io.type-bridge-schema-migration-typedb.path="crates/schema-migration-typedb"'
  --config 'patch.crates-io.type-bridge-workspace.path="crates/workspace"'
  --config 'patch.crates-io.type-bridge-cli.path="crates/cli"'
  --config 'patch.crates-io.type-bridge.path="crates/rust"'
)
cargo_command=("${CARGO_BIN:-cargo}")
if [[ -n "${CARGO_TOOLCHAIN:-}" ]]; then
  [[ "$CARGO_TOOLCHAIN" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
    echo "CARGO_TOOLCHAIN must be an exact numeric Rust version" >&2
    exit 2
  }
  cargo_command+=("+$CARGO_TOOLCHAIN")
fi
for crate in "${release_crates[@]}"; do
  "${cargo_command[@]}" package \
    --locked --allow-dirty --all-features -p "$crate" "${patches[@]}"
done
for crate in "${publish_crates[@]}"; do
  bash "$helper" --preflight "$crate"
done
