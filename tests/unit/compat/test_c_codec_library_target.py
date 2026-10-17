"""C codec acceptance must link the library its Cargo invocation builds."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
PROVIDER = ROOT / "tests/integration/schema/test_sdk_serialization_c_codec.py"


@pytest.mark.parametrize("configured", [None, "relative-target", "/absolute-target"])
def test_library_directory_follows_cargo_working_directory(
    monkeypatch: pytest.MonkeyPatch, configured: str | None
) -> None:
    spec = importlib.util.spec_from_file_location("c_codec_provider", PROVIDER)
    assert spec is not None and spec.loader is not None
    provider = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(provider)
    if configured is None:
        monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
        target = provider.CORE / "target"
    else:
        monkeypatch.setenv("CARGO_TARGET_DIR", configured)
        target = provider.CORE / configured
    assert provider.library_directory() == (target / "debug").resolve()
