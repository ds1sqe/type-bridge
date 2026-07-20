"""CLI entry point: V2 workspace verbs in-process, legacy verbs forwarded.

V2 invocations (``schema ...``, ``migration ...``, ``--manifest``,
``--help``, ``--version``) run the workspace CLI shipped inside the
native extension — no external binary is involved. Everything else keeps
the released 1.5 contract and is forwarded verbatim to the
``type-bridge-migration`` binary:

    python -m type_bridge.migration migrate --database mydb
    python -m type_bridge.migration showmigrations --database mydb
    python -m type_bridge.migration makemigrations --name add_phone --models myapp.models
    python -m type_bridge.migration plan
    python -m type_bridge.migration sqlmigrate 0001_initial

Exit codes are propagated unchanged on both paths.

Legacy binary discovery order:
  1. Sibling directory of ``sys.executable`` (venv ``bin/``).
  2. ``shutil.which("type-bridge-migration")`` (PATH).
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path


def _find_bin() -> Path | None:
    """Locate the ``type-bridge-migration`` binary.

    Resolution:
    1. Sibling ``bin/`` directory of the current Python interpreter (covers the
       maturin venv-install case where the Rust binary lands next to ``python``).
    2. ``shutil.which`` PATH fallback.
    """
    venv_bin = Path(sys.executable).parent / "type-bridge-migration"
    if venv_bin.exists():
        return venv_bin

    which = shutil.which("type-bridge-migration")
    if which:
        return Path(which)

    return None


#: First arguments that select the V2 workspace CLI shipped inside the
#: native extension. The legacy binary has no ``schema``/``migration``
#: verbs and no ``--manifest`` flag, so dispatch is unambiguous.
_V2_LEADING_ARGUMENTS = ("schema", "migration", "--manifest", "--help", "--version")


def _is_v2_invocation(argv: list[str]) -> bool:
    """Return whether the invocation targets the V2 workspace CLI."""
    if not argv:
        return False
    first = argv[0]
    return first in _V2_LEADING_ARGUMENTS or first.startswith("--manifest=")


def main() -> None:
    """Dispatch V2 verbs in-process; forward legacy verbs to the binary."""
    if _is_v2_invocation(sys.argv[1:]):
        from type_bridge._rust_runtime import rust_core

        sys.exit(int(rust_core().run_v2_cli(["type-bridge", *sys.argv[1:]])))

    bin_path = _find_bin()
    if bin_path is None:
        print(
            "error: type-bridge-migration binary not found.\n"
            "Make sure the package is installed (maturin develop / pip install) "
            "and the binary is on PATH.",
            file=sys.stderr,
        )
        sys.exit(1)

    result = subprocess.run([str(bin_path), *sys.argv[1:]])
    sys.exit(result.returncode)


if __name__ == "__main__":
    main()
