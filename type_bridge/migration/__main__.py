"""Workspace CLI entry point for the Python wheel."""

from __future__ import annotations

import sys


def main() -> None:
    """Run the canonical workspace CLI through the bundled native engine."""
    from type_bridge._native import core_module

    sys.exit(int(core_module().run_cli(["type-bridge", *sys.argv[1:]])))


if __name__ == "__main__":
    main()
