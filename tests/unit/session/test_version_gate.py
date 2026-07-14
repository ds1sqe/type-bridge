"""Unit tests for the version gate shim (type_bridge/version.py) and
driver-wrapper version helpers (type_bridge/typedb_driver.py).

No live server or network access is required.  PyO3 functions are monkeypatched
at the ``type_bridge_core`` module attribute level.
"""

from __future__ import annotations

import importlib.metadata
import re
import sys
from unittest.mock import MagicMock

import pytest
import type_bridge_core

import type_bridge.typedb_driver as _typedb_driver_mod
from type_bridge import version
from type_bridge.typedb_driver import driver_version, server_version


@pytest.fixture
def _python313_driver_runtime(monkeypatch: pytest.MonkeyPatch) -> None:
    """Run legacy-driver scenarios on an interpreter that supports their wheels."""
    monkeypatch.setattr(_typedb_driver_mod.sys, "version_info", (3, 13, 0))


# ---------------------------------------------------------------------------
# Import smoke — assert the re-exported constants have their expected values.
# ---------------------------------------------------------------------------


class TestImportSmoke:
    """Window constants must equal the documented values (not hand-copied)."""

    def test_min_supported_version(self):
        """min_supported_version() re-export must return the known window floor."""
        assert version.min_supported_version() == "3.8.0"

    def test_max_supported_line(self):
        """max_supported_line() re-export must return the known window ceiling line."""
        assert version.max_supported_line() == "3.12"


# ---------------------------------------------------------------------------
# band() re-export
# ---------------------------------------------------------------------------


class TestBandReexport:
    """band() is a plain re-export; verify it forwards correctly."""

    def test_band_7(self):
        """3.10.4 belongs to band 7."""
        assert version.band("3.10.4") == 7

    def test_band_8(self):
        """3.11.5 belongs to band 8."""
        assert version.band("3.11.5") == 8

    def test_band_9(self):
        """3.12.0 belongs to band 9."""
        assert version.band("3.12.0") == 9

    def test_band_unmapped_returns_none(self):
        """3.9.0 is not in a known band; core returns None."""
        assert version.band("3.9.0") is None


# ---------------------------------------------------------------------------
# ensure_supported — window boundaries
# ---------------------------------------------------------------------------


class TestEnsureSupportedWindowBoundaries:
    """ensure_supported raises UnsupportedVersionError for out-of-window pairs."""

    # For each in-window test the driver and server must share the same band so
    # the band check also passes.

    def test_below_floor_raises(self):
        """Server 3.7.9 is below the 3.8.0 floor — must reject."""
        with pytest.raises(version.UnsupportedVersionError):
            version.ensure_supported("3.8.1", "3.7.9")

    def test_floor_exact_accepted(self):
        """Driver 3.8.1 and server 3.8.0 are both band-7 at the floor — must accept."""
        # Succeeds with no exception.
        version.ensure_supported("3.8.1", "3.8.0")

    def test_top_line_accepted(self):
        """Driver 3.12.0 and server 3.12.0 are both band-9 at the ceiling — must accept."""
        version.ensure_supported("3.12.0", "3.12.0")

    def test_above_ceiling_raises(self):
        """Server 3.13.0 is above the 3.12 ceiling line — must reject."""
        with pytest.raises(version.UnsupportedVersionError):
            version.ensure_supported("3.12.0", "3.13.0")

    def test_ancient_version_raises(self):
        """Server 2.9.0 is a TypeDB 2.x version far below the floor — must reject."""
        with pytest.raises(version.UnsupportedVersionError):
            version.ensure_supported("3.10.0", "2.9.0")


# ---------------------------------------------------------------------------
# ensure_supported — band boundaries
# ---------------------------------------------------------------------------


class TestEnsureSupportedBandBoundaries:
    """Band-crossing pairs must be accepted or rejected as specified."""

    def test_cross_line_same_band_accepted(self):
        """Driver 3.10.0 (band-7) × server 3.8.3 (band-7) — same band, must accept."""
        version.ensure_supported("3.10.0", "3.8.3")

    def test_cross_line_same_band_reversed_accepted(self):
        """Driver 3.8.1 (band-7) × server 3.10.4 (band-7) — same band, must accept."""
        version.ensure_supported("3.8.1", "3.10.4")

    def test_driver_11_server_10_raises(self):
        """Driver 3.11.5 (band-8) × server 3.10.4 (band-7) — cross-band, must reject."""
        with pytest.raises(version.UnsupportedVersionError):
            version.ensure_supported("3.11.5", "3.10.4")

    def test_driver_10_server_11_raises(self):
        """Driver 3.10.0 (band-7) × server 3.11.5 (band-8) — cross-band, must reject."""
        with pytest.raises(version.UnsupportedVersionError):
            version.ensure_supported("3.10.0", "3.11.5")

    def test_driver_11_server_12_accepted(self):
        """Driver 3.11.5 (band-8) × server 3.12.0 (accepts 9 and 8) — must accept.

        Measured live: server 3.12 retains backward compatibility with
        band-8 drivers.
        """
        version.ensure_supported("3.11.5", "3.12.0")

    def test_driver_12_server_11_raises(self):
        """Driver 3.12.0 (band-9) × server 3.11.5 (accepts 8 only) — must reject.

        Measured live: the asymmetric direction — a band-9 driver is refused
        by a 3.11 server at connect.
        """
        with pytest.raises(version.UnsupportedVersionError):
            version.ensure_supported("3.12.0", "3.11.5")


# ---------------------------------------------------------------------------
# Error message content
# ---------------------------------------------------------------------------


class TestErrorMessageContent:
    """Error messages must contain versions and hints; must not contain band numbers."""

    def test_window_message_contains_version_and_floor(self):
        """Out-of-window message should name the rejected version and '3.8'."""
        with pytest.raises(version.UnsupportedVersionError, match=r"3\.7\.9"):
            version.ensure_supported("3.8.1", "3.7.9")

        try:
            version.ensure_supported("3.8.1", "3.7.9")
        except version.UnsupportedVersionError as exc:
            msg = str(exc)
            assert "3.8" in msg, f"Expected '3.8' in: {msg!r}"
            assert "band 7" not in msg, f"Protocol number leaked: {msg!r}"
            assert "band 8" not in msg, f"Protocol number leaked: {msg!r}"
            assert "0.0.0" not in msg, f"Placeholder version in message: {msg!r}"

    def test_cross_band_message_contains_both_versions_and_install(self):
        """Cross-band message should name both versions and 'install'."""
        try:
            version.ensure_supported("3.11.5", "3.10.4")
        except version.UnsupportedVersionError as exc:
            msg = str(exc)
            assert "3.11" in msg, f"Driver version missing from: {msg!r}"
            assert "3.10" in msg, f"Server version missing from: {msg!r}"
            assert "install" in msg.lower(), f"'install' hint missing from: {msg!r}"
            assert "band 7" not in msg, f"Protocol number leaked: {msg!r}"
            assert "band 8" not in msg, f"Protocol number leaked: {msg!r}"
            assert "0.0.0" not in msg, f"Placeholder version in message: {msg!r}"

    def test_window_message_no_protocol_numbers(self):
        """Ancient-version message must not expose raw band numbers."""
        try:
            version.ensure_supported("3.10.0", "2.9.0")
        except version.UnsupportedVersionError as exc:
            msg = str(exc)
            assert "band 7" not in msg
            assert "band 8" not in msg


# ---------------------------------------------------------------------------
# isinstance hierarchy
# ---------------------------------------------------------------------------


class TestIsinstanceHierarchy:
    """UnsupportedVersionError must satisfy isinstance for both types."""

    def test_isinstance_unsupported_version_error(self):
        """UnsupportedVersionError is an instance of UnsupportedVersionError."""
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            version.ensure_supported("3.8.1", "3.7.9")
        assert isinstance(exc_info.value, version.UnsupportedVersionError)

    def test_isinstance_core_version_error(self):
        """UnsupportedVersionError is also an instance of type_bridge_core.VersionError."""
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            version.ensure_supported("3.8.1", "3.7.9")
        assert isinstance(exc_info.value, type_bridge_core.VersionError)


# ---------------------------------------------------------------------------
# driver_version()
# ---------------------------------------------------------------------------


class TestDriverVersion:
    """driver_version() must return a plausible, installed version string."""

    def test_returns_nonempty_string(self):
        """driver_version() should return a non-empty string."""
        result = driver_version()
        assert isinstance(result, str)
        assert len(result) > 0

    def test_matches_version_pattern(self):
        """Return value should look like a version number."""
        result = driver_version()
        assert re.match(r"\d+\.\d+", result), f"Not a version string: {result!r}"

    def test_matches_importlib_metadata(self):
        """driver_version() is a tautological wiring smoke: matches importlib.metadata."""
        assert driver_version() == importlib.metadata.version("typedb-driver")


# ---------------------------------------------------------------------------
# server_version() delegation (monkeypatched)
# ---------------------------------------------------------------------------


class TestServerVersionDelegation:
    """server_version() must delegate to type_bridge_core.server_version."""

    def test_delegates_return_value(self, monkeypatch: pytest.MonkeyPatch):
        """Return value from core must be passed through unchanged."""
        monkeypatch.setattr(type_bridge_core, "server_version", lambda *a, **kw: "3.10.4")
        result = server_version("localhost:1729")
        assert result == "3.10.4"

    def test_passes_address(self, monkeypatch: pytest.MonkeyPatch):
        """Address argument must be forwarded to core."""
        received: list[tuple[tuple[object, ...], dict[str, object]]] = []

        def _fake(*args: object, **kwargs: object) -> str:
            received.append((args, kwargs))
            return "3.11.5"

        monkeypatch.setattr(type_bridge_core, "server_version", _fake)
        server_version("myhost:1729")
        assert received[0][0][0] == "myhost:1729"

    def test_passes_http_port(self, monkeypatch: pytest.MonkeyPatch):
        """http_port keyword argument must be forwarded to core."""
        received: list[tuple[tuple[object, ...], dict[str, object]]] = []

        def _fake(*args: object, **kwargs: object) -> str:
            received.append((args, kwargs))
            return "3.10.4"

        monkeypatch.setattr(type_bridge_core, "server_version", _fake)
        server_version("localhost:1729", http_port=9000)
        # Core function takes positional: (address, http_port, tls)
        args, _kwargs = received[0]
        assert args[1] == 9000

    def test_passes_tls_flag(self, monkeypatch: pytest.MonkeyPatch):
        """tls keyword argument must be forwarded to core."""
        received: list[tuple[tuple[object, ...], dict[str, object]]] = []

        def _fake(*args: object, **kwargs: object) -> str:
            received.append((args, kwargs))
            return "3.10.4"

        monkeypatch.setattr(type_bridge_core, "server_version", _fake)
        server_version("localhost:1729", tls=True)
        args, _kwargs = received[0]
        assert args[2] is True

    def test_propagates_version_error(self, monkeypatch: pytest.MonkeyPatch):
        """VersionError raised by core must propagate unmodified."""

        def _fail(*args: object, **kwargs: object) -> str:
            raise type_bridge_core.VersionError("unreachable")

        monkeypatch.setattr(type_bridge_core, "server_version", _fail)
        with pytest.raises(type_bridge_core.VersionError):
            server_version("localhost:1729")


# ---------------------------------------------------------------------------
# Phase 2 — create_driver_options band-keyed dispatch
# ---------------------------------------------------------------------------


@pytest.mark.usefixtures("_python313_driver_runtime")
class TestCreateDriverOptionsBand7:
    """Band-7 driver (3.10.x) uses DriverOptions(is_tls_enabled=...) keyword form."""

    def test_band7_calls_keyword_form_tls_off(self, monkeypatch: pytest.MonkeyPatch):
        """Band-7: DriverOptions called with is_tls_enabled=False."""
        mock_opts = MagicMock()
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.10.0")
        monkeypatch.setattr(_typedb_driver_mod, "DriverOptions", mock_opts)

        _typedb_driver_mod.create_driver_options(is_tls_enabled=False)
        mock_opts.assert_called_once_with(is_tls_enabled=False)

    def test_band7_calls_keyword_form_tls_on(self, monkeypatch: pytest.MonkeyPatch):
        """Band-7: DriverOptions called with is_tls_enabled=True."""
        mock_opts = MagicMock()
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.10.0")
        monkeypatch.setattr(_typedb_driver_mod, "DriverOptions", mock_opts)

        _typedb_driver_mod.create_driver_options(is_tls_enabled=True)
        mock_opts.assert_called_once_with(is_tls_enabled=True)

    def test_band7_returns_driver_options_result(self, monkeypatch: pytest.MonkeyPatch):
        """Band-7: return value comes from DriverOptions call."""
        sentinel = object()
        mock_opts = MagicMock(return_value=sentinel)
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.10.0")
        monkeypatch.setattr(_typedb_driver_mod, "DriverOptions", mock_opts)

        result = _typedb_driver_mod.create_driver_options()
        assert result is sentinel


@pytest.mark.usefixtures("_python313_driver_runtime")
class TestCreateDriverOptionsBand8:
    """Band-8 driver (3.11.x) uses DriverOptions(tls_config) positional form."""

    def _make_tls_config(self, enabled_obj: object, disabled_obj: object) -> type:
        """Return a fake DriverTlsConfig class with class-method stubs."""
        fake_cls = MagicMock()
        fake_cls.enabled_with_native_root_ca = MagicMock(return_value=enabled_obj)
        fake_cls.disabled = MagicMock(return_value=disabled_obj)
        return fake_cls  # type: ignore[return-value]

    def test_band8_tls_off_calls_disabled(self, monkeypatch: pytest.MonkeyPatch):
        """Band-8, TLS off: DriverTlsConfig.disabled() is used."""
        disabled_sentinel = object()
        tls_cls = self._make_tls_config(object(), disabled_sentinel)
        mock_opts = MagicMock()

        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.11.5")
        monkeypatch.setattr(_typedb_driver_mod, "DriverOptions", mock_opts)
        monkeypatch.setattr(_typedb_driver_mod, "_load_tls_config", lambda: tls_cls)

        _typedb_driver_mod.create_driver_options(is_tls_enabled=False)
        tls_cls.disabled.assert_called_once()
        mock_opts.assert_called_once_with(disabled_sentinel)

    def test_band8_tls_on_calls_enabled_with_native_root_ca(self, monkeypatch: pytest.MonkeyPatch):
        """Band-8, TLS on: DriverTlsConfig.enabled_with_native_root_ca() is used."""
        enabled_sentinel = object()
        tls_cls = self._make_tls_config(enabled_sentinel, object())
        mock_opts = MagicMock()

        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.11.5")
        monkeypatch.setattr(_typedb_driver_mod, "DriverOptions", mock_opts)
        monkeypatch.setattr(_typedb_driver_mod, "_load_tls_config", lambda: tls_cls)

        _typedb_driver_mod.create_driver_options(is_tls_enabled=True)
        tls_cls.enabled_with_native_root_ca.assert_called_once()
        mock_opts.assert_called_once_with(enabled_sentinel)

    def test_band8_positional_not_keyword(self, monkeypatch: pytest.MonkeyPatch):
        """Band-8: DriverOptions must be called with positional arg, not keyword."""
        calls: list[tuple[tuple[object, ...], dict[str, object]]] = []

        def _mock_opts(*args: object, **kwargs: object) -> object:
            calls.append((args, kwargs))
            return object()

        tls_cls = self._make_tls_config(object(), object())
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.11.5")
        monkeypatch.setattr(_typedb_driver_mod, "DriverOptions", _mock_opts)
        monkeypatch.setattr(_typedb_driver_mod, "_load_tls_config", lambda: tls_cls)

        _typedb_driver_mod.create_driver_options(is_tls_enabled=False)
        assert len(calls) == 1
        args, kwargs = calls[0]
        assert len(args) == 1, "Expected positional tls_config argument"
        assert kwargs == {}, "Expected no keyword arguments for band-8 DriverOptions"


class TestCreateDriverOptionsBand9:
    """Band-9 driver (3.12.x) uses the same DriverOptions(tls_config) form as band 8."""

    def test_band9_tls_off_calls_disabled(self, monkeypatch: pytest.MonkeyPatch):
        """Band-9, TLS off: DriverTlsConfig.disabled() is used, positionally."""
        disabled_sentinel = object()
        tls_cls = MagicMock()
        tls_cls.enabled_with_native_root_ca = MagicMock(return_value=object())
        tls_cls.disabled = MagicMock(return_value=disabled_sentinel)
        mock_opts = MagicMock()

        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.12.0")
        monkeypatch.setattr(_typedb_driver_mod, "DriverOptions", mock_opts)
        monkeypatch.setattr(_typedb_driver_mod, "_load_tls_config", lambda: tls_cls)

        _typedb_driver_mod.create_driver_options(is_tls_enabled=False)
        tls_cls.disabled.assert_called_once()
        mock_opts.assert_called_once_with(disabled_sentinel)


class TestCreateDriverOptionsBandNone:
    """Unknown band raises UnsupportedVersionError naming the installed version."""

    def test_unknown_band_raises(self, monkeypatch: pytest.MonkeyPatch):
        """driver_version 3.13.0 maps to band None → UnsupportedVersionError."""
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.13.0")
        with pytest.raises(version.UnsupportedVersionError):
            _typedb_driver_mod.create_driver_options()

    def test_unknown_band_message_contains_version(self, monkeypatch: pytest.MonkeyPatch):
        """UnsupportedVersionError message must name the rejected version."""
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.13.0")
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            _typedb_driver_mod.create_driver_options()
        assert "3.13.0" in str(exc_info.value)

    def test_unknown_band_message_no_band_numbers(self, monkeypatch: pytest.MonkeyPatch):
        """UnsupportedVersionError message must not expose raw band numbers."""
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.13.0")
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            _typedb_driver_mod.create_driver_options()
        msg = str(exc_info.value)
        assert "band 7" not in msg
        assert "band 8" not in msg
        assert "0.0.0" not in msg

    def test_unknown_band_message_is_interpreter_safe(self, monkeypatch: pytest.MonkeyPatch):
        """Remediation never recommends a native driver unavailable to this interpreter."""
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.13.0")
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            _typedb_driver_mod.create_driver_options()
        msg = str(exc_info.value)
        assert "type-bridge[typedb-driver]" in msg
        assert "typedb-driver~=3.10" not in msg
        if sys.version_info >= (3, 14):
            assert "driver 3.12.0" in msg
            assert "TypeDB 3.12" in msg

    def test_python314_rejects_band8_before_native_option_constructors(
        self, monkeypatch: pytest.MonkeyPatch
    ):
        """A manually installed 3.11 wheel cannot cross native FFI on CPython 3.14."""
        monkeypatch.setattr(_typedb_driver_mod.sys, "version_info", (3, 14, 0))
        monkeypatch.setattr(_typedb_driver_mod, "driver_version", lambda: "3.11.5")
        load_tls = MagicMock()
        options = MagicMock()
        monkeypatch.setattr(_typedb_driver_mod, "_load_tls_config", load_tls)
        monkeypatch.setattr(_typedb_driver_mod, "DriverOptions", options)

        with pytest.raises(version.UnsupportedVersionError, match=r"CPython 3\.14"):
            _typedb_driver_mod.create_driver_options()

        load_tls.assert_not_called()
        options.assert_not_called()


# ---------------------------------------------------------------------------
# Phase 2 — Database.connect() version gate wiring
# ---------------------------------------------------------------------------


@pytest.mark.usefixtures("_python313_driver_runtime")
class TestPythonDriverVersionGate:
    """Direct Python driver access must call the gate before TypeDB.driver()."""

    def test_gate_passes_on_supported_pair(self, monkeypatch: pytest.MonkeyPatch):
        """driver access succeeds when driver+server are in-window same-band."""
        import type_bridge.session as session_mod
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        # Patch at the module where each name is resolved during connect():
        # - driver_version / server_version → resolved via `typedb_driver.` module ref
        # - TypeDB / DriverOptions → resolved as names in session_mod (from ... import)
        monkeypatch.setattr(tdm, "driver_version", lambda: "3.10.0")
        monkeypatch.setattr(type_bridge_core, "server_version", lambda *a, **kw: "3.10.4")
        # Embedded read mocked too, so the test is independent of the wheel's
        # real embedded driver band.
        monkeypatch.setattr(tdm, "embedded_driver_version", lambda: "3.10.0")

        fake_driver = MagicMock()
        mock_typedb = MagicMock()
        mock_typedb.driver.return_value = fake_driver
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)
        monkeypatch.setattr(tdm, "DriverOptions", MagicMock())

        db = Database(address="localhost:1729", database="test_db")
        _ = db.driver
        mock_typedb.driver.assert_called_once()

    def test_server_version_pin_skips_http_probe(self, monkeypatch: pytest.MonkeyPatch):
        """driver access validates an explicit server_version without probing HTTP."""
        import type_bridge.session as session_mod
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        def _unexpected_server_version(*args: object, **kwargs: object) -> str:
            raise AssertionError("server_version HTTP probe should not be called")

        monkeypatch.setattr(tdm, "driver_version", lambda: "3.8.1")
        monkeypatch.setattr(tdm, "server_version", _unexpected_server_version)
        monkeypatch.setattr(tdm, "DriverOptions", MagicMock())

        fake_driver = MagicMock()
        mock_typedb = MagicMock()
        mock_typedb.driver.return_value = fake_driver
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)

        db = Database(
            address="localhost:1729",
            database="test_db",
            server_version="3.8.3",
        )
        _ = db.driver

        mock_typedb.driver.assert_called_once()

    def test_unsupported_server_version_pin_rejects_before_driver(
        self, monkeypatch: pytest.MonkeyPatch
    ):
        """unsupported explicit server_version fails before TypeDB.driver."""
        import type_bridge.session as session_mod
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        def _unexpected_server_version(*args: object, **kwargs: object) -> str:
            raise AssertionError("server_version HTTP probe should not be called")

        monkeypatch.setattr(tdm, "driver_version", lambda: "3.8.1")
        monkeypatch.setattr(tdm, "server_version", _unexpected_server_version)

        mock_typedb = MagicMock()
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)

        db = Database(
            address="localhost:1729",
            database="test_db",
            server_version="3.7.3",
        )
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            _ = db.driver

        assert "3.7.3" in str(exc_info.value)
        mock_typedb.driver.assert_not_called()

    def test_gate_fires_before_typedb_driver_on_unsupported_pair(
        self, monkeypatch: pytest.MonkeyPatch
    ):
        """driver access raises UnsupportedVersionError and TypeDB.driver is NOT called."""
        import type_bridge.session as session_mod
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        # Band-8 driver vs band-7 server — cross-band → rejected
        monkeypatch.setattr(tdm, "driver_version", lambda: "3.11.5")
        monkeypatch.setattr(type_bridge_core, "server_version", lambda *a, **kw: "3.10.4")

        mock_typedb = MagicMock()
        mock_credentials = MagicMock()
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)
        monkeypatch.setattr(session_mod, "Credentials", mock_credentials)

        db = Database(
            address="localhost:1729",
            database="test_db",
            username="admin",
            password="password",
        )
        with pytest.raises(version.UnsupportedVersionError):
            _ = db.driver
        mock_credentials.assert_not_called()
        mock_typedb.driver.assert_not_called()

    def test_python314_driver_rejection_precedes_credentials_constructor(
        self, monkeypatch: pytest.MonkeyPatch
    ):
        """Known-incompatible native Credentials is never called on CPython 3.14."""
        import type_bridge.session as session_mod
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        monkeypatch.setattr(tdm.sys, "version_info", (3, 14, 0))
        monkeypatch.setattr(tdm, "driver_version", lambda: "3.11.5")
        mock_credentials = MagicMock()
        mock_typedb = MagicMock()
        monkeypatch.setattr(session_mod, "Credentials", mock_credentials)
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)

        db = Database(
            address="localhost:1729",
            database="test_db",
            username="admin",
            password="password",
            server_version="3.12.0",
        )
        with pytest.raises(version.UnsupportedVersionError, match=r"CPython 3\.14"):
            _ = db.driver

        mock_credentials.assert_not_called()
        mock_typedb.driver.assert_not_called()

    def test_gate_fires_before_typedb_driver_below_window(self, monkeypatch: pytest.MonkeyPatch):
        """connect() raises UnsupportedVersionError for driver below window floor."""
        import type_bridge.session as session_mod
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        # Band-7 driver but server is 3.7.x — below window floor
        monkeypatch.setattr(tdm, "driver_version", lambda: "3.8.1")
        monkeypatch.setattr(type_bridge_core, "server_version", lambda *a, **kw: "3.7.9")

        mock_typedb = MagicMock()
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)
        monkeypatch.setattr(tdm, "DriverOptions", MagicMock())

        db = Database(address="localhost:1729", database="test_db")
        with pytest.raises(version.UnsupportedVersionError):
            _ = db.driver
        mock_typedb.driver.assert_not_called()

    def test_gate_error_message_contains_both_versions_and_install(
        self, monkeypatch: pytest.MonkeyPatch
    ):
        """UnsupportedVersionError from driver access must name both versions and 'install'."""
        import type_bridge.session as session_mod
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        monkeypatch.setattr(tdm, "driver_version", lambda: "3.11.5")
        monkeypatch.setattr(type_bridge_core, "server_version", lambda *a, **kw: "3.10.4")

        mock_typedb = MagicMock()
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)

        db = Database(address="localhost:1729", database="test_db")
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            _ = db.driver
        msg = str(exc_info.value)
        assert "3.11" in msg
        assert "3.10" in msg
        assert "install" in msg.lower()
        assert "band 7" not in msg
        assert "band 8" not in msg
        assert "0.0.0" not in msg

    def test_gate_error_message_no_band_numbers(self, monkeypatch: pytest.MonkeyPatch):
        """Gate error must not expose raw band numbers."""
        import type_bridge.session as session_mod
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        monkeypatch.setattr(tdm, "driver_version", lambda: "3.11.5")
        monkeypatch.setattr(type_bridge_core, "server_version", lambda *a, **kw: "3.10.4")

        mock_typedb = MagicMock()
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)

        db = Database(address="localhost:1729", database="test_db")
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            _ = db.driver
        msg = str(exc_info.value)
        assert "band 7" not in msg
        assert "band 8" not in msg
        assert "0.0.0" not in msg


@pytest.mark.usefixtures("_python313_driver_runtime")
class TestConnectHttpPortForwarding:
    """Database paths must forward http_port to their version probes."""

    def test_python_driver_gate_forwards_http_port(self, monkeypatch: pytest.MonkeyPatch):
        """http_port stored on Database is forwarded to server_version for direct driver access."""
        import type_bridge.typedb_driver as tdm
        from type_bridge.session import Database

        recorded: list[int] = []

        def _fake_server_version(address: str, *, http_port: int = 8000, tls: bool = False) -> str:
            recorded.append(http_port)
            # Return an out-of-window version so connect() aborts before driver construction.
            raise type_bridge_core.VersionError("gate test abort")

        monkeypatch.setattr(tdm, "driver_version", lambda: "3.10.0")
        monkeypatch.setattr(type_bridge_core, "server_version", lambda *a, **kw: "3.10.4")
        # Intercept server_version at the typedb_driver module level (session.py calls it via
        # the module reference, so patch there).
        monkeypatch.setattr(tdm, "server_version", _fake_server_version)

        db = Database(address="localhost:1729", database="test_db", http_port=9123)
        with pytest.raises(type_bridge_core.VersionError):
            _ = db.driver

        assert recorded == [9123], f"Expected http_port=9123 forwarded, got {recorded}"

    def test_database_embedded_gate_forwards_http_port(self, monkeypatch: pytest.MonkeyPatch):
        """http_port stored on Database is forwarded to PyRustDatabase.connect."""
        import type_bridge._rust_runtime as rust_mod
        import type_bridge.session as session_mod

        recorded: list[int] = []

        class _FakeRustDB:
            @staticmethod
            def connect(address, database, username, password, http_port):
                recorded.append(http_port)
                return _FakeRustDB()

        # Patch rust_core() so PyRustDatabase.connect records the port.
        fake_core = MagicMock()
        fake_core.PyRustDatabase = _FakeRustDB
        monkeypatch.setattr(rust_mod, "rust_core", lambda: fake_core)

        db = session_mod.Database(address="localhost:1729", database="test_db", http_port=9123)
        db.connect()

        assert recorded == [9123], f"Expected http_port=9123 forwarded to Rust, got {recorded}"

    def test_database_embedded_gate_forwards_server_version(self, monkeypatch: pytest.MonkeyPatch):
        """server_version stored on Database is forwarded to PyRustDatabase.connect."""
        import type_bridge._rust_runtime as rust_mod
        import type_bridge.session as session_mod

        recorded: list[tuple[int, str | None]] = []

        class _FakeRustDB:
            @staticmethod
            def connect(
                address,
                database,
                username,
                password,
                http_port,
                server_version=None,
            ):
                recorded.append((http_port, server_version))
                return _FakeRustDB()

        fake_core = MagicMock()
        fake_core.PyRustDatabase = _FakeRustDB
        monkeypatch.setattr(rust_mod, "rust_core", lambda: fake_core)

        db = session_mod.Database(
            address="localhost:1729",
            database="test_db",
            http_port=9123,
            server_version="3.10.4",
        )
        db.connect()

        assert recorded == [(9123, "3.10.4")]


class TestConnectProbeUnreachable:
    """Unreachable server probe propagates core VersionError out of connect (fail-closed)."""

    def test_probe_version_error_propagates(self, monkeypatch: pytest.MonkeyPatch):
        """VersionError from server_version propagates out of connect uncaught."""
        import type_bridge._rust_runtime as rust_mod
        from type_bridge.session import Database

        class _FailRustDB:
            @staticmethod
            def connect(*args: object, **kwargs: object) -> object:
                raise type_bridge_core.VersionError("probe unreachable")

        fake_core = MagicMock()
        fake_core.PyRustDatabase = _FailRustDB
        monkeypatch.setattr(rust_mod, "rust_core", lambda: fake_core)

        db = Database(address="localhost:1729", database="test_db")
        with pytest.raises(type_bridge_core.VersionError):
            db.connect()


class TestEnsureRuntimeSupported:
    """The embedded Rust runtime driver is gate-checked against the server."""

    def test_band7_server_passes(self):
        """Band-7 server (3.10.4) is accepted by the embedded gate (whole-window service)."""
        # Gate inversion: embedded runtime carries both bands, so band-7 servers pass.
        version.ensure_runtime_supported("3.10.4")

    def test_band8_server_passes(self):
        """Band-8 server (3.11.5) is accepted by the embedded gate."""
        version.ensure_runtime_supported("3.11.5")

    def test_below_window_raises_with_embedded_framing(self):
        """Server below the window (3.7.3) raises with wheel-appropriate framing."""
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            version.ensure_runtime_supported("3.7.3")
        msg = str(exc_info.value)
        # Message names the server and the supported window; no band tokens.
        assert "3.7.3" in msg
        assert "3.8" in msg
        assert "type-bridge release" in msg
        assert "band 7" not in msg
        assert "band 8" not in msg
        assert "0.0.0" not in msg

    def test_dual_band_server_passes(self):
        """Server 3.12.0 is serviceable; live connections normally negotiate band 9."""
        version.ensure_runtime_supported("3.12.0")

    def test_above_window_raises_with_embedded_framing(self):
        """Server above the window (3.13.0) raises with wheel-appropriate framing."""
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            version.ensure_runtime_supported("3.13.0")
        msg = str(exc_info.value)
        assert "3.13.0" in msg
        assert "3.8" in msg
        assert "band 7" not in msg
        assert "band 8" not in msg
        assert "0.0.0" not in msg

    def test_window_message_no_band_tokens(self):
        """Out-of-window embedded message must not expose raw band numbers."""
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            version.ensure_runtime_supported("3.7.3")
        msg = str(exc_info.value)
        assert "band 7" not in msg
        assert "band 8" not in msg
        assert "0.0.0" not in msg

    def test_embedded_driver_version_reads_core(self, monkeypatch: pytest.MonkeyPatch):
        """The wrapper delegates the embedded version read to core."""
        monkeypatch.setattr(type_bridge_core, "embedded_driver_version", lambda: "9.9.9")
        assert _typedb_driver_mod.embedded_driver_version() == "9.9.9"


class TestEmbeddedDriverVersions:
    """embedded_driver_versions() exposes both compiled-in pins."""

    def test_returns_dict(self):
        """embedded_driver_versions() must return a dict."""
        result = _typedb_driver_mod.embedded_driver_versions()
        assert isinstance(result, dict)

    def test_contains_both_bands(self):
        """Default build must include both band-7 and band-8 entries."""
        result = _typedb_driver_mod.embedded_driver_versions()
        assert 7 in result, f"band-7 missing from {result}"
        assert 8 in result, f"band-8 missing from {result}"

    def test_band8_matches_back_compat_single(self):
        """Band-8 entry must equal the back-compat embedded_driver_version()."""
        versions = _typedb_driver_mod.embedded_driver_versions()
        single = _typedb_driver_mod.embedded_driver_version()
        assert versions[8] == single

    def test_band7_is_3_8_line(self):
        """Band-7 pin must be on the 3.8.x server line."""
        versions = _typedb_driver_mod.embedded_driver_versions()
        pin = versions.get(7)
        if pin is None:
            return  # single-band8 build
        assert pin.startswith("3.8."), f"band-7 pin {pin!r} not on 3.8.x line"

    def test_delegates_to_core(self, monkeypatch: pytest.MonkeyPatch):
        """embedded_driver_versions() delegates to type_bridge_core."""
        fake = {7: "3.8.0", 8: "3.11.0"}
        monkeypatch.setattr(type_bridge_core, "embedded_driver_versions", lambda: fake)
        result = _typedb_driver_mod.embedded_driver_versions()
        assert result == fake


class TestConnectRuntimeGate:
    """connect() uses the embedded Rust runtime, not the external Python driver."""

    def test_band7_server_now_accepted(self, monkeypatch: pytest.MonkeyPatch):
        """Band-7 server is now ACCEPTED by the embedded gate (whole-window service)."""
        import type_bridge._rust_runtime as rust_mod
        import type_bridge.session as session_mod

        recorded: list[tuple[str, str, int]] = []

        class _FakeRustDB:
            @staticmethod
            def connect(address, database, username, password, http_port):
                recorded.append((address, database, http_port))
                return _FakeRustDB()

        fake_core = MagicMock()
        fake_core.PyRustDatabase = _FakeRustDB
        monkeypatch.setattr(rust_mod, "rust_core", lambda: fake_core)

        mock_typedb = MagicMock()
        monkeypatch.setattr(session_mod, "TypeDB", mock_typedb)

        db = session_mod.Database(address="localhost:1729", database="runtime_gate")
        db.connect()
        assert recorded == [("localhost:1729", "runtime_gate", 8000)]
        mock_typedb.driver.assert_not_called()

    def test_out_of_window_server_raises_before_driver(self, monkeypatch: pytest.MonkeyPatch):
        """Out-of-window server raises UnsupportedVersionError and TypeDB.driver is NOT called."""
        import type_bridge._rust_runtime as rust_mod
        import type_bridge.session as session_mod

        class _FailRustDB:
            @staticmethod
            def connect(*args: object, **kwargs: object) -> object:
                raise version.UnsupportedVersionError("server 3.7.3 is unsupported")

        fake_core = MagicMock()
        fake_core.PyRustDatabase = _FailRustDB
        monkeypatch.setattr(rust_mod, "rust_core", lambda: fake_core)

        driver_factory = MagicMock()
        monkeypatch.setattr(session_mod, "TypeDB", driver_factory)

        db = session_mod.Database(address="localhost:1729", database="runtime_gate")
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            db.connect()

        msg = str(exc_info.value)
        assert "3.7.3" in msg
        driver_factory.driver.assert_not_called()

    def test_runtime_gate_failure_message_no_band_tokens(self, monkeypatch: pytest.MonkeyPatch):
        """Runtime gate failure message must not expose raw band numbers."""
        import type_bridge._rust_runtime as rust_mod
        import type_bridge.session as session_mod

        class _FailRustDB:
            @staticmethod
            def connect(*args: object, **kwargs: object) -> object:
                raise version.UnsupportedVersionError("server 3.7.3 is unsupported")

        fake_core = MagicMock()
        fake_core.PyRustDatabase = _FailRustDB
        monkeypatch.setattr(rust_mod, "rust_core", lambda: fake_core)

        driver_factory = MagicMock()
        monkeypatch.setattr(session_mod, "TypeDB", driver_factory)

        db = session_mod.Database(address="localhost:1729", database="runtime_gate")
        with pytest.raises(version.UnsupportedVersionError) as exc_info:
            db.connect()

        msg = str(exc_info.value)
        assert "band 7" not in msg
        assert "band 8" not in msg
        assert "0.0.0" not in msg


class TestHttpPortBoundaries:
    """Range and SSOT pins for the HTTP probe port surface."""

    def test_default_http_port_matches_rust_ssot(self):
        """The re-exported Python default must equal the Rust core constant."""
        import type_bridge_core

        from type_bridge import typedb_driver

        assert typedb_driver.DEFAULT_HTTP_PORT == 8000
        assert typedb_driver.DEFAULT_HTTP_PORT == type_bridge_core.DEFAULT_HTTP_PORT

    def test_out_of_range_http_port_raises_before_io(self):
        """A port above u16::MAX fails at the PyO3 boundary, not mid-probe.

        The conversion error surfaces before any network I/O, so the test is
        deterministic and DB-free.
        """
        from type_bridge import typedb_driver

        with pytest.raises(OverflowError):
            typedb_driver.server_version("localhost:1729", http_port=70000)


class TestSchemaAnnotationGate:
    """The @doc/@meta schema-annotation feature gate (TypeDB 3.12+).

    `Database.detected_server_version()` surfaces the connect-time detected
    version; `check_schema_annotation_support()` rejects annotated DDL bound
    for a pre-3.12 server before it is sent; `SchemaManager.sync_schema`
    invokes that gate before opening the apply transaction.
    """

    def _database_with_rust_stub(self, monkeypatch, stub):
        import type_bridge._rust_runtime as rust_mod
        from type_bridge import session as session_mod

        monkeypatch.setattr(rust_mod, "rust_database_for", lambda _conn: stub)
        return session_mod.Database(address="localhost:1729", database="gate_unit")

    def test_detected_server_version_delegates_to_rust_handle(self, monkeypatch):
        stub = MagicMock()
        stub.server_version.return_value = "3.12.0"
        db = self._database_with_rust_stub(monkeypatch, stub)

        assert db.detected_server_version() == "3.12.0"

    def test_detected_server_version_unknown_returns_none(self, monkeypatch):
        """Band-7 gRPC fallback connections cannot report a server version."""
        stub = MagicMock()
        stub.server_version.return_value = None
        db = self._database_with_rust_stub(monkeypatch, stub)

        assert db.detected_server_version() is None

    def test_check_schema_annotation_support_propagates_versioned_error(self, monkeypatch):
        stub = MagicMock()
        stub.check_schema_annotation_support.side_effect = type_bridge_core.VersionError(
            "schema annotations (@doc/@meta) require TypeDB 3.12 or newer; detected server 3.11.5"
        )
        db = self._database_with_rust_stub(monkeypatch, stub)

        with pytest.raises(type_bridge_core.VersionError, match="3.12 or newer"):
            db.check_schema_annotation_support('define\nentity person @doc("A person.");')
        stub.check_schema_annotation_support.assert_called_once()

    def test_sync_schema_gates_annotations_before_apply(self, monkeypatch):
        """The gate must fire on the generated TypeQL before any transaction."""
        from type_bridge.migration.schema_manager import SchemaManager

        db = MagicMock()
        manager = SchemaManager(db)
        schema_text = 'define\nentity gate-person @doc("Gated.");'
        monkeypatch.setattr(manager, "generate_schema", lambda: schema_text)
        monkeypatch.setattr(manager, "has_existing_schema", lambda: False)

        manager.sync_schema()

        db.check_schema_annotation_support.assert_called_once_with(schema_text)
        gate_index = [name for name, *_ in db.mock_calls].index("check_schema_annotation_support")
        tx_index = [name for name, *_ in db.mock_calls].index("transaction")
        assert gate_index < tx_index

    def test_sync_schema_does_not_apply_when_gate_rejects(self, monkeypatch):
        from type_bridge.migration.schema_manager import SchemaManager

        db = MagicMock()
        db.check_schema_annotation_support.side_effect = type_bridge_core.VersionError(
            "schema annotations (@doc/@meta) require TypeDB 3.12 or newer"
        )
        manager = SchemaManager(db)
        monkeypatch.setattr(
            manager, "generate_schema", lambda: 'define\nentity gate-person @doc("Gated.");'
        )
        monkeypatch.setattr(manager, "has_existing_schema", lambda: False)

        with pytest.raises(type_bridge_core.VersionError):
            manager.sync_schema()
        db.transaction.assert_not_called()


class TestGivenStageSurface:
    """The given-stage parameterized-query surface (TypeDB 3.12+).

    `Database.supports_given_stage()` reports the feature check on the
    detected server version; `execute_with_rows()` runs one compiled pipeline
    over out-of-band input rows; `TransactionContext.execute_with_rows()`
    requires an open Rust transaction.
    """

    def _database_with_rust_stub(self, monkeypatch, stub):
        import type_bridge._rust_runtime as rust_mod
        from type_bridge import session as session_mod

        monkeypatch.setattr(rust_mod, "rust_database_for", lambda _conn: stub)
        return session_mod.Database(address="localhost:1729", database="given_unit")

    def test_supports_given_stage_delegates_to_rust_handle(self, monkeypatch):
        stub = MagicMock()
        stub.supports_given_stage.return_value = True
        db = self._database_with_rust_stub(monkeypatch, stub)

        assert db.supports_given_stage() is True
        stub.supports_given_stage.assert_called_once()

    def test_execute_with_rows_forwards_full_argument_contract(self, monkeypatch):
        stub = MagicMock()
        stub.execute_with_rows.return_value = [{"iid": "0x1e"}]
        db = self._database_with_rust_stub(monkeypatch, stub)

        query = "given $n: string; insert $p isa person, has name == $n;"
        result = db.execute_with_rows(query, "write", ["n"], ["string"], [["alice"]])

        assert result == [{"iid": "0x1e"}]
        stub.execute_with_rows.assert_called_once_with(
            query, "write", ["n"], ["string"], [["alice"]]
        )

    def test_execute_with_rows_propagates_versioned_error(self, monkeypatch):
        stub = MagicMock()
        stub.execute_with_rows.side_effect = type_bridge_core.VersionError(
            "given-stage parameterized queries require TypeDB 3.12 or newer; detected server 3.11.5"
        )
        db = self._database_with_rust_stub(monkeypatch, stub)

        with pytest.raises(type_bridge_core.VersionError, match="3.12 or newer"):
            db.execute_with_rows(
                "given $n: string; insert $p isa person, has name == $n;",
                "write",
                ["n"],
                ["string"],
                [["alice"]],
            )

    def test_transaction_context_execute_with_rows_requires_rust_transaction(self):
        from type_bridge import session as session_mod

        db = session_mod.Database(address="localhost:1729", database="given_unit")
        context = session_mod.TransactionContext(db, session_mod.TransactionType.WRITE)

        with pytest.raises(RuntimeError, match="Rust-backend transaction"):
            context.execute_with_rows(
                "given $n: string; insert $p isa person, has name == $n;",
                ["n"],
                ["string"],
                [["alice"]],
            )

    def test_transaction_context_execute_with_rows_forwards_to_rust_tx(self):
        from type_bridge import session as session_mod

        db = session_mod.Database(address="localhost:1729", database="given_unit")
        context = session_mod.TransactionContext(db, session_mod.TransactionType.WRITE)
        rust_tx = MagicMock()
        rust_tx.execute_with_rows.return_value = [{"iid": "0x1e"}]
        context._rust_tx = rust_tx

        query = "given $n: string; insert $p isa person, has name == $n;"
        result = context.execute_with_rows(query, ["n"], ["string"], [["alice"]])

        assert result == [{"iid": "0x1e"}]
        rust_tx.execute_with_rows.assert_called_once_with(query, ["n"], ["string"], [["alice"]])
