#!/usr/bin/env python3
"""Build and validate artifact-CLI-generated Sdk V6 language surfaces."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path, PurePosixPath
from typing import Any, NoReturn

import standalone_cli_artifact as cli

ROOT = Path(__file__).resolve().parents[2]
SDK = ROOT / "tests/contracts/sdk_conformance/sdk-v4/workspace"
FORMAT = "typebridge.sdk-v6-generated-surface/v1"
BINDING_DIRECTORIES = {"python": "python", "node": "typescript", "rust": "rust"}
MAX_ARCHIVE_BYTES = 128 * 1024 * 1024
MAX_EXPANDED_BYTES = 256 * 1024 * 1024
MAX_MEMBER_BYTES = 16 * 1024 * 1024


class SurfaceError(ValueError):
    """Stable fail-closed generated-surface rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise SurfaceError(code, message)


def canonical_json(value: Any) -> bytes:
    return json.dumps(
        value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":")
    ).encode()


def _members(root: Path) -> dict[PurePosixPath, bytes]:
    files: dict[PurePosixPath, bytes] = {}
    for path in sorted(root.rglob("*")):
        try:
            metadata = path.lstat()
        except OSError as error:
            reject("invalid_surface_tree", f"cannot inspect generated member: {error}")
        if stat.S_ISLNK(metadata.st_mode) or (
            not stat.S_ISREG(metadata.st_mode) and not stat.S_ISDIR(metadata.st_mode)
        ):
            reject("invalid_surface_tree", f"generated surface contains special member: {path}")
        if not stat.S_ISREG(metadata.st_mode):
            continue
        relative = PurePosixPath(path.relative_to(root).as_posix())
        if (
            any(part in ("", ".", "..") for part in relative.parts)
            or relative.name == "surface-manifest.json"
        ):
            reject(
                "invalid_surface_tree", f"generated surface path is reserved or unsafe: {relative}"
            )
        body = path.read_bytes()
        if len(body) > MAX_MEMBER_BYTES:
            reject("invalid_surface_tree", f"generated surface member has invalid size: {relative}")
        files[relative] = body
    if not files or sum(map(len, files.values())) > MAX_EXPANDED_BYTES:
        reject("invalid_surface_tree", "generated surface is empty or exceeds its budget")
    return files


def encode_surface(files: dict[PurePosixPath, bytes], manifest: dict[str, Any]) -> bytes:
    values = dict(files)
    values[PurePosixPath("surface-manifest.json")] = canonical_json(manifest)
    output = io.BytesIO()
    with gzip.GzipFile(
        filename="", mode="wb", fileobj=output, mtime=0, compresslevel=9
    ) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for path in sorted(values, key=lambda item: item.as_posix()):
                body = values[path]
                info = tarfile.TarInfo(path.as_posix())
                info.size = len(body)
                info.mode = 0o644
                info.mtime = 0
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                archive.addfile(info, io.BytesIO(body))
    body = output.getvalue()
    if len(body) > MAX_ARCHIVE_BYTES:
        reject("surface_archive_size_limit", "generated surface archive exceeds its budget")
    return body


def manifest_for(
    files: dict[PurePosixPath, bytes], *, binding: str, cli_manifest: dict[str, Any]
) -> dict[str, Any]:
    members = [
        {"path": path.as_posix(), "sha256": hashlib.sha256(body).hexdigest(), "size": len(body)}
        for path, body in sorted(files.items(), key=lambda item: item[0].as_posix())
    ]
    return {
        "format": FORMAT,
        "binding": binding,
        "source-commit": cli_manifest["source-commit"],
        "source-tree": cli_manifest["source-tree"],
        "cli-artifact-id": cli_manifest["artifact-id"],
        "members": members,
        "payload-sha256": hashlib.sha256(canonical_json(members)).hexdigest(),
        "publication-authority": False,
    }


def _archive_files(path: Path) -> dict[PurePosixPath, bytes]:
    try:
        metadata = path.lstat()
        body = path.read_bytes()
    except OSError as error:
        reject("invalid_surface_archive", f"cannot read surface archive: {error}")
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or len(body) > MAX_ARCHIVE_BYTES
    ):
        reject("invalid_surface_archive", "surface archive is not a bounded regular file")
    if len(body) < 10 or body[:3] != b"\x1f\x8b\x08" or body[3] != 0 or body[4:8] != b"\0" * 4:
        reject("noncanonical_surface_archive", "surface gzip metadata is not canonical")
    files: dict[PurePosixPath, bytes] = {}
    expanded = 0
    folded: set[str] = set()
    try:
        with tarfile.open(fileobj=io.BytesIO(body), mode="r:gz") as archive:
            for member in archive:
                relative = PurePosixPath(member.name)
                if (
                    relative.is_absolute()
                    or any(part in ("", ".", "..") for part in relative.parts)
                    or "\\" in member.name
                    or member.name.casefold() in folded
                ):
                    reject("unsafe_surface_member", f"unsafe or duplicate member {member.name!r}")
                folded.add(member.name.casefold())
                if (
                    not member.isfile()
                    or member.issym()
                    or member.islnk()
                    or member.mode != 0o644
                    or member.mtime != 0
                    or member.uid != 0
                    or member.gid != 0
                    or member.uname
                    or member.gname
                ):
                    reject(
                        "noncanonical_surface_member",
                        f"surface member metadata drifted: {member.name}",
                    )
                expanded += member.size
                if (
                    member.size < 0
                    or member.size > MAX_MEMBER_BYTES
                    or expanded > MAX_EXPANDED_BYTES
                ):
                    reject(
                        "surface_member_size_limit",
                        f"surface member exceeds its budget: {member.name}",
                    )
                stream = archive.extractfile(member)
                if stream is None:
                    reject("invalid_surface_archive", f"cannot extract {member.name}")
                payload = stream.read(member.size + 1)
                if len(payload) != member.size:
                    reject("invalid_surface_archive", f"member size drifted: {member.name}")
                files[relative] = payload
    except SurfaceError:
        raise
    except (EOFError, OSError, tarfile.TarError) as error:
        reject("invalid_surface_archive", f"cannot decode surface archive: {error}")
    return files


def validate_surface(path: Path, binding: str) -> dict[str, Any]:
    if binding not in BINDING_DIRECTORIES:
        reject("invalid_surface_binding", f"unknown generated surface binding {binding!r}")
    files = _archive_files(path)
    manifest_body = files.pop(PurePosixPath("surface-manifest.json"), None)
    if manifest_body is None:
        reject("missing_surface_manifest", "surface manifest is missing")
    try:
        manifest = json.loads(manifest_body, object_pairs_hook=cli._unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError, cli.ArtifactError) as error:
        reject("invalid_surface_manifest", f"cannot parse surface manifest: {error}")
    if (
        not isinstance(manifest, dict)
        or canonical_json(manifest) != manifest_body
        or set(manifest)
        != {
            "format",
            "binding",
            "source-commit",
            "source-tree",
            "cli-artifact-id",
            "members",
            "payload-sha256",
            "publication-authority",
        }
    ):
        reject("invalid_surface_manifest", "surface manifest shape or encoding drifted")
    if (
        manifest["format"] != FORMAT
        or manifest["binding"] != binding
        or manifest["publication-authority"] is not False
    ):
        reject("surface_authority_drift", "surface identity or authority drifted")
    for field, length in (("source-commit", 40), ("source-tree", 40)):
        value = manifest[field]
        if (
            not isinstance(value, str)
            or len(value) != length
            or any(c not in "0123456789abcdef" for c in value)
        ):
            reject("invalid_surface_source", f"surface {field} is invalid")
    artifact = manifest["cli-artifact-id"]
    if (
        not isinstance(artifact, str)
        or not artifact.startswith("sha256:")
        or len(artifact) != 71
        or any(c not in "0123456789abcdef" for c in artifact[7:])
    ):
        reject("invalid_surface_artifact", "surface CLI artifact ID is invalid")
    expected = [
        {"path": member.as_posix(), "sha256": hashlib.sha256(body).hexdigest(), "size": len(body)}
        for member, body in sorted(files.items(), key=lambda item: item[0].as_posix())
    ]
    if (
        manifest["members"] != expected
        or manifest["payload-sha256"] != hashlib.sha256(canonical_json(expected)).hexdigest()
    ):
        reject("surface_member_manifest_drift", "surface member ledger drifted")
    return manifest


def build(cli_archive: Path, output: Path) -> dict[str, Any]:
    cli_manifest = cli.validate(cli_archive)
    cli_files = cli.safe_archive_files(cli_archive)
    output.mkdir(parents=True, exist_ok=True)
    if output.is_symlink():
        reject("invalid_output_path", "surface output directory cannot be a symlink")
    with tempfile.TemporaryDirectory(prefix="typebridge-v6-surfaces-") as temporary:
        root = Path(temporary)
        installed = root / "artifact"
        workspace = root / "sdk"
        home = root / "home"
        shims = root / "empty-path"
        installed.mkdir()
        home.mkdir()
        shims.mkdir()
        cli.write_validated_files(cli_files, installed)
        shutil.copytree(SDK, workspace)
        environment = cli.clean_environment(home, shims)
        result = subprocess.run(
            [str(installed / "bin/type-bridge"), "schema", "generate"],
            cwd=workspace,
            env=environment,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        if result.returncode != 0:
            reject("surface_generation_failed", f"artifact CLI generation failed: {result.stdout}")
        summary: dict[str, Any] = {}
        for binding, directory in BINDING_DIRECTORIES.items():
            files = _members(workspace / "generated" / directory)
            manifest = manifest_for(files, binding=binding, cli_manifest=cli_manifest)
            archive = encode_surface(files, manifest)
            destination = output / f"type-bridge-{binding}-generated-sdk-v6.tar.gz"
            if destination.exists() or destination.is_symlink():
                reject(
                    "surface_publication_failed", f"surface output already exists: {destination}"
                )
            temporary_output = output / f".{destination.name}.{os.getpid()}.tmp"
            temporary_output.write_bytes(archive)
            os.chmod(temporary_output, 0o644)
            try:
                os.link(temporary_output, destination, follow_symlinks=False)
            finally:
                temporary_output.unlink(missing_ok=True)
            validate_surface(destination, binding)
            digest = hashlib.sha256(archive).hexdigest()
            summary[binding] = {
                "path": str(destination),
                "sha256": digest,
                "artifact-id": f"sha256:{digest}",
            }
        return {
            "format": "typebridge.sdk-v6-generated-surface-set/v1",
            "source-commit": cli_manifest["source-commit"],
            "cli-artifact-id": cli_manifest["artifact-id"],
            "surfaces": summary,
            "publication-authority": False,
        }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    builder = commands.add_parser("build")
    builder.add_argument("cli", type=Path)
    builder.add_argument("--output", required=True, type=Path)
    validator = commands.add_parser("validate")
    validator.add_argument("archive", type=Path)
    validator.add_argument("--binding", required=True, choices=BINDING_DIRECTORIES)
    arguments = parser.parse_args()
    try:
        if arguments.command == "build":
            print(canonical_json(build(arguments.cli, arguments.output)).decode())
        else:
            print(canonical_json(validate_surface(arguments.archive, arguments.binding)).decode())
    except (SurfaceError, cli.ArtifactError) as error:
        code = getattr(error, "code", "surface_failure")
        print(f"Sdk V6 surface rejected [{code}]: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
