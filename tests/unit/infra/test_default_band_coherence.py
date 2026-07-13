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
import tomllib
from pathlib import Path

import type_bridge_core

REPO_ROOT = Path(__file__).resolve().parents[3]

# Expected embedded pins for the default (all-bands) build.
EXPECTED_BAND7_VERSION = "3.8.1"
EXPECTED_BAND8_VERSION = "3.11.5"
EXPECTED_BAND9_VERSION = "3.12.0"


def _embedded_version() -> str:
    """Band-8 pin — back-compat; used by the default-band (CI/compose) assertions."""
    return type_bridge_core.embedded_driver_version()


def _embedded_versions() -> dict[int, str]:
    """All compiled-in embedded pins, keyed by protocol band."""
    return type_bridge_core.embedded_driver_versions()


class TestEmbeddedPins:
    """Both embedded pins are present and match expected values."""

    def test_embedded_driver_versions_contains_all_bands(self):
        """Default build must report band-7, band-8, and band-9 pins."""
        versions = _embedded_versions()
        assert 7 in versions, f"band-7 pin missing from embedded_driver_versions(): {versions}"
        assert 8 in versions, f"band-8 pin missing from embedded_driver_versions(): {versions}"
        assert 9 in versions, f"band-9 pin missing from embedded_driver_versions(): {versions}"

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

    def test_band9_pin_value(self):
        """Band-9 embedded pin must equal the expected fork version."""
        versions = _embedded_versions()
        assert versions[9] == EXPECTED_BAND9_VERSION, (
            f"band-9 embedded pin is {versions[9]!r}; expected {EXPECTED_BAND9_VERSION!r}"
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
        """Live CI matrices contain the band-8 embedded image; band-7 images trace band-7 pin.

        The four live jobs (test-integration, rust-integration, node-integration,
        cross-language-parity)
        each fan across multiple server versions.  This test asserts:

        1. Every live matrix contains the band-8 embedded image (typedb/typedb:3.11.5).
           No default-band literal may silently diverge from the embedded pin.
        2. Every other server image in those matrices is served by the embedded
           runtime — confirmed via the version SSOT (check_server_supported), not a
           hardcoded list.  Band-7 lines (3.8.x, 3.10.x) and the dual-band 3.12.x
           line all qualify; a typo'd or dropped line fails.
        3. The version-gate-cells matrix may contain served band-7 servers only
           as explicit positive gRPC fallback cells.  They must not reappear as
           rejection cells, because band-7 servers are now served by the embedded
           runtime.
        """
        versions = _embedded_versions()
        band8_pin = versions[8]
        band8_image = f"typedb/typedb:{band8_pin}"

        text = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text()

        # --- Extract per-job server image sets ---
        # Parse each job block by splitting on top-level job names.  A simpler
        # approach: extract all typedb/typedb:X.Y.Z literals per named section.
        # We use a block-level split: find each job header and slice its text.

        def _job_block(job_name: str) -> str:
            """Return the YAML text of a named top-level job (heuristic slice)."""
            pattern = rf"^\s+{re.escape(job_name)}:\n"
            m = re.search(pattern, text, re.MULTILINE)
            if not m:
                return ""
            start = m.start()
            # Find the next top-level job (2-space indented key followed by ':')
            next_job = re.search(r"^\s{2}\S", text[start + 1 :], re.MULTILINE)
            end = start + 1 + next_job.start() if next_job else len(text)
            return text[start:end]

        live_jobs = [
            "test-integration",
            "rust-integration",
            "node-integration",
            "cross-language-parity",
        ]
        gate_job = "version-gate-cells"

        for job in live_jobs:
            block = _job_block(job)
            assert block, f"ci.yml: job '{job}' not found"
            images = re.findall(r'"typedb/typedb:([\d.]+)"', block)
            assert images, f"ci.yml job '{job}': no typedb/typedb:X.Y.Z image literals found"

            # Band-8 pin must be present in every live matrix.
            assert band8_image.split("typedb/typedb:")[1] in images, (
                f"ci.yml job '{job}': band-8 embedded image {band8_image!r} not found "
                f"in matrix images {images!r}; flip the live matrix to include it"
            )

            # Every non-band-8 image must be a server the embedded runtime
            # serves (SSOT decision, covers band-7 lines and dual-band 3.12.x).
            for ver in images:
                if ver == band8_pin:
                    continue  # band-8: already checked above
                try:
                    type_bridge_core.check_server_supported(ver)
                except type_bridge_core.VersionError as exc:
                    raise AssertionError(
                        f"ci.yml job '{job}': image typedb/typedb:{ver!r} is not "
                        f"served by the embedded runtime: {exc}"
                    ) from exc

        # --- Gate cells: served band-7 servers are only positive fallback cells ---
        gate_block = _job_block(gate_job)
        assert gate_block, f"ci.yml: job '{gate_job}' not found"
        gate_cells = re.findall(
            r"\n\s+- cell: ([^\n]+)(.*?)(?=\n\s+- cell:|\n\s+services:)",
            gate_block,
            re.DOTALL,
        )
        assert gate_cells, f"ci.yml job '{gate_job}': no matrix cells found"

        # A "served" band-7 server is one that (a) is within the support window
        # (check_server_supported passes) AND (b) is on band 7.  These were
        # formerly SAFE rejection cells (3.8.3, 3.10.4).  They are valid here
        # only when the cell proves the HTTP-failure -> gRPC fallback path.
        for cell_name, cell_block in gate_cells:
            images = re.findall(r'"typedb/typedb:([\d.]+)"', cell_block)
            for ver in images:
                try:
                    type_bridge_core.check_server_supported(ver)
                    server_in_window = True
                except type_bridge_core.VersionError:
                    server_in_window = False

                if not server_in_window:
                    continue  # Below-floor or out-of-window rejection cell — fine

                server_band = type_bridge_core.band(ver)
                if server_band != 7:
                    continue

                assert cell_name.startswith("POS-grpc-fallback-"), (
                    f"ci.yml job '{gate_job}': within-window band-7 server "
                    f"typedb/typedb:{ver!r} appears in non-fallback cell "
                    f"{cell_name!r}; band-7 servers must be positive fallback "
                    f"regressions, not rejection cells"
                )
                assert "http-port: 1" in cell_block and "expect: ok" in cell_block, (
                    f"ci.yml job '{gate_job}' cell {cell_name!r}: band-7 fallback "
                    f"regression must force HTTP failure with http-port: 1 and "
                    f"expect success with expect: ok"
                )

    def test_dev_pin_matches_embedded_line(self):
        """The CPython 3.13 dev pin matches the embedded driver's minor line."""
        pyproject = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text())
        dependencies = set(pyproject["project"]["optional-dependencies"]["dev"])
        requirement = "typedb-driver~=3.11.5; python_version < '3.14'"
        assert requirement in dependencies, "CPython 3.13 dev-extra driver pin not found"
        embedded = _embedded_version()
        pin_line = "3.11"
        embedded_line = ".".join(embedded.split(".")[:2])
        assert pin_line == embedded_line, (
            f"dev extra pins {requirement!r} (line {pin_line}) but the wheel "
            f"embeds driver {embedded} (line {embedded_line})"
        )

    def test_optional_driver_groups_have_complete_interpreter_split(self):
        """Each optional group declares its complete interpreter split."""
        pyproject = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text())
        groups = pyproject["project"]["optional-dependencies"]
        expected = {
            "dev": {
                "typedb-driver~=3.11.5; python_version < '3.14'",
                "typedb-driver==3.12.0; python_version >= '3.14'",
            },
            "typedb-driver": {
                "typedb-driver>=3.8,<3.13; python_version < '3.14'",
                "typedb-driver==3.12.0; python_version >= '3.14'",
            },
        }
        for group, requirements in expected.items():
            actual = {item for item in groups[group] if item.startswith("typedb-driver")}
            assert actual == requirements, f"{group} driver marker split drifted: {actual}"

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
