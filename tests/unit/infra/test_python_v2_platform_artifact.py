"""Behavioral gates for the target-native Python artifact consumer."""

from __future__ import annotations

import argparse
from pathlib import Path

import pytest

from scripts.ci import run_python_v2_platform_artifact as artifact_smoke


def touch_wheel(directory: Path, name: str) -> Path:
    """Create one inert wheel-shaped file for pre-install path validation."""
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / name
    path.touch()
    return path


def smoke_args(
    tmp_path: Path,
    *,
    root_dist_dir: Path,
    core_dist_dir: Path,
    source_root: Path,
    declared_schema: Path,
) -> argparse.Namespace:
    """Build the closed argument shape consumed by the smoke runner."""
    return argparse.Namespace(
        root_dist_dir=root_dist_dir,
        core_dist_dir=core_dist_dir,
        work_dir=tmp_path / "work",
        expected_version="2.0.0rc0",
        source_root=source_root,
        declared_schema=declared_schema,
    )


def test_one_wheel_requires_exactly_one_direct_regular_file(tmp_path: Path) -> None:
    wheel_dir = tmp_path / "wheels"
    expected = touch_wheel(wheel_dir, "type_bridge-2.0.0rc0-py3-none-any.whl")

    assert artifact_smoke.one_wheel(wheel_dir, "type_bridge-*.whl", label="root") == expected

    touch_wheel(wheel_dir, "type_bridge-2.0.0rc0-py2-none-any.whl")
    with pytest.raises(
        artifact_smoke.ArtifactSmokeError,
        match="Expected exactly one root",
    ):
        artifact_smoke.one_wheel(wheel_dir, "type_bridge-*.whl", label="root")


def test_one_wheel_rejects_symbolic_directory_and_candidate(tmp_path: Path) -> None:
    actual_dir = tmp_path / "actual"
    wheel = touch_wheel(actual_dir, "type_bridge-2.0.0rc0-py3-none-any.whl")
    symbolic_dir = tmp_path / "symbolic-dir"
    symbolic_dir.symlink_to(actual_dir, target_is_directory=True)

    with pytest.raises(artifact_smoke.ArtifactSmokeError, match="symbolic"):
        artifact_smoke.one_wheel(
            symbolic_dir,
            "type_bridge-*.whl",
            label="root",
        )

    symbolic_wheel_dir = tmp_path / "symbolic-wheel"
    symbolic_wheel_dir.mkdir()
    (symbolic_wheel_dir / wheel.name).symlink_to(wheel)
    with pytest.raises(
        artifact_smoke.ArtifactSmokeError,
        match="found 0",
    ):
        artifact_smoke.one_wheel(
            symbolic_wheel_dir,
            "type_bridge-*.whl",
            label="root",
        )


@pytest.mark.parametrize("symbolic_input", ["source_root", "declared_schema"])
def test_run_rejects_symbolic_source_inputs_before_environment_creation(
    tmp_path: Path,
    symbolic_input: str,
) -> None:
    root_dist = tmp_path / "root-dist"
    core_dist = tmp_path / "core-dist"
    touch_wheel(root_dist, "type_bridge-2.0.0rc0-py3-none-any.whl")
    touch_wheel(core_dist, "type_bridge_core-2.0.0rc0-cp312-abi3-linux_x86_64.whl")

    actual_source = tmp_path / "source"
    actual_source.mkdir()
    symbolic_source = tmp_path / "source-link"
    symbolic_source.symlink_to(actual_source, target_is_directory=True)

    actual_schema = tmp_path / "declared.json"
    actual_schema.write_text("{}", encoding="utf-8")
    symbolic_schema = tmp_path / "declared-link.json"
    symbolic_schema.symlink_to(actual_schema)

    args = smoke_args(
        tmp_path,
        root_dist_dir=root_dist,
        core_dist_dir=core_dist,
        source_root=(symbolic_source if symbolic_input == "source_root" else actual_source),
        declared_schema=(symbolic_schema if symbolic_input == "declared_schema" else actual_schema),
    )

    with pytest.raises(artifact_smoke.ArtifactSmokeError, match="symbolic"):
        artifact_smoke.run(args)
    assert not args.work_dir.exists()


def test_clean_environment_removes_import_overrides(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    for key in artifact_smoke.IMPORT_ENVIRONMENT_KEYS:
        monkeypatch.setenv(key, "attacker-controlled")

    environment = artifact_smoke.clean_environment()

    assert all(key not in environment for key in artifact_smoke.IMPORT_ENVIRONMENT_KEYS)
    assert environment["PYTHONNOUSERSITE"] == "1"
    assert environment["PYTHONDONTWRITEBYTECODE"] == "1"
