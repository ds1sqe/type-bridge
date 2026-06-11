# Installation

## Requirements

- Python 3.12–3.13
- TypeDB 3.8.0–3.11.x server (for database operations; see [compatibility table](../development/typedb.md#server-and-driver-compatibility) for the full support window)

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
uv sync

# Or with pip
pip install -e .
```

## Development Setup

For contributing to TypeBridge, install with dev dependencies:

```bash
uv sync --extra dev

# Install pre-commit hooks
pre-commit install
```

See the [Development Setup](../development/setup.md) guide for full details.
