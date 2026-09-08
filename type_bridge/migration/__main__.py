"""V2 workspace CLI entry point for the Python wheel."""

from __future__ import annotations

import sys


def main() -> None:
    """Run the canonical workspace CLI through the bundled native engine."""
    from type_bridge._rust_runtime import rust_core

    sys.exit(int(rust_core().run_v2_cli(["type-bridge", *sys.argv[1:]])))


if __name__ == "__main__":
    main()
