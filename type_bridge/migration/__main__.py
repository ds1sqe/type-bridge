"""Subprocess shim — forwards all migration commands to the type-bridge-migration binary.

Usage (unchanged from the caller's perspective):
    python -m type_bridge.migration migrate --database mydb
    python -m type_bridge.migration showmigrations --database mydb
    python -m type_bridge.migration makemigrations --name add_phone --models myapp.models
    python -m type_bridge.migration plan
    python -m type_bridge.migration sqlmigrate 0001_initial

All arguments are forwarded verbatim to the ``type-bridge-migration`` binary.
Exit code is propagated unchanged.

Binary discovery order:
  1. Sibling directory of ``sys.executable`` (venv ``bin/``).
  2. ``shutil.which("type-bridge-migration")`` (PATH).

The module is intentionally thin: it only discovers and invokes the binary so
that all command logic lives in the Rust CLI.
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


def main() -> None:
    """Discover the binary, forward argv, and exit with the binary's exit code."""
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
