"""Unit coverage for exact Python release artifact validation."""

from __future__ import annotations

import base64
import csv
import hashlib
import importlib.util
import io
import json
import sys
import tarfile
import zipfile
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_python_release_artifacts.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load a script module without adding scripts/ci to the package surface."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


validator = load_module("validate_python_release_artifacts", VALIDATOR_PATH)
VERSION = "2.0.0rc0"
SPECS = validator.load_package_specs(ROOT, VERSION)
CORE_PLATFORMS = {
    "linux-x86_64": "manylinux_2_17_x86_64",
    "linux-aarch64": "manylinux_2_17_aarch64",
    "macos-x86_64": "macosx_11_0_x86_64",
    "macos-arm64": "macosx_11_0_arm64",
    "windows-x86_64": "win_amd64",
}


def copy_root_python_contract(tmp_path: Path) -> tuple[Path, Path]:
    """Copy only the authorities read before artifact inventory discovery."""
    manifest = tmp_path / "pyproject.toml"
    package_init = tmp_path / "type_bridge/__init__.py"
    package_init.parent.mkdir(parents=True, exist_ok=True)
    manifest.write_bytes((ROOT / "pyproject.toml").read_bytes())
    package_init.write_bytes((ROOT / "type_bridge/__init__.py").read_bytes())
    return manifest, package_init


def metadata_bytes(
    spec: Any,
    *,
    dependencies: tuple[str, ...] | None = None,
    extras: tuple[str, ...] | None = None,
    license_value: str | None = validator.MIT_LICENSE,
    license_expression: str | None = None,
    license_files: tuple[str, ...] | None = None,
    version: str | None = None,
) -> bytes:
    """Return minimal valid core metadata for a synthetic distribution."""
    lines = [
        "Metadata-Version: 2.4",
        f"Name: {spec.name}",
        f"Version: {version or spec.version}",
    ]
    if license_value is not None:
        lines.append(f"License: {license_value}")
    if license_expression is not None:
        lines.append(f"License-Expression: {license_expression}")
    selected_license_files = spec.license_files if license_files is None else license_files
    lines.extend(f"License-File: {name}" for name in selected_license_files)
    lines.append(f"Requires-Python: {spec.requires_python}")
    expected_extras, expected_dependencies = validator.dependency_metadata(spec)
    selected_extras = expected_extras if extras is None else extras
    lines.extend(f"Provides-Extra: {extra}" for extra in selected_extras)
    selected_dependencies = expected_dependencies if dependencies is None else dependencies
    lines.extend(f"Requires-Dist: {dependency}" for dependency in selected_dependencies)
    return ("\n".join(lines) + "\n").encode()


def record_bytes(members: dict[str, bytes], record_name: str) -> bytes:
    """Build a hash-complete wheel RECORD for the supplied members."""
    stream = io.StringIO()
    writer = csv.writer(stream, lineterminator="\n")
    for name, payload in members.items():
        digest = base64.urlsafe_b64encode(hashlib.sha256(payload).digest()).rstrip(b"=").decode()
        writer.writerow((name, f"sha256={digest}", str(len(payload))))
    writer.writerow((record_name, "", ""))
    return stream.getvalue().encode()


def native_binary(platform: str, *, glibc_version: str | None = None) -> bytes:
    """Return a minimal architecture-bearing native executable header."""
    if "linux" in platform:
        header = bytearray(64)
        header[:6] = b"\x7fELF\x02\x01"
        machine = 183 if platform.endswith("_aarch64") else 62
        header[18:20] = machine.to_bytes(2, "little")
        payload = bytes(header)
        if glibc_version is not None:
            payload += f"\0GLIBC_{glibc_version}\0".encode()
        return payload + b"\0PyInit_type_bridge_core\0"
    if platform.startswith("macosx"):
        cpu = 0x0100000C if platform.endswith("_arm64") else 0x01000007
        return (
            b"\xcf\xfa\xed\xfe"
            + cpu.to_bytes(4, "little")
            + bytes(56)
            + b"\0PyInit_type_bridge_core\0"
        )
    if platform == "win_amd64":
        header = bytearray(256)
        header[:2] = b"MZ"
        header[60:64] = (128).to_bytes(4, "little")
        header[128:132] = b"PE\0\0"
        header[132:134] = (0x8664).to_bytes(2, "little")
        return bytes(header) + b"\0PyInit_type_bridge_core\0"
    raise AssertionError(f"unsupported fixture platform: {platform}")


def write_wheel(
    directory: Path,
    spec: Any,
    *,
    metadata_dependencies: tuple[str, ...] | None = None,
    metadata_extras: tuple[str, ...] | None = None,
    metadata_license: str | None = validator.MIT_LICENSE,
    metadata_license_expression: str | None = None,
    metadata_license_files: tuple[str, ...] | None = None,
    native_platform: str | None = None,
    platform: str,
    metadata_version: str | None = None,
    omit_entry_points: bool = False,
    omit_member: str | None = None,
    script_target: str | None = None,
    tamper_record: bool = False,
    bytecode: bool = False,
    python_tag_override: str | None = None,
    abi_tag_override: str | None = None,
    extra_members: dict[str, bytes] | None = None,
    extra_entry_points: dict[str, dict[str, str]] | None = None,
    glibc_version: str | None = None,
    native_name: str | None = None,
    purelib_value: str | None = None,
) -> Path:
    """Write a structurally realistic pure or abi3 wheel."""
    directory.mkdir(parents=True, exist_ok=True)
    if spec.pure:
        python_tag, abi_tag, platform_tag = "py3", "none", "any"
    else:
        python_tag, abi_tag, platform_tag = "cp312", "abi3", platform
    python_tag = python_tag_override or python_tag
    abi_tag = abi_tag_override or abi_tag
    filename = f"{spec.distribution}-{spec.version}-{python_tag}-{abi_tag}-{platform_tag}.whl"
    dist_info = f"{spec.distribution}-{spec.version}.dist-info"
    members = {name: b"# fixture\n" for name in spec.wheel_members}
    if spec.distribution_notice is not None:
        members[validator.CORE_WHEEL_NOTICE] = spec.distribution_notice
    if spec.distribution_license is not None:
        dist_info = f"{spec.distribution}-{spec.version}.dist-info"
        for license_file in spec.license_files:
            members[f"{dist_info}/licenses/{license_file}"] = spec.distribution_license
    if omit_member is not None:
        members.pop(omit_member)
    if not spec.pure:
        default_native_name = (
            "type_bridge_core/type_bridge_core.pyd"
            if platform == "win_amd64"
            else "type_bridge_core/type_bridge_core.abi3.so"
        )
        members[native_name or default_native_name] = native_binary(
            native_platform or platform,
            glibc_version=glibc_version,
        )
    if bytecode:
        members[f"{spec.distribution}/__pycache__/bad.pyc"] = b"bytecode"
    members[f"{dist_info}/METADATA"] = metadata_bytes(
        spec,
        dependencies=metadata_dependencies,
        extras=metadata_extras,
        license_value=metadata_license,
        license_expression=metadata_license_expression,
        license_files=metadata_license_files,
        version=metadata_version,
    )
    members[f"{dist_info}/WHEEL"] = (
        "Wheel-Version: 1.0\n"
        f"Root-Is-Purelib: {purelib_value or str(spec.pure).lower()}\n"
        f"Tag: {python_tag}-{abi_tag}-{platform_tag}\n"
    ).encode()
    if (spec.entry_points or extra_entry_points) and not omit_entry_points:
        entry_points = {group: dict(entries) for group, entries in spec.entry_points}
        for group, entries in (extra_entry_points or {}).items():
            entry_points[group] = entries
        scripts = entry_points.get("console_scripts", {})
        if script_target is not None:
            scripts["type-bridge"] = script_target
        members[f"{dist_info}/entry_points.txt"] = (
            "".join(
                f"[{group}]\n"
                + "".join(f"{name} = {target}\n" for name, target in sorted(entries.items()))
                for group, entries in sorted(entry_points.items())
            )
        ).encode()
    members.update(extra_members or {})
    record_name = f"{dist_info}/RECORD"
    record = record_bytes(members, record_name)
    if tamper_record:
        first = next(iter(members))
        members[first] += b"changed-after-record"
    members[record_name] = record
    path = directory / filename
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for name, payload in members.items():
            archive.writestr(name, payload)
    return path


def write_sdist(
    directory: Path,
    spec: Any,
    *,
    duplicate_member: bool = False,
    omit_member: str | None = None,
    omit_optional_dependency: bool = False,
    script_target: str | None = None,
    unsafe_member: bool = False,
    symlink_target: str | None = None,
    extra_entry_points: dict[str, dict[str, str]] | None = None,
    extra_members: dict[str, bytes] | None = None,
    metadata_license: str | None = validator.MIT_LICENSE,
    metadata_license_files: tuple[str, ...] | None = None,
    project_license: str | None = validator.MIT_LICENSE,
) -> Path:
    """Write a minimal sdist with repository and PKG-INFO metadata."""
    directory.mkdir(parents=True, exist_ok=True)
    root = f"{spec.distribution}-{spec.version}"
    if spec.key == "core":
        authorities = validator.core_sdist_source_authorities(ROOT)
        members = {name: authority.read_bytes() for name, authority in authorities.items()}
        root_manifest = members["Cargo.toml"].decode()
        members_start = root_manifest.index("members = [")
        members_end = root_manifest.index("\n]", members_start) + 2
        rendered_members = (
            "members = [\n"
            + "".join(f'    "{name}",\n' for name in validator.CORE_SDIST_WORKSPACE_MEMBERS)
            + "]"
        )
        members["Cargo.toml"] = (
            root_manifest[:members_start] + rendered_members + root_manifest[members_end:]
        ).encode()
        for manifest_name in validator.CORE_SDIST_TRANSFORMED_FIRST_PARTY_MANIFESTS:
            manifest = members[manifest_name].decode()
            assert "license-file.workspace = true" in manifest
            members[manifest_name] = manifest.replace(
                "license-file.workspace = true",
                'license-file = "LICENSE"',
                1,
            ).encode()
        for manifest_name in validator.CORE_SDIST_README_TRANSFORMS:
            manifest = members[manifest_name].decode()
            members[manifest_name] = manifest.replace(
                "[package]\n",
                '[package]\nreadme = "README.md"\n',
                1,
            ).encode()
    else:
        members = {name: b"# fixture\n" for name in spec.sdist_members}
        assert spec.distribution_license is not None
        members[validator.ROOT_LICENSE_FILE] = spec.distribution_license
    if spec.distribution_notice is not None:
        members[validator.CORE_SDIST_NOTICE] = spec.distribution_notice
    members["PKG-INFO"] = metadata_bytes(
        spec,
        license_value=metadata_license,
        license_files=metadata_license_files,
    )
    if spec.key == "root":
        dependencies = ", ".join(json.dumps(value) for value in spec.dependencies)
        pyproject = (
            "[project]\n"
            f'name = "{spec.name}"\n'
            f'version = "{spec.version}"\n'
            f'requires-python = "{spec.requires_python}"\n'
            f"dependencies = [{dependencies}]\n"
        )
        if project_license is not None:
            pyproject += f'license = {{ text = "{project_license}" }}\n'
        if spec.scripts:
            pyproject += "\n[project.scripts]\n"
            for name, target in spec.scripts:
                if script_target is not None and name == "type-bridge":
                    target = script_target
                pyproject += f"{json.dumps(name)} = {json.dumps(target)}\n"
        for group, entries in sorted((extra_entry_points or {}).items()):
            pyproject += f"\n[project.entry-points.{json.dumps(group)}]\n"
            for name, target in sorted(entries.items()):
                pyproject += f"{json.dumps(name)} = {json.dumps(target)}\n"
        if spec.optional_dependencies:
            pyproject += "\n[project.optional-dependencies]\n"
            for extra, values in spec.optional_dependencies:
                selected = values[:-1] if omit_optional_dependency and values else values
                rendered = ", ".join(json.dumps(value) for value in selected)
                pyproject += f"{json.dumps(extra)} = [{rendered}]\n"
        members["pyproject.toml"] = pyproject.encode()
    members.update(extra_members or {})
    if omit_member is not None:
        members.pop(omit_member)
    path = directory / f"{root}.tar.gz"
    with tarfile.open(path, "w:gz") as archive:
        for name, payload in members.items():
            info = tarfile.TarInfo(f"{root}/{name}")
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
        if unsafe_member:
            payload = b"escape"
            info = tarfile.TarInfo(f"{root}/../escape")
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
        if duplicate_member:
            payload = metadata_bytes(spec)
            info = tarfile.TarInfo(f"{root}/PKG-INFO")
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
        if symlink_target is not None:
            target = b"# target\n"
            info = tarfile.TarInfo(f"{root}/CLAUDE.md")
            info.size = len(target)
            archive.addfile(info, io.BytesIO(target))
            info = tarfile.TarInfo(f"{root}/AGENTS.md")
            info.type = tarfile.SYMTYPE
            info.linkname = symlink_target
            archive.addfile(info)
    return path


def write_release_set(tmp_path: Path) -> tuple[Path, Path, Path]:
    """Write all eight artifacts in the supported release matrix."""
    core_wheels = tmp_path / "core-wheels"
    core_sdist = tmp_path / "core-sdist"
    root_dist = tmp_path / "root-dist"
    for platform in CORE_PLATFORMS.values():
        write_wheel(core_wheels, SPECS["core"], platform=platform)
    write_sdist(core_sdist, SPECS["core"])
    write_wheel(root_dist, SPECS["root"], platform="any")
    write_sdist(root_dist, SPECS["root"])
    return core_wheels, core_sdist, root_dist


def validate(directories: tuple[Path, Path, Path]) -> dict[str, Any]:
    """Run the release-set validator against a synthetic directory triple."""
    core_wheels, core_sdist, root_dist = directories
    return validator.validate_release_set(
        core_wheels_dir=core_wheels,
        core_sdist_dir=core_sdist,
        root_dist_dir=root_dist,
        expected_version=VERSION,
        repository_root=ROOT,
    )


def test_exact_release_set_accepts_all_platforms_and_both_sdists(tmp_path: Path) -> None:
    report = validate(write_release_set(tmp_path))

    assert report["status"] == "ok"
    assert len(report["artifacts"]) == 8
    assert {item["bucket"] for item in report["artifacts"]} == {
        *CORE_PLATFORMS,
        "source",
        "universal",
    }
    assert all(len(item["sha256"]) == 64 for item in report["artifacts"])


def test_release_set_rejects_missing_platform_and_unexpected_file(tmp_path: Path) -> None:
    directories = write_release_set(tmp_path)
    next(directories[0].glob("*aarch64.whl")).unlink()
    with pytest.raises(validator.ValidationError, match="five core wheels"):
        validate(directories)

    directories = write_release_set(tmp_path / "extra")
    (directories[2] / "checksums.txt").write_text("unexpected", encoding="utf-8")
    with pytest.raises(validator.ValidationError, match="unexpected files"):
        validate(directories)


def test_wheel_rejects_record_tampering_and_bytecode(tmp_path: Path) -> None:
    tampered = write_wheel(
        tmp_path,
        SPECS["root"],
        platform="any",
        tamper_record=True,
    )
    with pytest.raises(validator.ValidationError, match="RECORD digest/size mismatch"):
        validator.validate_wheel(tampered, SPECS["root"])

    bytecode = write_wheel(
        tmp_path / "bytecode",
        SPECS["root"],
        platform="any",
        bytecode=True,
    )
    with pytest.raises(validator.ValidationError, match="bytecode leaked"):
        validator.validate_wheel(bytecode, SPECS["root"])


def test_wheel_rejects_filename_metadata_disagreement(tmp_path: Path) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["root"],
        platform="any",
        metadata_version="9.9.9",
    )
    with pytest.raises(validator.ValidationError, match="has version"):
        validator.validate_wheel(wheel, SPECS["root"])

    dependencies = write_wheel(
        tmp_path / "dependencies",
        SPECS["root"],
        platform="any",
        metadata_dependencies=("not-type-bridge>=9",),
    )
    with pytest.raises(validator.ValidationError, match="Requires-Dist metadata disagrees"):
        validator.validate_wheel(dependencies, SPECS["root"])


@pytest.mark.parametrize("spec_key", ["root", "core"])
@pytest.mark.parametrize(
    ("license_value", "message"),
    [(None, "missing License"), ("Apache-2.0", "has license")],
)
def test_wheel_metadata_license_must_be_explicit_mit(
    tmp_path: Path,
    spec_key: str,
    license_value: str | None,
    message: str,
) -> None:
    spec = SPECS[spec_key]
    platform = "any" if spec.pure else "manylinux_2_17_x86_64"
    wheel = write_wheel(
        tmp_path,
        spec,
        platform=platform,
        metadata_license=license_value,
    )

    with pytest.raises(validator.ValidationError, match=message):
        validator.validate_wheel(wheel, spec)


def test_wheel_rejects_conflicting_license_expression(tmp_path: Path) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["root"],
        platform="any",
        metadata_license_expression="Apache-2.0",
    )

    with pytest.raises(validator.ValidationError, match="only the emitted License: MIT"):
        validator.validate_wheel(wheel, SPECS["root"])


@pytest.mark.parametrize("spec_key", ["root", "core"])
@pytest.mark.parametrize(
    ("license_value", "message"),
    [(None, "missing License"), ("MPL-2.0", "has license")],
)
def test_sdist_metadata_license_must_be_explicit_mit(
    tmp_path: Path,
    spec_key: str,
    license_value: str | None,
    message: str,
) -> None:
    spec = SPECS[spec_key]
    sdist = write_sdist(tmp_path, spec, metadata_license=license_value)

    with pytest.raises(validator.ValidationError, match=message):
        validator.validate_sdist(sdist, spec)


@pytest.mark.parametrize("project_license", [None, "Apache-2.0"])
def test_sdist_embedded_project_license_must_be_explicit_mit(
    tmp_path: Path,
    project_license: str | None,
) -> None:
    sdist = write_sdist(
        tmp_path,
        SPECS["root"],
        project_license=project_license,
    )

    with pytest.raises(validator.ValidationError, match="pyproject license disagrees"):
        validator.validate_sdist(sdist, SPECS["root"])


def test_wheel_rejects_missing_optional_dependency_metadata(tmp_path: Path) -> None:
    extras, requirements = validator.dependency_metadata(SPECS["root"])
    assert extras == ("dev", "docs", "typedb-driver")
    omitted = next(requirement for requirement in requirements if "extra=='docs'" in requirement)
    wheel = write_wheel(
        tmp_path,
        SPECS["root"],
        platform="any",
        metadata_dependencies=tuple(
            requirement for requirement in requirements if requirement != omitted
        ),
    )
    with pytest.raises(validator.ValidationError, match="Requires-Dist metadata disagrees"):
        validator.validate_wheel(wheel, SPECS["root"])

    missing_extra = write_wheel(
        tmp_path / "missing-extra",
        SPECS["root"],
        platform="any",
        metadata_extras=extras[:-1],
    )
    with pytest.raises(validator.ValidationError, match="Provides-Extra metadata disagrees"):
        validator.validate_wheel(missing_extra, SPECS["root"])


def test_requirement_markers_ignore_builder_parentheses_around_atoms() -> None:
    repository_form = (
        "typedb-driver>=3.8,<3.13; python_version < '3.14' and extra == 'typedb-driver'"
    )
    wheel_form = 'typedb-driver<3.13,>=3.8; (python_version < "3.14") and extra == "typedb-driver"'

    assert validator.normalize_requirement(repository_form) == validator.normalize_requirement(
        wheel_form
    )


def test_core_wheel_rejects_mislabeled_native_architecture(tmp_path: Path) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["core"],
        platform="manylinux_2_17_x86_64",
        native_platform="manylinux_2_17_aarch64",
    )
    with pytest.raises(validator.ValidationError, match="binary header does not match"):
        validator.validate_wheel(wheel, SPECS["core"])


def test_core_wheel_rejects_non_importable_native_filename(tmp_path: Path) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["core"],
        platform="manylinux_2_17_x86_64",
        native_name="type_bridge_core/type_bridge_core.garbage.so",
    )

    with pytest.raises(validator.ValidationError, match="must contain exactly"):
        validator.validate_wheel(wheel, SPECS["core"])


def test_wheel_rejects_malformed_root_is_purelib(tmp_path: Path) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["core"],
        platform="manylinux_2_17_x86_64",
        purelib_value="garbage",
    )

    with pytest.raises(validator.ValidationError, match="invalid Root-Is-Purelib"):
        validator.validate_wheel(wheel, SPECS["core"])


@pytest.mark.parametrize("platform", ["linux_x86_64", "musllinux_1_2_x86_64"])
def test_core_wheel_rejects_non_manylinux_gnu_tags(tmp_path: Path, platform: str) -> None:
    directories = write_release_set(tmp_path)
    next(directories[0].glob("*manylinux*x86_64.whl")).unlink()
    write_wheel(directories[0], SPECS["core"], platform=platform)
    with pytest.raises(validator.ValidationError, match="must use manylinux"):
        validate(directories)


@pytest.mark.parametrize(
    "platform",
    [
        "manylinux_garbage_x86_64",
        "manylinux_2_17_x86_64.hostile_x86_64",
        "manylinux_2_17_x86_64.macosx_11_0_x86_64",
    ],
)
def test_core_wheel_rejects_malformed_or_mixed_platform_tags(
    tmp_path: Path,
    platform: str,
) -> None:
    wheel = write_wheel(tmp_path, SPECS["core"], platform=platform)

    with pytest.raises(validator.ValidationError, match="Unknown|mixes"):
        validator.validate_wheel(wheel, SPECS["core"])


@pytest.mark.parametrize(
    ("python_tag", "abi_tag"),
    [("cp312.py3", "abi3"), ("cp312", "abi3.none")],
)
def test_core_wheel_rejects_compressed_python_or_abi_tags(
    tmp_path: Path,
    python_tag: str,
    abi_tag: str,
) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["core"],
        platform="manylinux_2_17_x86_64",
        python_tag_override=python_tag,
        abi_tag_override=abi_tag,
    )

    with pytest.raises(validator.ValidationError, match="exact cp312-abi3"):
        validator.validate_wheel(wheel, SPECS["core"])


def test_core_wheel_rejects_elf_newer_than_manylinux_policy(tmp_path: Path) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["core"],
        platform="manylinux_2_17_x86_64",
        glibc_version="2.34",
    )

    with pytest.raises(validator.ValidationError, match="exceed the manylinux_2_17 policy"):
        validator.validate_wheel(wheel, SPECS["core"])


@pytest.mark.parametrize(
    "extra_member",
    [
        "hostile/__init__.py",
        "type_bridge-2.0.0rc0.data/purelib/hostile.py",
        "type_bridge-2.0.0rc0.data/platlib/hostile.py",
        "type_bridge-2.0.0rc0.data/scripts/hostile",
    ],
)
def test_root_wheel_rejects_unexpected_install_payload(
    tmp_path: Path,
    extra_member: str,
) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["root"],
        platform="any",
        extra_members={extra_member: b"hostile\n"},
    )

    with pytest.raises(validator.ValidationError, match="unexpected install payload"):
        validator.validate_wheel(wheel, SPECS["root"])


def test_core_wheel_rejects_extra_package_payload(tmp_path: Path) -> None:
    wheel = write_wheel(
        tmp_path,
        SPECS["core"],
        platform="manylinux_2_17_x86_64",
        extra_members={"type_bridge_core/backdoor.py": b"hostile\n"},
    )

    with pytest.raises(validator.ValidationError, match="package inventory disagrees"):
        validator.validate_wheel(wheel, SPECS["core"])


def test_core_sdist_rejects_extra_package_payload(tmp_path: Path) -> None:
    sdist = write_sdist(
        tmp_path,
        SPECS["core"],
        extra_members={"python/type_bridge_core/backdoor.py": b"hostile\n"},
    )

    with pytest.raises(validator.ValidationError, match="source inventory disagrees"):
        validator.validate_sdist(sdist, SPECS["core"])


@pytest.mark.parametrize(
    "fork_name",
    [
        "typedb-driver-b9",
        "typedb_protocol_b9",
        "type-bridge-typedb-driver-b9",
        "type_bridge_typedb_protocol_b9",
    ],
)
def test_python_artifacts_reject_historical_band9_payload_names(
    tmp_path: Path,
    fork_name: str,
) -> None:
    spec = SPECS["root"]
    dist_info = f"{spec.distribution}-{spec.version}.dist-info"
    wheel = write_wheel(
        tmp_path / "wheel",
        spec,
        platform="any",
        extra_members={f"{dist_info}/{fork_name}.json": b"hostile\n"},
    )
    with pytest.raises(validator.ValidationError, match="Historical band-9 fork payload"):
        validator.validate_wheel(wheel, spec)

    sdist = write_sdist(
        tmp_path / "sdist",
        spec,
        extra_members={f"vendor/{fork_name}/Cargo.toml": b"hostile\n"},
    )
    with pytest.raises(validator.ValidationError, match="Historical band-9 fork payload"):
        validator.validate_sdist(sdist, spec)


def test_python_artifacts_allow_legitimate_non_license_metadata_members(tmp_path: Path) -> None:
    spec = SPECS["root"]
    wheel = write_wheel(
        tmp_path / "wheel",
        spec,
        platform="any",
    )
    assert validator.validate_wheel(wheel, spec)["kind"] == "wheel"

    sdist = write_sdist(
        tmp_path / "sdist",
        spec,
        extra_members={"docs/release-notes.md": b"fixture\n"},
    )
    assert validator.validate_sdist(sdist, spec)["kind"] == "sdist"


@pytest.mark.parametrize("license_files", [(), ("LICENSE", "LICENSE"), ("OTHER",)])
def test_root_artifacts_reject_license_file_metadata_drift(
    tmp_path: Path,
    license_files: tuple[str, ...],
) -> None:
    wheel = write_wheel(
        tmp_path / "wheel",
        SPECS["root"],
        platform="any",
        metadata_license_files=license_files,
    )
    with pytest.raises(validator.ValidationError, match="License-File metadata disagrees"):
        validator.validate_wheel(wheel, SPECS["root"])

    sdist = write_sdist(
        tmp_path / "sdist",
        SPECS["root"],
        metadata_license_files=license_files,
    )
    with pytest.raises(validator.ValidationError, match="License-File metadata disagrees"):
        validator.validate_sdist(sdist, SPECS["root"])


def test_artifacts_reject_unrecognized_license_like_members(tmp_path: Path) -> None:
    core = SPECS["core"]
    dist_info = f"{core.distribution}-{core.version}.dist-info"
    wheel = write_wheel(
        tmp_path / "wheel",
        core,
        platform="manylinux_2_17_x86_64",
        extra_members={f"{dist_info}/licenses/LICENSE.evil": b"hostile\n"},
    )
    with pytest.raises(validator.ValidationError, match="license-file inventory disagrees"):
        validator.validate_wheel(wheel, core)

    sdist = write_sdist(
        tmp_path / "sdist",
        core,
        extra_members={"vendor/LICENSE.evil": b"hostile\n"},
    )
    with pytest.raises(validator.ValidationError, match="source inventory disagrees"):
        validator.validate_sdist(sdist, core)


@pytest.mark.parametrize(
    "source_name",
    [
        "vendor/typedb-driver-b7/src/lib.rs",
        "vendor/typedb-driver-b8/Cargo.toml",
    ],
)
def test_core_sdist_rejects_changed_vendor_sources(
    tmp_path: Path,
    source_name: str,
) -> None:
    sdist = write_sdist(
        tmp_path,
        SPECS["core"],
        extra_members={source_name: b"hostile downstream source\n"},
    )

    with pytest.raises(validator.ValidationError, match="repository checkout"):
        validator.validate_sdist(sdist, SPECS["core"])


def test_checked_in_core_sdist_license_projection_must_match_canonical(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    core = tmp_path / "type-bridge-core"
    crate = core / "crates/fixture"
    crate.mkdir(parents=True)
    for name in ("Cargo.lock", "Cargo.toml", "pyproject.toml"):
        (core / name).write_text("fixture\n", encoding="utf-8")
    canonical = b"canonical MIT license\n"
    (core / validator.ROOT_LICENSE_FILE).write_bytes(canonical)
    (crate / "Cargo.toml").write_text('[package]\nname = "fixture"\n', encoding="utf-8")
    projection = crate / validator.ROOT_LICENSE_FILE
    projection.write_bytes(canonical)
    monkeypatch.setattr(validator, "CORE_SDIST_SOURCE_ROOTS", ("crates/fixture",))
    monkeypatch.setattr(
        validator,
        "CORE_SDIST_GENERATED_LICENSES",
        frozenset({"crates/fixture/LICENSE"}),
    )

    authorities = validator.core_sdist_source_authorities(tmp_path)
    assert authorities["crates/fixture/LICENSE"] == projection

    projection.write_bytes(b"different license\n")
    with pytest.raises(validator.ValidationError, match="canonical TypeBridge license"):
        validator.core_sdist_source_authorities(tmp_path)


def test_core_sdist_excludes_only_the_closed_nested_test_package(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    core = tmp_path / "type-bridge-core"
    core.mkdir()
    for name in ("Cargo.lock", "Cargo.toml", "pyproject.toml", validator.ROOT_LICENSE_FILE):
        (core / name).write_text("fixture\n", encoding="utf-8")

    production_source = core / "crates/core/src/lib.rs"
    production_source.parent.mkdir(parents=True)
    production_source.write_text("pub fn production() {}\n", encoding="utf-8")

    excluded_root = "crates/core/tests/fixtures/rule-wire-standalone"
    nested_package = core / excluded_root
    (nested_package / "src").mkdir(parents=True)
    (nested_package / "target/debug").mkdir(parents=True)
    for name in ("Cargo.lock", "Cargo.toml"):
        (nested_package / name).write_text("nested fixture\n", encoding="utf-8")
    (nested_package / "src/lib.rs").write_text("fn fixture() {}\n", encoding="utf-8")
    (nested_package / "target/debug/leak.rlib").write_bytes(b"ignored build output\n")

    near_prefix = core / f"{excluded_root}-extra/keep.rs"
    near_prefix.parent.mkdir(parents=True)
    near_prefix.write_text("fn keep() {}\n", encoding="utf-8")

    monkeypatch.setattr(validator, "CORE_SDIST_SOURCE_ROOTS", ("crates/core",))
    monkeypatch.setattr(validator, "CORE_SDIST_GENERATED_LICENSES", frozenset())

    authorities = validator.core_sdist_source_authorities(tmp_path)

    assert validator.CORE_SDIST_EXCLUDED_NESTED_PACKAGE_ROOTS == frozenset({excluded_root})
    assert authorities["crates/core/src/lib.rs"] == production_source
    assert authorities[f"{excluded_root}-extra/keep.rs"] == near_prefix
    assert not any(
        name == excluded_root or name.startswith(f"{excluded_root}/") for name in authorities
    )


def test_core_sdist_optional_derive_source_is_a_transformed_workspace_member() -> None:
    assert "crates/orm-derive" in validator.CORE_SDIST_SOURCE_ROOTS
    assert "crates/orm-derive" in validator.CORE_SDIST_WORKSPACE_MEMBERS
    assert "crates/orm-derive/Cargo.toml" in validator.CORE_SDIST_TRANSFORMED_FIRST_PARTY_MANIFESTS


def test_sdist_rejects_historical_band9_symlink_target(tmp_path: Path) -> None:
    sdist = write_sdist(
        tmp_path,
        SPECS["root"],
        symlink_target="typedb-driver-b9/../CLAUDE.md",
    )

    with pytest.raises(validator.ValidationError, match="Historical band-9 fork payload"):
        validator.validate_sdist(sdist, SPECS["root"])


def test_core_artifacts_require_exact_third_party_notice(tmp_path: Path) -> None:
    missing_wheel = write_wheel(
        tmp_path / "missing-wheel",
        SPECS["core"],
        platform="manylinux_2_17_x86_64",
        omit_member=validator.CORE_WHEEL_NOTICE,
    )
    with pytest.raises(validator.ValidationError, match="license-file inventory disagrees"):
        validator.validate_wheel(missing_wheel, SPECS["core"])

    changed_wheel = write_wheel(
        tmp_path / "changed-wheel",
        SPECS["core"],
        platform="manylinux_2_17_x86_64",
        extra_members={validator.CORE_WHEEL_NOTICE: b"incomplete notice\n"},
    )
    with pytest.raises(validator.ValidationError, match="notice disagrees"):
        validator.validate_wheel(changed_wheel, SPECS["core"])

    missing_sdist = write_sdist(
        tmp_path / "missing-sdist",
        SPECS["core"],
        omit_member=validator.CORE_SDIST_NOTICE,
    )
    with pytest.raises(validator.ValidationError, match="source inventory disagrees"):
        validator.validate_sdist(missing_sdist, SPECS["core"])

    changed_sdist = write_sdist(
        tmp_path / "changed-sdist",
        SPECS["core"],
        extra_members={validator.CORE_SDIST_NOTICE: b"incomplete notice\n"},
    )
    with pytest.raises(validator.ValidationError, match="repository checkout"):
        validator.validate_sdist(changed_sdist, SPECS["core"])


def test_root_artifacts_require_complete_source_inventory(tmp_path: Path) -> None:
    deep_public_file = "type_bridge/migration/snapshots.py"
    assert len(SPECS["root"].wheel_members) > 100
    assert deep_public_file in SPECS["root"].wheel_members

    wheel = write_wheel(
        tmp_path / "wheel",
        SPECS["root"],
        platform="any",
        omit_member=deep_public_file,
    )
    with pytest.raises(validator.ValidationError, match="package inventory disagrees"):
        validator.validate_wheel(wheel, SPECS["root"])

    sdist = write_sdist(
        tmp_path / "sdist",
        SPECS["root"],
        omit_member=deep_public_file,
    )
    with pytest.raises(validator.ValidationError, match="missing release sources"):
        validator.validate_sdist(sdist, SPECS["root"])


def test_root_artifacts_require_complete_entry_point_inventory(tmp_path: Path) -> None:
    missing = write_wheel(
        tmp_path / "missing",
        SPECS["root"],
        platform="any",
        omit_entry_points=True,
    )
    with pytest.raises(validator.ValidationError, match="missing .*entry_points.txt"):
        validator.validate_wheel(missing, SPECS["root"])

    wrong_wheel = write_wheel(
        tmp_path / "wrong-wheel",
        SPECS["root"],
        platform="any",
        script_target="type_bridge:wrong",
    )
    with pytest.raises(validator.ValidationError, match="entry points disagree"):
        validator.validate_wheel(wrong_wheel, SPECS["root"])

    wrong_sdist = write_sdist(
        tmp_path / "wrong-sdist",
        SPECS["root"],
        script_target="type_bridge:wrong",
    )
    with pytest.raises(validator.ValidationError, match="entry points disagree"):
        validator.validate_sdist(wrong_sdist, SPECS["root"])

    extra_wheel = write_wheel(
        tmp_path / "extra-wheel",
        SPECS["root"],
        platform="any",
        extra_entry_points={"hostile.plugins": {"backdoor": "hostile:main"}},
    )
    with pytest.raises(validator.ValidationError, match="entry points disagree"):
        validator.validate_wheel(extra_wheel, SPECS["root"])

    extra_sdist = write_sdist(
        tmp_path / "extra-sdist",
        SPECS["root"],
        extra_entry_points={"hostile.plugins": {"backdoor": "hostile:main"}},
    )
    with pytest.raises(validator.ValidationError, match="entry points disagree"):
        validator.validate_sdist(extra_sdist, SPECS["root"])


def test_sdist_rejects_incomplete_optional_dependency_table(tmp_path: Path) -> None:
    sdist = write_sdist(
        tmp_path,
        SPECS["root"],
        omit_optional_dependency=True,
    )
    with pytest.raises(validator.ValidationError, match="optional-dependencies disagree"):
        validator.validate_sdist(sdist, SPECS["root"])


def test_sdist_rejects_unsafe_member(tmp_path: Path) -> None:
    sdist = write_sdist(tmp_path, SPECS["root"], unsafe_member=True)
    with pytest.raises(validator.ValidationError, match="Unsafe member path"):
        validator.validate_sdist(sdist, SPECS["root"])

    duplicate = write_sdist(tmp_path / "duplicate", SPECS["root"], duplicate_member=True)
    with pytest.raises(validator.ValidationError, match="Duplicate sdist member"):
        validator.validate_sdist(duplicate, SPECS["root"])


def test_sdist_accepts_internal_symlink_and_rejects_escape(tmp_path: Path) -> None:
    safe = write_sdist(tmp_path / "safe", SPECS["root"], symlink_target="CLAUDE.md")
    assert validator.validate_sdist(safe, SPECS["root"])["bucket"] == "source"

    escaping = write_sdist(
        tmp_path / "escaping",
        SPECS["root"],
        symlink_target="../../escape",
    )
    with pytest.raises(validator.ValidationError, match="escapes sdist root"):
        validator.validate_sdist(escaping, SPECS["root"])


def test_repository_version_must_match_release_tag() -> None:
    with pytest.raises(validator.ValidationError, match="release identity"):
        validator.load_package_specs(ROOT, "9.9.9")


@pytest.mark.parametrize(
    "replacement",
    (
        "type-bridge-core>=2.0.0rc0",
        "type-bridge-core==2.0.0rc0; python_version >= '3.12'",
        "TYPE-BRIDGE-CORE==2.0.0rc0",
        "type_bridge_core==2.0.0rc0",
    ),
)
def test_artifact_gate_rejects_noncanonical_root_core_requirements(
    tmp_path: Path,
    replacement: str,
) -> None:
    manifest, _ = copy_root_python_contract(tmp_path)
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace(
            "type-bridge-core==2.0.0rc0",
            replacement,
            1,
        ),
        encoding="utf-8",
    )

    with pytest.raises(validator.ValidationError, match="canonical, unmarked, exact"):
        validator.load_package_specs(tmp_path, VERSION)


def test_artifact_gate_rejects_duplicate_normalized_root_core_requirements(
    tmp_path: Path,
) -> None:
    manifest, _ = copy_root_python_contract(tmp_path)
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace(
            '"type-bridge-core==2.0.0rc0",',
            '"type-bridge-core==2.0.0rc0",\n    "type.bridge.core==2.0.0rc0",',
            1,
        ),
        encoding="utf-8",
    )

    with pytest.raises(validator.ValidationError, match="exactly one type-bridge-core"):
        validator.load_package_specs(tmp_path, VERSION)


def test_artifact_gate_binds_import_visible_python_version(tmp_path: Path) -> None:
    _, package_init = copy_root_python_contract(tmp_path)
    package_init.write_text(
        package_init.read_text(encoding="utf-8").replace(
            '__version__ = "2.0.0rc0"',
            '__version__ = "2.0.0rc1"',
            1,
        ),
        encoding="utf-8",
    )

    with pytest.raises(validator.ValidationError, match="type_bridge.__version__"):
        validator.load_package_specs(tmp_path, VERSION)


def test_cli_writes_validated_hash_manifest(tmp_path: Path, capsys: Any) -> None:
    core_wheels, core_sdist, root_dist = write_release_set(tmp_path)
    manifest = tmp_path / "accepted.json"

    assert (
        validator.main(
            [
                "--core-wheels-dir",
                str(core_wheels),
                "--core-sdist-dir",
                str(core_sdist),
                "--root-dist-dir",
                str(root_dist),
                "--expected-version",
                VERSION,
                "--repository-root",
                str(ROOT),
                "--manifest",
                str(manifest),
            ]
        )
        == 0
    )
    report = json.loads(manifest.read_text(encoding="utf-8"))
    assert report["status"] == "ok"
    assert len(report["artifacts"]) == 8
    assert json.loads(capsys.readouterr().out) == report
