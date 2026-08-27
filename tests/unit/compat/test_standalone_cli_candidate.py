"""Fail-closed tests for the standalone CLI candidate archive."""

from __future__ import annotations

import gzip
import importlib.util
import io
import json
import platform
import sys
import tarfile
from pathlib import Path, PurePosixPath
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/standalone_cli_candidate.py"
SPEC = importlib.util.spec_from_file_location("standalone_cli_candidate", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CANDIDATE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CANDIDATE
SPEC.loader.exec_module(CANDIDATE)


def _native_linux() -> bool:
    return platform.system() == "Linux" and platform.machine() in {"x86_64", "AMD64"}


@pytest.fixture
def accepted_archive(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    if not _native_linux():
        pytest.skip("the frozen Phase 1 candidate matrix contains only native x86_64 Linux")
    monkeypatch.setattr(CANDIDATE, "verify_version", lambda *args, **kwargs: None)
    binary = Path("/bin/true").resolve()
    return CANDIDATE.package_binary(
        binary,
        tmp_path,
        commit="1" * 40,
        tree="2" * 40,
    )


def _raw_archive(entries: list[tuple[str, bytes, str]], *, mode: int = 0o644) -> bytes:
    output = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name, payload, kind in entries:
                info = tarfile.TarInfo(name)
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.mode = mode
                if kind == "file":
                    info.size = len(payload)
                    archive.addfile(info, io.BytesIO(payload))
                elif kind == "symlink":
                    info.type = tarfile.SYMTYPE
                    info.linkname = "LICENSE"
                    archive.addfile(info)
                else:
                    raise AssertionError(kind)
    return output.getvalue()


def test_candidate_archive_is_byte_deterministic_and_self_validating(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    if not _native_linux():
        pytest.skip("the frozen Phase 1 candidate matrix contains only native x86_64 Linux")
    monkeypatch.setattr(CANDIDATE, "verify_version", lambda *args, **kwargs: None)
    binary = Path("/bin/true").resolve()
    first = CANDIDATE.package_binary(binary, tmp_path / "first", commit="1" * 40, tree="2" * 40)
    second = CANDIDATE.package_binary(binary, tmp_path / "second", commit="1" * 40, tree="2" * 40)

    assert first.read_bytes() == second.read_bytes()
    manifest = CANDIDATE.validate(first)
    assert manifest["target"] == CANDIDATE.TARGET
    assert manifest["publication-disposition"] == CANDIDATE.PUBLICATION_DISPOSITION
    assert manifest["candidate-id"].startswith("sha256:")


def test_version_probe_accepts_a_repository_relative_executable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "relative-version"
    executable.write_text("#!/bin/sh\nprintf 'expected-version\\n'\n")
    executable.chmod(0o755)
    monkeypatch.chdir(tmp_path.parent)
    relative = executable.relative_to(tmp_path.parent)
    monkeypatch.setattr(
        CANDIDATE,
        "expected_version_report",
        lambda **kwargs: "expected-version",
    )

    CANDIDATE.verify_version(relative, version="test", commit="1" * 40, tree="2" * 40)


def test_validator_rejects_payload_mutation(
    accepted_archive: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(CANDIDATE, "verify_version", lambda *args, **kwargs: None)
    files = CANDIDATE.safe_archive_files(accepted_archive)
    files[PurePosixPath("bin/type-bridge")] += b"hostile"
    hostile = tmp_path / accepted_archive.name
    hostile.write_bytes(CANDIDATE.encode_archive(files))

    with pytest.raises(CANDIDATE.CandidateError, match="executable identity drifted"):
        CANDIDATE.validate(hostile)


@pytest.mark.parametrize(
    ("name", "kind", "message"),
    [
        ("../escape", "file", "unsafe path"),
        ("bin/type-bridge", "symlink", "non-regular member"),
    ],
)
def test_archive_reader_rejects_unsafe_members(
    tmp_path: Path, name: str, kind: str, message: str
) -> None:
    archive = tmp_path / "hostile.tar.gz"
    archive.write_bytes(_raw_archive([(name, b"payload", kind)]))

    with pytest.raises(CANDIDATE.CandidateError, match=message):
        CANDIDATE.safe_archive_files(archive)


def test_archive_reader_rejects_case_collisions(tmp_path: Path) -> None:
    archive = tmp_path / "hostile.tar.gz"
    archive.write_bytes(_raw_archive([("LICENSE", b"one", "file"), ("license", b"two", "file")]))

    with pytest.raises(CANDIDATE.CandidateError, match="duplicate/colliding"):
        CANDIDATE.safe_archive_files(archive)


def test_manifest_parser_rejects_duplicate_keys() -> None:
    with pytest.raises(CANDIDATE.CandidateError, match="duplicate key"):
        CANDIDATE.load_manifest(b'{"format":"one","format":"two"}\n')


def test_connected_commands_bind_every_exact_workforce_approval() -> None:
    apply = CANDIDATE.migration_apply_arguments()
    rollback = CANDIDATE.migration_rollback_arguments()

    assert apply[:4] == ["migration", "apply", "--environment", "live"]
    assert rollback[:5] == ["migration", "rollback", "--environment", "live", "--execute"]
    for migration_id in CANDIDATE.WORKFORCE_MIGRATIONS:
        assert apply.count(migration_id) == 1
        assert rollback.count(migration_id) == 2
    assert apply.count("--approve") == len(CANDIDATE.WORKFORCE_MIGRATIONS)
    assert rollback.count("--remove") == len(CANDIDATE.WORKFORCE_MIGRATIONS)
    assert rollback.count("--approve") == len(CANDIDATE.WORKFORCE_MIGRATIONS)


def test_frozen_contract_matches_the_candidate_implementation() -> None:
    contract: dict[str, Any] = json.loads(CANDIDATE.CONTRACT_PATH.read_text(encoding="utf-8"))
    assert contract["archive_layouts"]["cli"] == [
        path.as_posix() for path in CANDIDATE.ARCHIVE_MEMBERS
    ]
    assert (
        contract["identities"]["cli"]["archive_stem"].format(target=CANDIDATE.TARGET)
        == f"type-bridge-cli-candidate-{CANDIDATE.TARGET}"
    )
    assert contract["publication_disposition"] == CANDIDATE.PUBLICATION_DISPOSITION


def test_candidate_build_environment_remaps_machine_specific_source_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cargo_home = tmp_path / "private-cargo-home"
    monkeypatch.setenv("CARGO_HOME", str(cargo_home))
    monkeypatch.setenv("RUSTFLAGS", "hostile caller flags")

    environment = CANDIDATE.candidate_build_environment(tmp_path / "target")
    flags = environment["CARGO_ENCODED_RUSTFLAGS"].split("\x1f")

    assert environment["CARGO_INCREMENTAL"] == "0"
    assert environment["SOURCE_DATE_EPOCH"] == "0"
    assert "RUSTFLAGS" not in environment
    assert "-C" in flags
    assert "strip=debuginfo" in flags
    assert f"--remap-path-prefix={cargo_home.resolve()}=/cargo" in flags
    assert any(flag.endswith("=/type-bridge") for flag in flags)
    assert any(flag.endswith("=/type-bridge-core") for flag in flags)
