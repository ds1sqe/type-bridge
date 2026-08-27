"""Fail-closed tests for native runtime and generated C package candidates."""

from __future__ import annotations

import gzip
import importlib.util
import io
import json
import sys
import tarfile
from pathlib import Path, PurePosixPath

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT_DIRECTORY = ROOT / "scripts/ci"
sys.path.insert(0, str(SCRIPT_DIRECTORY))
SPEC = importlib.util.spec_from_file_location(
    "c_package_candidates", SCRIPT_DIRECTORY / "c_package_candidates.py"
)
assert SPEC is not None and SPEC.loader is not None
CANDIDATE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = CANDIDATE
SPEC.loader.exec_module(CANDIDATE)


def _raw_archive(entries: list[tuple[str, bytes, str]]) -> bytes:
    output = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name, payload, kind in entries:
                info = tarfile.TarInfo(name)
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.mode = 0o644
                if kind == "file":
                    info.size = len(payload)
                    archive.addfile(info, io.BytesIO(payload))
                else:
                    info.type = tarfile.SYMTYPE
                    info.linkname = "LICENSE"
                    archive.addfile(info)
    return output.getvalue()


def test_frozen_archive_layouts_match_candidate_implementation() -> None:
    contract = json.loads(CANDIDATE.CONTRACT_PATH.read_text(encoding="utf-8"))
    runtime_layout = [
        item.replace("{shared-library}", CANDIDATE.RUNTIME_LIBRARY)
        for item in contract["archive_layouts"]["runtime"]
    ]
    package_layout = [
        item.replace("{schema-package}", CANDIDATE.PACKAGE_NAME)
        for item in contract["archive_layouts"]["generated_package"]
    ]

    assert runtime_layout == [path.as_posix() for path in CANDIDATE.RUNTIME_MEMBERS]
    assert package_layout == [path.as_posix() for path in CANDIDATE.PACKAGE_MEMBERS]
    assert contract["abi"]["version"] == CANDIDATE.ABI_VERSION
    assert contract["publication_disposition"] == CANDIDATE.PUBLICATION_DISPOSITION


def test_archive_encoding_is_byte_deterministic_and_exact(tmp_path: Path) -> None:
    members = (PurePosixPath("one"), PurePosixPath("nested/two"))
    files = {members[0]: b"first", members[1]: b"second"}
    first = CANDIDATE.encode_archive(files, members)
    second = CANDIDATE.encode_archive(files, members)
    archive = tmp_path / "candidate.tar.gz"
    archive.write_bytes(first)

    assert first == second
    assert CANDIDATE.safe_archive_files(archive, members) == files


@pytest.mark.parametrize(
    ("name", "kind", "message"),
    [
        ("../escape", "file", "unsafe archive path"),
        ("member", "symlink", "non-regular archive member"),
    ],
)
def test_archive_reader_rejects_unsafe_members(
    tmp_path: Path, name: str, kind: str, message: str
) -> None:
    archive = tmp_path / "hostile.tar.gz"
    archive.write_bytes(_raw_archive([(name, b"payload", kind)]))

    with pytest.raises(CANDIDATE.CandidateError, match=message):
        CANDIDATE.safe_archive_files(archive, (PurePosixPath("member"),))


def test_generated_metadata_has_exact_runtime_fences_and_source_discovery() -> None:
    config = CANDIDATE.generated_config().decode()
    package_config = CANDIDATE.generated_pc().decode()

    assert "find_dependency(TypeBridge 1.6 CONFIG)" in config
    assert f"src/{CANDIDATE.PACKAGE_NAME}.c" in config
    assert "type-bridge >= 1.6.0, type-bridge < 2.0.0" in package_config
    assert f"generated_source=${{prefix}}/src/{CANDIDATE.PACKAGE_NAME}.c" in package_config


def test_embedded_fingerprint_parser_requires_canonical_json() -> None:
    value = {
        "algorithm": "sha256",
        "canonicalization": "test/v1",
        "digest": "1" * 64,
        "domain": "test",
    }
    body = CANDIDATE.canonical_json(value)[:-1]
    literals = ", ".join(f"0x{byte:02x}u" for byte in body)
    source = (
        f"static const uint8_t {CANDIDATE.PACKAGE_NAME}_binding_fingerprint_json_chunk_0[] = {{\n"
        f"  {literals},\n"
        "};\n"
    ).encode()

    assert CANDIDATE.embedded_json(source, "binding_fingerprint_json") == value
    noncanonical = json.dumps(value, indent=2).encode()
    noncanonical_literals = ", ".join(f"0x{byte:02x}u" for byte in noncanonical)
    hostile = (
        f"static const uint8_t {CANDIDATE.PACKAGE_NAME}_binding_fingerprint_json_chunk_0[] = {{\n"
        f"  {noncanonical_literals},\n"
        "};\n"
    ).encode()
    with pytest.raises(CANDIDATE.CandidateError, match="not canonical JSON"):
        CANDIDATE.embedded_json(hostile, "binding_fingerprint_json")
