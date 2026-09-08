#!/usr/bin/env python3
"""Validate one immutable TypeBridge server OCI-layout archive."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import math
import re
import tarfile
from dataclasses import dataclass
from datetime import datetime
from pathlib import PurePosixPath
from typing import Any

OCI_IMAGE_MANIFEST = "application/vnd.oci.image.manifest.v1+json"
OCI_IMAGE_INDEX = "application/vnd.oci.image.index.v1+json"
DOCKER_IMAGE_MANIFEST = "application/vnd.docker.distribution.manifest.v2+json"
REQUIRED_FILES = {
    "etc/ssl/certs/ca-certificates.crt",
    "etc/type-bridge/server.toml",
    "usr/local/bin/type-bridge-server",
    "usr/share/licenses/type-bridge/LICENSE",
}
EXPECTED_LABELS = {
    "org.opencontainers.image.title": "TypeBridge Server",
    "org.opencontainers.image.description": (
        "TypeBridge TypeDB query proxy with the V1 compatibility and V2 prepared-query surfaces"
    ),
    "org.opencontainers.image.url": "https://ds1sqe.github.io/type-bridge/",
    "org.opencontainers.image.source": "https://github.com/ds1sqe/type-bridge",
    "org.opencontainers.image.documentation": (
        "https://ds1sqe.github.io/type-bridge/guide/server-container/"
    ),
    "org.opencontainers.image.licenses": "MIT",
}
EXPECTED_RUNTIME_SHADOW_ENTRY = "typebridge:!:0:0:99999:7:::"
FORBIDDEN_PATH_PATTERNS = (
    re.compile(r"(^|/)\.cargo/(credentials|credentials\.toml)$"),
    re.compile(r"(^|/)(id_rsa|id_ed25519|id_ecdsa)$"),
    re.compile(r"(^|/)[^/]+\.(key|p12|pfx|pem)$"),
    re.compile(r"(^|/)target(/|$)"),
    re.compile(r"(^|/)(Cargo\.toml|Cargo\.lock)$"),
    re.compile(r"(^|/)[^/]+\.(rs|rlib|rmeta)$"),
)


class ValidationError(RuntimeError):
    """The OCI archive violates the release contract."""


@dataclass(frozen=True)
class LayerEntry:
    """One final visible filesystem entry after applying the image layers."""

    kind: str
    payload: bytes | None
    mtime: int | float
    linkname: str | None = None


def _digest(payload: bytes) -> str:
    return f"sha256:{hashlib.sha256(payload).hexdigest()}"


def _read_json(payload: bytes, *, label: str) -> dict[str, Any]:
    try:
        value = json.loads(payload)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"{label} is not valid JSON: {error}") from error
    if not isinstance(value, dict):
        raise ValidationError(f"{label} must be a JSON object")
    return value


def _created_timestamp(value: Any, *, label: str) -> datetime:
    if not isinstance(value, str) or not value:
        raise ValidationError(f"{label} created timestamp is missing")
    try:
        timestamp = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValidationError(f"{label} created timestamp is not RFC 3339: {value!r}") from error
    if timestamp.utcoffset() is None:
        raise ValidationError(f"{label} created timestamp has no UTC offset: {value!r}")
    return timestamp


def _require_created_timestamp(
    value: Any,
    *,
    label: str,
    expected_text: str,
    expected: datetime,
) -> str:
    actual = _created_timestamp(value, label=label)
    # Equal instants can have multiple RFC 3339 spellings.  The release
    # artifact must bind the exact commit timestamp text as well as the instant
    # so alternate offsets or fractional spellings cannot produce different
    # accepted bytes.
    if actual != expected or value != expected_text:
        raise ValidationError(
            f"{label} created timestamp diverges from the release commit: "
            f"actual={value!r}, expected={expected_text!r}"
        )
    assert isinstance(value, str)
    return value


def _mtime_is_at_or_before(value: int | float, expected: int) -> bool:
    if isinstance(value, float) and not math.isfinite(value):
        return False
    return value <= expected


def _normalized_layer_path(name: str) -> str:
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts:
        raise ValidationError(f"image layer contains unsafe path {name!r}")
    return str(path)


def _is_descendant(path: str, parent: str) -> bool:
    return path != parent and PurePosixPath(parent) in PurePosixPath(path).parents


def _remove_visible_subtree(entries: dict[str, LayerEntry], path: str) -> None:
    for candidate in tuple(entries):
        if candidate == path or _is_descendant(candidate, path):
            entries.pop(candidate)


def _layer_entry_kind(member: tarfile.TarInfo) -> str:
    if member.isfile():
        return "regular"
    if member.isdir():
        return "directory"
    if member.issym():
        return "symlink"
    if member.islnk():
        return "hardlink"
    if member.ischr():
        return "character-device"
    if member.isblk():
        return "block-device"
    if member.isfifo():
        return "fifo"
    return "other"


class OciArchive:
    def __init__(self, path: str) -> None:
        try:
            self.archive = tarfile.open(path, mode="r:*")
        except (OSError, tarfile.TarError) as error:
            raise ValidationError(f"cannot open OCI archive {path!r}: {error}") from error
        self.members: dict[str, tarfile.TarInfo] = {}
        for member in self.archive.getmembers():
            normalized = member.name.removeprefix("./")
            if normalized in self.members:
                raise ValidationError(f"OCI archive contains duplicate member {normalized!r}")
            if PurePosixPath(normalized).is_absolute() or ".." in PurePosixPath(normalized).parts:
                raise ValidationError(f"OCI archive contains unsafe member {member.name!r}")
            self.members[normalized] = member

    def read(self, name: str) -> bytes:
        member = self.members.get(name)
        if member is None or not member.isfile():
            raise ValidationError(f"OCI archive is missing regular file {name!r}")
        stream = self.archive.extractfile(member)
        if stream is None:
            raise ValidationError(f"cannot read OCI archive member {name!r}")
        return stream.read()

    def blob(self, digest: str) -> bytes:
        algorithm, separator, encoded = digest.partition(":")
        if separator != ":" or algorithm != "sha256" or not re.fullmatch(r"[0-9a-f]{64}", encoded):
            raise ValidationError(f"unsupported OCI descriptor digest {digest!r}")
        payload = self.read(f"blobs/sha256/{encoded}")
        actual = _digest(payload)
        if actual != digest:
            raise ValidationError(
                f"OCI blob digest mismatch: descriptor={digest!r}, actual={actual!r}"
            )
        return payload


def _one_descriptor(index: dict[str, Any], *, label: str) -> dict[str, Any]:
    manifests = index.get("manifests")
    if not isinstance(manifests, list) or len(manifests) != 1:
        raise ValidationError(f"{label} must contain exactly one descriptor")
    descriptor = manifests[0]
    if not isinstance(descriptor, dict):
        raise ValidationError(f"{label} descriptor must be an object")
    return descriptor


def _image_manifest(archive: OciArchive, descriptor: dict[str, Any]) -> tuple[dict[str, Any], str]:
    digest = descriptor.get("digest")
    media_type = descriptor.get("mediaType")
    if not isinstance(digest, str) or not isinstance(media_type, str):
        raise ValidationError("OCI descriptor has no digest/mediaType")
    payload = archive.blob(digest)
    document = _read_json(payload, label=f"OCI blob {digest}")
    if media_type == OCI_IMAGE_INDEX:
        return _image_manifest(
            archive,
            _one_descriptor(document, label="nested platform index"),
        )
    if media_type not in (OCI_IMAGE_MANIFEST, DOCKER_IMAGE_MANIFEST):
        raise ValidationError(f"unexpected OCI manifest media type {media_type!r}")
    return document, digest


def _layer_files(
    archive: OciArchive,
    manifest: dict[str, Any],
) -> tuple[dict[str, LayerEntry], list[tuple[str, int | float]]]:
    layers = manifest.get("layers")
    if not isinstance(layers, list) or not layers:
        raise ValidationError("OCI image manifest has no layers")
    merged: dict[str, LayerEntry] = {}
    member_mtimes: list[tuple[str, int | float]] = []
    for descriptor in layers:
        if not isinstance(descriptor, dict) or not isinstance(descriptor.get("digest"), str):
            raise ValidationError("OCI image layer descriptor is malformed")
        payload = archive.blob(descriptor["digest"])
        try:
            with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as layer:
                members = [(_normalized_layer_path(member.name), member) for member in layer]
                for path, member in members:
                    member_mtimes.append((path, member.mtime))

                # Whiteouts operate only on entries inherited from lower layers,
                # regardless of their position relative to upper entries in the tar.
                for path, _ in members:
                    pure = PurePosixPath(path)
                    if pure.name == ".wh..wh..opq":
                        directory = str(pure.parent)
                        for candidate in tuple(merged):
                            if _is_descendant(candidate, directory):
                                merged.pop(candidate)
                    elif pure.name.startswith(".wh."):
                        target_name = pure.name.removeprefix(".wh.")
                        if not target_name:
                            raise ValidationError(
                                f"image layer contains malformed whiteout {path!r}"
                            )
                        _remove_visible_subtree(merged, str(pure.with_name(target_name)))

                for path, member in members:
                    pure = PurePosixPath(path)
                    if pure.name.startswith(".wh."):
                        continue
                    for parent in pure.parents:
                        parent_path = str(parent)
                        if parent_path == ".":
                            continue
                        ancestor = merged.get(parent_path)
                        if ancestor is not None and ancestor.kind != "directory":
                            raise ValidationError(
                                "image layer entry descends from a non-directory path: "
                                f"entry={path!r}, ancestor={parent_path!r}, "
                                f"kind={ancestor.kind!r}"
                            )

                    kind = _layer_entry_kind(member)
                    previous = merged.get(path)
                    if kind != "directory" or (
                        previous is not None and previous.kind != "directory"
                    ):
                        for candidate in tuple(merged):
                            if _is_descendant(candidate, path):
                                merged.pop(candidate)

                    entry_payload: bytes | None = None
                    if kind == "regular":
                        stream = layer.extractfile(member)
                        if stream is None:
                            raise ValidationError(f"cannot read image layer file {path!r}")
                        entry_payload = stream.read()
                    merged[path] = LayerEntry(
                        kind=kind,
                        payload=entry_payload,
                        mtime=member.mtime,
                        linkname=member.linkname or None,
                    )
        except tarfile.TarError as error:
            raise ValidationError(f"cannot read OCI image layer: {error}") from error
    return merged, member_mtimes


def _required_regular_payload(entries: dict[str, LayerEntry], path: str) -> bytes:
    entry = entries.get(path)
    if entry is None or entry.kind != "regular" or entry.payload is None:
        raise ValidationError(f"OCI runtime path is not a final regular file: {path!r}")
    return entry.payload


def validate(args: argparse.Namespace) -> dict[str, Any]:
    archive = OciArchive(args.archive)
    expected_created = _created_timestamp(args.created, label="expected release")
    expected_source_date_epoch = int(expected_created.timestamp())
    layout = _read_json(archive.read("oci-layout"), label="OCI layout marker")
    if layout != {"imageLayoutVersion": "1.0.0"}:
        raise ValidationError("archive is not the canonical OCI image-layout version 1.0.0")
    index_payload = archive.read("index.json")
    index = _read_json(index_payload, label="OCI index")
    top = _one_descriptor(index, label="OCI index")
    top_annotations = top.get("annotations")
    if not isinstance(top_annotations, dict):
        raise ValidationError("OCI index descriptor has no annotations")
    index_created = _require_created_timestamp(
        top_annotations.get("org.opencontainers.image.created"),
        label="OCI index descriptor",
        expected_text=args.created,
        expected=expected_created,
    )
    manifest, manifest_digest = _image_manifest(archive, top)

    config_descriptor = manifest.get("config")
    if not isinstance(config_descriptor, dict) or not isinstance(
        config_descriptor.get("digest"), str
    ):
        raise ValidationError("OCI image manifest has no config descriptor")
    config_digest = config_descriptor["digest"]
    config = _read_json(archive.blob(config_digest), label="OCI image config")
    config_created = _require_created_timestamp(
        config.get("created"),
        label="OCI image config",
        expected_text=args.created,
        expected=expected_created,
    )
    history = config.get("history")
    if not isinstance(history, list) or not history:
        raise ValidationError("OCI image config has no creation history")
    history_timestamps: list[datetime] = []
    for index, entry in enumerate(history):
        if not isinstance(entry, dict):
            raise ValidationError(f"OCI image history entry {index} is malformed")
        timestamp = _created_timestamp(
            entry.get("created"),
            label=f"OCI image history entry {index}",
        )
        if timestamp > expected_created:
            raise ValidationError(
                f"OCI image history entry {index} was created after the release commit: "
                f"actual={entry.get('created')!r}, expected-at-or-before={args.created!r}"
            )
        history_timestamps.append(timestamp)
    rootfs = config.get("rootfs")
    if not isinstance(rootfs, dict) or rootfs.get("type") != "layers":
        raise ValidationError("OCI image config has no layered rootfs descriptor")
    rootfs_diff_ids = rootfs.get("diff_ids")
    if not isinstance(rootfs_diff_ids, list) or not rootfs_diff_ids:
        raise ValidationError("OCI image config has no rootfs diff IDs")
    if not all(
        isinstance(digest, str) and re.fullmatch(r"sha256:[0-9a-f]{64}", digest)
        for digest in rootfs_diff_ids
    ):
        raise ValidationError("OCI image config contains malformed rootfs diff IDs")
    expected_os, expected_arch = args.platform.split("/", maxsplit=1)
    if config.get("os") != expected_os or config.get("architecture") != expected_arch:
        raise ValidationError(
            "OCI platform mismatch: "
            f"actual={config.get('os')}/{config.get('architecture')}, "
            f"expected={args.platform}"
        )

    runtime = config.get("config")
    if not isinstance(runtime, dict):
        raise ValidationError("OCI runtime config is missing")
    if runtime.get("User") != "10001:10001":
        raise ValidationError("OCI runtime user must be exactly 10001:10001")
    if runtime.get("WorkingDir") != "/":
        raise ValidationError("OCI runtime working directory must be /")
    if runtime.get("Entrypoint") != ["/usr/local/bin/type-bridge-server"]:
        raise ValidationError("OCI entrypoint is not the standalone server binary")
    if runtime.get("Cmd") != ["--config", "/etc/type-bridge/server.toml"]:
        raise ValidationError("OCI default command does not select the container config")
    if runtime.get("Healthcheck") is not None:
        raise ValidationError("OCI image must not declare a shell-based healthcheck")

    labels = runtime.get("Labels")
    if not isinstance(labels, dict):
        raise ValidationError("OCI image has no labels")
    expected = {
        **EXPECTED_LABELS,
        "org.opencontainers.image.revision": args.revision,
        "org.opencontainers.image.version": args.version,
        "org.opencontainers.image.created": args.created,
        "io.type-bridge.release-identity": args.release_identity,
    }
    mismatches = {
        key: {"actual": labels.get(key), "expected": value}
        for key, value in expected.items()
        if labels.get(key) != value
    }
    if mismatches:
        raise ValidationError(f"OCI label mismatch: {mismatches!r}")

    entries, layer_member_mtimes = _layer_files(archive, manifest)
    layers = manifest["layers"]
    if len(rootfs_diff_ids) != len(layers):
        raise ValidationError(
            "OCI rootfs diff ID count does not match the image layer count: "
            f"diff_ids={len(rootfs_diff_ids)}, layers={len(layers)}"
        )
    invalid_required = sorted(
        path for path in REQUIRED_FILES if path not in entries or entries[path].kind != "regular"
    )
    if invalid_required:
        raise ValidationError(
            f"OCI runtime required paths are not final regular files: {invalid_required!r}"
        )
    forbidden = sorted(
        path
        for path in entries
        if path != "etc/ssl/certs/ca-certificates.crt"
        and any(pattern.search(path) for pattern in FORBIDDEN_PATH_PATTERNS)
    )
    if forbidden:
        raise ValidationError(f"OCI runtime contains forbidden source/secret paths: {forbidden!r}")

    later_members = sorted(
        path
        for path, mtime in layer_member_mtimes
        if not _mtime_is_at_or_before(mtime, expected_source_date_epoch)
    )
    if later_members:
        raise ValidationError(
            "OCI runtime contains invalid or post-release layer member timestamps: "
            f"{later_members!r}"
        )

    if "etc/shadow-" in entries:
        raise ValidationError("OCI runtime must not retain the service account shadow backup")
    shadow_entry = entries.get("etc/shadow")
    if shadow_entry is None or shadow_entry.kind != "regular" or shadow_entry.payload is None:
        raise ValidationError("OCI runtime is missing a regular service account shadow database")
    try:
        shadow = shadow_entry.payload.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValidationError("OCI runtime shadow database is not UTF-8") from error
    typebridge_shadow_entries = [
        line for line in shadow.splitlines() if line.partition(":")[0] == "typebridge"
    ]
    if typebridge_shadow_entries != [EXPECTED_RUNTIME_SHADOW_ENTRY]:
        raise ValidationError(
            "OCI runtime must contain exactly the deterministic locked typebridge shadow entry"
        )

    example = _required_regular_payload(entries, "etc/type-bridge/server.toml").decode("utf-8")
    for forbidden_text in ("username", "password", "secret", "token", "private"):
        if forbidden_text in example.lower():
            raise ValidationError(
                f"container example config contains forbidden credential marker {forbidden_text!r}"
            )
    if "[v2]\nenabled = false" not in example:
        raise ValidationError("container example config must leave V2 explicitly disabled")

    binary = _required_regular_payload(entries, "usr/local/bin/type-bridge-server")
    if not binary.startswith(b"\x7fELF"):
        raise ValidationError("server runtime payload is not an ELF binary")

    layer_bytes = sum(int(layer.get("size", 0)) for layer in layers)
    return {
        "architecture": expected_arch,
        "config_digest": config_digest,
        "config_created": config_created,
        "index_digest": _digest(index_payload),
        "index_created": index_created,
        "history_entry_count": len(history_timestamps),
        "latest_layer_mtime": max(mtime for _, mtime in layer_member_mtimes),
        "layer_bytes": layer_bytes,
        "manifest_digest": manifest_digest,
        "os": expected_os,
        "release_identity": args.release_identity,
        "revision": args.revision,
        "rootfs_diff_ids": rootfs_diff_ids,
        "runtime_file_count": sum(entry.kind == "regular" for entry in entries.values()),
        "source_date_epoch": expected_source_date_epoch,
        "version": args.version,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--archive", required=True)
    parser.add_argument("--platform", choices=("linux/amd64", "linux/arm64"), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--created", required=True)
    parser.add_argument("--release-identity", required=True)
    parser.add_argument("--report", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    report = validate(args)
    with open(args.report, "w", encoding="utf-8") as stream:
        json.dump(report, stream, indent=2, sort_keys=True)
        stream.write("\n")
    print(json.dumps(report, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        raise SystemExit(f"server OCI validation failed: {error}") from error
