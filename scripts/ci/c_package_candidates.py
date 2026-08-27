#!/usr/bin/env python3
"""Build, validate, and consume frozen ABI 1.6 C package candidates."""

from __future__ import annotations

import argparse
import gzip
import io
import json
import os
import platform
import re
import shlex
import shutil
import stat
import tarfile
import tempfile
from collections.abc import Mapping, Sequence
from pathlib import Path, PurePosixPath
from typing import Any

import standalone_cli_candidate as shared

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
CONTRACT_PATH = ROOT / "tests/contracts/c-distribution-v1.json"
ABI_PATH = ROOT / "tests/contracts/c-abi-1-6.json"
SCHEMA_PATH = ROOT / "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml"
LICENSE_PATH = ROOT / "LICENSE"
NOTICE_PATH = CORE / "crates/cli/THIRD_PARTY_NOTICES.md"
TARGET = "x86_64-unknown-linux-gnu"
RUNTIME_NAME = "type-bridge-c-runtime"
RUNTIME_LIBRARY = "libtype_bridge_c.so"
PACKAGE_NAME = "tb_workforcev3"
PACKAGE_VERSION = "1.0.0"
ABI_VERSION = "1.6.0"
PUBLICATION_DISPOSITION = "candidate-only-unpublished-unsupported"
RUNTIME_FORMAT = "typebridge.c-runtime-artifact/v1"
PACKAGE_FORMAT = "typebridge.generated-c-package-artifact/v1"
MAX_ARCHIVE_BYTES = 192 * 1024 * 1024
MAX_MEMBER_BYTES = 128 * 1024 * 1024
MAX_EXPANDED_BYTES = 192 * 1024 * 1024

RUNTIME_MEMBERS = (
    PurePosixPath("include/typebridge/type_bridge.h"),
    PurePosixPath("include/typebridge/type_bridge_abi_1_4.h"),
    PurePosixPath("include/typebridge/type_bridge_abi_1_5.h"),
    PurePosixPath("include/typebridge/type_bridge_abi_1_6.h"),
    PurePosixPath(f"lib/{RUNTIME_LIBRARY}"),
    PurePosixPath("lib/cmake/TypeBridge/TypeBridgeConfig.cmake"),
    PurePosixPath("lib/cmake/TypeBridge/TypeBridgeConfigVersion.cmake"),
    PurePosixPath("lib/pkgconfig/type-bridge.pc"),
    PurePosixPath("LICENSE"),
    PurePosixPath("THIRD_PARTY_NOTICES.md"),
    PurePosixPath("artifact-manifest.json"),
)
PACKAGE_MEMBERS = (
    PurePosixPath(f"include/{PACKAGE_NAME}/{PACKAGE_NAME}.h"),
    PurePosixPath(f"src/{PACKAGE_NAME}.c"),
    PurePosixPath(f"lib/cmake/{PACKAGE_NAME}/{PACKAGE_NAME}Config.cmake"),
    PurePosixPath(f"lib/cmake/{PACKAGE_NAME}/{PACKAGE_NAME}ConfigVersion.cmake"),
    PurePosixPath(f"lib/pkgconfig/{PACKAGE_NAME}.pc"),
    PurePosixPath(f"share/{PACKAGE_NAME}/package-manifest.json"),
    PurePosixPath("LICENSE.template"),
)


class CandidateError(RuntimeError):
    """A C package candidate is incomplete, stale, or unsafe."""


def run(
    command: Sequence[str],
    *,
    cwd: Path = ROOT,
    env: Mapping[str, str] | None = None,
) -> str:
    try:
        return shared.run(command, cwd=cwd, env=env)
    except shared.CandidateError as error:
        raise CandidateError(str(error)) from error


def canonical_json(value: object) -> bytes:
    return shared.canonical_json(value)


def sha256(value: bytes) -> str:
    return shared.sha256(value)


def read_regular(path: Path, *, maximum: int = MAX_MEMBER_BYTES) -> bytes:
    try:
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
            raise CandidateError(f"linked or non-regular input: {path}")
        if metadata.st_size < 0 or metadata.st_size > maximum:
            raise CandidateError(f"input exceeds its byte budget: {path}")
        body = path.read_bytes()
    except OSError as error:
        raise CandidateError(f"cannot read {path}: {error}") from error
    if len(body) != metadata.st_size:
        raise CandidateError(f"input changed while being read: {path}")
    return body


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(read_regular(path).decode("utf-8"), object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CandidateError(f"invalid JSON in {path}: {error}") from error
    if not isinstance(value, dict):
        raise CandidateError(f"JSON authority is not an object: {path}")
    return value


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise CandidateError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def git_identity() -> tuple[str, str]:
    try:
        return shared.git_identity()
    except shared.CandidateError as error:
        raise CandidateError(str(error)) from error


def toolchain_identity() -> dict[str, str]:
    toolchain = shared.candidate_toolchain()
    return {
        "cargo": run(["cargo", f"+{toolchain}", "-V"]),
        "rustc": run(["rustc", f"+{toolchain}", "-Vv"]),
        "version": toolchain,
    }


def member_record(path: PurePosixPath, body: bytes) -> dict[str, object]:
    return {
        "mode": "0644",
        "path": path.as_posix(),
        "sha256": sha256(body),
        "size": len(body),
    }


def encode_archive(files: Mapping[PurePosixPath, bytes], members: Sequence[PurePosixPath]) -> bytes:
    output = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=output, mtime=0, compresslevel=9) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for path in members:
                body = files[path]
                info = tarfile.TarInfo(path.as_posix())
                info.size = len(body)
                info.mode = 0o644
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                archive.addfile(info, io.BytesIO(body))
    return output.getvalue()


def publish_archive(output: Path, filename: str, body: bytes) -> Path:
    output.mkdir(parents=True, exist_ok=True)
    destination = output / filename
    temporary = output / f".{filename}.tmp"
    temporary.write_bytes(body)
    os.chmod(temporary, 0o644)
    os.replace(temporary, destination)
    return destination


def safe_archive_files(
    archive_path: Path, members: Sequence[PurePosixPath]
) -> dict[PurePosixPath, bytes]:
    body = read_regular(archive_path, maximum=MAX_ARCHIVE_BYTES)
    if len(body) < 10 or body[:3] != b"\x1f\x8b\x08" or body[3] != 0 or body[4:8] != b"\0" * 4:
        raise CandidateError("archive gzip metadata is not canonical")
    files: dict[PurePosixPath, bytes] = {}
    folded: set[str] = set()
    expanded = 0
    try:
        with tarfile.open(fileobj=io.BytesIO(body), mode="r:gz") as archive:
            for member in archive:
                name = member.name
                if (
                    not name
                    or "\\" in name
                    or name.startswith("/")
                    or "\0" in name
                    or any(part in ("", ".", "..") for part in name.split("/"))
                ):
                    raise CandidateError(f"unsafe archive path: {name!r}")
                if name.casefold() in folded:
                    raise CandidateError(f"duplicate or case-colliding archive path: {name}")
                folded.add(name.casefold())
                if not member.isfile() or member.issym() or member.islnk():
                    raise CandidateError(f"non-regular archive member: {name}")
                if (
                    member.mode != 0o644
                    or member.mtime != 0
                    or member.uid != 0
                    or member.gid != 0
                    or member.uname
                    or member.gname
                ):
                    raise CandidateError(f"archive metadata drifted: {name}")
                if member.size < 0 or member.size > MAX_MEMBER_BYTES:
                    raise CandidateError(f"archive member exceeds its byte budget: {name}")
                expanded += member.size
                if expanded > MAX_EXPANDED_BYTES:
                    raise CandidateError("archive exceeds its expanded byte budget")
                stream = archive.extractfile(member)
                if stream is None:
                    raise CandidateError(f"unreadable archive member: {name}")
                payload = stream.read(member.size + 1)
                if len(payload) != member.size:
                    raise CandidateError(f"archive member size drifted: {name}")
                files[PurePosixPath(name)] = payload
    except CandidateError:
        raise
    except (EOFError, OSError, tarfile.TarError) as error:
        raise CandidateError(f"cannot read archive: {error}") from error
    if tuple(files) != tuple(members):
        raise CandidateError(f"archive layout/order drifted: {[str(path) for path in files]}")
    return files


def write_files(files: Mapping[PurePosixPath, bytes], destination: Path) -> None:
    for relative, body in files.items():
        path = destination.joinpath(*relative.parts)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)
        os.chmod(path, 0o644)


def inspect_shared_elf(library: Path) -> dict[str, object]:
    body = read_regular(library)
    if len(body) < 20 or body[:4] != b"\x7fELF" or body[4:6] != b"\x02\x01":
        raise CandidateError("runtime is not little-endian ELF64")
    if int.from_bytes(body[18:20], "little") != 62:
        raise CandidateError("runtime ELF machine is not x86_64")
    dynamic = run(["readelf", "-dW", str(library)])
    needed = sorted(set(re.findall(r"Shared library: \[([^\]]+)\]", dynamic)))
    soname = re.findall(r"Library soname: \[([^\]]+)\]", dynamic)
    rpath = re.findall(r"Library (?:runpath|rpath): \[([^\]]+)\]", dynamic, flags=re.I)
    if not needed:
        raise CandidateError("runtime has no ELF dependency inventory")
    if rpath:
        raise CandidateError(f"runtime carries a forbidden RPATH/RUNPATH: {rpath}")
    exports = sorted(
        line.split(maxsplit=2)[2]
        for line in run(["nm", "-D", "--defined-only", str(library)]).splitlines()
        if len(line.split(maxsplit=2)) == 3
    )
    required = sorted(
        set(
            re.findall(
                rb"\b(type_bridge_[a-zA-Z0-9_]+)\s*\(",
                b"\n".join(
                    read_regular(CORE / f"crates/c/include/typebridge/{name}")
                    for name in (
                        "type_bridge.h",
                        "type_bridge_abi_1_4.h",
                        "type_bridge_abi_1_5.h",
                        "type_bridge_abi_1_6.h",
                    )
                ),
            )
        )
    )
    required_names = sorted(value.decode("ascii") for value in required)
    missing = sorted(set(required_names) - set(exports))
    if missing:
        raise CandidateError(f"runtime omits header exports: {missing}")
    return {
        "exports": exports,
        "format": "elf64-x86-64-shared",
        "needed": needed,
        "rpath": [],
        "soname": soname[0] if len(soname) == 1 else None,
    }


def embedded_json(source: bytes, label: str) -> dict[str, Any]:
    text = source.decode("utf-8")
    pattern = re.compile(
        rf"static const uint8_t {PACKAGE_NAME}_{label}_chunk_\d+\[\] = \{{\n(.*?)\n\}};",
        re.DOTALL,
    )
    chunks = pattern.findall(text)
    if not chunks:
        raise CandidateError(f"generated source omits embedded {label}")
    body = bytes(
        int(value, 16) for chunk in chunks for value in re.findall(r"0x([0-9a-fA-F]{2})u", chunk)
    )
    try:
        value = json.loads(body, object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CandidateError(f"embedded {label} is invalid: {error}") from error
    if not isinstance(value, dict) or canonical_json(value)[:-1] != body:
        raise CandidateError(f"embedded {label} is not canonical JSON")
    return value


def runtime_manifest(
    files: Mapping[PurePosixPath, bytes], *, commit: str, tree: str, library_path: Path
) -> dict[str, object]:
    payload_members = [member_record(path, files[path]) for path in RUNTIME_MEMBERS[:-1]]
    payload_digest = sha256(canonical_json(payload_members))
    identity = sha256(
        canonical_json(
            {
                "payload-sha256": payload_digest,
                "source-commit": commit,
                "source-tree": tree,
                "target": TARGET,
            }
        )
    )
    library = files[PurePosixPath(f"lib/{RUNTIME_LIBRARY}")]
    return {
        "abi": ABI_VERSION,
        "candidate-id": f"sha256:{identity}",
        "dependencies": inspect_shared_elf(library_path),
        "filename": f"type-bridge-c-runtime-abi-1.6-{TARGET}.tar.gz",
        "format": RUNTIME_FORMAT,
        "members": payload_members,
        "name": RUNTIME_NAME,
        "payload-sha256": payload_digest,
        "publication-disposition": PUBLICATION_DISPOSITION,
        "sha256": sha256(library),
        "size": len(library),
        "source-commit": commit,
        "source-tree": tree,
        "target": TARGET,
        "toolchain": toolchain_identity(),
        "version": "2.1.0",
    }


def build_runtime(output: Path, build_root: Path, *, commit: str, tree: str) -> Path:
    toolchain = shared.candidate_toolchain()
    target_dir = build_root / "cargo"
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    run(
        [
            "cargo",
            f"+{toolchain}",
            "build",
            "--locked",
            "--release",
            "--target",
            TARGET,
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-c",
            "--lib",
        ],
        env=environment,
    )
    library = (target_dir / TARGET / "release" / RUNTIME_LIBRARY).resolve()
    cmake_build = build_root / "runtime-cmake"
    staging = build_root / "runtime-stage"
    run(
        [
            "cmake",
            "-S",
            str(CORE / "crates/c"),
            "-B",
            str(cmake_build),
            f"-DTYPE_BRIDGE_C_LIBRARY={library}",
            "-DCMAKE_INSTALL_LIBDIR=lib",
            "-DCMAKE_INSTALL_INCLUDEDIR=include",
            "-DCMAKE_INSTALL_BINDIR=bin",
        ]
    )
    run(["cmake", "--install", str(cmake_build), "--prefix", str(staging)])
    files = {path: read_regular(staging.joinpath(*path.parts)) for path in RUNTIME_MEMBERS[:-3]}
    files[PurePosixPath("LICENSE")] = read_regular(LICENSE_PATH)
    files[PurePosixPath("THIRD_PARTY_NOTICES.md")] = read_regular(NOTICE_PATH)
    manifest = runtime_manifest(files, commit=commit, tree=tree, library_path=library)
    files[PurePosixPath("artifact-manifest.json")] = canonical_json(manifest)
    archive = encode_archive(files, RUNTIME_MEMBERS)
    return publish_archive(output, str(manifest["filename"]), archive)


def generated_config() -> bytes:
    return f'''# Generated TypeBridge C schema source-package config.
include(CMakeFindDependencyMacro)
find_dependency(TypeBridge 1.6 CONFIG)
get_filename_component(_tb_package_prefix "${{CMAKE_CURRENT_LIST_DIR}}/../../.." ABSOLUTE)
if(NOT TARGET {PACKAGE_NAME}::schema)
  add_library({PACKAGE_NAME}_schema STATIC "${{_tb_package_prefix}}/src/{PACKAGE_NAME}.c")
  add_library({PACKAGE_NAME}::schema ALIAS {PACKAGE_NAME}_schema)
  set_target_properties({PACKAGE_NAME}_schema PROPERTIES EXPORT_NAME schema POSITION_INDEPENDENT_CODE ON)
  target_compile_features({PACKAGE_NAME}_schema PUBLIC c_std_17)
  target_include_directories({PACKAGE_NAME}_schema PUBLIC "${{_tb_package_prefix}}/include")
  target_link_libraries({PACKAGE_NAME}_schema PUBLIC TypeBridge::C)
endif()
set({PACKAGE_NAME}_VERSION "{PACKAGE_VERSION}")
'''.encode()


def generated_config_version() -> bytes:
    return f'''set(PACKAGE_VERSION "{PACKAGE_VERSION}")
if(PACKAGE_FIND_VERSION_MAJOR EQUAL 1 AND NOT PACKAGE_FIND_VERSION VERSION_GREATER PACKAGE_VERSION)
  set(PACKAGE_VERSION_COMPATIBLE TRUE)
endif()
if(PACKAGE_FIND_VERSION VERSION_EQUAL PACKAGE_VERSION)
  set(PACKAGE_VERSION_EXACT TRUE)
endif()
'''.encode()


def generated_pc() -> bytes:
    return f"""prefix=${{pcfiledir}}/../..
includedir=${{prefix}}/include
generated_source=${{prefix}}/src/{PACKAGE_NAME}.c

Name: {PACKAGE_NAME} schema
Description: Generated TypeBridge C schema source package
Version: {PACKAGE_VERSION}
Requires: type-bridge >= 1.6.0, type-bridge < 2.0.0
Cflags: -I${{includedir}}
""".encode()


def build_generation_workspace(root: Path) -> Path:
    workspace = root / "workspace"
    (workspace / "schema").mkdir(parents=True)
    (workspace / "migrations/v2").mkdir(parents=True)
    shutil.copy2(SCHEMA_PATH, workspace / "schema/workforce.yaml")
    (workspace / "schema/schema.yaml").write_text(
        "format: typebridge.schema-set/v1\nsources: [workforce.yaml]\n", encoding="utf-8"
    )
    (workspace / "typebridge.yaml").write_text(
        "format: typebridge.workspace/v1\n"
        "schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n"
        "  managed-scope: workforce-v3-package\n"
        "compatibility:\n  semantic-profile: typedb-3.12.1/v1\n"
        "migrations:\n  directory: migrations/v2\n  app-label: workforcev3\n"
        "bindings:\n  c:\n    output: generated/c\n",
        encoding="utf-8",
    )
    return workspace


def build_generated(output: Path, build_root: Path, *, commit: str, tree: str) -> Path:
    toolchain = shared.candidate_toolchain()
    target_dir = build_root / "cargo"
    environment = os.environ.copy()
    environment.update(
        {
            "CARGO_TARGET_DIR": str(target_dir),
            "TYPE_BRIDGE_BUILD_SOURCE_COMMIT": commit,
            "TYPE_BRIDGE_BUILD_SOURCE_TREE": tree,
        }
    )
    run(
        [
            "cargo",
            f"+{toolchain}",
            "build",
            "--locked",
            "--release",
            "--target",
            TARGET,
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-cli",
            "--bin",
            "type-bridge",
        ],
        env=environment,
    )
    workspace = build_generation_workspace(build_root / "generation")
    binary = (target_dir / TARGET / "release/type-bridge").resolve()
    run([str(binary), "schema", "generate"], cwd=workspace)
    generated = workspace / "generated/c"
    original_header = read_regular(generated / f"include/{PACKAGE_NAME}/models.h")
    original_source = read_regular(generated / "src/models.c")
    old_include = f"#include <{PACKAGE_NAME}/models.h>".encode()
    new_include = f"#include <{PACKAGE_NAME}/{PACKAGE_NAME}.h>".encode()
    if original_source.count(old_include) != 1:
        raise CandidateError("generated source include authority drifted")
    source = original_source.replace(old_include, new_include, 1)
    semantic = embedded_json(source, "semantic_fingerprint_json")
    projection = embedded_json(source, "binding_fingerprint_json")
    projection_digest = projection.get("digest")
    if (
        not isinstance(projection_digest, str)
        or re.fullmatch(r"[0-9a-f]{64}", projection_digest) is None
    ):
        raise CandidateError("generated projection fingerprint is malformed")
    files: dict[PurePosixPath, bytes] = {
        PACKAGE_MEMBERS[0]: original_header,
        PACKAGE_MEMBERS[1]: source,
        PACKAGE_MEMBERS[2]: generated_config(),
        PACKAGE_MEMBERS[3]: generated_config_version(),
        PACKAGE_MEMBERS[4]: generated_pc(),
        PACKAGE_MEMBERS[6]: read_regular(LICENSE_PATH),
    }
    payload_members = [
        member_record(path, files[path]) for path in (*PACKAGE_MEMBERS[:5], PACKAGE_MEMBERS[6])
    ]
    payload_digest = sha256(canonical_json(payload_members))
    identity = sha256(
        canonical_json(
            {
                "payload-sha256": payload_digest,
                "projection-fingerprint": projection_digest,
                "source-commit": commit,
                "source-tree": tree,
            }
        )
    )
    filename = f"{PACKAGE_NAME}-c-{projection_digest}.tar.gz"
    manifest = {
        "candidate-id": f"sha256:{identity}",
        "declared-schema-source": "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml",
        "declared-schema-source-sha256": sha256(read_regular(SCHEMA_PATH)),
        "filename": filename,
        "format": PACKAGE_FORMAT,
        "members": payload_members,
        "name": PACKAGE_NAME,
        "payload-sha256": payload_digest,
        "projection-fingerprint": projection,
        "publication-disposition": PUBLICATION_DISPOSITION,
        "required-runtime-abi": {"minimum-inclusive": "1.6.0", "maximum-exclusive": "2.0.0"},
        "semantic-fingerprint": semantic,
        "source-commit": commit,
        "source-generated": {
            "header-sha256": sha256(original_header),
            "source-sha256": sha256(original_source),
        },
        "source-tree": tree,
        "version": PACKAGE_VERSION,
    }
    files[PACKAGE_MEMBERS[5]] = canonical_json(manifest)
    archive = encode_archive(files, PACKAGE_MEMBERS)
    return publish_archive(output, filename, archive)


def build(output: Path) -> dict[str, str]:
    if platform.system() != "Linux" or platform.machine() not in {"x86_64", "AMD64"}:
        raise CandidateError(f"C candidates require native {TARGET}")
    contract = load_json(CONTRACT_PATH)
    if contract.get("publication_disposition") != PUBLICATION_DISPOSITION:
        raise CandidateError("frozen publication disposition drifted")
    commit, tree = git_identity()
    build_root = output / "build"
    runtime = build_runtime(output, build_root, commit=commit, tree=tree)
    generated = build_generated(output, build_root, commit=commit, tree=tree)
    validate_runtime(runtime)
    validate_generated(generated)
    return {"generated-package": str(generated), "runtime": str(runtime)}


def validate_runtime(archive: Path) -> dict[str, Any]:
    files = safe_archive_files(archive, RUNTIME_MEMBERS)
    manifest_body = files[RUNTIME_MEMBERS[-1]]
    manifest = json.loads(manifest_body, object_pairs_hook=unique_object)
    if not isinstance(manifest, dict) or canonical_json(manifest) != manifest_body:
        raise CandidateError("runtime manifest is not canonical JSON")
    if manifest.get("format") != RUNTIME_FORMAT or manifest.get("abi") != ABI_VERSION:
        raise CandidateError("runtime format or ABI drifted")
    if manifest.get("filename") != archive.name or manifest.get("target") != TARGET:
        raise CandidateError("runtime filename or target drifted")
    expected = [member_record(path, files[path]) for path in RUNTIME_MEMBERS[:-1]]
    if manifest.get("members") != expected:
        raise CandidateError("runtime member manifest drifted")
    payload_digest = sha256(canonical_json(expected))
    if manifest.get("payload-sha256") != payload_digest:
        raise CandidateError("runtime payload digest drifted")
    if files[PurePosixPath("LICENSE")] != read_regular(LICENSE_PATH):
        raise CandidateError("runtime license drifted")
    if files[PurePosixPath("THIRD_PARTY_NOTICES.md")] != read_regular(NOTICE_PATH):
        raise CandidateError("runtime notice drifted")
    with tempfile.TemporaryDirectory(prefix="type-bridge-runtime-validate-") as temporary:
        root = Path(temporary)
        write_files(files, root)
        observed = inspect_shared_elf(root / f"lib/{RUNTIME_LIBRARY}")
    if manifest.get("dependencies") != observed:
        raise CandidateError("runtime dependency/export inventory drifted")
    if manifest.get("sha256") != sha256(files[PurePosixPath(f"lib/{RUNTIME_LIBRARY}")]):
        raise CandidateError("runtime library digest drifted")
    if manifest.get("publication-disposition") != PUBLICATION_DISPOSITION:
        raise CandidateError("runtime publication disposition widened")
    return manifest


def validate_generated(archive: Path) -> dict[str, Any]:
    files = safe_archive_files(archive, PACKAGE_MEMBERS)
    manifest_body = files[PACKAGE_MEMBERS[5]]
    manifest = json.loads(manifest_body, object_pairs_hook=unique_object)
    if not isinstance(manifest, dict) or canonical_json(manifest) != manifest_body:
        raise CandidateError("generated package manifest is not canonical JSON")
    if manifest.get("format") != PACKAGE_FORMAT or manifest.get("name") != PACKAGE_NAME:
        raise CandidateError("generated package identity drifted")
    if manifest.get("filename") != archive.name:
        raise CandidateError("generated package filename drifted")
    expected_paths = (*PACKAGE_MEMBERS[:5], PACKAGE_MEMBERS[6])
    expected = [member_record(path, files[path]) for path in expected_paths]
    if manifest.get("members") != expected:
        raise CandidateError("generated package member manifest drifted")
    if manifest.get("payload-sha256") != sha256(canonical_json(expected)):
        raise CandidateError("generated package payload digest drifted")
    source = files[PACKAGE_MEMBERS[1]]
    projection = embedded_json(source, "binding_fingerprint_json")
    semantic = embedded_json(source, "semantic_fingerprint_json")
    if (
        manifest.get("projection-fingerprint") != projection
        or manifest.get("semantic-fingerprint") != semantic
    ):
        raise CandidateError("generated embedded compatibility identity drifted")
    digest = projection.get("digest")
    if archive.name != f"{PACKAGE_NAME}-c-{digest}.tar.gz":
        raise CandidateError("generated archive name is not projection-bound")
    if files[PACKAGE_MEMBERS[6]] != read_regular(LICENSE_PATH):
        raise CandidateError("generated license template drifted")
    config = files[PACKAGE_MEMBERS[2]].decode("utf-8")
    pc = files[PACKAGE_MEMBERS[4]].decode("utf-8")
    if "find_dependency(TypeBridge 1.6 CONFIG)" not in config:
        raise CandidateError("generated CMake runtime ABI fence drifted")
    if "type-bridge >= 1.6.0, type-bridge < 2.0.0" not in pc:
        raise CandidateError("generated pkg-config runtime ABI fence drifted")
    if manifest.get("publication-disposition") != PUBLICATION_DISPOSITION:
        raise CandidateError("generated publication disposition widened")
    return manifest


def cmake_consumer_source() -> tuple[bytes, bytes, bytes]:
    cmake = f"""cmake_minimum_required(VERSION 3.20)
project(type_bridge_artifact_consumer LANGUAGES C CXX)
find_package(TypeBridge 1.6.0 EXACT CONFIG REQUIRED)
find_package({PACKAGE_NAME} 1.0.0 EXACT CONFIG REQUIRED)
add_executable(consumer main.c)
add_library(cpp_header OBJECT header.cpp)
set_target_properties(consumer PROPERTIES C_STANDARD 17 C_STANDARD_REQUIRED YES C_EXTENSIONS NO)
set_target_properties(cpp_header PROPERTIES CXX_STANDARD 17 CXX_STANDARD_REQUIRED YES CXX_EXTENSIONS NO)
target_link_libraries(consumer PRIVATE {PACKAGE_NAME}::schema)
target_link_libraries(cpp_header PRIVATE {PACKAGE_NAME}::schema)
if(MSVC)
  target_compile_options(consumer PRIVATE /W4 /WX)
else()
  target_compile_options(consumer PRIVATE -Wall -Wextra -Werror -pedantic-errors)
  target_compile_options(cpp_header PRIVATE -Wall -Wextra -Werror -pedantic-errors)
endif()
""".encode()
    main = f"""#include <string.h>
#include <{PACKAGE_NAME}/{PACKAGE_NAME}.h>
int main(void) {{
  type_bridge_byte_view_t version = {{0}};
  type_bridge_schema_package_t *package = NULL;
  type_bridge_execution_diagnostics_t *diagnostics = NULL;
  if (type_bridge_runtime_version(&version) != TYPE_BRIDGE_STATUS_OK ||
      version.length != 5u || memcmp(version.data, "2.1.0", 5u) != 0) return 10;
  if ({PACKAGE_NAME}_schema_package_open_v2(&package, &diagnostics) != TYPE_BRIDGE_STATUS_OK ||
      package == NULL || diagnostics != NULL) return 11;
  if (type_bridge_schema_package_close(&package) != TYPE_BRIDGE_STATUS_OK || package != NULL) return 12;
  return 0;
}}
""".encode()
    cpp = f"""#include <type_traits>
#include <{PACKAGE_NAME}/{PACKAGE_NAME}.h>
static_assert(std::is_standard_layout_v<type_bridge_byte_view_t>);
void type_bridge_cpp_header_probe() {{ auto open = &{PACKAGE_NAME}_schema_package_open_v2; (void)open; }}
""".encode()
    return cmake, main, cpp


def smoke(runtime_archive: Path, generated_archive: Path) -> dict[str, object]:
    runtime_manifest_value = validate_runtime(runtime_archive)
    generated_manifest_value = validate_generated(generated_archive)
    runtime_files = safe_archive_files(runtime_archive, RUNTIME_MEMBERS)
    generated_files = safe_archive_files(generated_archive, PACKAGE_MEMBERS)
    with tempfile.TemporaryDirectory(prefix="type-bridge-c-consumer-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime prefix"
        generated = root / "generated prefix"
        runtime.mkdir()
        generated.mkdir()
        write_files(runtime_files, runtime)
        write_files(generated_files, generated)
        relocated_runtime = root / "relocated runtime ü"
        relocated_generated = root / "relocated generated ü"
        runtime.rename(relocated_runtime)
        generated.rename(relocated_generated)
        source = root / "consumer source"
        build = root / "consumer build"
        source.mkdir()
        cmake, main, cpp = cmake_consumer_source()
        (source / "CMakeLists.txt").write_bytes(cmake)
        (source / "main.c").write_bytes(main)
        (source / "header.cpp").write_bytes(cpp)
        environment = os.environ.copy()
        environment["LD_LIBRARY_PATH"] = str(relocated_runtime / "lib")
        run(
            [
                "cmake",
                "-S",
                str(source),
                "-B",
                str(build),
                f"-DTypeBridge_DIR={relocated_runtime / 'lib/cmake/TypeBridge'}",
                f"-D{PACKAGE_NAME}_DIR={relocated_generated / f'lib/cmake/{PACKAGE_NAME}'}",
            ],
            env=environment,
        )
        run(["cmake", "--build", str(build), "--parallel", "2"], env=environment)
        run([str(build / "consumer")], env=environment)

        pkg_source_output = run(
            ["pkg-config", "--variable=generated_source", PACKAGE_NAME],
            env={
                **environment,
                "PKG_CONFIG_PATH": f"{relocated_generated / 'lib/pkgconfig'}:{relocated_runtime / 'lib/pkgconfig'}",
            },
        )
        pkg_sources = shlex.split(pkg_source_output)
        if len(pkg_sources) != 1:
            raise CandidateError("pkg-config generated source path is ambiguous")
        pkg_source = pkg_sources[0]
        pkg_environment = {
            **environment,
            "PKG_CONFIG_PATH": f"{relocated_generated / 'lib/pkgconfig'}:{relocated_runtime / 'lib/pkgconfig'}",
        }
        flags = shlex.split(
            run(["pkg-config", "--cflags", "--libs", PACKAGE_NAME], env=pkg_environment)
        )
        pkg_executable = root / "pkg consumer"
        run(
            [
                "cc",
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
                str(source / "main.c"),
                pkg_source,
                *flags,
                "-o",
                str(pkg_executable),
            ],
            env=pkg_environment,
        )
        run([str(pkg_executable)], env=pkg_environment)
        shutil.rmtree(relocated_runtime)
        shutil.rmtree(relocated_generated)
        if relocated_runtime.exists() or relocated_generated.exists():
            raise CandidateError("candidate uninstall left package files behind")
    return {
        "cmake-c17-cpp17": True,
        "generated-candidate-id": generated_manifest_value["candidate-id"],
        "pkg-config-c17": True,
        "relocation": True,
        "runtime-candidate-id": runtime_manifest_value["candidate-id"],
        "uninstall": "clean",
    }


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build")
    build_parser.add_argument("--output", type=Path, required=True)
    runtime_parser = commands.add_parser("validate-runtime")
    runtime_parser.add_argument("archive", type=Path)
    generated_parser = commands.add_parser("validate-generated")
    generated_parser.add_argument("archive", type=Path)
    smoke_parser = commands.add_parser("smoke")
    smoke_parser.add_argument("runtime", type=Path)
    smoke_parser.add_argument("generated", type=Path)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    arguments = parse_args(argv)
    try:
        if arguments.command == "build":
            result: object = build(arguments.output)
        elif arguments.command == "validate-runtime":
            result = validate_runtime(arguments.archive)
        elif arguments.command == "validate-generated":
            result = validate_generated(arguments.archive)
        else:
            result = smoke(arguments.runtime, arguments.generated)
    except (CandidateError, shared.CandidateError) as error:
        print(f"C package candidate rejected: {error}", file=os.sys.stderr)
        return 1
    print(canonical_json(result).decode("utf-8"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
