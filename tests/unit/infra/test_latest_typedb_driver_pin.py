"""Band 9 must consume the latest stable official TypeDB 3.12 driver."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
VALIDATOR = REPO_ROOT / "scripts/ci/validate_latest_typedb_driver_pin.py"
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release.yml"
CRATES_IO_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"
DRIVER_CHECKSUM = "1" * 64
PROTOCOL_CHECKSUM = "2" * 64


def _lock_entry(
    version: str,
    *,
    name: str = "typedb-driver",
    source: str = CRATES_IO_SOURCE,
    checksum: str | None = DRIVER_CHECKSUM,
    dependencies: list[str] | None = None,
) -> str:
    fields = [
        "[[package]]",
        f'name = "{name}"',
        f'version = "{version}"',
        f'source = "{source}"',
    ]
    if checksum is not None:
        fields.append(f'checksum = "{checksum}"')
    if name == "typedb-driver" and dependencies is None:
        dependencies = ["typedb-protocol"]
    if dependencies is not None:
        fields.append("dependencies = [")
        fields.extend(f" {json.dumps(dependency)}," for dependency in dependencies)
        fields.append("]")
    return "\n".join(fields)


def _run_validator(
    tmp_path: Path,
    requirement: str,
    versions: list[dict[str, object]],
    *,
    dependency: str | None = None,
    lock_entries: list[str] | None = None,
    manifest_suffix: str = "",
    cargo_config: str | None = None,
    driver_dependencies: list[dict[str, object]] | None = None,
    protocol_versions: list[dict[str, object]] | None = None,
    feature_block: str | None = None,
    committed_cutoff: bool = False,
) -> subprocess.CompletedProcess[str]:
    manifest = tmp_path / "Cargo.toml"
    dependency = dependency or (f'typedb-driver = {{ version = "{requirement}", optional = true }}')
    if feature_block is None:
        feature_block = '[features]\ndefault = ["band9"]\nband9 = ["dep:typedb-driver"]\n'
    manifest.write_text(
        '[package]\nname = "probe"\nversion = "0.0.0"\n'
        f"[dependencies]\n{dependency}\n\n{feature_block}\n{manifest_suffix}",
        encoding="utf-8",
    )
    candidate = requirement.removeprefix("=")
    lock_version = (
        candidate
        if len(candidate.split(".")) == 3
        and all(component.isdigit() for component in candidate.split("."))
        else "3.12.0"
    )
    if lock_entries is None:
        lock_entries = [
            _lock_entry(lock_version),
            _lock_entry(
                lock_version,
                name="typedb-protocol",
                checksum=PROTOCOL_CHECKSUM,
            ),
        ]
    lock = tmp_path / "Cargo.lock"
    lock.write_text("version = 4\n\n" + "\n\n".join(lock_entries) + "\n", encoding="utf-8")
    if cargo_config is not None:
        config_directory = tmp_path / ".cargo"
        config_directory.mkdir()
        (config_directory / "config.toml").write_text(cargo_config, encoding="utf-8")
    metadata = tmp_path / "metadata.json"
    metadata.write_text(json.dumps({"versions": versions}), encoding="utf-8")
    dependencies = tmp_path / "dependencies.json"
    if driver_dependencies is None:
        driver_dependencies = [_dependency(f"={lock_version}")]
    dependencies.write_text(
        json.dumps({"dependencies": driver_dependencies}),
        encoding="utf-8",
    )
    protocol_metadata = tmp_path / "protocol-metadata.json"
    if protocol_versions is None:
        protocol_versions = [_protocol_version(lock_version)]
    protocol_metadata.write_text(
        json.dumps({"versions": protocol_versions}),
        encoding="utf-8",
    )
    arguments = [
        sys.executable,
        str(VALIDATOR),
        "--manifest",
        str(manifest),
        "--metadata",
        str(metadata),
        "--dependencies",
        str(dependencies),
        "--protocol-metadata",
        str(protocol_metadata),
        "--lock",
        str(lock),
    ]
    if committed_cutoff:
        arguments.append("--committed-cutoff")
    return subprocess.run(
        arguments,
        check=False,
        capture_output=True,
        text=True,
    )


def _version(
    num: str,
    *,
    yanked: bool = False,
    license_value: object = "Apache-2.0",
    checksum: object = DRIVER_CHECKSUM,
) -> dict[str, object]:
    return {
        "num": num,
        "yanked": yanked,
        "license": license_value,
        "checksum": checksum,
    }


def _dependency(
    requirement: object,
    *,
    optional: object = False,
    kind: object = "normal",
) -> dict[str, object]:
    return {
        "crate_id": "typedb-protocol",
        "req": requirement,
        "optional": optional,
        "kind": kind,
    }


def _protocol_version(
    num: str,
    *,
    yanked: bool = False,
    license_value: object = "MPL-2.0",
    checksum: object = PROTOCOL_CHECKSUM,
) -> dict[str, object]:
    return {
        "num": num,
        "yanked": yanked,
        "license": license_value,
        "checksum": checksum,
    }


def test_accepts_the_exact_latest_non_yanked_stable_patch(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.1",
        [_version("3.12.0"), _version("3.12.1"), _version("3.13.0")],
    )

    assert result.returncode == 0, result.stderr
    assert "=3.12.1" in result.stdout


def test_committed_cutoff_accepts_an_older_official_non_yanked_patch(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0"), _version("3.12.1")],
        committed_cutoff=True,
    )

    assert result.returncode == 0, result.stderr
    assert "committed-cutoff official typedb-driver" in result.stdout


def test_committed_cutoff_still_rejects_a_yanked_selected_driver(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0", yanked=True), _version("3.12.1")],
        committed_cutoff=True,
    )

    assert result.returncode == 1
    assert "typedb-driver =3.12.0 as yanked" in result.stderr


@pytest.mark.parametrize(
    ("version", "message"),
    [
        (
            _version("3.12.0", license_value="MIT"),
            "expected Apache-2.0",
        ),
        (
            _version("3.12.0", checksum="not-a-canonical-sha256"),
            "no canonical SHA-256 checksum for typedb-driver =3.12.0",
        ),
    ],
)
def test_rejects_driver_license_or_checksum_metadata_drift(
    tmp_path: Path,
    version: dict[str, object],
    message: str,
) -> None:
    result = _run_validator(tmp_path, "=3.12.0", [version])

    assert result.returncode == 1
    assert message in result.stderr


def test_rejects_driver_checksum_drift_from_the_official_lock_row(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0", checksum="3" * 64)],
    )

    assert result.returncode == 1
    assert "checksum for typedb-driver =3.12.0" in result.stderr
    assert "canonical crates.io checksum" in result.stderr


def test_rejects_an_exact_pin_when_a_newer_patch_has_been_released(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0"), _version("3.12.1")],
    )

    assert result.returncode == 1
    assert "newest non-yanked stable 3.12.x release is 3.12.1" in result.stderr


@pytest.mark.parametrize("requirement", ["3.12.0", "=3.12.x", "^3.12.0", "=3.12.0-rc.1"])
def test_rejects_floating_or_non_stable_requirements(
    tmp_path: Path,
    requirement: str,
) -> None:
    result = _run_validator(tmp_path, requirement, [_version("3.12.0")])

    assert result.returncode == 1
    assert "pin" in result.stderr


def test_ignores_yanked_and_prerelease_versions(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [
            _version("3.12.0"),
            _version("3.12.1", yanked=True),
            _version("3.12.2-rc.1"),
        ],
    )

    assert result.returncode == 0, result.stderr


@pytest.mark.parametrize(
    ("dependencies", "message"),
    [
        ([], "exactly one typedb-protocol dependency, found 0"),
        (
            [_dependency("=3.12.0"), _dependency("=3.12.0")],
            "exactly one typedb-protocol dependency, found 2",
        ),
        (
            [_dependency("=3.12.0", optional=True)],
            "one nonoptional normal dependency",
        ),
        (
            [_dependency("=3.12.0", kind="dev")],
            "one nonoptional normal dependency",
        ),
        (
            [_dependency("^3.12.0")],
            "must exact-pin typedb-protocol",
        ),
        (
            [_dependency("=3.12.0-rc.1")],
            "must exact-pin a stable typedb-protocol release",
        ),
    ],
)
def test_rejects_noncanonical_driver_protocol_dependency_metadata(
    tmp_path: Path,
    dependencies: list[dict[str, object]],
    message: str,
) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        driver_dependencies=dependencies,
    )

    assert result.returncode == 1
    assert message in result.stderr


@pytest.mark.parametrize(
    ("versions", "message"),
    [
        (
            [_protocol_version("3.12.0", yanked=True)],
            "typedb-protocol =3.12.0 as yanked",
        ),
        (
            [_protocol_version("3.12.0", license_value="Apache-2.0")],
            "expected MPL-2.0",
        ),
        (
            [_protocol_version("3.12.0", checksum="short")],
            "no canonical SHA-256 checksum for typedb-protocol =3.12.0",
        ),
        (
            [_protocol_version("3.12.1")],
            "typedb-protocol =3.12.0 record, found 0",
        ),
        (
            [_protocol_version("3.12.0"), _protocol_version("3.12.0")],
            "typedb-protocol =3.12.0 record, found 2",
        ),
    ],
)
def test_rejects_protocol_registry_metadata_drift(
    tmp_path: Path,
    versions: list[dict[str, object]],
    message: str,
) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        protocol_versions=versions,
    )

    assert result.returncode == 1
    assert message in result.stderr


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("git", '"https://example.invalid/typedb-driver"'),
        ("path", '"../typedb-driver"'),
        ("registry", '"private"'),
        ("registry-index", '"https://example.invalid/index"'),
        ("package", '"typedb-driver"'),
        ("workspace", "true"),
        ("branch", '"downstream"'),
    ],
)
def test_rejects_non_registry_dependency_selectors(
    tmp_path: Path,
    field: str,
    value: str,
) -> None:
    dependency = f'typedb-driver = {{ version = "=3.12.0", optional = true, {field} = {value} }}'

    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        dependency=dependency,
    )

    assert result.returncode == 1
    assert f"unsupported dependency fields: {field}" in result.stderr


def test_rejects_a_package_alias_instead_of_the_exact_dependency_key(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        dependency=(
            'official-driver = { package = "typedb-driver", version = "=3.12.0", optional = true }'
        ),
    )

    assert result.returncode == 1
    assert "exact dependency key 'typedb-driver'" in result.stderr


def test_rejects_a_package_alias_beside_the_exact_dependency_key(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        dependency=(
            'typedb-driver = { version = "=3.12.0", optional = true }\n'
            'driver-alias = { package = "typedb-driver", version = "=3.12.0" }'
        ),
    )

    assert result.returncode == 1
    assert "package aliases are not allowed: driver-alias" in result.stderr


def test_rejects_default_features_that_do_not_include_band9(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        feature_block=(
            '[features]\ndefault = ["band8"]\nband8 = []\nband9 = ["dep:typedb-driver"]\n'
        ),
    )

    assert result.returncode == 1
    assert "default features must include band9" in result.stderr


def test_rejects_band9_without_a_direct_official_driver_activation(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        dependency=(
            'typedb-driver = { version = "=3.12.0", optional = true }\n'
            'type-bridge-typedb-driver-b8 = { version = "=3.11.5", optional = true }'
        ),
        feature_block=(
            '[features]\ndefault = ["band9"]\nband9 = ["dep:type-bridge-typedb-driver-b8"]\n'
        ),
    )

    assert result.returncode == 1
    assert "band9 must directly activate exactly dep:typedb-driver" in result.stderr


def test_rejects_a_transitive_legacy_driver_activation_from_band9(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        dependency=(
            'typedb-driver = { version = "=3.12.0", optional = true }\n'
            'type-bridge-typedb-driver-b8 = { version = "=3.11.5", optional = true }'
        ),
        feature_block=(
            '[features]\ndefault = ["band9"]\n'
            'legacy = ["dep:type-bridge-typedb-driver-b8"]\n'
            'band9 = ["dep:typedb-driver", "legacy"]\n'
        ),
    )

    assert result.returncode == 1
    assert "band9 must activate only the official typedb-driver dependency" in result.stderr


def test_rejects_a_lockfile_version_that_does_not_match_the_pin(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        lock_entries=[_lock_entry("3.12.1")],
    )

    assert result.returncode == 1
    assert "resolves typedb-driver '3.12.1', but band 9 pins =3.12.0" in result.stderr


@pytest.mark.parametrize(
    "source",
    [
        "git+https://example.invalid/typedb-driver#0123456789abcdef",
        "registry+https://example.invalid/index",
    ],
)
def test_rejects_a_non_crates_io_lock_source(tmp_path: Path, source: str) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        lock_entries=[_lock_entry("3.12.0", source=source)],
    )

    assert result.returncode == 1
    assert "official crates.io source" in result.stderr


@pytest.mark.parametrize("checksum", [None, "", "   "])
def test_rejects_a_missing_lock_checksum(tmp_path: Path, checksum: str | None) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        lock_entries=[_lock_entry("3.12.0", checksum=checksum)],
    )

    assert result.returncode == 1
    assert "has no crates.io checksum" in result.stderr


def test_rejects_a_noncanonical_driver_lock_checksum(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        lock_entries=[_lock_entry("3.12.0", checksum="ABCDEF")],
    )

    assert result.returncode == 1
    assert "no canonical crates.io SHA-256 checksum for typedb-driver" in result.stderr


@pytest.mark.parametrize(
    "entries",
    [[], [_lock_entry("3.12.0"), _lock_entry("3.12.0")]],
)
def test_rejects_missing_or_duplicate_driver_lock_entries(
    tmp_path: Path,
    entries: list[str],
) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        lock_entries=entries,
    )

    assert result.returncode == 1
    assert "must contain exactly one typedb-driver package" in result.stderr


@pytest.mark.parametrize(
    ("protocol_entry", "message"),
    [
        (
            _lock_entry(
                "3.12.0",
                name="typedb-protocol",
                source="registry+https://example.invalid/index",
                checksum=PROTOCOL_CHECKSUM,
            ),
            "official crates.io source",
        ),
        (
            _lock_entry(
                "3.12.1",
                name="typedb-protocol",
                checksum=PROTOCOL_CHECKSUM,
            ),
            "official typedb-driver selects =3.12.0",
        ),
        (
            _lock_entry(
                "3.12.0",
                name="typedb-protocol",
                checksum="4" * 64,
            ),
            "does not match the canonical crates.io checksum",
        ),
        (
            _lock_entry(
                "3.12.0",
                name="typedb-protocol",
                checksum="not-canonical",
            ),
            "no canonical crates.io SHA-256 checksum for typedb-protocol",
        ),
    ],
)
def test_rejects_protocol_lock_source_version_or_checksum_drift(
    tmp_path: Path,
    protocol_entry: str,
    message: str,
) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        lock_entries=[_lock_entry("3.12.0"), protocol_entry],
    )

    assert result.returncode == 1
    assert message in result.stderr


@pytest.mark.parametrize(
    "protocol_entries",
    [
        [],
        [
            _lock_entry(
                "3.12.0",
                name="typedb-protocol",
                checksum=PROTOCOL_CHECKSUM,
            ),
            _lock_entry(
                "3.12.0",
                name="typedb-protocol",
                checksum=PROTOCOL_CHECKSUM,
            ),
        ],
    ],
)
def test_rejects_missing_or_duplicate_protocol_lock_entries(
    tmp_path: Path,
    protocol_entries: list[str],
) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        lock_entries=[_lock_entry("3.12.0"), *protocol_entries],
    )

    assert result.returncode == 1
    assert "must contain exactly one typedb-protocol package" in result.stderr


@pytest.mark.parametrize(
    ("dependencies", "message"),
    [
        ([], "exactly one typedb-protocol dependency edge, found 0"),
        (
            ["typedb-protocol", "typedb-protocol"],
            "exactly one typedb-protocol dependency edge, found 2",
        ),
        (
            ["typedb-protocol 3.12.0 (registry+https://example.invalid/crates.io-index)"],
            "does not resolve the official typedb-protocol =3.12.0 row",
        ),
    ],
)
def test_rejects_a_missing_duplicate_or_forged_driver_protocol_lock_edge(
    tmp_path: Path,
    dependencies: list[str],
    message: str,
) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        lock_entries=[
            _lock_entry("3.12.0", dependencies=dependencies),
            _lock_entry(
                "3.12.0",
                name="typedb-protocol",
                checksum=PROTOCOL_CHECKSUM,
            ),
        ],
    )

    assert result.returncode == 1
    assert message in result.stderr


@pytest.mark.parametrize(
    ("patch_table", "patch"),
    [
        ("patch.crates-io", 'typedb-driver = { path = "../fork" }'),
        (
            'patch."https://github.com/rust-lang/crates.io-index"',
            'driver-fork = { package = "typedb-driver", path = "../fork" }',
        ),
    ],
)
def test_rejects_workspace_driver_patches(
    tmp_path: Path,
    patch_table: str,
    patch: str,
) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        manifest_suffix=f"\n[{patch_table}]\n{patch}\n",
    )

    assert result.returncode == 1
    assert "through [patch." in result.stderr


def test_rejects_fully_qualified_workspace_replacement_keys(tmp_path: Path) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        manifest_suffix=(
            '\n[replace]\n"registry+https://github.com/rust-lang/'
            'crates.io-index#typedb-driver@3.12.0" = { path = "../fork" }\n'
        ),
    )

    assert result.returncode == 1
    assert "uses [replace]" in result.stderr


@pytest.mark.parametrize(
    "config",
    [
        (
            '[source.crates-io]\nreplace-with = "mirror"\n'
            '[source.mirror]\nregistry = "https://example.invalid/index"\n'
        ),
        '[registries.crates-io]\nindex = "https://example.invalid/index"\n',
        '[patch.crates-io]\ntypedb-driver = { path = "../fork" }\n',
        'include = "source-policy.toml"\n',
        'paths = ["../fork"]\n',
        '[registry]\ndefault = "private"\n',
    ],
)
def test_rejects_cargo_source_overrides(tmp_path: Path, config: str) -> None:
    result = _run_validator(
        tmp_path,
        "=3.12.0",
        [_version("3.12.0")],
        cargo_config=config,
    )

    assert result.returncode == 1
    assert "Cargo source configuration" in result.stderr


def test_release_workflow_binds_the_driver_cutoff_without_cargo_publication() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    validation_command = "python scripts/ci/validate_latest_typedb_driver_pin.py"
    committed_option = "--committed-cutoff"
    validation = workflow.index(validation_command)
    committed = workflow.index(committed_option, validation)
    preflight = workflow.index("  channel-preflight:")
    first_publish = workflow.index("  publish-node-npm:")

    assert workflow.count(validation_command) == 1
    assert validation < committed < preflight < first_publish
    assert "  preflight-publication:" not in workflow
    assert "  publish-crates:" not in workflow
    assert "--cutoff-state" not in workflow
    assert "publish_crate_idempotently" not in workflow
    assert "cargo publish" not in workflow
    assert (
        "needs: [validate-release-identity, accept-python-artifacts, "
        "accept-node-package, accept-live-artifact-parity, accept-server-oci]"
    ) in workflow[preflight:]
    assert "needs: [channel-preflight, publish-server-oci]" in workflow[first_publish:]
