#!/usr/bin/env python3
"""Generate and verify the native Python/Node Rust dependency notice.

The custom TypeDB ownership, compatibility-band provenance, and TypeBridge MIT
sections remain hand-audited in the notice. This tool owns only the delimited
cargo-about-evaluated dependency-license appendix. It runs cargo-about from
each released binding root independently and forms a deterministic union so
neither root can hide evaluated dependencies reachable only from the other.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_WORKSPACE = REPOSITORY_ROOT / "type-bridge-core"
CARGO_ABOUT_VERSION = "0.9.1"
RUST_TOOLCHAIN = "1.94.1"
BEGIN_MARKER = "<!-- BEGIN GENERATED RUST DEPENDENCY NOTICE -->"
END_MARKER = "<!-- END GENERATED RUST DEPENDENCY NOTICE -->"
PYTHON_NOTICE = Path("python/type_bridge_core/THIRD_PARTY_NOTICES.md")
NODE_NOTICE = Path("crates/node/THIRD_PARTY_NOTICES.md")
ABOUT_CONFIG = Path("about.toml")
EXPECTED_TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
)
EXPECTED_WORKAROUNDS = ("chrono", "prost", "ring", "rustix", "rustls")
CURVE25519_LICENSE_CHECKSUM = "cca0bd3c4fcdba74145ef9d49c62337e2c9fbf9368288f11d0547f1b0273219f"
EXCEPTION_MARKERS: Mapping[str, tuple[str, ...]] = {
    "LLVM-exception": ("LLVM Exceptions to the Apache 2.0 License",),
}
CRATES_IO_SOURCES = {
    "registry+https://github.com/rust-lang/crates.io-index",
    "registry+https://index.crates.io/",
}


class ValidationError(RuntimeError):
    """The locked notice contract is incomplete, stale, or ambiguous."""


@dataclass(frozen=True)
class RootSpec:
    name: str
    manifest: Path
    features: tuple[str, ...] = ()


ROOT_SPECS = (
    RootSpec("Python", Path("crates/python/Cargo.toml"), ("pyo3/extension-module",)),
    RootSpec("Node", Path("crates/node/Cargo.toml")),
)


@dataclass(frozen=True, order=True)
class PackageKey:
    name: str
    version: str
    source: str


@dataclass(frozen=True, order=True)
class LicenseTextKey:
    license_id: str
    digest: str


@dataclass
class PackageRecord:
    expression: str
    roots: set[str] = field(default_factory=set)
    license_texts: set[LicenseTextKey] = field(default_factory=set)


@dataclass(frozen=True)
class NoticePolicy:
    accepted: tuple[str, ...]
    targets: tuple[str, ...]


SpdxNode = tuple[Any, ...]


def sha256_bytes(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def require_string(value: Any, *, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValidationError(f"{label} must be a non-empty string")
    return value


def require_list(value: Any, *, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise ValidationError(f"{label} must be an array")
    return value


def load_policy(workspace: Path) -> NoticePolicy:
    config_path = workspace / ABOUT_CONFIG
    try:
        config = tomllib.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ValidationError(f"Cannot read native notice policy {config_path}: {error}") from error

    accepted_values = require_list(config.get("accepted"), label="about.toml accepted")
    accepted = tuple(
        require_string(value, label=f"about.toml accepted[{index}]")
        for index, value in enumerate(accepted_values)
    )
    if len(accepted) != len(set(accepted)):
        raise ValidationError("about.toml accepted SPDX identifiers must be unique")

    target_values = require_list(config.get("targets"), label="about.toml targets")
    targets = tuple(
        require_string(value, label=f"about.toml targets[{index}]")
        for index, value in enumerate(target_values)
    )
    if targets != EXPECTED_TARGETS:
        raise ValidationError(
            "about.toml targets must exactly match the Python/Node release union: "
            f"actual={targets!r}, expected={EXPECTED_TARGETS!r}"
        )
    for boolean_key in ("ignore-build-dependencies", "ignore-dev-dependencies"):
        if config.get(boolean_key) is not True:
            raise ValidationError(f"about.toml {boolean_key} must be true")
    private = config.get("private")
    if not isinstance(private, dict) or private.get("ignore") is not True:
        raise ValidationError("about.toml private.ignore must be true")
    workarounds = tuple(config.get("workarounds", ()))
    if workarounds != EXPECTED_WORKAROUNDS:
        raise ValidationError(
            "about.toml workarounds drifted: "
            f"actual={workarounds!r}, expected={EXPECTED_WORKAROUNDS!r}"
        )
    curve = config.get("curve25519-dalek")
    if not isinstance(curve, dict) or not isinstance(curve.get("clarify"), dict):
        raise ValidationError("about.toml must clarify curve25519-dalek licensing")
    clarification = curve["clarify"]
    if clarification.get("license") != "BSD-3-Clause":
        raise ValidationError("curve25519-dalek clarification must remain BSD-3-Clause")
    files = clarification.get("files")
    expected_file = {
        "path": "LICENSE",
        "checksum": CURVE25519_LICENSE_CHECKSUM,
    }
    if files != [expected_file]:
        raise ValidationError(
            "curve25519-dalek clarification must bind the complete upstream LICENSE: "
            f"actual={files!r}, expected={[expected_file]!r}"
        )

    for accepted_id in accepted:
        node = parse_spdx_expression(accepted_id)
        if node[0] not in {"license", "with"}:
            raise ValidationError(
                "about.toml accepted entries must be one SPDX license or license WITH exception: "
                f"{accepted_id!r}"
            )
        if node[0] == "with" and node[2] not in EXCEPTION_MARKERS:
            raise ValidationError(
                f"No notice-text marker is defined for accepted exception {node[2]!r}"
            )
    return NoticePolicy(accepted=accepted, targets=targets)


TOKEN_PATTERN = re.compile(r"\(|\)|[A-Za-z0-9][A-Za-z0-9.+-]*")


def tokenize_spdx(expression: str) -> tuple[str, ...]:
    tokens: list[str] = []
    cursor = 0
    for match in TOKEN_PATTERN.finditer(expression):
        if expression[cursor : match.start()].strip():
            raise ValidationError(f"Unsupported SPDX syntax in {expression!r}")
        tokens.append(match.group(0))
        cursor = match.end()
    if expression[cursor:].strip() or not tokens:
        raise ValidationError(f"Unsupported SPDX syntax in {expression!r}")
    return tuple(tokens)


class SpdxParser:
    def __init__(self, expression: str) -> None:
        self.expression = expression
        self.tokens = tokenize_spdx(expression)
        self.index = 0

    def parse(self) -> SpdxNode:
        node = self.parse_or()
        if self.index != len(self.tokens):
            raise ValidationError(f"Trailing SPDX tokens in {self.expression!r}")
        return node

    def parse_or(self) -> SpdxNode:
        node = self.parse_and()
        while self.take("OR"):
            node = ("or", node, self.parse_and())
        return node

    def parse_and(self) -> SpdxNode:
        node = self.parse_with()
        while self.take("AND"):
            node = ("and", node, self.parse_with())
        return node

    def parse_with(self) -> SpdxNode:
        node = self.parse_primary()
        if self.take("WITH"):
            if node[0] != "license":
                raise ValidationError(
                    f"SPDX WITH must follow one license identifier in {self.expression!r}"
                )
            exception = self.take_identifier()
            node = ("with", node[1], exception)
        return node

    def parse_primary(self) -> SpdxNode:
        if self.take("("):
            node = self.parse_or()
            if not self.take(")"):
                raise ValidationError(f"Unclosed SPDX parenthesis in {self.expression!r}")
            return node
        return ("license", self.take_identifier())

    def take(self, token: str) -> bool:
        if self.index < len(self.tokens) and self.tokens[self.index] == token:
            self.index += 1
            return True
        return False

    def take_identifier(self) -> str:
        if self.index >= len(self.tokens):
            raise ValidationError(f"Missing SPDX identifier in {self.expression!r}")
        token = self.tokens[self.index]
        if token in {"(", ")", "AND", "OR", "WITH"}:
            raise ValidationError(f"Expected SPDX identifier in {self.expression!r}")
        self.index += 1
        return token


def parse_spdx_expression(expression: str) -> SpdxNode:
    return SpdxParser(expression).parse()


def iter_spdx_atoms(node: SpdxNode) -> Iterable[tuple[str, str | None]]:
    kind = node[0]
    if kind == "license":
        yield (node[1], None)
        return
    if kind == "with":
        yield (node[1], node[2])
        return
    yield from iter_spdx_atoms(node[1])
    yield from iter_spdx_atoms(node[2])


def atom_is_satisfied(
    atom: tuple[str, str | None],
    selected: Sequence[tuple[LicenseTextKey, str]],
) -> bool:
    license_id, exception = atom
    for key, text in selected:
        if key.license_id != license_id:
            continue
        if exception is None:
            return True
        markers = EXCEPTION_MARKERS.get(exception)
        if markers is not None and all(marker in text for marker in markers):
            return True
    return False


def expression_is_satisfied(
    node: SpdxNode,
    selected: Sequence[tuple[LicenseTextKey, str]],
) -> bool:
    kind = node[0]
    if kind == "license":
        return atom_is_satisfied((node[1], None), selected)
    if kind == "with":
        return atom_is_satisfied((node[1], node[2]), selected)
    if kind == "and":
        return expression_is_satisfied(node[1], selected) and expression_is_satisfied(
            node[2], selected
        )
    if kind == "or":
        return expression_is_satisfied(node[1], selected) or expression_is_satisfied(
            node[2], selected
        )
    raise AssertionError(f"Unknown SPDX AST node {kind!r}")


def normalize_license_text(text: str, *, label: str) -> str:
    if not text or "\x00" in text:
        raise ValidationError(f"{label} must contain non-NUL license text")
    # A closing Markdown fence needs its own line. Preserve every harvested
    # byte and add only a missing terminal LF, then fingerprint what ships.
    return text if text.endswith("\n") else f"{text}\n"


def normalize_source(package: Mapping[str, Any], *, workspace: Path) -> str:
    source = package.get("source")
    if source in CRATES_IO_SOURCES:
        return "crates.io"
    if source is None:
        manifest_path = Path(require_string(package.get("manifest_path"), label="manifest path"))
        try:
            relative = manifest_path.resolve().relative_to(workspace.resolve())
        except ValueError as error:
            raise ValidationError(
                f"Local dependency manifest escapes the native workspace: {manifest_path}"
            ) from error
        return f"workspace:{relative.as_posix()}"
    raise ValidationError(f"Unsupported native dependency source {source!r}")


def package_key(package: Mapping[str, Any], *, workspace: Path) -> PackageKey:
    return PackageKey(
        name=require_string(package.get("name"), label="package name"),
        version=require_string(package.get("version"), label="package version"),
        source=normalize_source(package, workspace=workspace),
    )


def accepted_output_ids(policy: NoticePolicy) -> set[str]:
    output: set[str] = set()
    for entry in policy.accepted:
        node = parse_spdx_expression(entry)
        output.add(node[1])
    return output


def parse_cargo_about_payload(
    payload: Mapping[str, Any],
    *,
    root: RootSpec,
    policy: NoticePolicy,
    workspace: Path,
) -> tuple[dict[PackageKey, PackageRecord], dict[LicenseTextKey, str]]:
    crates = require_list(payload.get("crates"), label=f"{root.name} cargo-about crates")
    licenses = require_list(payload.get("licenses"), label=f"{root.name} cargo-about licenses")
    packages: dict[PackageKey, PackageRecord] = {}
    metadata_ids: dict[str, PackageKey] = {}

    for index, row in enumerate(crates):
        if not isinstance(row, dict) or not isinstance(row.get("package"), dict):
            raise ValidationError(f"{root.name} cargo-about crates[{index}] is malformed")
        package = row["package"]
        key = package_key(package, workspace=workspace)
        expression = require_string(
            row.get("license"), label=f"{root.name} {key.name} license expression"
        )
        parse_spdx_expression(expression)
        if key in packages:
            raise ValidationError(f"{root.name} cargo-about repeated package {key!r}")
        packages[key] = PackageRecord(expression=expression, roots={root.name})
        metadata_id = require_string(package.get("id"), label=f"{root.name} package id")
        if metadata_id in metadata_ids:
            raise ValidationError(f"{root.name} cargo-about repeated metadata id {metadata_id!r}")
        metadata_ids[metadata_id] = key

    texts: dict[LicenseTextKey, str] = {}
    allowed_ids = accepted_output_ids(policy)
    for index, license_record in enumerate(licenses):
        if not isinstance(license_record, dict):
            raise ValidationError(f"{root.name} cargo-about licenses[{index}] is malformed")
        license_id = require_string(
            license_record.get("id"), label=f"{root.name} license identifier"
        )
        if license_id not in allowed_ids:
            raise ValidationError(
                f"{root.name} cargo-about selected unknown/unaccepted SPDX id {license_id!r}"
            )
        text = normalize_license_text(
            require_string(license_record.get("text"), label=f"{root.name} {license_id} text"),
            label=f"{root.name} {license_id} text",
        )
        key = LicenseTextKey(license_id, sha256_bytes(text.encode("utf-8")))
        prior = texts.setdefault(key, text)
        if prior != text:
            raise ValidationError(f"SHA-256 collision for harvested license text {key!r}")
        used_by = require_list(
            license_record.get("used_by"), label=f"{root.name} {license_id} used_by"
        )
        if not used_by:
            raise ValidationError(f"{root.name} cargo-about emitted orphan license text {key!r}")
        for used_index, usage in enumerate(used_by):
            if not isinstance(usage, dict) or not isinstance(usage.get("crate"), dict):
                raise ValidationError(
                    f"{root.name} {license_id} used_by[{used_index}] is malformed"
                )
            metadata_id = require_string(
                usage["crate"].get("id"), label=f"{root.name} used_by package id"
            )
            package = metadata_ids.get(metadata_id)
            if package is None:
                raise ValidationError(
                    f"{root.name} license {license_id!r} references unknown package {metadata_id!r}"
                )
            packages[package].license_texts.add(key)

    for key, record in packages.items():
        if not record.license_texts:
            raise ValidationError(f"{root.name} package has no selected license text: {key!r}")
    return packages, texts


def merge_payloads(
    payloads: Mapping[str, Mapping[str, Any]],
    *,
    policy: NoticePolicy,
    workspace: Path,
) -> tuple[dict[PackageKey, PackageRecord], dict[LicenseTextKey, str]]:
    union_packages: dict[PackageKey, PackageRecord] = {}
    union_texts: dict[LicenseTextKey, str] = {}
    for root in ROOT_SPECS:
        payload = payloads.get(root.name)
        if payload is None:
            raise ValidationError(f"Missing cargo-about result for {root.name} root")
        packages, texts = parse_cargo_about_payload(
            payload, root=root, policy=policy, workspace=workspace
        )
        for key, text in texts.items():
            prior = union_texts.setdefault(key, text)
            if prior != text:
                raise ValidationError(f"SHA-256 collision for union license text {key!r}")
        for key, incoming in packages.items():
            current = union_packages.get(key)
            if current is None:
                union_packages[key] = incoming
                continue
            if current.expression != incoming.expression:
                raise ValidationError(
                    f"Cargo roots disagree on {key.name} {key.version} SPDX expression: "
                    f"{current.expression!r} != {incoming.expression!r}"
                )
            if current.license_texts != incoming.license_texts:
                raise ValidationError(
                    f"Cargo roots disagree on harvested license texts for {key.name} {key.version}"
                )
            current.roots.update(incoming.roots)

    used_policy_entries: set[str] = set()
    for key, record in union_packages.items():
        selected = tuple((text_key, union_texts[text_key]) for text_key in record.license_texts)
        expression = parse_spdx_expression(record.expression)
        if not expression_is_satisfied(expression, selected):
            selected_labels = tuple(
                f"{text_key.license_id}@{text_key.digest}" for text_key in record.license_texts
            )
            raise ValidationError(
                f"Selected texts do not satisfy {key.name} {key.version} compound SPDX "
                f"expression {record.expression!r}: {selected_labels!r}"
            )
        expression_atoms = set(iter_spdx_atoms(expression))
        for entry in policy.accepted:
            accepted_node = parse_spdx_expression(entry)
            accepted_atom = (
                (accepted_node[1], None)
                if accepted_node[0] == "license"
                else (accepted_node[1], accepted_node[2])
            )
            if accepted_atom in expression_atoms and atom_is_satisfied(accepted_atom, selected):
                used_policy_entries.add(entry)

    stale = tuple(entry for entry in policy.accepted if entry not in used_policy_entries)
    if stale:
        raise ValidationError(f"about.toml contains stale accepted SPDX entries: {stale!r}")
    return union_packages, union_texts


def markdown_cell(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", " ")


def license_fence(text: str) -> str:
    longest = max((len(match.group(0)) for match in re.finditer(r"~+", text)), default=0)
    return "~" * max(4, longest + 1)


def closure_fingerprint_payload(
    packages: Mapping[PackageKey, PackageRecord],
    texts: Mapping[LicenseTextKey, str],
    policy: NoticePolicy,
) -> bytes:
    payload = {
        "cargo_about": CARGO_ABOUT_VERSION,
        "rust_toolchain": RUST_TOOLCHAIN,
        "accepted": list(policy.accepted),
        "targets": list(policy.targets),
        "roots": [
            {
                "name": root.name,
                "manifest": root.manifest.as_posix(),
                "features": list(root.features),
            }
            for root in ROOT_SPECS
        ],
        "packages": [
            {
                "name": key.name,
                "version": key.version,
                "source": key.source,
                "expression": record.expression,
                "roots": sorted(record.roots),
                "license_texts": [
                    {"id": text_key.license_id, "sha256": text_key.digest}
                    for text_key in sorted(record.license_texts)
                ],
            }
            for key, record in sorted(packages.items())
        ],
        "texts": [
            {
                "id": key.license_id,
                "sha256": key.digest,
                "bytes": len(text.encode("utf-8")),
            }
            for key, text in sorted(texts.items())
        ],
    }
    return json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")


def render_generated_block(
    packages: Mapping[PackageKey, PackageRecord],
    texts: Mapping[LicenseTextKey, str],
    policy: NoticePolicy,
) -> str:
    fingerprint = sha256_bytes(closure_fingerprint_payload(packages, texts, policy))
    target_list = ", ".join(f"`{target}`" for target in policy.targets)
    lines = [
        BEGIN_MARKER,
        "## Locked Rust dependency closure",
        "",
        "This appendix is generated by",
        "`scripts/ci/generate_native_dependency_notice.py`; do not edit it by hand.",
        f"It is the deterministic union produced by `cargo-about {CARGO_ABOUT_VERSION}`",
        f"under Rust `{RUST_TOOLCHAIN}` from the locked Python and Node release roots.",
        "It covers the complete cargo-about-evaluated third-party and public-local",
        "license union for those roots; the private TypeBridge-authored crates omitted",
        "by policy remain covered by the canonical MIT section above.",
        "The scan uses online source-text discovery intentionally: replacing a unique",
        "upstream copyright/license file with a generic SPDX body changes this appendix",
        "and fails the byte-for-byte CI freshness check.",
        "",
        f"- Python root: `{ROOT_SPECS[0].manifest.as_posix()}` with feature "
        f"`{ROOT_SPECS[0].features[0]}`",
        f"- Node root: `{ROOT_SPECS[1].manifest.as_posix()}` with default features",
        f"- Release targets: {target_list}",
        "- Excluded from this package inventory: build-only and development-only "
        "dependencies, plus private TypeBridge-authored crates covered by the MIT section",
        f"- Closure fingerprint: `sha256:{fingerprint}`",
        "",
        "Every evaluated package's complete cargo-about-resolved SPDX expression is retained",
        "below. The",
        "selected-text column identifies the exact UTF-8 bodies that satisfy that",
        "expression; `AND` branches require every body, while `OR` branches follow the",
        "priority order in `type-bridge-core/about.toml`. A terminal LF is added only when",
        "cargo-about returns a text without one, and the displayed SHA-256 covers the",
        "bytes actually reproduced in this notice.",
        "",
        "### Package inventory",
        "",
        "| Package | Version | Release roots | Source | Resolved SPDX expression | Selected text(s) |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    root_order = {root.name: index for index, root in enumerate(ROOT_SPECS)}
    for key, record in sorted(packages.items()):
        roots = ", ".join(sorted(record.roots, key=root_order.__getitem__))
        selected = ", ".join(
            f"`{text_key.license_id}@sha256:{text_key.digest}`"
            for text_key in sorted(record.license_texts)
        )
        lines.append(
            "| "
            f"`{markdown_cell(key.name)}` | `{markdown_cell(key.version)}` | "
            f"{markdown_cell(roots)} | `{markdown_cell(key.source)}` | "
            f"`{markdown_cell(record.expression)}` | {selected} |"
        )

    lines.extend(
        [
            "",
            "### Harvested license texts",
            "",
            "Each distinct cargo-about-selected text is reproduced in full, including",
            "source-specific copyright and attribution language when present. Inventory",
            "rows reference these bodies by",
            "their complete SHA-256 digest; unreferenced or missing bodies are rejected.",
        ]
    )
    for key, text in sorted(texts.items()):
        fence = license_fence(text)
        lines.extend(
            [
                "",
                f"#### `{key.license_id}` — `sha256:{key.digest}`",
                "",
                f"{fence}text",
                text.removesuffix("\n"),
                fence,
            ]
        )
    lines.extend(["", END_MARKER])
    return "\n".join(lines)


def decode_notice(payload: bytes, *, label: str) -> str:
    try:
        return payload.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValidationError(f"{label} is not UTF-8") from error


def generated_block_bounds(notice: str) -> tuple[int, int] | None:
    begin_count = notice.count(BEGIN_MARKER)
    end_count = notice.count(END_MARKER)
    if begin_count == end_count == 0:
        return None
    if begin_count != 1 or end_count != 1:
        raise ValidationError(
            "Native notice must contain exactly one generated appendix marker pair"
        )
    begin = notice.index(BEGIN_MARKER)
    end = notice.index(END_MARKER, begin) + len(END_MARKER)
    return begin, end


def hand_maintained_notice_skeleton(notice: str) -> str | None:
    bounds = generated_block_bounds(notice)
    if bounds is None:
        return None
    begin, end = bounds
    return f"{notice[:begin]}{BEGIN_MARKER}{END_MARKER}{notice[end:]}"


def replace_generated_block(notice: str, block: str, *, allow_missing: bool) -> str:
    bounds = generated_block_bounds(notice)
    if bounds is None:
        if not allow_missing:
            raise ValidationError("Native notice has no generated Rust dependency appendix")
        separator = "" if notice.endswith("\n\n") else ("\n" if notice.endswith("\n") else "\n\n")
        return f"{notice}{separator}{block}\n"
    begin, end = bounds
    return f"{notice[:begin]}{block}{notice[end:]}"


def load_json(path: Path, *, label: str) -> Mapping[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValidationError(f"Cannot read {label} cargo-about JSON {path}: {error}") from error
    if not isinstance(payload, dict):
        raise ValidationError(f"{label} cargo-about JSON must be an object")
    return payload


def verify_cargo_about_version(executable: str, *, workspace: Path) -> None:
    try:
        result = subprocess.run(
            [executable, "--version"],
            cwd=workspace,
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        raise ValidationError(f"Cannot execute pinned cargo-about: {error}") from error
    expected = f"cargo-about {CARGO_ABOUT_VERSION}"
    if result.stdout.strip() != expected:
        raise ValidationError(
            f"cargo-about version drifted: actual={result.stdout.strip()!r}, expected={expected!r}"
        )


def run_cargo_about(executable: str, root: RootSpec, *, workspace: Path) -> Mapping[str, Any]:
    command = [
        executable,
        "generate",
        "--format",
        "json",
        "--locked",
        "--fail",
        "--manifest-path",
        root.manifest.as_posix(),
        "--config",
        ABOUT_CONFIG.as_posix(),
    ]
    if root.features:
        command.extend(("--features", " ".join(root.features)))
    try:
        result = subprocess.run(
            command,
            cwd=workspace,
            check=True,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except subprocess.CalledProcessError as error:
        detail = error.stderr.strip() or error.stdout.strip() or str(error)
        raise ValidationError(f"cargo-about failed for {root.name} root: {detail}") from error
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValidationError(f"Cannot scan {root.name} native dependency root: {error}") from error
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ValidationError(f"cargo-about returned invalid {root.name} JSON: {error}") from error
    if not isinstance(payload, dict):
        raise ValidationError(f"cargo-about returned non-object {root.name} JSON")
    return payload


def atomic_write(path: Path, payload: bytes) -> None:
    mode = path.stat().st_mode
    temporary_name: str | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="wb", dir=path.parent, prefix=f".{path.name}.", delete=False
        ) as stream:
            temporary_name = stream.name
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(temporary_name, mode)
        os.replace(temporary_name, path)
    finally:
        if temporary_name is not None:
            Path(temporary_name).unlink(missing_ok=True)


def check_or_write_notices(
    *,
    workspace: Path,
    block: str,
    write: bool,
) -> None:
    python_path = workspace / PYTHON_NOTICE
    node_path = workspace / NODE_NOTICE
    for path in (python_path, node_path):
        if not path.is_file() or path.is_symlink():
            raise ValidationError(f"Native distribution notice is missing or non-regular: {path}")
    try:
        python_bytes = python_path.read_bytes()
        node_bytes = node_path.read_bytes()
    except OSError as error:
        raise ValidationError(f"Cannot read native distribution notices: {error}") from error
    python_notice = decode_notice(python_bytes, label="Python third-party notice")
    node_notice = decode_notice(node_bytes, label="Node third-party notice")
    if python_bytes != node_bytes:
        if not write:
            raise ValidationError("Python and Node third-party notices must be byte-identical")
        python_skeleton = hand_maintained_notice_skeleton(python_notice)
        node_skeleton = hand_maintained_notice_skeleton(node_notice)
        if python_skeleton is None or python_skeleton != node_skeleton:
            raise ValidationError(
                "Python and Node hand-maintained third-party notice sections disagree"
            )
    current = python_notice
    expected = replace_generated_block(current, block, allow_missing=write).encode("utf-8")
    if write:
        atomic_write(python_path, expected)
        atomic_write(node_path, expected)
        return
    if python_bytes != expected:
        raise ValidationError(
            "Native Rust dependency appendix is stale; regenerate with "
            "python scripts/ci/generate_native_dependency_notice.py --write: "
            f"actual_sha256={sha256_bytes(python_bytes)!r}, "
            f"expected_sha256={sha256_bytes(expected)!r}"
        )


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--workspace",
        type=Path,
        default=DEFAULT_WORKSPACE,
        help="type-bridge-core workspace root",
    )
    parser.add_argument(
        "--cargo-about",
        default="cargo-about",
        help=f"cargo-about {CARGO_ABOUT_VERSION} executable",
    )
    parser.add_argument("--python-json", type=Path, help="pre-generated Python JSON for tests")
    parser.add_argument("--node-json", type=Path, help="pre-generated Node JSON for tests")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="verify committed notices (default)")
    mode.add_argument("--write", action="store_true", help="regenerate both committed notices")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    workspace = args.workspace.resolve()
    try:
        policy = load_policy(workspace)
        supplied_json = (args.python_json, args.node_json)
        if any(supplied_json) and not all(supplied_json):
            raise ValidationError("--python-json and --node-json must be supplied together")
        if all(supplied_json):
            payloads = {
                "Python": load_json(args.python_json, label="Python"),
                "Node": load_json(args.node_json, label="Node"),
            }
        else:
            verify_cargo_about_version(args.cargo_about, workspace=workspace)
            payloads = {
                root.name: run_cargo_about(args.cargo_about, root, workspace=workspace)
                for root in ROOT_SPECS
            }
        packages, texts = merge_payloads(payloads, policy=policy, workspace=workspace)
        block = render_generated_block(packages, texts, policy)
        check_or_write_notices(workspace=workspace, block=block, write=args.write)
    except ValidationError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    action = "regenerated" if args.write else "verified"
    print(
        f"Native dependency notice {action} with cargo-about {CARGO_ABOUT_VERSION} "
        f"under Rust {RUST_TOOLCHAIN}."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
