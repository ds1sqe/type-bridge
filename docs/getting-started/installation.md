# Installation

## Requirements

- Python 3.13–3.14
- TypeDB 3.8.0–3.12.x server (for database operations; see [compatibility table](../development/typedb.md#server-and-driver-compatibility) for the full support window)

## Install from PyPI

```bash
pip install type-bridge
```

Or with [uv](https://docs.astral.sh/uv/):

```bash
uv add type-bridge
```

## Install from Source

```bash
git clone https://github.com/ds1sqe/type-bridge.git
cd type-bridge

# Install with uv (recommended)
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 uv sync

# Or with pip
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 pip install -e .
```

The variable enables the current PyO3 release's forward-compatible abi3 mode
for CPython 3.14 source builds and is harmless on 3.13. Published wheels do not
need it.

## Development Setup

For contributing to TypeBridge, install with dev dependencies:

```bash
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 uv sync --extra dev

# Install pre-commit hooks
pre-commit install
```

See the [Development Setup](../development/setup.md) guide for full details.
