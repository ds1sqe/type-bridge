"""Contracts for native-distribution licensing and source availability."""

from __future__ import annotations

import hashlib
import json
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PYTHON_NOTICE = ROOT / "type-bridge-core/python/type_bridge_core/THIRD_PARTY_NOTICES.md"
NODE_NOTICE = ROOT / "type-bridge-core/crates/node/THIRD_PARTY_NOTICES.md"
ABOUT_POLICY = ROOT / "type-bridge-core/about.toml"


def test_native_distributions_ship_one_byte_identical_consolidated_notice() -> None:
    notice = PYTHON_NOTICE.read_bytes()

    assert notice == NODE_NOTICE.read_bytes()

    core = tomllib.loads((ROOT / "type-bridge-core/pyproject.toml").read_text(encoding="utf-8"))
    assert core["project"]["license"] == {"text": "MIT"}

    node = json.loads(
        (ROOT / "type-bridge-core/crates/node/package.json").read_text(encoding="utf-8")
    )
    assert node["license"] == "MIT"
    assert "THIRD_PARTY_NOTICES.md" in node["files"]


def test_notice_pins_namespaced_sources_and_complete_license_texts() -> None:
    notice = PYTHON_NOTICE.read_text(encoding="utf-8")
    normalized_notice = " ".join(notice.split())

    for identity in (
        "type-bridge-typedb-driver-b8` 3.11.5",
        "type-bridge-typedb-protocol-b8` 3.11.0",
        "official `typedb-driver` 3.12.1",
        "official `typedb-protocol` 3.12.0",
        "`ed25519-dalek` | 2.2.0",
        "`curve25519-dalek` | 4.1.3",
    ):
        assert identity in notice

    for upstream_commit in (
        "7e669e41d9fee22fde8d5e60be7edbf00c6ec64b",
        "1db5bdd6579352d31343da28be41844ed07da1b5",
    ):
        assert upstream_commit in notice

    for official_source in (
        "https://crates.io/crates/typedb-driver/3.12.1",
        "https://crates.io/crates/typedb-protocol/3.12.0",
    ):
        assert official_source in notice

    assert "no active, consumed, or release-input TypeBridge band-9" in normalized_notice
    assert "no transaction terminal-close patch or other behavioral change" in normalized_notice
    assert "driver `src/` tree and protocol generated source" in normalized_notice
    assert "TypeDB remains the original upstream owner and source" in normalized_notice
    assert "immutable TypeBridge v2.0.0 source tag" in normalized_notice
    assert "band-7" not in normalized_notice.lower()
    assert "typedb-driver-b7" not in normalized_notice
    assert "typedb-protocol-b7" not in normalized_notice

    assert "Permission is hereby granted, free of charge" in notice
    assert "Apache License\n                           Version 2.0" in notice
    assert "Mozilla Public License Version 2.0" in notice
    assert 'Exhibit B - "Incompatible With Secondary Licenses" Notice' in notice

    ed25519 = notice.split("## ed25519-dalek 2.2.0 — BSD 3-Clause License\n\n", 1)[1].split(
        "\n\n## curve25519-dalek 4.1.3 — BSD 3-Clause License", 1
    )[0]
    curve25519 = notice.split("## curve25519-dalek 4.1.3 — BSD 3-Clause License\n\n", 1)[1].split(
        "\n\n<!-- BEGIN GENERATED RUST DEPENDENCY NOTICE -->", 1
    )[0]
    assert hashlib.sha256((ed25519.strip() + "\n").encode()).hexdigest() == (
        "7b6a19666b1304f2dec9202b0dd2d92ca220558aa23f07d4c5e86dbd271050b9"
    )
    assert hashlib.sha256((curve25519.strip() + "\n").encode()).hexdigest() == (
        "6737ef630c5e038c2c1d1f45e25f00e51e9493dab7fbfb6b4a3a178e76c8187b"
    )

    lock = tomllib.loads((ROOT / "type-bridge-core/Cargo.lock").read_text(encoding="utf-8"))
    locked = {(package["name"], package["version"]) for package in lock["package"]}
    assert ("ed25519-dalek", "2.2.0") in locked
    assert ("curve25519-dalek", "4.1.3") in locked


def test_generated_notice_covers_locked_compound_and_platform_licenses() -> None:
    notice = PYTHON_NOTICE.read_text(encoding="utf-8")
    assert notice.count("<!-- BEGIN GENERATED RUST DEPENDENCY NOTICE -->") == 1
    assert notice.count("<!-- END GENERATED RUST DEPENDENCY NOTICE -->") == 1
    generated = notice.split("<!-- BEGIN GENERATED RUST DEPENDENCY NOTICE -->", 1)[1]

    for contract in (
        "`cargo-about 0.9.1`",
        "under Rust `1.94.1`",
        "Closure fingerprint: `sha256:",
        "`ring` | `0.17.14`",
        "`Apache-2.0 AND ISC`",
        "`matchit` | `0.7.3`",
        "`MIT AND BSD-3-Clause`",
        "`unicode-ident` | `1.0.24`",
        "`(MIT OR Apache-2.0) AND Unicode-3.0`",
        "`webpki-roots` | `1.0.7`",
        "`CDLA-Permissive-2.0`",
        "`rustls-webpki` | `0.103.10`",
        "`untrusted` | `0.9.0`",
        "`subtle` | `2.6.1`",
        "`icu_collections` | `2.2.0`",
        "`Unicode-3.0`",
        "`curve25519-dalek` | `4.1.3`",
        "BSD-3-Clause@sha256:cca0bd3c4fcdba74145ef9d49c62337e2c9fbf9368288f11d0547f1b0273219f",
    ):
        assert contract in generated

    policy = tomllib.loads(ABOUT_POLICY.read_text(encoding="utf-8"))
    assert policy["accepted"] == [
        "MIT",
        "Apache-2.0",
        "MPL-2.0",
        "BSD-3-Clause",
        "ISC",
        "Unicode-3.0",
        "CDLA-Permissive-2.0",
        "Apache-2.0 WITH LLVM-exception",
    ]
    assert policy["targets"] == [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "aarch64-pc-windows-msvc",
    ]
    assert policy["curve25519-dalek"]["clarify"]["files"] == [
        {
            "path": "LICENSE",
            "checksum": "cca0bd3c4fcdba74145ef9d49c62337e2c9fbf9368288f11d0547f1b0273219f",
        }
    ]
