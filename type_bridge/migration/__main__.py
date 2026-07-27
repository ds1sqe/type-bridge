"""CLI entry point for the wheel's V2 and released V1 migration verbs.

V2 invocations (``schema ...``, ``migration ...``, ``--manifest``, or
``--manifest=...``) run the workspace CLI shipped inside the native extension
using the V2 parser. Everything else, including the global ``--help``/``-h``
and ``--version``/``-V`` forms, runs the released 1.5 parser and command runner
from that same extension:

    python -m type_bridge.migration migrate --database mydb
    python -m type_bridge.migration showmigrations --database mydb
    python -m type_bridge.migration makemigrations --name add_phone --models myapp.models
    python -m type_bridge.migration plan
    python -m type_bridge.migration sqlmigrate 0001_initial

Both paths preserve their standalone stdout, stderr, and exit-code contracts.
No separately installed or source-built helper binary is required.
"""

from __future__ import annotations

import sys

#: First arguments that select the V2 workspace CLI shipped inside the
#: native extension. The legacy binary has no ``schema``/``migration``
#: verbs and no ``--manifest`` flag, so dispatch is unambiguous.
_V2_LEADING_ARGUMENTS = ("schema", "migration", "--manifest")


def _is_v2_invocation(argv: list[str]) -> bool:
    """Return whether the invocation targets the V2 workspace CLI."""
    if not argv:
        return False
    first = argv[0]
    return first in _V2_LEADING_ARGUMENTS or first.startswith("--manifest=")


def main() -> None:
    """Dispatch both parsers through the required native wheel."""
    from type_bridge._rust_runtime import rust_core

    if _is_v2_invocation(sys.argv[1:]):
        sys.exit(int(rust_core().run_v2_cli(["type-bridge", *sys.argv[1:]])))

    sys.exit(int(rust_core().run_legacy_migration_cli(["type-bridge-migration", *sys.argv[1:]])))


if __name__ == "__main__":
    main()
