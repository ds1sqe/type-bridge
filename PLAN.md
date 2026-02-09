# Plan: CI-Built Wheels for type-bridge-core

## Problem

`type-bridge-core` is a hard dependency but it's a native Rust/PyO3 extension. Without pre-built wheels, every user needs a Rust toolchain to install `type-bridge`. CI doesn't build or test the Rust crate at all.

## Changes

### 1. `.github/workflows/ci.yml` — Add Rust crate jobs

Add two new jobs to the existing CI workflow:

**`rust-check`** (fast, parallel with existing jobs):

- Runs `cargo check` and `cargo test` on the Rust crate
- Matrix: `ubuntu-latest`, `macos-latest`, `windows-latest`
- Ensures the crate compiles and tests pass on all platforms

**`build-wheels`** (depends on `rust-check`):

- Uses `PyO3/maturin-action@v1` to build wheels
- Matrix:
  - Linux: x86_64, aarch64 (via QEMU for arm cross-compilation)
  - macOS: x86_64, aarch64 (universal2)
  - Windows: x86_64
- Python target: 3.13
- Uploads wheels as artifacts (for release workflow to consume)

### 2. `.github/workflows/release.yml` — Publish wheels to PyPI

Modify the existing release workflow to:

- Build `type-bridge-core` wheels on all platforms (same maturin matrix as CI)
- Build the pure-Python `type-bridge` sdist/wheel
- Publish both packages to PyPI using trusted publishing
- Source distribution (sdist) included as fallback for exotic platforms (requires Rust toolchain)

### 3. Existing CI jobs — Install Rust crate before Python tests

Update `test-unit`, `typecheck`, and `test-integration` jobs to:

- Install Rust toolchain (`dtolnay/rust-toolchain@stable`)
- Build `type-bridge-core` so the Rust path is exercised in CI tests

## Verification

- Push branch, confirm CI builds wheels on all 5 platform/arch combos
- Confirm `cargo test` passes on linux/mac/windows runners
- Confirm existing Python unit + integration tests still pass with Rust core active
