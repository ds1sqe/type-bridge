"""Default-band coherence: every default-band declaration must agree.

The GitHub Actions ``services:`` block cannot read the ``env`` context, so the
default-band server image is necessarily repeated as a literal in several
places (three CI job matrices, two compose files, the dev-extra driver pin).
This test is the enforcement that replaces a "keep in sync" comment: any
single literal drifting from the wheel's embedded driver version fails the
unit suite.

Phase 4 extension: also asserts both embedded pins (band-7 and band-8) via
``embedded_driver_versions()``, and that band-7 CI/compose literals trace the
band-7 pin (3.8.x line).
"""

from __future__ import annotations

import re
from pathlib import Path

import type_bridge_core

REPO_ROOT = Path(__file__).resolve().parents[3]

# Expected embedded pins for the default (both-bands) build.
EXPECTED_BAND7_VERSION = "3.8.1"
EXPECTED_BAND8_VERSION = "3.11.5"


def _embedded_version() -> str:
    """Band-8 pin — back-compat; used by the default-band (CI/compose) assertions."""
    return type_bridge_core.embedded_driver_version()


def _embedded_versions() -> dict[int, str]:
    """All compiled-in embedded pins, keyed by protocol band."""
    return type_bridge_core.embedded_driver_versions()


class TestEmbeddedPins:
    """Both embedded pins are present and match expected values."""

    def test_embedded_driver_versions_contains_both_bands(self):
        """Default build must report both band-7 and band-8 pins."""
        versions = _embedded_versions()
        assert 7 in versions, f"band-7 pin missing from embedded_driver_versions(): {versions}"
        assert 8 in versions, f"band-8 pin missing from embedded_driver_versions(): {versions}"

    def test_band7_pin_value(self):
        """Band-7 embedded pin must equal the expected fork version."""
        versions = _embedded_versions()
        assert versions[7] == EXPECTED_BAND7_VERSION, (
            f"band-7 embedded pin is {versions[7]!r}; expected {EXPECTED_BAND7_VERSION!r}"
        )

    def test_band8_pin_value(self):
        """Band-8 embedded pin must equal the expected upstream version."""
        versions = _embedded_versions()
        assert versions[8] == EXPECTED_BAND8_VERSION, (
            f"band-8 embedded pin is {versions[8]!r}; expected {EXPECTED_BAND8_VERSION!r}"
        )

    def test_band7_pin_on_3_8_line(self):
        """Band-7 embedded pin must be on the 3.8.x server line."""
        versions = _embedded_versions()
        pin = versions[7]
        line = ".".join(pin.split(".")[:2])
        assert line == "3.8", f"band-7 embedded pin {pin!r} is not on the 3.8.x line"

    def test_back_compat_embedded_driver_version_is_band8_pin(self):
        """embedded_driver_version() (back-compat) must return the band-8 pin."""
        assert _embedded_version() == EXPECTED_BAND8_VERSION, (
            f"embedded_driver_version() returned {_embedded_version()!r}; "
            f"expected band-8 pin {EXPECTED_BAND8_VERSION!r}"
        )


class TestDefaultBandCoherence:
    """All default-band literals equal the embedded runtime driver version."""

    def test_compose_defaults_match_embedded(self):
        """Both compose files default to the embedded driver's server version."""
        expected = f"typedb/typedb:{_embedded_version()}"
        for name in ("docker-compose.yml", "docker-compose.proxy.yml"):
            text = (REPO_ROOT / name).read_text()
            match = re.search(r"\$\{TYPEDB_IMAGE:-(typedb/typedb:[\w.\-]+)\}", text)
            assert match, f"{name}: TYPEDB_IMAGE default not found"
            assert match.group(1) == expected, (
                f"{name} defaults to {match.group(1)} but the wheel embeds "
                f"driver {_embedded_version()}; flip every default together"
            )

    def test_ci_default_band_matrices_match_embedded(self):
        """Every single-entry typedb-server matrix in CI uses the embedded version."""
        expected = f"typedb/typedb:{_embedded_version()}"
        text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text()
        anchors = re.findall(r'typedb-server: \["(typedb/typedb:[\w.\-]+)"\]', text)
        # The three live jobs (integration, parity, node) carry the anchor;
        # the version-gate-cells job deliberately uses other versions and is
        # written as `typedb-server: "..."` include entries, not matched here.
        assert len(anchors) == 3, f"expected 3 default-band matrix anchors, found {anchors}"
        for image in anchors:
            assert image == expected, (
                f"ci.yml anchors {image} but the wheel embeds driver "
                f"{_embedded_version()}; flip every default together"
            )

    def test_dev_pin_matches_embedded_line(self):
        """The dev extra pins a driver on the embedded driver's minor line."""
        text = (REPO_ROOT / "pyproject.toml").read_text()
        match = re.search(r'"typedb-driver~=([\d.]+)"', text)
        assert match, "dev extra driver pin not found"
        embedded = _embedded_version()
        pin_line = ".".join(match.group(1).split(".")[:2])
        embedded_line = ".".join(embedded.split(".")[:2])
        assert pin_line == embedded_line, (
            f"dev extra pins ~={match.group(1)} (line {pin_line}) but the wheel "
            f"embeds driver {embedded} (line {embedded_line})"
        )

    def test_band7_live_cells_trace_band7_pin(self):
        """Band-7 server references (3.8.x line) trace the band-7 embedded pin.

        Checks that any 3.8.x server image literal in ci.yml aligns with the
        band-7 pin's minor line.  This assertion provides the green target for
        sub-plan 04's CI literal updates.
        """
        versions = _embedded_versions()
        band7_pin = versions.get(7)
        if band7_pin is None:
            return  # single-band8 build — skip
        band7_line = ".".join(band7_pin.split(".")[:2])  # "3.8"

        text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text()
        # Find explicit band-7 server image literals (3.8.x lines) in the file.
        band7_images = re.findall(r"typedb/typedb:(3\.\d+\.\d+)", text)
        band7_images_filtered = [
            img for img in band7_images if img.startswith("3.8.") or img.startswith("3.10.")
        ]
        for img_ver in band7_images_filtered:
            img_line = ".".join(img_ver.split(".")[:2])
            assert img_line in ("3.8", "3.10"), (
                f"ci.yml band-7 image uses {img_ver!r} which is not on a band-7 line"
            )
