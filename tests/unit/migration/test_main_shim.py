"""Unit tests verifying __main__.py stays a thin V2 native CLI dispatcher.

No TypeDB connection required. Tests assert the dispatcher's structure:
- No Typer, no Command class, no MigrationExecutor imports.
- The canonical workspace parser comes from the required native extension.
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


def test_main_shim_uses_only_the_v2_native_runner_without_an_external_binary() -> None:
    """The wheel entry point must not depend on a separately installed bin."""
    module_name = "type_bridge.migration.__main__"
    if module_name in sys.modules:
        del sys.modules[module_name]

    mod = importlib.import_module(module_name)
    source = inspect.getsource(mod)

    assert "subprocess" not in source
    assert "shutil" not in source
    assert "_find_bin" not in source
    assert "run_legacy_migration_cli" not in source
    assert "run_v2_cli" in source
    assert "sys.exit" in source, "__main__.py must propagate the native exit code"
