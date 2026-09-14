#!/usr/bin/env bash
# Audit workspace and retained consumer lockfiles, including development inputs.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

audit_version="$(cargo audit --version)"
case "$audit_version" in
    'cargo-audit 0.22.2' | 'cargo-audit-audit 0.22.2') ;;
    *)
        printf 'Expected cargo-audit 0.22.2, got: %s\n' "$audit_version" >&2
        exit 1
        ;;
esac

# Do not filter advisories, targets, severity, or yanked crates, or allow a stale
# database. rustls-pemfile's informational unmaintained notice remains visible;
# the pinned TypeDB transport graph still requires it (see DEVELOPMENT.md).
for lockfile in \
    type-bridge-core/Cargo.lock \
    type-bridge-core/crates/core/tests/fixtures/rule-wire-standalone/Cargo.lock \
    tests/contracts/sdk-v6-rust-surface-Cargo.lock \
    tests/support/provider/Cargo.lock \
    tests/support/rust-live/Cargo.lock \
    tests/support/rust-offline/Cargo.lock \
    type-bridge-core/crates/schema-codegen/tests/rust_projection_live/consumer-Cargo.lock
do
    cargo audit --file "$lockfile" --deny unsound --deny yanked
done
