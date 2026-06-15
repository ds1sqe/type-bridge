"""Integration tests for the version gate (type_bridge/version.py wiring).

Requires a running TypeDB server (band-8, 3.11.5 compose default).
All tests use @pytest.mark.integration.
"""

from __future__ import annotations

import os
from typing import Any

import pytest

import type_bridge.typedb_driver as _tdm
from type_bridge import version
from type_bridge.session import Database

# ---------------------------------------------------------------------------
# Positive: live connect + round trip
# ---------------------------------------------------------------------------


@pytest.mark.integration
@pytest.mark.order(410)
class TestVersionGateLivePositive:
    """Gate passes on the live compose server (band-8 driver × band-8 server)."""

    def test_connect_passes_version_gate(self, clean_db: Database):
        """Database.connect() succeeds against the live server — gate green."""
        # clean_db fixture already called connect(); assert Rust handle is up.
        assert getattr(clean_db, "_rust_backend_database", None) is not None

    def test_connect_minimal_round_trip(self, clean_db: Database):
        """Gate passes and a trivial schema query returns a non-error response."""
        # Define a unique attribute to confirm the connection is genuinely open.
        clean_db.execute_query(
            "define attribute vg_smoke_attr, value string;",
            transaction_type="schema",
        )
        schema = clean_db.get_schema()
        assert "vg_smoke_attr" in schema


# ---------------------------------------------------------------------------
# server_version() probe against the live server
# ---------------------------------------------------------------------------


@pytest.mark.integration
@pytest.mark.order(411)
class TestServerVersionLive:
    """server_version() returns the live server's exact version string."""

    def test_server_version_returns_string(self):
        """server_version(address) returns a non-empty version string."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        result = _tdm.server_version(TEST_DB_ADDRESS)
        assert isinstance(result, str)
        assert len(result) > 0

    def test_server_version_looks_like_semver(self):
        """server_version(address) returns something that looks like a version."""
        import re

        from tests.integration.conftest import TEST_DB_ADDRESS

        result = _tdm.server_version(TEST_DB_ADDRESS)
        assert re.match(r"\d+\.\d+\.\d+", result), (
            f"server_version did not return a semver-like string: {result!r}"
        )

    def test_server_version_matches_runtime_accepted_range(self):
        """server_version result is accepted by the embedded Rust runtime."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        sv = _tdm.server_version(TEST_DB_ADDRESS)
        # TypeBridge's default backend uses embedded Rust drivers, not the
        # optional Python typedb-driver package.  The installed Python driver
        # may target a different protocol band from the live test server.
        version.ensure_runtime_supported(sv)
        assert version.band(sv) in _tdm.embedded_driver_versions()


# ---------------------------------------------------------------------------
# Negative: monkeypatched detector — gate fires before driver construction
# ---------------------------------------------------------------------------


def _mismatched_driver_version(server: str) -> str:
    """Pick an installed-driver version from the opposite protocol band.

    The embedded runtime now serves every in-window server, so the only
    in-window rejection left is the installed-driver band mismatch.  The
    mismatching driver must be chosen relative to the live server's band
    for the cell to fire on every server line.
    """
    import type_bridge_core

    return "3.10.0" if type_bridge_core.band(server) == 8 else "3.11.5"


@pytest.mark.integration
@pytest.mark.order(412)
class TestVersionGateLiveNegative:
    """Python driver gate fires before driver construction when monkeypatched."""

    def test_cross_band_raises_before_driver_constructed(self, monkeypatch: pytest.MonkeyPatch):
        """Monkeypatching driver_version to the opposite band of the live server
        causes direct driver access to raise UnsupportedVersionError before TypeDB.driver is
        called."""
        import type_bridge.session as session_mod
        from tests.integration.conftest import TEST_DB_ADDRESS

        detected_server = _tdm.server_version(TEST_DB_ADDRESS, tls=False)
        mismatched = _mismatched_driver_version(detected_server)
        monkeypatch.setattr(_tdm, "driver_version", lambda: mismatched)

        # Spy on TypeDB.driver to assert it is never reached.
        # Patch at session_mod.TypeDB — the name bound there via `from ... import`.
        real_typedb = session_mod.TypeDB
        driver_called: list[bool] = []

        class _SpyTypeDB:
            def driver(self, *args: Any, **kwargs: Any) -> object:
                driver_called.append(True)
                return real_typedb.driver(*args, **kwargs)

            def __getattr__(self, name: str) -> object:
                return getattr(real_typedb, name)

        monkeypatch.setattr(session_mod, "TypeDB", _SpyTypeDB())

        db = Database(address=TEST_DB_ADDRESS, database="test_gate_negative")
        with pytest.raises(version.UnsupportedVersionError):
            _ = db.driver

        assert driver_called == [], "TypeDB.driver should not have been called"

    def test_unsupported_version_message_contains_both_versions(
        self, monkeypatch: pytest.MonkeyPatch
    ):
        """Error message from live gate contains both driver and server version strings."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        detected_server = _tdm.server_version(TEST_DB_ADDRESS, tls=False)
        mismatched = _mismatched_driver_version(detected_server)
        monkeypatch.setattr(_tdm, "driver_version", lambda: mismatched)

        db = Database(address=TEST_DB_ADDRESS, database="test_gate_message")
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            _ = db.driver

        msg = str(exc_info.value)
        driver_line = mismatched.rsplit(".", 1)[0]
        server_line = detected_server.rsplit(".", 1)[0]
        assert driver_line in msg, f"Driver version missing from: {msg!r}"
        assert server_line in msg, f"Server version missing from: {msg!r}"
        assert "install" in msg.lower(), f"'install' hint missing from: {msg!r}"


# ---------------------------------------------------------------------------
# @distinct ordered-role form defines cleanly on a live server
# ---------------------------------------------------------------------------


@pytest.mark.integration
@pytest.mark.order(4121)
class TestVersionGateExplicitHttpPort:
    """Database constructed with explicit http_port=8000 connects and round-trips cleanly."""

    def test_database_gate_explicit_default_http_port(self, test_database):
        """Explicit http_port=8000 is accepted by the gate and a live connection succeeds."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db = Database(
            address=TEST_DB_ADDRESS,
            database=test_database,
            http_port=8000,
        )
        db.connect()
        assert getattr(db, "_rust_backend_database", None) is not None
        db.close()


@pytest.mark.integration
@pytest.mark.order(413)
class TestDistinctOrderedFormLive:
    """The ordered-role @distinct form is the one every live server accepts."""

    def test_ordered_distinct_defines_cleanly(self, clean_db: Database):
        """`relates name[] @distinct` defines without error on the live server."""
        clean_db.execute_query(
            "define\n"
            "entity vg_member_ent;\n"
            "relation vg_distinct_team,\n"
            "    relates vg_member[] @distinct;\n"
            "vg_member_ent plays vg_distinct_team:vg_member;",
            transaction_type="schema",
        )
        schema = clean_db.get_schema()
        assert "vg_distinct_team" in schema
        assert "@distinct" in schema


# ---------------------------------------------------------------------------
# Opt-in: non-default HTTP port proof (demand-only, not in CI)
# ---------------------------------------------------------------------------

_PROOF_ADDRESS = os.environ.get("TYPEDB_PROOF_ADDRESS", "")
_PROOF_HTTP_PORT = os.environ.get("TYPEDB_PROOF_HTTP_PORT", "")
_proof_vars_set = bool(_PROOF_ADDRESS and _PROOF_HTTP_PORT)


@pytest.mark.integration
@pytest.mark.order(414)
@pytest.mark.skipif(
    not _proof_vars_set,
    reason="TYPEDB_PROOF_ADDRESS and TYPEDB_PROOF_HTTP_PORT must both be set to run this test",
)
class TestGateNonDefaultHttpPort:
    """Validates that the version gate probes the correct server when http_port is remapped.

    On a host running multiple TypeDB instances (e.g. one on :8000, one on :9000), the gate
    must probe the port that matches the target server, not always :8000.  This test exercises
    that path on demand against a real alternative-port server; it is skipped in CI where no
    such server is available.

    Set TYPEDB_PROOF_ADDRESS and TYPEDB_PROOF_HTTP_PORT to opt in.
    """

    def test_gate_validates_non_default_http_port(self):
        """Gate succeeds and driver is live when http_port points at the correct server."""
        db = Database(
            address=os.environ["TYPEDB_PROOF_ADDRESS"],
            database="proof_http_port",
            http_port=int(os.environ["TYPEDB_PROOF_HTTP_PORT"]),
        )
        db.connect()
        assert getattr(db, "_rust_backend_database", None) is not None
        db.close()
