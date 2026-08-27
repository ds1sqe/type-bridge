#!/usr/bin/env python3
"""Build, validate, and consume the frozen standalone CLI candidate archive."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from collections.abc import Mapping, Sequence
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
CONTRACT_PATH = ROOT / "tests/contracts/c-distribution-v1.json"
LICENSE_PATH = ROOT / "LICENSE"
NOTICE_PATH = CORE / "crates/cli/THIRD_PARTY_NOTICES.md"
CLI_MANIFEST = CORE / "crates/cli/Cargo.toml"
TOOLCHAIN_INVENTORY = ROOT / "scripts/ci/cargo_release_inventory.toml"
WORKFORCE = ROOT / "tests/contracts/sdk_conformance/workforce-v4/workspace"
FORMAT = "typebridge.standalone-cli-artifact/v1"
TARGET = "x86_64-unknown-linux-gnu"
PUBLICATION_DISPOSITION = "candidate-only-unpublished-unsupported"
SEMANTIC_PROFILES = ["typedb-3.11.5/v1", "typedb-3.12.1/v1"]
WORKFORCE_MIGRATIONS = (
    "workforcev4/0001_initial",
    "workforcev4/0002_expand-display-name",
    "workforcev4/0003_backfill-display-name",
    "workforcev4/0004_contract-legacy-name",
)
ARCHIVE_MEMBERS = (
    PurePosixPath("bin/type-bridge"),
    PurePosixPath("LICENSE"),
    PurePosixPath("THIRD_PARTY_NOTICES.md"),
    PurePosixPath("artifact-manifest.json"),
)
MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
MAX_MEMBER_BYTES = 96 * 1024 * 1024
MAX_EXPANDED_BYTES = 128 * 1024 * 1024
HEX40 = re.compile(r"[0-9a-f]{40}\Z")


class CandidateError(RuntimeError):
    """A standalone CLI candidate is incomplete, stale, or unsafe."""


def canonical_json(payload: object) -> bytes:
    return (
        json.dumps(
            payload,
            ensure_ascii=False,
            allow_nan=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        + b"\n"
    )


def sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def read_regular(path: Path, *, maximum: int, label: str) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise CandidateError(f"cannot inspect {label} {path}: {error}") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise CandidateError(f"{label} is linked or non-regular: {path}")
    if metadata.st_size < 0 or metadata.st_size > maximum:
        raise CandidateError(f"{label} exceeds its byte budget: {metadata.st_size}")
    try:
        payload = path.read_bytes()
    except OSError as error:
        raise CandidateError(f"cannot read {label} {path}: {error}") from error
    if len(payload) != metadata.st_size:
        raise CandidateError(f"{label} changed while it was read: {path}")
    return payload


def run(command: Sequence[str], *, cwd: Path = ROOT, env: Mapping[str, str] | None = None) -> str:
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=None if env is None else dict(env),
            check=True,
            capture_output=True,
            text=True,
            timeout=900,
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        detail = ""
        if isinstance(error, subprocess.CalledProcessError):
            detail = (error.stderr or error.stdout or "").strip()
        raise CandidateError(f"command failed: {' '.join(command)}: {detail or error}") from error
    return result.stdout.strip()


def load_contract() -> dict[str, Any]:
    try:
        contract = json.loads(CONTRACT_PATH.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise CandidateError(f"cannot read frozen distribution contract: {error}") from error
    if not isinstance(contract, dict):
        raise CandidateError("frozen distribution contract is not an object")
    return contract


def package_version() -> str:
    try:
        manifest = tomllib.loads(CLI_MANIFEST.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise CandidateError(f"cannot read CLI version authority: {error}") from error
    version = manifest.get("package", {}).get("version")
    if not isinstance(version, str) or not version:
        raise CandidateError("CLI version authority has no package version")
    return version


def candidate_toolchain() -> str:
    try:
        inventory = tomllib.loads(TOOLCHAIN_INVENTORY.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise CandidateError(f"cannot read candidate toolchain authority: {error}") from error
    toolchain = inventory.get("candidate-toolchain")
    if not isinstance(toolchain, str) or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", toolchain) is None:
        raise CandidateError("candidate toolchain authority is malformed")
    return toolchain


def candidate_build_environment(target_directory: Path) -> dict[str, str]:
    """Return the fixed native-candidate build environment.

    Rust retains source locations used by panic diagnostics even without debug
    information. Remap every machine-specific source prefix before compilation
    so accepted binaries do not disclose checkout, Cargo-home, or user-home
    paths and remain reproducible across runners.
    """
    environment = os.environ.copy()
    cargo_home = Path(environment.get("CARGO_HOME", Path.home() / ".cargo")).resolve()
    mappings = (
        (CORE.resolve(), Path("/type-bridge-core")),
        (ROOT.resolve(), Path("/type-bridge")),
        (cargo_home, Path("/cargo")),
        (Path.home().resolve(), Path("/build-home")),
    )
    flags = ["-C", "strip=debuginfo"]
    flags.extend(f"--remap-path-prefix={source}={destination}" for source, destination in mappings)
    environment.pop("RUSTFLAGS", None)
    environment.update(
        {
            "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags),
            "CARGO_INCREMENTAL": "0",
            "CARGO_TARGET_DIR": str(target_directory),
            "SOURCE_DATE_EPOCH": "0",
        }
    )
    return environment


def git_identity() -> tuple[str, str]:
    status = run(["git", "status", "--porcelain=v1", "--untracked-files=no"])
    if status:
        raise CandidateError("candidate source has tracked changes; commit them before building")
    commit = run(["git", "rev-parse", "HEAD"])
    tree = run(["git", "rev-parse", "HEAD^{tree}"])
    if HEX40.fullmatch(commit) is None or HEX40.fullmatch(tree) is None:
        raise CandidateError("Git returned a malformed source identity")
    return commit, tree


def inspect_elf(binary: Path) -> dict[str, Any]:
    payload = read_regular(binary, maximum=MAX_MEMBER_BYTES, label="CLI executable")
    if len(payload) < 20 or payload[:4] != b"\x7fELF":
        raise CandidateError("CLI executable is not ELF")
    if payload[4:6] != b"\x02\x01" or int.from_bytes(payload[18:20], "little") != 62:
        raise CandidateError("CLI executable is not little-endian ELF64 x86_64")
    dynamic = run(["readelf", "-dW", str(binary)])
    needed = sorted(set(re.findall(r"Shared library: \[([^\]]+)\]", dynamic)))
    if not needed:
        raise CandidateError("CLI executable has no recorded ELF dependency closure")
    program = run(["readelf", "-lW", str(binary)])
    interpreter_match = re.search(r"Requesting program interpreter: ([^\]]+)", program)
    if interpreter_match is None:
        raise CandidateError("CLI executable has no ELF interpreter")
    interpreter = interpreter_match.group(1)
    if interpreter != "/lib64/ld-linux-x86-64.so.2":
        raise CandidateError(f"unexpected ELF interpreter: {interpreter}")
    return {"format": "elf64-x86-64", "interpreter": interpreter, "needed": needed}


def expected_version_report(*, version: str, commit: str, tree: str) -> str:
    return (
        f"type-bridge {version}\n"
        f"target: {TARGET}\n"
        f"semantic-profiles: {','.join(SEMANTIC_PROFILES)}\n"
        f"source-commit: {commit}\n"
        f"source-tree: {tree}"
    )


def verify_version(binary: Path, *, version: str, commit: str, tree: str) -> None:
    executable = binary.resolve()
    observed = run([str(executable), "--version"], cwd=executable.parent)
    expected = expected_version_report(version=version, commit=commit, tree=tree)
    if observed != expected:
        raise CandidateError(
            f"CLI build identity drifted: actual={observed!r}, expected={expected!r}"
        )


def tar_info(path: PurePosixPath, payload: bytes) -> tarfile.TarInfo:
    info = tarfile.TarInfo(path.as_posix())
    info.size = len(payload)
    info.mode = 0o755 if path == PurePosixPath("bin/type-bridge") else 0o644
    info.mtime = 0
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    return info


def encode_archive(files: Mapping[PurePosixPath, bytes]) -> bytes:
    compressed = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=compressed, mtime=0, compresslevel=9) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for path in ARCHIVE_MEMBERS:
                payload = files[path]
                archive.addfile(tar_info(path, payload), io.BytesIO(payload))
    return compressed.getvalue()


def build_manifest(
    *, binary: bytes, dependencies: dict[str, Any], version: str, commit: str, tree: str
) -> dict[str, Any]:
    license_body = read_regular(LICENSE_PATH, maximum=1024 * 1024, label="license")
    notice = read_regular(NOTICE_PATH, maximum=8 * 1024 * 1024, label="third-party notice")
    binary_digest = sha256(binary)
    identity_input = canonical_json(
        {"source-commit": commit, "source-tree": tree, "target": TARGET, "sha256": binary_digest}
    )
    toolchain = candidate_toolchain()
    return {
        "abi": "not-applicable",
        "candidate-id": f"sha256:{sha256(identity_input)}",
        "dependencies": dependencies,
        "filename": f"type-bridge-cli-candidate-{TARGET}.tar.gz",
        "format": FORMAT,
        "members": [
            {
                "mode": "0755",
                "path": "bin/type-bridge",
                "sha256": binary_digest,
                "size": len(binary),
            },
            {
                "mode": "0644",
                "path": "LICENSE",
                "sha256": sha256(license_body),
                "size": len(license_body),
            },
            {
                "mode": "0644",
                "path": "THIRD_PARTY_NOTICES.md",
                "sha256": sha256(notice),
                "size": len(notice),
            },
        ],
        "name": "type-bridge",
        "publication-disposition": PUBLICATION_DISPOSITION,
        "semantic-profiles": SEMANTIC_PROFILES,
        "sha256": binary_digest,
        "size": len(binary),
        "source-commit": commit,
        "source-tree": tree,
        "target": TARGET,
        "toolchain": {
            "cargo": run(["cargo", f"+{toolchain}", "-V"]),
            "rustc": run(["rustc", f"+{toolchain}", "-Vv"]),
            "version": toolchain,
        },
        "version": version,
    }


def package_binary(binary_path: Path, output_directory: Path, *, commit: str, tree: str) -> Path:
    binary = read_regular(binary_path, maximum=MAX_MEMBER_BYTES, label="CLI executable")
    version = package_version()
    verify_version(binary_path, version=version, commit=commit, tree=tree)
    dependencies = inspect_elf(binary_path)
    manifest = build_manifest(
        binary=binary,
        dependencies=dependencies,
        version=version,
        commit=commit,
        tree=tree,
    )
    files = {
        PurePosixPath("bin/type-bridge"): binary,
        PurePosixPath("LICENSE"): read_regular(LICENSE_PATH, maximum=1024 * 1024, label="license"),
        PurePosixPath("THIRD_PARTY_NOTICES.md"): read_regular(
            NOTICE_PATH, maximum=8 * 1024 * 1024, label="third-party notice"
        ),
        PurePosixPath("artifact-manifest.json"): canonical_json(manifest),
    }
    archive = encode_archive(files)
    output_directory.mkdir(parents=True, exist_ok=True)
    destination = output_directory / manifest["filename"]
    temporary = output_directory / f".{destination.name}.tmp"
    temporary.write_bytes(archive)
    os.chmod(temporary, 0o644)
    os.replace(temporary, destination)
    return destination


def safe_archive_files(archive_path: Path) -> dict[PurePosixPath, bytes]:
    body = read_regular(archive_path, maximum=MAX_ARCHIVE_BYTES, label="CLI archive")
    if len(body) < 10 or body[:3] != b"\x1f\x8b\x08" or body[3] != 0 or body[4:8] != b"\0" * 4:
        raise CandidateError("CLI archive has non-canonical gzip metadata")
    files: dict[PurePosixPath, bytes] = {}
    names: set[str] = set()
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
                    raise CandidateError(f"CLI archive contains an unsafe path: {name!r}")
                if name in names or name.casefold() in folded:
                    raise CandidateError(f"CLI archive contains a duplicate/colliding path: {name}")
                names.add(name)
                folded.add(name.casefold())
                if not member.isfile() or member.issym() or member.islnk():
                    raise CandidateError(f"CLI archive contains a non-regular member: {name}")
                path = PurePosixPath(name)
                expected_mode = 0o755 if path == PurePosixPath("bin/type-bridge") else 0o644
                if (
                    member.mode != expected_mode
                    or member.mtime != 0
                    or member.uid != 0
                    or member.gid != 0
                    or member.uname
                    or member.gname
                ):
                    raise CandidateError(f"CLI archive metadata drifted: {name}")
                if member.size < 0 or member.size > MAX_MEMBER_BYTES:
                    raise CandidateError(f"CLI archive member exceeds its byte budget: {name}")
                expanded += member.size
                if expanded > MAX_EXPANDED_BYTES:
                    raise CandidateError("CLI archive exceeds its expanded byte budget")
                source = archive.extractfile(member)
                if source is None:
                    raise CandidateError(f"CLI archive member is unreadable: {name}")
                payload = source.read(member.size + 1)
                if len(payload) != member.size:
                    raise CandidateError(f"CLI archive member size drifted: {name}")
                files[path] = payload
    except CandidateError:
        raise
    except (EOFError, OSError, tarfile.TarError) as error:
        raise CandidateError(f"cannot read CLI archive: {error}") from error
    if tuple(files) != ARCHIVE_MEMBERS:
        raise CandidateError(
            f"CLI archive member order/layout drifted: actual={[str(path) for path in files]}"
        )
    return files


def load_manifest(body: bytes) -> dict[str, Any]:
    try:
        manifest = json.loads(body, object_pairs_hook=_unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CandidateError(f"artifact manifest is invalid JSON: {error}") from error
    if not isinstance(manifest, dict) or canonical_json(manifest) != body:
        raise CandidateError("artifact manifest is not canonical JSON")
    return manifest


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise CandidateError(f"artifact manifest has duplicate key: {key}")
        result[key] = value
    return result


def write_validated_files(files: Mapping[PurePosixPath, bytes], destination: Path) -> None:
    for relative, payload in files.items():
        path = destination.joinpath(*relative.parts)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(payload)
        os.chmod(path, 0o755 if relative == PurePosixPath("bin/type-bridge") else 0o644)


def validate(archive_path: Path) -> dict[str, Any]:
    files = safe_archive_files(archive_path)
    manifest = load_manifest(files[PurePosixPath("artifact-manifest.json")])
    required = {
        "abi",
        "candidate-id",
        "dependencies",
        "filename",
        "format",
        "members",
        "name",
        "publication-disposition",
        "semantic-profiles",
        "sha256",
        "size",
        "source-commit",
        "source-tree",
        "target",
        "toolchain",
        "version",
    }
    if set(manifest) != required:
        raise CandidateError(
            f"artifact manifest fields drifted: {sorted(set(manifest) ^ required)}"
        )
    if manifest["format"] != FORMAT or manifest["name"] != "type-bridge":
        raise CandidateError("artifact identity drifted")
    if manifest["target"] != TARGET or manifest["filename"] != archive_path.name:
        raise CandidateError("artifact target or filename drifted")
    if manifest["publication-disposition"] != PUBLICATION_DISPOSITION:
        raise CandidateError("artifact publication disposition widened")
    if manifest["semantic-profiles"] != SEMANTIC_PROFILES or manifest["abi"] != "not-applicable":
        raise CandidateError("artifact semantic profile or ABI classification drifted")
    commit = manifest["source-commit"]
    tree = manifest["source-tree"]
    if not isinstance(commit, str) or HEX40.fullmatch(commit) is None:
        raise CandidateError("artifact source commit is malformed")
    if not isinstance(tree, str) or HEX40.fullmatch(tree) is None:
        raise CandidateError("artifact source tree is malformed")
    binary = files[PurePosixPath("bin/type-bridge")]
    if manifest["sha256"] != sha256(binary) or manifest["size"] != len(binary):
        raise CandidateError("artifact executable identity drifted")
    expected_id = sha256(
        canonical_json(
            {
                "source-commit": commit,
                "source-tree": tree,
                "target": TARGET,
                "sha256": sha256(binary),
            }
        )
    )
    if manifest["candidate-id"] != f"sha256:{expected_id}":
        raise CandidateError("candidate identity drifted")
    expected_members = []
    for relative in ARCHIVE_MEMBERS[:-1]:
        payload = files[relative]
        expected_members.append(
            {
                "mode": "0755" if relative == PurePosixPath("bin/type-bridge") else "0644",
                "path": relative.as_posix(),
                "sha256": sha256(payload),
                "size": len(payload),
            }
        )
    if manifest["members"] != expected_members:
        raise CandidateError("artifact payload manifest drifted")
    if files[PurePosixPath("LICENSE")] != LICENSE_PATH.read_bytes():
        raise CandidateError("artifact license drifted")
    if files[PurePosixPath("THIRD_PARTY_NOTICES.md")] != NOTICE_PATH.read_bytes():
        raise CandidateError("artifact third-party notice drifted")
    with tempfile.TemporaryDirectory(prefix="type-bridge-cli-validate-") as temporary:
        root = Path(temporary)
        write_validated_files(files, root)
        binary_path = root / "bin/type-bridge"
        dependencies = inspect_elf(binary_path)
        if manifest["dependencies"] != dependencies:
            raise CandidateError("artifact ELF dependency inventory drifted")
        verify_version(binary_path, version=manifest["version"], commit=commit, tree=tree)
    if manifest["version"] != package_version():
        raise CandidateError("artifact product version drifted")
    toolchain = manifest["toolchain"]
    if not isinstance(toolchain, dict) or toolchain.get("version") != candidate_toolchain():
        raise CandidateError("artifact toolchain identity drifted")
    return manifest


def build(output_directory: Path) -> Path:
    if platform.system() != "Linux" or platform.machine() not in {"x86_64", "AMD64"}:
        raise CandidateError(f"candidate build requires native {TARGET}")
    commit, tree = git_identity()
    toolchain = candidate_toolchain()
    target_directory = output_directory / "build"
    environment = candidate_build_environment(target_directory)
    environment.update(
        {
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
    archive = package_binary(
        target_directory / TARGET / "release/type-bridge",
        output_directory,
        commit=commit,
        tree=tree,
    )
    validate(archive)
    return archive


def clean_environment(home: Path, shims: Path) -> dict[str, str]:
    return {
        "HOME": str(home),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "PATH": str(shims),
        "HTTP_PROXY": "http://127.0.0.1:9",
        "HTTPS_PROXY": "http://127.0.0.1:9",
        "NO_PROXY": "",
    }


def migration_apply_arguments() -> list[str]:
    arguments = ["migration", "apply", "--environment", "live"]
    for migration_id in WORKFORCE_MIGRATIONS:
        arguments.extend(("--approve", migration_id))
    return arguments


def migration_rollback_arguments() -> list[str]:
    arguments = ["migration", "rollback", "--environment", "live", "--execute"]
    for migration_id in WORKFORCE_MIGRATIONS:
        arguments.extend(("--remove", migration_id, "--approve", migration_id))
    return arguments


def smoke(archive_path: Path) -> dict[str, Any]:
    manifest = validate(archive_path)
    files = safe_archive_files(archive_path)
    with tempfile.TemporaryDirectory(prefix="type-bridge-cli-consumer-") as temporary:
        root = Path(temporary)
        installed = root / "initial prefix"
        relocated = root / "relocated prefix ü"
        workspace = root / "clean workforce"
        home = root / "empty home"
        shims = root / "forbidden tools"
        sentinel = root / "forbidden-tool-used"
        installed.mkdir()
        write_validated_files(files, installed)
        installed.rename(relocated)
        shutil.copytree(WORKFORCE, workspace)
        home.mkdir()
        shims.mkdir()
        for name in ("cargo", "node", "python", "python3", "rustc"):
            shim = shims / name
            shim.write_text(f"#!/bin/sh\nprintf '%s\\n' {name} >> '{sentinel}'\nexit 97\n")
            shim.chmod(0o755)
        environment = clean_environment(home, shims)
        binary = relocated / "bin/type-bridge"
        commands = [
            ["--help"],
            ["-V"],
            ["schema", "check"],
            ["schema", "generate"],
            ["migration", "plan"],
        ]
        observations: list[str] = []
        for arguments in commands:
            observations.append(run([str(binary), *arguments], cwd=workspace, env=environment))
        for binding in ("python", "typescript", "rust", "c"):
            history = workspace / f"generated/{binding}/typebridge/migration-history.json"
            if not history.is_file() or history.is_symlink():
                raise CandidateError(f"clean CLI did not generate {binding} history authority")
        if sentinel.exists():
            raise CandidateError(f"clean CLI discovered a forbidden tool: {sentinel.read_text()}")
        shutil.rmtree(relocated)
        if binary.exists():
            raise CandidateError("clean CLI uninstall left its executable behind")
    return {
        "candidate-id": manifest["candidate-id"],
        "commands": len(commands),
        "four-binding-generation": True,
        "forbidden-tool-discovery": False,
        "relocation": True,
        "uninstall": "clean",
    }


def connected_smoke(
    archive_path: Path,
    *,
    address: str,
    http_port: str,
    database: str,
    username: str,
    password: str,
) -> dict[str, Any]:
    if re.fullmatch(r"[a-z][a-z0-9_]{2,62}", database) is None:
        raise CandidateError("connected-smoke database name is not a safe isolated identifier")
    manifest = validate(archive_path)
    files = safe_archive_files(archive_path)
    with tempfile.TemporaryDirectory(prefix="type-bridge-cli-live-") as temporary:
        root = Path(temporary)
        installed = root / "installed candidate"
        workspace = root / "workforce history"
        home = root / "empty home"
        shims = root / "forbidden tools"
        sentinel = root / "forbidden-tool-used"
        installed.mkdir()
        write_validated_files(files, installed)
        shutil.copytree(WORKFORCE, workspace)
        workspace_manifest = workspace / "typebridge.yaml"
        source = workspace_manifest.read_text(encoding="utf-8")
        expected_environment = (
            '    database: workforce_v4\n    uri: 127.0.0.1:1729\n    migrate: "true"\n'
        )
        if source.count(expected_environment) != 1:
            raise CandidateError("workforce connected environment authority drifted")
        workspace_manifest.write_text(
            source.replace(
                expected_environment,
                f"    database: {database}\n"
                f"    uri: {address}\n"
                f"    http-port: '{http_port}'\n"
                '    migrate: "true"\n',
                1,
            ),
            encoding="utf-8",
        )
        home.mkdir()
        shims.mkdir()
        for name in ("cargo", "node", "python", "python3", "rustc"):
            shim = shims / name
            shim.write_text(f"#!/bin/sh\nprintf '%s\\n' {name} >> '{sentinel}'\nexit 97\n")
            shim.chmod(0o755)
        environment = clean_environment(home, shims)
        environment.update(
            {
                "TYPEBRIDGE_WORKFORCE_V4_USERNAME": username,
                "TYPEBRIDGE_WORKFORCE_V4_PASSWORD": password,
            }
        )
        binary = installed / "bin/type-bridge"
        apply = migration_apply_arguments()
        run([str(binary), *apply], cwd=workspace, env=environment)
        run(
            [str(binary), "migration", "verify", "--environment", "live"],
            cwd=workspace,
            env=environment,
        )
        rollback = migration_rollback_arguments()
        run([str(binary), *rollback], cwd=workspace, env=environment)
        run([str(binary), *apply], cwd=workspace, env=environment)
        run(
            [str(binary), "migration", "verify", "--environment", "live"],
            cwd=workspace,
            env=environment,
        )
        if sentinel.exists():
            raise CandidateError(
                f"connected CLI discovered a forbidden tool: {sentinel.read_text()}"
            )
        shutil.rmtree(installed)
        if binary.exists():
            raise CandidateError("connected CLI uninstall left its executable behind")
    return {
        "candidate-id": manifest["candidate-id"],
        "database": database,
        "explicit-credentials": True,
        "history-length": len(WORKFORCE_MIGRATIONS),
        "operations": ["apply", "verify", "rollback", "reapply", "verify"],
        "uninstall": "clean",
    }


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_parser = commands.add_parser("build")
    build_parser.add_argument("--output", type=Path, required=True)
    validate_parser = commands.add_parser("validate")
    validate_parser.add_argument("archive", type=Path)
    smoke_parser = commands.add_parser("smoke")
    smoke_parser.add_argument("archive", type=Path)
    connected_parser = commands.add_parser("connected-smoke")
    connected_parser.add_argument("archive", type=Path)
    connected_parser.add_argument("--address", default="127.0.0.1:1729")
    connected_parser.add_argument("--http-port", default="8000")
    connected_parser.add_argument("--database", required=True)
    connected_parser.add_argument("--username", default="admin")
    connected_parser.add_argument("--password", default="password")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    arguments = parse_args(argv)
    try:
        contract = load_contract()
        if contract.get("publication_disposition") != PUBLICATION_DISPOSITION:
            raise CandidateError("frozen publication disposition drifted")
        if contract.get("archive_layouts", {}).get("cli") != [
            path.as_posix() for path in ARCHIVE_MEMBERS
        ]:
            raise CandidateError("frozen CLI archive layout drifted")
        if arguments.command == "build":
            result: object = {"archive": str(build(arguments.output))}
        elif arguments.command == "validate":
            result = validate(arguments.archive)
        elif arguments.command == "smoke":
            result = smoke(arguments.archive)
        else:
            result = connected_smoke(
                arguments.archive,
                address=arguments.address,
                http_port=arguments.http_port,
                database=arguments.database,
                username=arguments.username,
                password=arguments.password,
            )
    except CandidateError as error:
        print(f"standalone CLI candidate rejected: {error}", file=sys.stderr)
        return 1
    print(canonical_json(result).decode("utf-8"), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
