"""Unit coverage for hash-bound auditwheel policy acceptance."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
AUDITOR_PATH = ROOT / "scripts/ci/audit_manylinux_release_wheels.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI helper without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


auditor = load_module("audit_manylinux_release_wheels", AUDITOR_PATH)

CORE_WHEELS = {
    "linux-x86_64": "type_bridge_core-1.5.7-cp312-abi3-manylinux_2_17_x86_64.whl",
    "linux-aarch64": "type_bridge_core-1.5.7-cp312-abi3-manylinux_2_17_aarch64.whl",
    "macos-x86_64": "type_bridge_core-1.5.7-cp312-abi3-macosx_11_0_x86_64.whl",
    "macos-arm64": "type_bridge_core-1.5.7-cp312-abi3-macosx_11_0_arm64.whl",
    "windows-x86_64": "type_bridge_core-1.5.7-cp312-abi3-win_amd64.whl",
}


def write_validated_fixture(tmp_path: Path) -> tuple[Path, Path]:
    """Write five hash-bound core candidates and their validator manifest."""
    wheel_directory = tmp_path / "wheels"
    wheel_directory.mkdir(parents=True)
    artifacts: list[dict[str, Any]] = []
    for bucket, filename in CORE_WHEELS.items():
        path = wheel_directory / filename
        path.write_bytes(f"fixture:{bucket}".encode())
        artifacts.append(
            {
                "bucket": bucket,
                "filename": filename,
                "kind": "wheel",
                "package": "type-bridge-core",
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
        )
    manifest = tmp_path / "manifest.json"
    manifest.write_text(
        json.dumps({"artifacts": artifacts, "status": "ok"}),
        encoding="utf-8",
    )
    return manifest, wheel_directory


def passing_result(policy_name: str) -> Any:
    """Return one auditwheel result satisfying the declared policy."""
    return auditor.AuditwheelResult(
        actual_policy=policy_name,
        blacklisted_symbols=(),
        declared_policy=policy_name,
        external_libraries=(),
    )


def test_audits_both_hash_bound_gnu_candidates(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    manifest, wheel_directory = write_validated_fixture(tmp_path)
    calls: list[tuple[str, str]] = []

    def fake_auditwheel(path: Path, policy_name: str) -> Any:
        calls.append((path.name, policy_name))
        return passing_result(policy_name)

    monkeypatch.setattr(auditor, "auditwheel_result", fake_auditwheel)

    report = auditor.audit_release_wheels(
        manifest_path=manifest,
        wheel_directory=wheel_directory,
    )

    assert report["status"] == "ok"
    assert {item["bucket"] for item in report["artifacts"]} == auditor.LINUX_BUCKETS
    assert calls == [
        (CORE_WHEELS["linux-aarch64"], "manylinux_2_17_aarch64"),
        (CORE_WHEELS["linux-x86_64"], "manylinux_2_17_x86_64"),
    ]


def test_missing_or_changed_gnu_candidate_fails_before_auditwheel(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    manifest, wheel_directory = write_validated_fixture(tmp_path)
    (wheel_directory / CORE_WHEELS["linux-aarch64"]).unlink()
    monkeypatch.setattr(
        auditor,
        "auditwheel_result",
        lambda path, policy_name: pytest.fail("auditwheel must not run for an incomplete set"),
    )

    with pytest.raises(auditor.AuditError, match="disagrees with validated manifest"):
        auditor.audit_release_wheels(
            manifest_path=manifest,
            wheel_directory=wheel_directory,
        )

    manifest, wheel_directory = write_validated_fixture(tmp_path / "changed")
    (wheel_directory / CORE_WHEELS["linux-x86_64"]).write_bytes(b"changed")
    with pytest.raises(auditor.AuditError, match="changed after validation"):
        auditor.audit_release_wheels(
            manifest_path=manifest,
            wheel_directory=wheel_directory,
        )


@pytest.mark.parametrize("failure", ["newer-policy", "external-library", "blacklist"])
def test_auditwheel_policy_failure_propagates(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    failure: str,
) -> None:
    manifest, wheel_directory = write_validated_fixture(tmp_path)

    def fake_auditwheel(path: Path, policy_name: str) -> Any:
        if path.name != CORE_WHEELS["linux-x86_64"]:
            return passing_result(policy_name)
        return auditor.AuditwheelResult(
            actual_policy=("manylinux_2_34_x86_64" if failure == "newer-policy" else policy_name),
            blacklisted_symbols=("libc.so.6:forbidden",) if failure == "blacklist" else (),
            declared_policy=policy_name,
            external_libraries=("libhost.so.1",) if failure == "external-library" else (),
        )

    monkeypatch.setattr(auditor, "auditwheel_result", fake_auditwheel)

    with pytest.raises(auditor.AuditError, match="claims|violates"):
        auditor.audit_release_wheels(
            manifest_path=manifest,
            wheel_directory=wheel_directory,
        )


def test_auditwheel_api_disables_grafting_and_isa_exceptions(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    calls: dict[str, Any] = {}
    policy: Any = type("Policy", (), {"name": "manylinux_2_17_x86_64"})()
    external = type("External", (), {"blacklist": {}, "libs": {}})()
    policies = type(
        "Policies",
        (),
        {"get_policy_by_name": lambda self, name: policy},
    )()
    wheel_info = type(
        "WheelInfo",
        (),
        {
            "external_refs": {policy.name: external},
            "overall_policy": policy,
            "policies": policies,
        },
    )()

    def fake_analyze(*args: Any, **kwargs: Any) -> Any:
        calls["args"] = args
        calls["kwargs"] = kwargs
        return wheel_info

    package = ModuleType("auditwheel")
    package.__path__ = []  # type: ignore[attr-defined]
    wheel_abi = ModuleType("auditwheel.wheel_abi")
    wheel_abi.analyze_wheel_abi = fake_analyze  # type: ignore[attr-defined]
    wheeltools = ModuleType("auditwheel.wheeltools")
    wheeltools.get_wheel_architecture = lambda filename: "x86_64"  # type: ignore[attr-defined]
    wheeltools.get_wheel_libc = lambda filename: "glibc"  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "auditwheel", package)
    monkeypatch.setitem(sys.modules, "auditwheel.wheel_abi", wheel_abi)
    monkeypatch.setitem(sys.modules, "auditwheel.wheeltools", wheeltools)
    monkeypatch.setattr(auditor.importlib_metadata, "version", lambda name: "6.7.0")
    wheel = tmp_path / CORE_WHEELS["linux-x86_64"]
    wheel.write_bytes(b"fixture")

    result = auditor.auditwheel_result(wheel, policy.name)

    assert result.actual_policy == policy.name
    assert calls["kwargs"] == {
        "allow_graft": False,
        "args_ldpaths": None,
        "disable_isa_ext_check": False,
    }


def test_auditwheel_api_version_is_pinned(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(auditor.importlib_metadata, "version", lambda name: "6.6.0")
    wheel = tmp_path / CORE_WHEELS["linux-x86_64"]
    wheel.write_bytes(b"fixture")

    with pytest.raises(auditor.AuditError, match="6.7.0 is required"):
        auditor.auditwheel_result(wheel, "manylinux_2_17_x86_64")
