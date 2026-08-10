#!/usr/bin/env bash
# Run one named ignored Rust test, rejecting a stale or misspelled selection
# before Cargo can report a successful zero-test invocation.
set -euo pipefail

if [[ $# -lt 2 ]]; then
    printf 'usage: %s TEST_NAME CARGO_TEST_ARGUMENT...\n' "$0" >&2
    exit 2
fi

test_name="$1"
shift
if [[ ! "$test_name" =~ ^[A-Za-z0-9_:]+$ ]]; then
    printf 'invalid Rust test name: %s\n' "$test_name" >&2
    exit 2
fi

cargo_arguments=("$@")
selection="$(
    cargo test "${cargo_arguments[@]}" "$test_name" \
        -- --ignored --exact --list
)"
printf '%s\n' "$selection"

selected_count="$(grep -Fxc -- "$test_name: test" <<<"$selection" || true)"
selected_count="${selected_count:-0}"
if [[ "$selected_count" -ne 1 ]]; then
    printf 'expected exactly one ignored Rust test named %s; selected %s\n' \
        "$test_name" "$selected_count" >&2
    exit 1
fi

exec cargo test "${cargo_arguments[@]}" "$test_name" \
    -- --ignored --exact --nocapture
