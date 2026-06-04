from __future__ import annotations

# pyright: reportMissingImports=false
import pytest

from type_bridge._backend import BACKEND_ENV_VAR, manager_class, selected_backend
from type_bridge.crud import TypeDBManager


def test_backend_defaults_to_rust(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv(BACKEND_ENV_VAR, raising=False)

    from type_bridge.crud.rust_manager import RustTypeDBManager

    assert selected_backend() == "rust"
    assert manager_class(TypeDBManager) is RustTypeDBManager


def test_backend_selects_python_manager(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(BACKEND_ENV_VAR, "python")

    assert selected_backend() == "python"
    assert manager_class(TypeDBManager) is TypeDBManager


def test_backend_selects_rust_manager(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(BACKEND_ENV_VAR, "rust")

    from type_bridge.crud.rust_manager import RustTypeDBManager

    assert selected_backend() == "rust"
    assert manager_class(TypeDBManager) is RustTypeDBManager


def test_backend_rejects_unknown_value(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv(BACKEND_ENV_VAR, "sqlite")

    with pytest.raises(ValueError, match="TYPE_BRIDGE_BACKEND"):
        selected_backend()
