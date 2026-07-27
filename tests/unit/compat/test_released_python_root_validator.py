"""Hostile coverage for the immutable published-root compatibility authority."""

from __future__ import annotations

import hashlib
import importlib.util
import sys
import zipfile
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_released_python_root.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI validator without creating a scripts package API."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


validator = load_module("validate_released_python_root", VALIDATOR_PATH)


def write_wheel(
    tmp_path: Path,
    *,
    metadata_version: str = "1.5.11",
    core_requirements: tuple[str, ...] = ("type-bridge-core>=1.5.11",),
    purelib: str = "true",
    tag: str = "py3-none-any",
) -> Path:
    """Write a minimal released-root wheel fixture."""
    path = tmp_path / "type_bridge-1.5.11-py3-none-any.whl"
    requirements = "".join(f"Requires-Dist: {requirement}\n" for requirement in core_requirements)
    metadata = (
        "Metadata-Version: 2.4\n"
        "Name: type-bridge\n"
        f"Version: {metadata_version}\n"
        "Requires-Python: <3.15,>=3.12\n"
        f"{requirements}\n"
    ).encode()
    wheel_metadata = (
        f"Wheel-Version: 1.0\nGenerator: hostile-test\nRoot-Is-Purelib: {purelib}\nTag: {tag}\n\n"
    ).encode()
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("type_bridge-1.5.11.dist-info/METADATA", metadata)
        archive.writestr("type_bridge-1.5.11.dist-info/WHEEL", wheel_metadata)
    return path


def validate_fixture(path: Path) -> dict[str, object]:
    """Validate a synthetic wheel while retaining every semantic production check."""
    body = path.read_bytes()
    return validator.validate_released_root_wheel(
        path,
        expected_size=len(body),
        expected_sha256=hashlib.sha256(body).hexdigest(),
    )


def pypi_authority_payload() -> dict[str, Any]:
    """Return the minimal current PyPI project authority."""
    return {
        "info": {"version": "1.5.11"},
        "releases": {
            "1.5.11": [
                {
                    "filename": "type_bridge-1.5.11-py3-none-any.whl",
                    "digests": {
                        "sha256": (
                            "f2e5ac0a59488f18d294295a2d08ab82b57f750d816e485b83273292d37a9d41"
                        )
                    },
                    "packagetype": "bdist_wheel",
                    "python_version": "py3",
                    "size": 286_440,
                    "url": (
                        "https://files.pythonhosted.org/packages/frozen/"
                        "type_bridge-1.5.11-py3-none-any.whl"
                    ),
                    "yanked": False,
                }
            ]
        },
    }


def test_current_published_root_authority_is_exact() -> None:
    assert validator.RELEASED_ROOT_VERSION == "1.5.11"
    assert validator.RELEASED_ROOT_FILENAME == "type_bridge-1.5.11-py3-none-any.whl"
    assert (
        validator.RELEASED_ROOT_SHA256
        == "f2e5ac0a59488f18d294295a2d08ab82b57f750d816e485b83273292d37a9d41"
    )
    assert validator.RELEASED_ROOT_SIZE == 286_440
    assert validator.RELEASED_CORE_REQUIREMENT == "type-bridge-core>=1.5.11"


def test_current_pypi_authority_is_accepted() -> None:
    report = validator.validate_pypi_authority(pypi_authority_payload())

    assert report == {
        "project_latest_version": "1.5.11",
        "released_root_version": "1.5.11",
        "missing_durable_tag_files": "1.5.7",
        "status": "ok",
        "wheel": "type_bridge-1.5.11-py3-none-any.whl",
    }


def test_later_major_release_does_not_break_frozen_root_authority() -> None:
    payload = pypi_authority_payload()
    payload["info"]["version"] = "2.0.0"

    report = validator.validate_pypi_authority(payload)

    assert report["project_latest_version"] == "2.0.0"
    assert report["released_root_version"] == "1.5.11"


@pytest.mark.parametrize("latest_version", (None, ""))
def test_pypi_authority_requires_a_well_formed_project_latest_version(
    latest_version: object,
) -> None:
    payload = pypi_authority_payload()
    payload["info"]["version"] = latest_version

    with pytest.raises(validator.ValidationError, match="project latest version"):
        validator.validate_pypi_authority(payload)


def test_newly_published_durable_tag_forces_authority_review() -> None:
    payload = pypi_authority_payload()
    payload["releases"]["1.5.7"] = [{"filename": "unexpected.whl"}]

    with pytest.raises(validator.ValidationError, match="now exposes 1.5.7"):
        validator.validate_pypi_authority(payload)


def test_pypi_wheel_record_cannot_drift_from_frozen_bytes() -> None:
    payload = pypi_authority_payload()
    payload["releases"]["1.5.11"][0]["digests"]["sha256"] = "0" * 64

    with pytest.raises(validator.ValidationError, match="wheel record disagrees"):
        validator.validate_pypi_authority(payload)


def test_valid_published_root_contract_is_accepted(tmp_path: Path) -> None:
    report = validate_fixture(write_wheel(tmp_path))

    assert report["status"] == "ok"
    assert report["version"] == "1.5.11"
    assert report["core_requirement"] == "type-bridge-core>=1.5.11"


@pytest.mark.parametrize(
    "core_requirements",
    (
        ("type-bridge-core>=1.5.11,<2",),
        ("TYPE_BRIDGE_CORE>=1.5.11",),
        ("type-bridge-core>=1.5.11; python_version >= '3.12'",),
        ("type-bridge-core>=1.5.11", "type.bridge.core>=1.5.11"),
        (),
    ),
)
def test_published_root_core_requirement_cannot_be_repaired_or_duplicated(
    tmp_path: Path,
    core_requirements: tuple[str, ...],
) -> None:
    wheel = write_wheel(tmp_path, core_requirements=core_requirements)

    with pytest.raises(validator.ValidationError, match="published unbounded core requirement"):
        validate_fixture(wheel)


def test_published_root_metadata_version_cannot_drift(tmp_path: Path) -> None:
    wheel = write_wheel(tmp_path, metadata_version="1.5.10")

    with pytest.raises(validator.ValidationError, match="metadata version drifted"):
        validate_fixture(wheel)


@pytest.mark.parametrize(
    ("purelib", "tag", "message"),
    (("false", "py3-none-any", "pure Python"), ("true", "cp312-abi3-manylinux", "tag")),
)
def test_published_root_must_remain_cross_interpreter_pure_python(
    tmp_path: Path,
    purelib: str,
    tag: str,
    message: str,
) -> None:
    wheel = write_wheel(tmp_path, purelib=purelib, tag=tag)

    with pytest.raises(validator.ValidationError, match=message):
        validate_fixture(wheel)


def test_published_root_hash_and_size_are_immutable(tmp_path: Path) -> None:
    wheel = write_wheel(tmp_path)
    body = wheel.read_bytes()

    with pytest.raises(validator.ValidationError, match="size disagrees"):
        validator.validate_released_root_wheel(
            wheel,
            expected_size=len(body) + 1,
            expected_sha256=hashlib.sha256(body).hexdigest(),
        )
    with pytest.raises(validator.ValidationError, match="SHA-256 disagrees"):
        validator.validate_released_root_wheel(
            wheel,
            expected_size=len(body),
            expected_sha256="0" * 64,
        )
