"""Cross-path tests for filterable TypeDB server deprecation notices."""

from __future__ import annotations

import warnings
from pathlib import Path
from unittest.mock import MagicMock

import pytest
import type_bridge_core

import type_bridge._rust_runtime as rust_runtime
import type_bridge.session as session

ROOT = Path(__file__).resolve().parents[3]


class _RustDatabase:
    def __init__(self, notice: str | None) -> None:
        self.notice = notice
        self.closed = False

    def server_deprecation_notice(self) -> str | None:
        return self.notice

    def close(self) -> None:
        self.closed = True


def test_warning_code_is_the_native_rust_ssot_export() -> None:
    assert (
        session.TypeDBServerDeprecationWarning.code
        == type_bridge_core.TYPEDB_LEGACY_SERVER_DEPRECATION_CODE
    )


def test_deprecation_guide_freezes_behavior_not_a_v1_engine_and_nonfatal_notices() -> None:
    guide = (ROOT / "docs/guide/v2-deprecations.md").read_text(encoding="utf-8")
    upgrade = (ROOT / "docs/guide/upgrade-v2.md").read_text(encoding="utf-8")

    assert "keeps its released behavior and public contract" in guide
    assert "keeps its released engine" not in guide
    assert "deprecated V1 facades keep their released behavior and public contracts" in upgrade
    assert "deprecated V1 facades keep their released engines" not in upgrade
    assert "Rust `MatchRequest` execution may instead delegate" in upgrade
    assert (
        "do not change\nconnection, query, or teardown behavior under any warning policy" in guide
    )
    assert "`--throw-deprecation` suppresses this notice" in guide
    assert "synchronous replacement of Node's `process.emitWarning` that throws" in guide
    assert "application's asynchronous `warning` event listener remains\napplication-owned" in guide
    assert "throwing custom Node warning hook" not in guide


@pytest.mark.parametrize(
    ("server_version", "expected_fragment"),
    [
        ("3.8.3", "TypeDB 3.8.3"),
        ("3.10.4", "TypeDB 3.10.4"),
        ("3.11.5", None),
        ("3.12.1", None),
        (None, "exact server version"),
    ],
)
def test_native_notice_matrix(
    server_version: str | None,
    expected_fragment: str | None,
) -> None:
    notice = type_bridge_core.typedb_server_deprecation_notice(server_version)
    if expected_fragment is None:
        assert notice is None
    else:
        assert notice is not None
        assert expected_fragment in notice
        assert "band 7" not in notice
        assert "band 8" not in notice
        assert "band 9" not in notice


def test_rust_unknown_band7_connection_warns_once_and_cached_access_does_not_repeat(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    native_database = _RustDatabase(type_bridge_core.typedb_server_deprecation_notice(None))
    core = MagicMock()
    core.PyRustDatabase.connect.return_value = native_database
    monkeypatch.setattr(rust_runtime, "rust_core", lambda: core)
    database = session.Database(server_version=None)

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert rust_runtime.rust_database_for(database) is native_database
        assert rust_runtime.rust_database_for(database) is native_database

    assert len(caught) == 1
    assert caught[0].category is session.TypeDBServerDeprecationWarning
    assert str(caught[0].message) == type_bridge_core.typedb_server_deprecation_notice(None)
    assert (
        session.TypeDBServerDeprecationWarning.code
        == type_bridge_core.TYPEDB_LEGACY_SERVER_DEPRECATION_CODE
    )


@pytest.mark.parametrize(
    ("case", "server_version"),
    [
        pytest.param("unknown-band8", None, id="unknown-band8"),
        pytest.param("unknown-band9", None, id="unknown-band9"),
        pytest.param("known-3.12-over-band8", "3.12.1", id="known-3.12-over-band8"),
    ],
)
def test_rust_current_band_classification_does_not_warn(
    monkeypatch: pytest.MonkeyPatch,
    case: str,
    server_version: str | None,
) -> None:
    native_database = _RustDatabase(None)
    core = MagicMock()
    core.PyRustDatabase.connect.return_value = native_database
    monkeypatch.setattr(rust_runtime, "rust_core", lambda: core)
    database = session.Database(server_version=server_version)

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        rust_runtime.rust_database_for(database)

    assert caught == [], case


@pytest.mark.parametrize(
    ("driver_version", "server_version"),
    [("3.8.1", "3.8.3"), ("3.10.0", "3.10.4")],
)
def test_installed_driver_warns_after_success_and_keeps_connection(
    monkeypatch: pytest.MonkeyPatch,
    driver_version: str,
    server_version: str,
) -> None:
    created_driver = MagicMock()
    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: driver_version)
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _: None,
    )
    monkeypatch.setattr(session.version, "ensure_supported", lambda *_: None)
    monkeypatch.setattr(session.version, "ensure_runtime_supported", lambda *_: None)
    monkeypatch.setattr(session, "create_driver_options", lambda **_: object())
    monkeypatch.setattr(session.TypeDB, "driver", lambda *_: created_driver)
    database = session.Database(server_version=server_version)

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert database.driver is created_driver
        assert database.driver is created_driver

    assert len(caught) == 1
    assert caught[0].category is session.TypeDBServerDeprecationWarning
    assert str(caught[0].message) == type_bridge_core.typedb_server_deprecation_notice(
        server_version
    )
    assert Path(caught[0].filename).resolve() == Path(__file__).resolve()
    created_driver.close.assert_not_called()


@pytest.mark.parametrize(
    ("driver_version", "server_version"),
    [("3.11.5", "3.11.5"), ("3.12.1", "3.12.1")],
)
def test_supported_installed_driver_does_not_warn(
    monkeypatch: pytest.MonkeyPatch,
    driver_version: str,
    server_version: str,
) -> None:
    created_driver = MagicMock()
    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: driver_version)
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _: None,
    )
    monkeypatch.setattr(session.version, "ensure_supported", lambda *_: None)
    monkeypatch.setattr(session.version, "ensure_runtime_supported", lambda *_: None)
    monkeypatch.setattr(session, "create_driver_options", lambda **_: object())
    monkeypatch.setattr(session.TypeDB, "driver", lambda *_: created_driver)
    database = session.Database(server_version=server_version)

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert database.driver is created_driver

    assert caught == []


def test_promoted_warning_cannot_abort_successful_connection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created_driver = MagicMock()
    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.10.0")
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _: None,
    )
    monkeypatch.setattr(session.version, "ensure_supported", lambda *_: None)
    monkeypatch.setattr(session.version, "ensure_runtime_supported", lambda *_: None)
    monkeypatch.setattr(session, "create_driver_options", lambda **_: object())
    monkeypatch.setattr(session.TypeDB, "driver", lambda *_: created_driver)
    monkeypatch.setattr(
        session,
        "_known_server_deprecation_notice",
        lambda _: "shared legacy notice",
    )
    database = session.Database(server_version="3.10.4")

    with warnings.catch_warnings():
        warnings.simplefilter("error", session.TypeDBServerDeprecationWarning)
        assert database.driver is created_driver

    created_driver.close.assert_not_called()
    assert database._driver is created_driver


def test_installed_driver_notice_failure_preserves_successful_connection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created_driver = MagicMock()
    notice_failure = RuntimeError("notice lookup failed")
    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.10.0")
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _: None,
    )
    monkeypatch.setattr(session.version, "ensure_supported", lambda *_: None)
    monkeypatch.setattr(session.version, "ensure_runtime_supported", lambda *_: None)
    monkeypatch.setattr(session, "create_driver_options", lambda **_: object())
    monkeypatch.setattr(session.TypeDB, "driver", lambda *_: created_driver)
    monkeypatch.setattr(
        session,
        "_known_server_deprecation_notice",
        lambda _: (_ for _ in ()).throw(notice_failure),
    )
    database = session.Database(server_version="3.10.4")

    assert database.driver is created_driver

    created_driver.close.assert_not_called()
    assert database._driver is created_driver


def test_rust_notice_failure_preserves_successful_connection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    notice_failure = RuntimeError("notice lookup failed")
    native_database = _RustDatabase(None)

    def fail_notice() -> str | None:
        raise notice_failure

    native_database.server_deprecation_notice = fail_notice  # type: ignore[method-assign]
    core = MagicMock()
    core.PyRustDatabase.connect.return_value = native_database
    monkeypatch.setattr(rust_runtime, "rust_core", lambda: core)
    database = session.Database(server_version="3.10.4")

    assert rust_runtime.rust_database_for(database) is native_database

    assert not native_database.closed
    assert getattr(database, "_rust_backend_database", None) is native_database


def test_installed_driver_base_exception_discards_prepared_transport(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    created_driver = MagicMock()
    notice_failure = KeyboardInterrupt("notice lookup interrupted")
    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.10.0")
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _: None,
    )
    monkeypatch.setattr(session.version, "ensure_supported", lambda *_: None)
    monkeypatch.setattr(session.version, "ensure_runtime_supported", lambda *_: None)
    monkeypatch.setattr(session, "create_driver_options", lambda **_: object())
    monkeypatch.setattr(session.TypeDB, "driver", lambda *_: created_driver)
    monkeypatch.setattr(
        session,
        "_known_server_deprecation_notice",
        lambda _: (_ for _ in ()).throw(notice_failure),
    )
    database = session.Database(server_version="3.10.4")
    database._prepared_connection_options()
    snapshot = MagicMock()
    database._tls_root_ca_snapshot = snapshot
    database._tls_root_ca_snapshot_path = "retained-snapshot"

    with pytest.raises(KeyboardInterrupt) as raised:
        _ = database.driver

    assert raised.value is notice_failure
    created_driver.close.assert_called_once()
    snapshot.cleanup.assert_called_once()
    assert database._prepared_connection is None
    assert database._tls_root_ca_snapshot is None
    assert database._tls_root_ca_snapshot_path is None


def test_rust_base_exception_discards_prepared_transport(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    notice_failure = KeyboardInterrupt("notice lookup interrupted")
    native_database = _RustDatabase(None)

    def fail_notice() -> str | None:
        raise notice_failure

    native_database.server_deprecation_notice = fail_notice  # type: ignore[method-assign]
    core = MagicMock()
    core.PyRustDatabase.connect.return_value = native_database
    monkeypatch.setattr(rust_runtime, "rust_core", lambda: core)
    database = session.Database(server_version="3.10.4")
    database._prepared_connection_options()
    snapshot = MagicMock()
    database._tls_root_ca_snapshot = snapshot
    database._tls_root_ca_snapshot_path = "retained-snapshot"

    with pytest.raises(KeyboardInterrupt) as raised:
        rust_runtime.rust_database_for(database)

    assert raised.value is notice_failure
    assert native_database.closed
    snapshot.cleanup.assert_called_once()
    assert database._prepared_connection is None
    assert database._tls_root_ca_snapshot is None
    assert database._tls_root_ca_snapshot_path is None
