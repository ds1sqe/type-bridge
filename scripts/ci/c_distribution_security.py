#!/usr/bin/env python3
"""Assemble and validate supply-chain evidence for C distribution candidates."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import tomllib
from collections.abc import Mapping, Sequence
from pathlib import Path, PurePosixPath
from typing import Any

import c_package_candidates as packages
import standalone_cli_candidate as cli

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
CONTRACT = ROOT / "tests/contracts/c-distribution-v1.json"
LOCKFILE = CORE / "Cargo.lock"
WORKFLOW = ROOT / ".github/workflows/ci.yml"
FORMAT = "typebridge.c-distribution-candidate-set/v1"
PROVENANCE_FORMAT = "typebridge.c-candidate-provenance/v1"
SECURITY_FORMAT = "typebridge.c-candidate-security-scan/v1"
SIGNATURE_FORMAT = "typebridge.c-signature-policy/v1"
AUDIT_VERSION = "0.22.2"
TARGET = "x86_64-unknown-linux-gnu"
DISPOSITION = "candidate-only-unpublished-unsupported"
EVIDENCE_NAMES = (
    "candidate-manifest.json",
    "cli.spdx.json",
    "runtime.spdx.json",
    "generated.spdx.json",
    "rustsec-report.json",
    "security-scan.json",
    "provenance.json",
    "signature-policy.json",
)
HEX40 = re.compile(r"[0-9a-f]{40}\Z")
HEX64 = re.compile(r"[0-9a-f]{64}\Z")
AUDIT_ADJUDICATIONS = (
    {
        "advisory": "RUSTSEC-2025-0134",
        "classification": "unmaintained",
        "package": "rustls-pemfile",
        "version": "2.2.0",
        "rationale": (
            "non-vulnerability thin compatibility wrapper required by the frozen "
            "Tonic 0.12/TypeDB driver graph; remove with its upstream compatibility upgrade"
        ),
    },
)
SECRET_PATTERNS = (
    ("aws-access-key", re.compile(rb"AKIA[0-9A-Z]{16}")),
    ("github-token", re.compile(rb"gh[pousr]_[A-Za-z0-9]{36,}")),
    ("private-key", re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----")),
)


class SecurityError(RuntimeError):
    """Candidate evidence is incomplete, stale, ambiguous, or unsafe."""


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(
            value, ensure_ascii=False, allow_nan=False, sort_keys=True, separators=(",", ":")
        ).encode()
        + b"\n"
    )


def sha256(body: bytes) -> str:
    return hashlib.sha256(body).hexdigest()


def read_regular(path: Path, *, maximum: int = 256 * 1024 * 1024) -> bytes:
    try:
        metadata = path.lstat()
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
            raise SecurityError(f"linked or non-regular input: {path}")
        if metadata.st_size < 0 or metadata.st_size > maximum:
            raise SecurityError(f"input exceeds byte budget: {path}")
        body = path.read_bytes()
    except OSError as error:
        raise SecurityError(f"cannot read {path}: {error}") from error
    if len(body) != metadata.st_size:
        raise SecurityError(f"input changed while being read: {path}")
    return body


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise SecurityError(f"duplicate JSON key: {key}")
        value[key] = item
    return value


def load_json(path: Path, *, canonical: bool = False) -> dict[str, Any]:
    body = read_regular(path, maximum=32 * 1024 * 1024)
    try:
        value = json.loads(body, object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise SecurityError(f"invalid JSON in {path}: {error}") from error
    if not isinstance(value, dict):
        raise SecurityError(f"JSON input is not an object: {path}")
    if canonical and canonical_json(value) != body:
        raise SecurityError(f"JSON input is not canonical: {path}")
    return value


def run(command: Sequence[str], *, cwd: Path = ROOT) -> str:
    try:
        result = subprocess.run(
            command, cwd=cwd, check=True, capture_output=True, text=True, timeout=300
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        detail = ""
        if isinstance(error, subprocess.CalledProcessError):
            detail = (error.stderr or error.stdout or "").strip()
        raise SecurityError(f"command failed: {' '.join(command)}: {detail or error}") from error
    return result.stdout


def cargo_metadata() -> dict[str, Any]:
    value = json.loads(
        run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--filter-platform",
                TARGET,
                "--format-version",
                "1",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
            ]
        )
    )
    if not isinstance(value, dict) or not isinstance(value.get("resolve"), dict):
        raise SecurityError("cargo metadata omitted the resolved dependency graph")
    return value


def runtime_closure(metadata: Mapping[str, Any], roots: Sequence[str]) -> set[str]:
    package_names = {item["id"]: item["name"] for item in metadata["packages"]}
    root_ids = {identifier for identifier, name in package_names.items() if name in roots}
    if {package_names[item] for item in root_ids} != set(roots):
        raise SecurityError(f"cargo metadata omitted roots: {roots!r}")
    nodes = {item["id"]: item for item in metadata["resolve"]["nodes"]}
    closure = set(root_ids)
    pending = list(root_ids)
    while pending:
        identifier = pending.pop()
        node = nodes.get(identifier)
        if not isinstance(node, dict):
            raise SecurityError(f"cargo metadata omitted node {identifier}")
        for dependency in node.get("deps", []):
            kinds = dependency.get("dep_kinds")
            if not isinstance(kinds, list) or not any(item.get("kind") is None for item in kinds):
                continue
            target = dependency.get("pkg")
            if target not in closure:
                closure.add(target)
                pending.append(target)
    return closure


def pruned_lock_payload(metadata: Mapping[str, Any], closure: set[str]) -> bytes:
    lock = tomllib.loads(LOCKFILE.read_text(encoding="utf-8"))
    selected = {
        (item["name"], item["version"], item.get("source"))
        for item in metadata["packages"]
        if item["id"] in closure
    }
    records = [
        item
        for item in lock["package"]
        if (item["name"], item["version"], item.get("source")) in selected
    ]
    if len(records) != len(selected):
        raise SecurityError("pruned lock cannot map every candidate dependency")
    lines = [f"version = {lock['version']}", ""]
    for item in records:
        lines.extend(["[[package]]", f"name = {json.dumps(item['name'])}"])
        lines.append(f"version = {json.dumps(item['version'])}")
        if "source" in item:
            lines.append(f"source = {json.dumps(item['source'])}")
        if "checksum" in item:
            lines.append(f"checksum = {json.dumps(item['checksum'])}")
        lines.append("")
    return "\n".join(lines).encode()


def write_pruned_lock(destination: Path) -> None:
    metadata = cargo_metadata()
    closure = runtime_closure(metadata, ("type-bridge-cli", "type-bridge-c"))
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(pruned_lock_payload(metadata, closure))


def artifact_record(path: Path, manifest: Mapping[str, Any], kind: str) -> dict[str, Any]:
    body = read_regular(path)
    return {
        "candidate-id": manifest["candidate-id"],
        "filename": path.name,
        "kind": kind,
        "sha256": sha256(body),
        "size": len(body),
    }


def package_spdx_id(identifier: str) -> str:
    return "SPDXRef-Package-" + re.sub(r"[^A-Za-z0-9.-]", "-", identifier)


def cargo_sbom(
    artifact: Mapping[str, Any], metadata: Mapping[str, Any], closure: set[str], root_name: str
) -> dict[str, Any]:
    packages_by_id = {item["id"]: item for item in metadata["packages"]}
    nodes = {item["id"]: item for item in metadata["resolve"]["nodes"]}
    root_id = next(item for item in closure if packages_by_id[item]["name"] == root_name)
    document = f"SPDXRef-Artifact-{root_name}"
    entries: list[dict[str, Any]] = []
    relationships = [
        {
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relationshipType": "DESCRIBES",
            "relatedSpdxElement": document,
        }
    ]
    for identifier in sorted(closure):
        item = packages_by_id[identifier]
        spdx_id = package_spdx_id(identifier)
        entry: dict[str, Any] = {
            "SPDXID": spdx_id,
            "name": item["name"],
            "versionInfo": item["version"],
            "downloadLocation": item.get("source") or "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": item.get("license") or "NOASSERTION",
            "copyrightText": "NOASSERTION",
        }
        if item.get("checksum"):
            entry["checksums"] = [{"algorithm": "SHA256", "checksumValue": item["checksum"]}]
        entries.append(entry)
        if identifier == root_id:
            relationships.append(
                {
                    "spdxElementId": document,
                    "relationshipType": "CONTAINS",
                    "relatedSpdxElement": spdx_id,
                }
            )
        for dependency in nodes[identifier].get("deps", []):
            target = dependency.get("pkg")
            kinds = dependency.get("dep_kinds", [])
            if target in closure and any(kind.get("kind") is None for kind in kinds):
                relationships.append(
                    {
                        "spdxElementId": spdx_id,
                        "relationshipType": "DEPENDS_ON",
                        "relatedSpdxElement": package_spdx_id(target),
                    }
                )
    entries.append(
        {
            "SPDXID": document,
            "name": artifact["filename"],
            "versionInfo": artifact["candidate-id"],
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": "MIT",
            "copyrightText": "NOASSERTION",
            "checksums": [{"algorithm": "SHA256", "checksumValue": artifact["sha256"]}],
        }
    )
    spdx_ids = [item["SPDXID"] for item in entries]
    if len(spdx_ids) != len(set(spdx_ids)):
        raise SecurityError("Cargo package identities collide after SPDX normalization")
    return {
        "SPDXID": "SPDXRef-DOCUMENT",
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "name": f"{artifact['filename']}-sbom",
        "documentNamespace": f"https://github.com/ds1sqe/type-bridge/sbom/{artifact['sha256']}",
        "creationInfo": {
            "created": "1970-01-01T00:00:00Z",
            "creators": ["Tool: typebridge-c-distribution-security/v1"],
        },
        "packages": sorted(entries, key=lambda item: item["SPDXID"]),
        "relationships": sorted(
            relationships,
            key=lambda item: (
                item["spdxElementId"],
                item["relationshipType"],
                item["relatedSpdxElement"],
            ),
        ),
    }


def generated_sbom(artifact: Mapping[str, Any], runtime: Mapping[str, Any]) -> dict[str, Any]:
    return {
        "SPDXID": "SPDXRef-DOCUMENT",
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "name": f"{artifact['filename']}-sbom",
        "documentNamespace": f"https://github.com/ds1sqe/type-bridge/sbom/{artifact['sha256']}",
        "creationInfo": {
            "created": "1970-01-01T00:00:00Z",
            "creators": ["Tool: typebridge-c-distribution-security/v1"],
        },
        "packages": [
            {
                "SPDXID": "SPDXRef-Generated",
                "name": artifact["filename"],
                "versionInfo": artifact["candidate-id"],
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": "MIT",
                "copyrightText": "NOASSERTION",
                "checksums": [{"algorithm": "SHA256", "checksumValue": artifact["sha256"]}],
            },
            {
                "SPDXID": "SPDXRef-Runtime",
                "name": runtime["filename"],
                "versionInfo": runtime["candidate-id"],
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": False,
                "licenseConcluded": "NOASSERTION",
                "licenseDeclared": "MIT",
                "copyrightText": "NOASSERTION",
                "checksums": [{"algorithm": "SHA256", "checksumValue": runtime["sha256"]}],
            },
        ],
        "relationships": [
            {
                "spdxElementId": "SPDXRef-DOCUMENT",
                "relationshipType": "DESCRIBES",
                "relatedSpdxElement": "SPDXRef-Generated",
            },
            {
                "spdxElementId": "SPDXRef-Generated",
                "relationshipType": "DEPENDS_ON",
                "relatedSpdxElement": "SPDXRef-Runtime",
            },
        ],
    }


def validate_audit(report: Mapping[str, Any]) -> dict[str, Any]:
    database = report.get("database")
    vulnerabilities = report.get("vulnerabilities")
    warnings = report.get("warnings")
    if not isinstance(database, dict) or not HEX40.fullmatch(str(database.get("last-commit"))):
        raise SecurityError("RustSec report lacks an exact advisory database commit")
    if (
        not isinstance(vulnerabilities, dict)
        or vulnerabilities.get("count") != 0
        or vulnerabilities.get("list") != []
    ):
        raise SecurityError("RustSec report contains candidate vulnerabilities")
    if not isinstance(warnings, dict):
        raise SecurityError("RustSec report omitted warning classifications")
    actual_warnings: list[dict[str, str]] = []
    for classification, items in warnings.items():
        if not isinstance(items, list):
            raise SecurityError("RustSec warning classification is malformed")
        for item in items:
            try:
                actual_warnings.append(
                    {
                        "advisory": item["advisory"]["id"],
                        "classification": classification,
                        "package": item["package"]["name"],
                        "version": item["package"]["version"],
                    }
                )
            except (KeyError, TypeError) as error:
                raise SecurityError("RustSec warning entry is malformed") from error
    accepted = [
        {key: item[key] for key in ("advisory", "classification", "package", "version")}
        for item in AUDIT_ADJUDICATIONS
    ]
    if sorted(actual_warnings, key=str) != sorted(accepted, key=str):
        raise SecurityError("RustSec report contains unadjudicated warnings")
    return dict(report)


def binary_security(binary: Path) -> None:
    sections = run(["readelf", "-SW", str(binary)])
    if re.search(r"\.(?:debug|symtab)(?:\s|$)", sections):
        raise SecurityError(f"candidate binary retains forbidden debug/symbol sections: {binary}")


def validate_binary_payloads(
    cli_files: Mapping[PurePosixPath, bytes],
    runtime_files: Mapping[PurePosixPath, bytes],
) -> None:
    with tempfile.TemporaryDirectory(prefix="typebridge-security-") as temporary:
        root = Path(temporary)
        cli_binary = root / "type-bridge"
        runtime_binary = root / packages.RUNTIME_LIBRARY
        cli_binary.write_bytes(cli_files[PurePosixPath("bin/type-bridge")])
        runtime_binary.write_bytes(runtime_files[PurePosixPath(f"lib/{packages.RUNTIME_LIBRARY}")])
        binary_security(cli_binary)
        binary_security(runtime_binary)


def scan_payloads(
    payloads: Mapping[str, bytes], forbidden_paths: Sequence[bytes]
) -> dict[str, Any]:
    for label, body in payloads.items():
        for name, pattern in SECRET_PATTERNS:
            if pattern.search(body):
                raise SecurityError(f"{label} contains a {name} signature")
        for path in forbidden_paths:
            if path and path in body:
                raise SecurityError(f"{label} contains a machine-specific source path")
    return {
        "format": SECURITY_FORMAT,
        "adjudications": list(AUDIT_ADJUDICATIONS),
        "audit-policy": {
            "scanner": "cargo-audit",
            "version": AUDIT_VERSION,
            "vulnerabilities": "deny-all",
            "warnings": "deny-unless-exactly-adjudicated",
        },
        "findings": [],
        "payload-count": len(payloads),
        "rules": [name for name, _ in SECRET_PATTERNS],
        "source-paths": "absent",
        "debug-symbols": "stripped-no-separate-package",
    }


def signature_policy() -> dict[str, Any]:
    return {
        "format": SIGNATURE_FORMAT,
        "candidate-signatures": [],
        "candidate-state": "unsigned-awaiting-protected-publication-authorization",
        "protected-release": {
            "required": True,
            "issuer": "https://token.actions.githubusercontent.com",
            "identity-regexp": r"^https://github\.com/ds1sqe/type-bridge/\.github/workflows/release\.yml@refs/tags/v2\.1\.0$",
            "cosign-version": "3.0.6",
            "offline-verification-required": True,
        },
        "publication-disposition": DISPOSITION,
    }


def validate_frozen_security_policy() -> None:
    contract = load_json(CONTRACT)
    policy = contract.get("security_policy")
    expected_adjudications = list(AUDIT_ADJUDICATIONS)
    if not isinstance(policy, dict) or policy.get("adjudications") != expected_adjudications:
        raise SecurityError("frozen RustSec adjudication policy drifted")
    if policy.get("cargo_audit_version") != AUDIT_VERSION:
        raise SecurityError("frozen cargo-audit version drifted")
    signature = policy.get("protected_signature")
    actual_signature = signature_policy()["protected-release"]
    if not isinstance(signature, dict) or (
        signature.get("cosign_version") != actual_signature["cosign-version"]
        or signature.get("issuer") != actual_signature["issuer"]
        or signature.get("identity_regexp") != actual_signature["identity-regexp"]
        or signature.get("candidate_signatures") != "forbidden-before-authorization"
    ):
        raise SecurityError("frozen protected signature policy drifted")


def provenance(artifacts: Sequence[Mapping[str, Any]], commit: str, tree: str) -> dict[str, Any]:
    return {
        "format": PROVENANCE_FORMAT,
        "builder": {
            "workflow": ".github/workflows/ci.yml",
            "jobs": ["standalone-cli-candidate", "c-package-candidates"],
            "runner": "ubuntu-latest",
        },
        "materials": [
            {
                "path": "tests/contracts/c-distribution-v1.json",
                "sha256": sha256(read_regular(CONTRACT)),
            },
            {"path": "type-bridge-core/Cargo.lock", "sha256": sha256(read_regular(LOCKFILE))},
            {"path": ".github/workflows/ci.yml", "sha256": sha256(read_regular(WORKFLOW))},
        ],
        "publication-disposition": DISPOSITION,
        "source": {
            "commit": commit,
            "repository": "https://github.com/ds1sqe/type-bridge",
            "tree": tree,
        },
        "subjects": [
            {"filename": item["filename"], "sha256": item["sha256"]} for item in artifacts
        ],
    }


def write_evidence(output: Path, values: Mapping[str, object]) -> None:
    output.mkdir(parents=True, exist_ok=True)
    for name, value in values.items():
        destination = output / name
        destination.write_bytes(canonical_json(value))
        os.chmod(destination, 0o644)


def evidence_records(output: Path) -> list[dict[str, object]]:
    return [
        {
            "filename": name,
            "sha256": sha256(read_regular(output / name)),
            "size": len(read_regular(output / name)),
        }
        for name in sorted(EVIDENCE_NAMES)
        if name != "candidate-manifest.json"
    ]


def validate_evidence_records(declared: object, output: Path) -> None:
    if declared != evidence_records(output):
        raise SecurityError("candidate evidence digest drifted")


def validate_signature_document(value: object) -> None:
    if value != signature_policy():
        raise SecurityError("signature identity or protected-release policy drifted")


def validate_provenance_document(
    value: object, artifacts: Sequence[Mapping[str, Any]], commit: str, tree: str
) -> None:
    if value != provenance(artifacts, commit, tree):
        raise SecurityError("candidate provenance source, materials, builder, or subjects drifted")


def assemble(
    cli_path: Path, runtime_path: Path, generated_path: Path, audit_path: Path, output: Path
) -> None:
    validate_frozen_security_policy()
    cli_manifest = cli.validate(cli_path)
    runtime_manifest = packages.validate_runtime(runtime_path)
    generated_manifest = packages.validate_generated(generated_path)
    identities = {
        (item["source-commit"], item["source-tree"])
        for item in (cli_manifest, runtime_manifest, generated_manifest)
    }
    if len(identities) != 1:
        raise SecurityError("candidate artifacts do not share one source commit and tree")
    commit, tree = identities.pop()
    if not HEX40.fullmatch(commit) or not HEX40.fullmatch(tree):
        raise SecurityError("candidate source identity is malformed")
    artifacts = [
        artifact_record(cli_path, cli_manifest, "cli"),
        artifact_record(runtime_path, runtime_manifest, "runtime"),
        artifact_record(generated_path, generated_manifest, "generated-package"),
    ]
    metadata = cargo_metadata()
    cli_closure = runtime_closure(metadata, ("type-bridge-cli",))
    c_closure = runtime_closure(metadata, ("type-bridge-c",))
    audit = validate_audit(load_json(audit_path))
    cli_files = cli.safe_archive_files(cli_path)
    runtime_files = packages.safe_archive_files(runtime_path, packages.RUNTIME_MEMBERS)
    generated_files = packages.safe_archive_files(generated_path, packages.PACKAGE_MEMBERS)
    forbidden = tuple(
        {
            str(ROOT).encode(),
            str(CORE).encode(),
            str(Path.home()).encode(),
            os.environ.get("CARGO_HOME", "").encode(),
        }
    )
    payloads = {f"cli:{path}": body for path, body in cli_files.items()}
    payloads.update({f"runtime:{path}": body for path, body in runtime_files.items()})
    payloads.update({f"generated:{path}": body for path, body in generated_files.items()})
    security = scan_payloads(payloads, forbidden)
    validate_binary_payloads(cli_files, runtime_files)
    values: dict[str, object] = {
        "cli.spdx.json": cargo_sbom(artifacts[0], metadata, cli_closure, "type-bridge-cli"),
        "runtime.spdx.json": cargo_sbom(artifacts[1], metadata, c_closure, "type-bridge-c"),
        "generated.spdx.json": generated_sbom(artifacts[2], artifacts[1]),
        "rustsec-report.json": audit,
        "security-scan.json": security,
        "provenance.json": provenance(artifacts, commit, tree),
        "signature-policy.json": signature_policy(),
    }
    evidence = [
        {
            "filename": name,
            "sha256": sha256(canonical_json(value)),
            "size": len(canonical_json(value)),
        }
        for name, value in sorted(values.items())
    ]
    payload = {
        "artifacts": artifacts,
        "evidence": evidence,
        "format": FORMAT,
        "publication-disposition": DISPOSITION,
        "source-commit": commit,
        "source-tree": tree,
    }
    payload["candidate-set-id"] = "sha256:" + sha256(canonical_json(payload))
    values["candidate-manifest.json"] = payload
    write_evidence(output, values)
    validate_evidence(output, cli_path, runtime_path, generated_path)


def validate_evidence(
    output: Path, cli_path: Path, runtime_path: Path, generated_path: Path
) -> None:
    validate_frozen_security_policy()
    if not output.is_dir() or output.is_symlink():
        raise SecurityError("candidate evidence directory is absent or linked")
    if tuple(sorted(path.name for path in output.iterdir())) != tuple(sorted(EVIDENCE_NAMES)):
        raise SecurityError("candidate evidence member set drifted")
    values = {name: load_json(output / name, canonical=True) for name in EVIDENCE_NAMES}
    manifest = values["candidate-manifest.json"]
    required_manifest = {
        "artifacts",
        "candidate-set-id",
        "evidence",
        "format",
        "publication-disposition",
        "source-commit",
        "source-tree",
    }
    if set(manifest) != required_manifest:
        raise SecurityError("candidate manifest fields drifted")
    candidate_id = manifest.pop("candidate-set-id", None)
    expected_id = "sha256:" + sha256(canonical_json(manifest))
    manifest["candidate-set-id"] = candidate_id
    if candidate_id != expected_id or manifest.get("format") != FORMAT:
        raise SecurityError("candidate-set identity drifted")
    if manifest.get("publication-disposition") != DISPOSITION:
        raise SecurityError("candidate publication disposition widened")
    expected_artifacts = [
        (cli_path, cli.validate(cli_path), "cli"),
        (runtime_path, packages.validate_runtime(runtime_path), "runtime"),
        (generated_path, packages.validate_generated(generated_path), "generated-package"),
    ]
    if manifest.get("artifacts") != [artifact_record(*item) for item in expected_artifacts]:
        raise SecurityError("candidate artifact identity drifted")
    source_identities = {
        (item[1]["source-commit"], item[1]["source-tree"]) for item in expected_artifacts
    }
    if source_identities != {(manifest["source-commit"], manifest["source-tree"])}:
        raise SecurityError("candidate source identity drifted")
    validate_evidence_records(manifest.get("evidence"), output)
    validate_audit(values["rustsec-report.json"])
    validate_signature_document(values["signature-policy.json"])
    validate_provenance_document(
        values["provenance.json"],
        manifest["artifacts"],
        manifest["source-commit"],
        manifest["source-tree"],
    )

    metadata = cargo_metadata()
    expected_sboms = {
        "cli.spdx.json": cargo_sbom(
            manifest["artifacts"][0],
            metadata,
            runtime_closure(metadata, ("type-bridge-cli",)),
            "type-bridge-cli",
        ),
        "runtime.spdx.json": cargo_sbom(
            manifest["artifacts"][1],
            metadata,
            runtime_closure(metadata, ("type-bridge-c",)),
            "type-bridge-c",
        ),
        "generated.spdx.json": generated_sbom(manifest["artifacts"][2], manifest["artifacts"][1]),
    }
    for name, expected in expected_sboms.items():
        if values[name] != expected:
            raise SecurityError(f"SBOM package graph or artifact identity drifted: {name}")

    cli_files = cli.safe_archive_files(cli_path)
    runtime_files = packages.safe_archive_files(runtime_path, packages.RUNTIME_MEMBERS)
    generated_files = packages.safe_archive_files(generated_path, packages.PACKAGE_MEMBERS)
    forbidden = tuple(
        {
            str(ROOT).encode(),
            str(CORE).encode(),
            str(Path.home()).encode(),
            os.environ.get("CARGO_HOME", "").encode(),
        }
    )
    payloads = {f"cli:{path}": body for path, body in cli_files.items()}
    payloads.update({f"runtime:{path}": body for path, body in runtime_files.items()})
    payloads.update({f"generated:{path}": body for path, body in generated_files.items()})
    if values["security-scan.json"] != scan_payloads(payloads, forbidden):
        raise SecurityError("candidate security scan policy or result drifted")
    validate_binary_payloads(cli_files, runtime_files)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    prune = commands.add_parser("prune-lock")
    prune.add_argument("--output", type=Path, required=True)
    for name in ("assemble", "validate"):
        command = commands.add_parser(name)
        command.add_argument("--cli", type=Path, required=True)
        command.add_argument("--runtime", type=Path, required=True)
        command.add_argument("--generated", type=Path, required=True)
        command.add_argument("--output", type=Path, required=True)
        if name == "assemble":
            command.add_argument("--audit-report", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        if args.command == "prune-lock":
            write_pruned_lock(args.output)
        elif args.command == "assemble":
            assemble(args.cli, args.runtime, args.generated, args.audit_report, args.output)
        else:
            validate_evidence(args.output, args.cli, args.runtime, args.generated)
    except (SecurityError, cli.CandidateError, packages.CandidateError) as error:
        print(f"C distribution security gate rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
