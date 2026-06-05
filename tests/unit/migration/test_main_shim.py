"""Unit tests verifying __main__.py is a pure subprocess shim (P3b D5).

No TypeDB connection required.  Tests assert the shim's structural properties:
- No Typer, no Command class, no MigrationExecutor imports.
- The module's source contains only bin discovery + subprocess.run + sys.exit.
"""

from __future__ import annotations

import importlib
import inspect
import sys


def test_main_shim_has_no_typer_import() -> None:
    """__main__.py must not import typer or define a Command."""
    # Force a fresh import so we see the current module source.
    module_name = "type_bridge.migration.__main__"
    if module_name in sys.modules:
        del sys.modules[module_name]

    mod = importlib.import_module(module_name)
    source = inspect.getsource(mod)

    assert "typer" not in source, "__main__.py must not import typer (shim only)"
    assert "Command" not in source, (
        "__main__.py must not define or import a Command class (shim only)"
    )
    assert "MigrationExecutor" not in source, (
        "__main__.py must not import MigrationExecutor (shim only)"
    )


def test_main_shim_imports_subprocess() -> None:
    """__main__.py must import subprocess (the shim mechanism)."""
    module_name = "type_bridge.migration.__main__"
    if module_name in sys.modules:
        del sys.modules[module_name]

    mod = importlib.import_module(module_name)
    source = inspect.getsource(mod)

    assert "subprocess" in source, "__main__.py must use subprocess to forward to the bin"
    assert "sys.exit" in source, "__main__.py must propagate the bin's exit code via sys.exit"
