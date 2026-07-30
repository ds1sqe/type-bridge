#!/usr/bin/env python3
"""Validate one immutable TypeBridge server OCI-layout archive."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import tarfile
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


def _layer_files(archive: OciArchive, manifest: dict[str, Any]) -> dict[str, bytes]:
    layers = manifest.get("layers")
    if not isinstance(layers, list) or not layers:
        raise ValidationError("OCI image manifest has no layers")
    merged: dict[str, bytes] = {}
    for descriptor in layers:
        if not isinstance(descriptor, dict) or not isinstance(descriptor.get("digest"), str):
            raise ValidationError("OCI image layer descriptor is malformed")
        payload = archive.blob(descriptor["digest"])
        try:
            with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as layer:
                for member in layer.getmembers():
                    path = member.name.removeprefix("./")
                    pure = PurePosixPath(path)
                    if pure.is_absolute() or ".." in pure.parts:
                        raise ValidationError(f"image layer contains unsafe path {path!r}")
                    if pure.name.startswith(".wh."):
                        target = str(pure.with_name(pure.name.removeprefix(".wh.")))
                        merged.pop(target, None)
                        continue
                    if member.isfile():
                        stream = layer.extractfile(member)
                        if stream is None:
                            raise ValidationError(f"cannot read image layer file {path!r}")
                        merged[path] = stream.read()
        except tarfile.TarError as error:
            raise ValidationError(f"cannot read OCI image layer: {error}") from error
    return merged


def validate(args: argparse.Namespace) -> dict[str, Any]:
    archive = OciArchive(args.archive)
    layout = _read_json(archive.read("oci-layout"), label="OCI layout marker")
    if layout != {"imageLayoutVersion": "1.0.0"}:
        raise ValidationError("archive is not the canonical OCI image-layout version 1.0.0")
    index_payload = archive.read("index.json")
    index = _read_json(index_payload, label="OCI index")
    top = _one_descriptor(index, label="OCI index")
    manifest, manifest_digest = _image_manifest(archive, top)

    config_descriptor = manifest.get("config")
    if not isinstance(config_descriptor, dict) or not isinstance(
        config_descriptor.get("digest"), str
    ):
        raise ValidationError("OCI image manifest has no config descriptor")
    config_digest = config_descriptor["digest"]
    config = _read_json(archive.blob(config_digest), label="OCI image config")
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

    files = _layer_files(archive, manifest)
    missing = sorted(REQUIRED_FILES - files.keys())
    if missing:
        raise ValidationError(f"OCI runtime file set is incomplete: {missing!r}")
    forbidden = sorted(
        path
        for path in files
        if path != "etc/ssl/certs/ca-certificates.crt"
        and any(pattern.search(path) for pattern in FORBIDDEN_PATH_PATTERNS)
    )
    if forbidden:
        raise ValidationError(f"OCI runtime contains forbidden source/secret paths: {forbidden!r}")

    example = files["etc/type-bridge/server.toml"].decode("utf-8")
    for forbidden_text in ("username", "password", "secret", "token", "private"):
        if forbidden_text in example.lower():
            raise ValidationError(
                f"container example config contains forbidden credential marker {forbidden_text!r}"
            )
    if "[v2]\nenabled = false" not in example:
        raise ValidationError("container example config must leave V2 explicitly disabled")

    binary = files["usr/local/bin/type-bridge-server"]
    if not binary.startswith(b"\x7fELF"):
        raise ValidationError("server runtime payload is not an ELF binary")

    layers = manifest["layers"]
    layer_bytes = sum(int(layer.get("size", 0)) for layer in layers)
    return {
        "architecture": expected_arch,
        "config_digest": config_digest,
        "index_digest": _digest(index_payload),
        "layer_bytes": layer_bytes,
        "manifest_digest": manifest_digest,
        "os": expected_os,
        "release_identity": args.release_identity,
        "revision": args.revision,
        "runtime_file_count": len(files),
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
