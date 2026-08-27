"""Hostile validation tests for candidate-CLI-generated V6 surfaces."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path, PurePosixPath

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "workforce_v6_surfaces", CI / "workforce_v6_surfaces.py"
)
assert SPEC is not None and SPEC.loader is not None
SURFACES = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = SURFACES
SPEC.loader.exec_module(SURFACES)


def _cli_manifest() -> dict[str, str]:
    return {
        "source-commit": "1" * 40,
        "source-tree": "2" * 40,
        "candidate-id": f"sha256:{'3' * 64}",
    }


def _surface(tmp_path: Path, binding: str = "python") -> Path:
    files = {
        PurePosixPath("typebridge/__init__.py"): b"generated = True\n",
        PurePosixPath("typebridge/migration-history.json"): b"{}\n",
    }
    manifest = SURFACES.manifest_for(files, binding=binding, cli_manifest=_cli_manifest())
    path = tmp_path / f"{binding}.tar.gz"
    path.write_bytes(SURFACES.encode_surface(files, manifest))
    return path


def test_accepts_exact_canonical_surface(tmp_path: Path) -> None:
    path = _surface(tmp_path)
    manifest = SURFACES.validate_surface(path, "python")
    assert manifest["source-commit"] == "1" * 40
    assert manifest["publication-authority"] is False


def test_rejects_cross_binding_surface(tmp_path: Path) -> None:
    path = _surface(tmp_path, "python")
    with pytest.raises(SURFACES.SurfaceError) as raised:
        SURFACES.validate_surface(path, "node")
    assert raised.value.code == "surface_authority_drift"


def test_rejects_member_ledger_tampering(tmp_path: Path) -> None:
    files = {PurePosixPath("package.txt"): b"measured"}
    manifest = SURFACES.manifest_for(files, binding="rust", cli_manifest=_cli_manifest())
    manifest["members"][0]["sha256"] = "0" * 64
    path = tmp_path / "hostile.tar.gz"
    path.write_bytes(SURFACES.encode_surface(files, manifest))
    with pytest.raises(SURFACES.SurfaceError) as raised:
        SURFACES.validate_surface(path, "rust")
    assert raised.value.code == "surface_member_manifest_drift"


def test_rejects_symlink_archive_path(tmp_path: Path) -> None:
    path = _surface(tmp_path)
    link = tmp_path / "surface-link.tar.gz"
    link.symlink_to(path)
    with pytest.raises(SURFACES.SurfaceError) as raised:
        SURFACES.validate_surface(link, "python")
    assert raised.value.code == "invalid_surface_archive"
