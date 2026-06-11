"""Default-band coherence: every default-band declaration must agree.

The GitHub Actions ``services:`` block cannot read the ``env`` context, so the
default-band server image is necessarily repeated as a literal in several
places (three CI job matrices, two compose files, the dev-extra driver pin).
This test is the enforcement that replaces a "keep in sync" comment: any
single literal drifting from the wheel's embedded driver version fails the
unit suite.
"""

from __future__ import annotations

import re
from pathlib import Path

import type_bridge_core

REPO_ROOT = Path(__file__).resolve().parents[3]


def _embedded_version() -> str:
    return type_bridge_core.embedded_driver_version()


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
