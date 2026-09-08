"""Contract tests for isolated V6 generated-surface consumers."""

from __future__ import annotations

import hashlib
import importlib.util
import sys
from pathlib import Path, PurePosixPath
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "sdk_v6_surface_consumer", CI / "validate_sdk_v6_surface_consumer.py"
)
assert SPEC is not None and SPEC.loader is not None
CONSUMER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CONSUMER
SPEC.loader.exec_module(CONSUMER)


def _surface(tmp_path: Path) -> Path:
    files = {PurePosixPath("__init__.py"): b"generated = True\n"}
    cli_manifest = {
        "source-commit": "1" * 40,
        "source-tree": "2" * 40,
        "artifact-id": f"sha256:{'3' * 64}",
    }
    manifest = CONSUMER.surfaces.manifest_for(files, binding="python", cli_manifest=cli_manifest)
    path = tmp_path / "surface.tar.gz"
    path.write_bytes(CONSUMER.surfaces.encode_surface(files, manifest))
    return path


def test_consumer_report_binds_surface_and_cleanup(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    path = _surface(tmp_path)
    monkeypatch.setattr(
        CONSUMER, "python_consumer", lambda _surface, _root: ["public-package-import"]
    )
    report = CONSUMER.consume(path, "python")
    assert report["surface-sha256"] == hashlib.sha256(path.read_bytes()).hexdigest()
    assert report["source-commit"] == "1" * 40
    assert report["cleanup"] == {"temporary-consumer-absent": True}
    assert report["publication-authority"] is False


def test_failed_compiler_is_rejected(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        CONSUMER.subprocess,
        "run",
        lambda *_args, **_kwargs: SimpleNamespace(returncode=9, stdout="measured failure"),
    )
    with pytest.raises(CONSUMER.ConsumerError) as raised:
        CONSUMER.run(["compiler"], cwd=tmp_path)
    assert raised.value.code == "surface_consumer_failed"


def test_cross_binding_archive_is_rejected(tmp_path: Path) -> None:
    with pytest.raises(CONSUMER.surfaces.SurfaceError):
        CONSUMER.consume(_surface(tmp_path), "node")


def test_python_consumer_binds_generated_surface_and_current_sdk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    environments: list[dict[str, str]] = []
    monkeypatch.setattr(CONSUMER, "extract_surface", lambda *_args: {})
    monkeypatch.setattr(
        CONSUMER,
        "run",
        lambda _command, **kwargs: environments.append(kwargs["env"]),
    )
    assert CONSUMER.python_consumer(tmp_path / "surface.tar.gz", tmp_path) == [
        "public-package-import",
        "bytecode-compile",
    ]
    assert environments
    assert environments[0]["PYTHONPATH"].split(CONSUMER.os.pathsep) == [
        str(tmp_path),
        str(ROOT),
        str(CONSUMER.PYTHON_CORE),
    ]


def test_rust_consumer_stages_frozen_dependency_graph(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []

    def extract(_surface: Path, package: Path) -> dict[str, object]:
        package.mkdir()
        return {}

    monkeypatch.setattr(CONSUMER, "extract_surface", extract)
    monkeypatch.setattr(
        CONSUMER,
        "run",
        lambda command, **_kwargs: commands.append(command),
    )

    assert CONSUMER.rust_consumer(tmp_path / "surface.tar.gz", tmp_path) == [
        "offline-cargo-check",
        "all-targets",
    ]
    lock = (tmp_path / "rust/Cargo.lock").read_text(encoding="utf-8")
    assert lock == CONSUMER.RUST_CONSUMER_LOCK.read_text(encoding="utf-8")
    assert lock.count('name = "type-bridge-generated-schema"') == 1
    assert 'name = "tinyvec"\nversion = "1.12.0"' in lock
    assert 'name = "tinyvec"\nversion = "1.13.0"' not in lock
    assert commands == [
        [
            "cargo",
            "check",
            "--locked",
            "--offline",
            "--manifest-path",
            str(tmp_path / "rust/Cargo.toml"),
            "--all-targets",
        ]
    ]
