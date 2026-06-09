# Installation

## Requirements

- Python 3.13+
- TypeDB 3.11.5 server (for database operations)

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
