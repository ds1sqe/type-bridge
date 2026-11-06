#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
case "${1:---check}" in
  --check) unset TYPE_BRIDGE_UPDATE_PLAN_FIXTURES ;;
  --write) export TYPE_BRIDGE_UPDATE_PLAN_FIXTURES=1 ;;
  *) echo 'usage: check-plan-v3-fixtures.sh [--check|--write]' >&2; exit 2 ;;
esac
exec cargo test --manifest-path type-bridge-core/Cargo.toml \
  -p type-bridge-contract --test query_plan shared_plan_fixtures -- --exact
