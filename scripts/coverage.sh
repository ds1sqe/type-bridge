#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/coverage.sh [line|branch|mcdc] [--open] [--lcov]
#
# Modes:
#   line    — Line/region coverage (nightly)
#   branch  — Branch coverage (nightly, default)
#   mcdc    — MC/DC coverage (nightly, slowest)
#
# Options:
#   --open  — Generate HTML report and open in browser
#   --lcov  — Generate lcov.info (for CI / codecov upload)
#
# Default: branch coverage, HTML report to target/llvm-cov/html/
#
# LLVM bug (https://github.com/llvm/llvm-project/issues/119558):
#   llvm-cov may crash (SIGSEGV in getInstantiationGroups) when processing
#   branch/MC/DC coverage data from #[async_trait] macro expansions. If this
#   occurs, the async_trait macros need to be replaced with manual desugaring.

MODE="${1:-branch}"
case "$MODE" in
  line|branch|mcdc) shift ;;
  --*) MODE="branch" ;;  # no mode given, first arg is an option
  *) echo "Unknown mode: $MODE (expected: line, branch, mcdc)" >&2; exit 1 ;;
esac

OPEN=false
LCOV=false
for arg in "$@"; do
  case "$arg" in
    --open) OPEN=true ;;
    --lcov) LCOV=true ;;
    *) echo "Unknown option: $arg" >&2; exit 1 ;;
  esac
done

# Navigate to the Rust workspace root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$SCRIPT_DIR/../type-bridge-core"

if [ ! -f "$WORKSPACE_ROOT/Cargo.toml" ]; then
  echo "Workspace Cargo.toml not found at $WORKSPACE_ROOT/Cargo.toml" >&2
  exit 1
fi

# Prerequisites
if ! command -v cargo-llvm-cov &>/dev/null; then
  echo "cargo-llvm-cov not found. Install with:" >&2
  echo "  cargo install cargo-llvm-cov" >&2
  exit 1
fi

if ! rustup component list --toolchain nightly 2>/dev/null | grep -q 'llvm-tools.*installed'; then
  echo "Installing llvm-tools-preview for nightly..."
  rustup component add llvm-tools-preview --toolchain nightly
fi

# Package selection:
#   Only run coverage for testable crates. type-bridge-python is excluded
#   because pyo3's cdylib has linker issues under coverage instrumentation
#   and pyo3 may fail on nightly Python version detection.
PACKAGES=(
  --package type-bridge-core-lib
  --package type-bridge-server
)

# File exclusions:
#   real_driver.rs — requires live TypeDB server, always 0% in unit tests
IGNORE_REGEX="--ignore-filename-regex=real_driver\\.rs"

# Mode flag and environment
EXTRA_RUSTFLAGS="--cfg coverage_nightly"
case "$MODE" in
  line)   MODE_FLAG=() ;;
  branch) MODE_FLAG=(--branch) ;;
  mcdc)   MODE_FLAG=(); EXTRA_RUSTFLAGS="$EXTRA_RUSTFLAGS -Z coverage-options=condition" ;;
esac

echo -e "\033[1;33m==> Running $MODE coverage...\033[0m"

cd "$WORKSPACE_ROOT"

if $LCOV; then
  OUTPUT_PATH="target/llvm-cov/lcov.${MODE}.info"
  mkdir -p "$(dirname "$OUTPUT_PATH")"
  RUSTFLAGS="$EXTRA_RUSTFLAGS" cargo +nightly llvm-cov "${MODE_FLAG[@]}" \
    "${PACKAGES[@]}" "$IGNORE_REGEX" \
    --lcov --output-path "$OUTPUT_PATH"
  echo -e "\033[0;32m✓ LCOV report: $OUTPUT_PATH\033[0m"
else
  OUTPUT_DIR="target/llvm-cov/html"
  OPEN_FLAG=()
  if $OPEN; then
    OPEN_FLAG=(--open)
  fi
  RUSTFLAGS="$EXTRA_RUSTFLAGS" cargo +nightly llvm-cov "${MODE_FLAG[@]}" \
    "${PACKAGES[@]}" "$IGNORE_REGEX" \
    --html --output-dir "$OUTPUT_DIR" "${OPEN_FLAG[@]}"
  echo -e "\033[0;32m✓ HTML report: $OUTPUT_DIR/index.html\033[0m"
fi
