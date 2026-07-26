"""Compatibility and fail-closed tests for the Python TLS surface."""

from __future__ import annotations

import os
import pickle
import threading
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock

import pytest
import type_bridge_core

import type_bridge.typedb_driver as typedb_driver
from type_bridge.session import Database

VALID_ROOT_CA = (
    Path(__file__).parents[3] / "type-bridge-core/crates/core/tests/fixtures/valid-root.pem"
).read_bytes()


class _CurrentRustDatabaseHandle:
    """Minimal successful native handle used by transport-focused test doubles."""

    def server_deprecation_notice(self) -> None:
        return None

    def close(self) -> None:
        pass


@pytest.mark.parametrize(
    ("address", "tls", "expected_address", "expected_tls"),
    [
        ("localhost:1729", None, "localhost:1729", None),
        ("https://localhost:1729", None, "localhost:1729", True),
        ("https://localhost:1729", False, "localhost:1729", False),
        ("localhost:1729", True, "localhost:1729", True),
        ("http://localhost:1729", None, "localhost:1729", None),
    ],
)
def test_rust_database_preserves_exact_https_inference_and_normalizes_address(
    monkeypatch: pytest.MonkeyPatch,
    address: str,
    tls: bool | None,
    expected_address: str,
    expected_tls: bool | None,
) -> None:
    import type_bridge._rust_runtime as rust_runtime

    calls: list[tuple[str, dict[str, object]]] = []

    class FakeRustDatabase:
        @staticmethod
        def connect(
            actual_address: str,
            database: str,
            username: str,
            password: str,
            **kwargs: object,
        ) -> object:
            del database, username, password
            calls.append((actual_address, kwargs))
            return _CurrentRustDatabaseHandle()

    monkeypatch.setattr(
        rust_runtime,
        "rust_core",
        lambda: SimpleNamespace(PyRustDatabase=FakeRustDatabase),
    )

    db = Database(address=address, database="tls_unit", tls=tls)
    rust_runtime.rust_database_for(db)

    assert calls[0][0] == expected_address
    assert calls[0][1].get("tls") is expected_tls


def test_mixed_case_https_never_downgrades_to_a_plaintext_host(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import type_bridge._rust_runtime as rust_runtime
    import type_bridge.session as session

    rust_host = MagicMock()
    monkeypatch.setattr(
        rust_runtime,
        "rust_core",
        lambda: SimpleNamespace(PyRustDatabase=rust_host),
    )
    database = Database(address="HTTPS://db.example:1729", database="tls_unit")
    with pytest.raises(ValueError) as rust_error:
        rust_runtime.rust_database_for(database)
    assert str(rust_error.value) == (
        "mixed-case HTTPS scheme does not enable TLS; use lowercase https:// "
        "or pass tls=True explicitly"
    )
    rust_host.connect.assert_not_called()

    python_host = MagicMock()
    monkeypatch.setattr(session, "TypeDB", python_host)
    database = Database(address="HTTPS://db.example:1729", database="tls_unit")
    with pytest.raises(ValueError) as python_error:
        _ = database.driver
    assert str(python_error.value) == str(rust_error.value)
    python_host.driver.assert_not_called()


def test_custom_root_is_forwarded_only_with_explicit_tls(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    import type_bridge._rust_runtime as rust_runtime

    calls: list[dict[str, object]] = []

    class FakeRustDatabase:
        @staticmethod
        def connect(*args: object, **kwargs: object) -> object:
            del args
            calls.append(kwargs)
            return _CurrentRustDatabaseHandle()

    monkeypatch.setattr(
        rust_runtime,
        "rust_core",
        lambda: SimpleNamespace(PyRustDatabase=FakeRustDatabase),
    )
    root = tmp_path / "root.pem"
    root.write_bytes(VALID_ROOT_CA)
    db = Database(tls=True, tls_root_ca=root)

    rust_runtime.rust_database_for(db)

    assert len(calls) == 1
    assert calls[0]["http_port"] == typedb_driver.DEFAULT_HTTP_PORT
    assert calls[0]["tls"] is True
    snapshot_path = Path(str(calls[0]["tls_root_ca"]))
    assert snapshot_path != root
    assert snapshot_path.read_bytes() == VALID_ROOT_CA

    db.close()
    assert not snapshot_path.exists()


@pytest.mark.parametrize("tls", [None, False])
def test_root_without_explicit_tls_rejects_before_native_connect(tls: bool | None) -> None:
    with pytest.raises(ValueError, match="tls_root_ca"):
        Database(tls=tls, tls_root_ca="root.pem")


def test_invalid_tls_switch_rejects_before_native_connect() -> None:
    with pytest.raises(TypeError, match="tls must be True, False, or None"):
        Database(tls="true")  # type: ignore[arg-type]

    with pytest.raises(ValueError, match="must not be empty"):
        Database(tls=True, tls_root_ca="")
    with pytest.raises(TypeError, match="string or path-like"):
        Database(tls=True, tls_root_ca=object())  # type: ignore[arg-type]


def test_partially_initialized_database_destructor_is_safe() -> None:
    database = Database.__new__(Database)

    # Python can finalize an instance after __init__ raises before assigning
    # any attributes. The destructor must remain a no-op in that state.
    database.__del__()


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="POSIX FIFO contract")
def test_python_custom_root_fifo_rejects_without_blocking(tmp_path: Path) -> None:
    import type_bridge.session as session

    fifo = tmp_path / "root.pem"
    os.mkfifo(fifo)
    outcome: list[BaseException | None] = []

    def capture() -> None:
        try:
            session._snapshot_tls_root_ca(str(fifo))
        except BaseException as error:  # noqa: BLE001 - assertion captures the public failure
            outcome.append(error)
        else:
            outcome.append(None)

    worker = threading.Thread(target=capture, daemon=True)
    worker.start()
    worker.join(timeout=2)
    assert not worker.is_alive(), "opening a FIFO must not block the Python caller"
    assert len(outcome) == 1
    assert isinstance(outcome[0], ValueError)
    assert "tls_custom_root_ca_not_file" in str(outcome[0])


@pytest.mark.skipif(os.name != "posix", reason="POSIX symlink contract")
def test_python_custom_root_follows_caller_alias_once(tmp_path: Path) -> None:
    import type_bridge.session as session

    original = tmp_path / "original.pem"
    replacement = tmp_path / "replacement.pem"
    alias = tmp_path / "configured.pem"
    original.write_bytes(VALID_ROOT_CA)
    replacement.write_bytes(b"replacement is not a certificate")
    alias.symlink_to(original)

    snapshot, snapshot_path_text = session._snapshot_tls_root_ca(str(alias))
    snapshot_path = Path(snapshot_path_text)
    try:
        alias.unlink()
        alias.symlink_to(replacement)
        original.write_bytes(b"overwritten original is not a certificate")

        assert snapshot_path.read_bytes() == VALID_ROOT_CA
    finally:
        snapshot.cleanup()


def test_rust_snapshot_rewinds_before_each_driver_path_borrow(tmp_path: Path) -> None:
    root = tmp_path / "root.pem"
    root.write_bytes(VALID_ROOT_CA)
    snapshot = type_bridge_core._CustomRootCaSnapshot(root)
    try:
        first_path = Path(os.fspath(snapshot.path))
        assert first_path.read_bytes() == VALID_ROOT_CA
        second_path = Path(os.fspath(snapshot.path))
        assert second_path.read_bytes() == VALID_ROOT_CA
    finally:
        snapshot.cleanup()


def test_malformed_root_rejects_before_python_native_host_with_version_override(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    import type_bridge.session as session

    invalid = tmp_path / "invalid.pem"
    invalid.write_bytes(b"not a certificate")
    options = MagicMock()
    host = MagicMock()
    monkeypatch.setattr(session, "create_driver_options", options)
    monkeypatch.setattr(session, "TypeDB", host)

    database = Database(
        database="tls_unit",
        server_version="3.12.0",
        tls=True,
        tls_root_ca=invalid,
    )
    with pytest.raises(ValueError, match="tls_custom_root_ca_invalid_pem"):
        _ = database.driver

    options.assert_not_called()
    host.driver.assert_not_called()


def test_low_level_python_tls_configuration_errors_are_typed(tmp_path: Path) -> None:
    missing = tmp_path / "missing-root.pem"
    with pytest.raises(ValueError, match="requires explicit tls=True"):
        type_bridge_core.PyRustDatabase.connect(
            "127.0.0.1:1",
            "tls_unit",
            server_version="3.12.0",
            tls_root_ca=str(missing),
        )
    with pytest.raises(TypeError, match="tls must be True, False, or None"):
        type_bridge_core.PyRustDatabase.connect(
            "127.0.0.1:1",
            "tls_unit",
            server_version="3.12.0",
            tls=1,  # type: ignore[arg-type]
        )
    with pytest.raises(ValueError, match="tls_custom_root_ca_unreadable"):
        type_bridge_core.PyRustDatabase.connect(
            "127.0.0.1:1",
            "tls_unit",
            server_version="3.12.0",
            tls=True,
            tls_root_ca=str(missing),
        )


def test_driver_options_lower_custom_roots_for_each_driver_api(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    options = MagicMock()
    monkeypatch.setattr(typedb_driver, "DriverOptions", options)
    monkeypatch.setattr(typedb_driver, "driver_version", lambda: "3.10.0")

    typedb_driver.create_driver_options(True, tls_root_ca="root.pem")
    options.assert_called_once_with(is_tls_enabled=True, tls_root_ca_path="root.pem")

    options.reset_mock()
    custom_tls = object()
    tls_config = MagicMock()
    tls_config.enabled_with_root_ca.return_value = custom_tls
    monkeypatch.setattr(typedb_driver, "driver_version", lambda: "3.11.5")
    monkeypatch.setattr(typedb_driver, "_load_tls_config", lambda: tls_config)

    typedb_driver.create_driver_options(True, tls_root_ca="root.pem")
    tls_config.enabled_with_root_ca.assert_called_once_with("root.pem")
    options.assert_called_once_with(custom_tls)


def test_custom_root_version_probe_uses_typed_native_path(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    probe = MagicMock(return_value="3.12.0")
    monkeypatch.setattr(typedb_driver._core, "server_version", probe)

    assert (
        typedb_driver.server_version(
            "db.example.com:1729",
            http_port=8443,
            tls=True,
            tls_root_ca="root.pem",
        )
        == "3.12.0"
    )
    probe.assert_called_once_with("db.example.com:1729", 8443, True, "root.pem")


def test_custom_root_never_enables_version_probe_tls_implicitly(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    probe = MagicMock()
    monkeypatch.setattr(typedb_driver._core, "server_version", probe)

    with pytest.raises(ValueError, match="requires explicit tls=True"):
        typedb_driver.server_version("db.example.com:1729", tls_root_ca="root.pem")
    probe.assert_not_called()


@pytest.mark.parametrize(
    ("address", "tls", "expected_tls"),
    [
        ("https://db.example.com:1729", None, True),
        ("https://db.example.com:1729", False, False),
    ],
)
def test_direct_python_driver_uses_the_same_exact_inference(
    monkeypatch: pytest.MonkeyPatch,
    address: str,
    tls: bool | None,
    expected_tls: bool,
) -> None:
    import type_bridge.session as session

    option_calls: list[dict[str, object]] = []
    driver_calls: list[str] = []

    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.12.0")
    monkeypatch.setattr(
        session,
        "create_driver_options",
        lambda **kwargs: option_calls.append(kwargs) or object(),
    )
    monkeypatch.setattr(session, "Credentials", lambda username, password: (username, password))
    fake_type_db = MagicMock()

    def fake_driver(actual_address: str, credentials: object, options: object) -> MagicMock:
        del credentials, options
        driver_calls.append(actual_address)
        return MagicMock()

    fake_type_db.driver.side_effect = fake_driver
    monkeypatch.setattr(session, "TypeDB", fake_type_db)

    db = Database(address=address, database="tls_unit", server_version="3.12.0", tls=tls)
    _ = db.driver

    assert driver_calls == ["db.example.com:1729"]
    assert option_calls == [{"is_tls_enabled": expected_tls}]


def test_direct_python_driver_pins_one_custom_root_snapshot(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    import type_bridge.session as session

    configured = tmp_path / "configured-root.pem"
    configured.write_bytes(VALID_ROOT_CA)
    observed: list[tuple[str, Path, bytes]] = []

    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.12.0")
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _installed: 9,
    )

    def probe(
        _address: str,
        *,
        http_port: int,
        tls: bool,
        tls_root_ca: str,
    ) -> str:
        del http_port, tls
        snapshot = Path(tls_root_ca)
        observed.append(("probe", snapshot, snapshot.read_bytes()))
        configured.write_bytes(b"replacement is not a certificate")
        return "3.12.0"

    def options(*, is_tls_enabled: bool, tls_root_ca: str) -> object:
        assert is_tls_enabled
        snapshot = Path(tls_root_ca)
        observed.append(("driver", snapshot, snapshot.read_bytes()))
        return object()

    monkeypatch.setattr(session.typedb_driver, "server_version", probe)
    monkeypatch.setattr(session, "create_driver_options", options)
    monkeypatch.setattr(session, "Credentials", lambda username, password: (username, password))
    fake_type_db = MagicMock()
    fake_type_db.driver.return_value = MagicMock()
    monkeypatch.setattr(session, "TypeDB", fake_type_db)

    database = Database(
        address="localhost:1729",
        database="tls_unit",
        tls=True,
        tls_root_ca=configured,
    )
    _ = database.driver

    assert [entry[0] for entry in observed] == ["probe", "driver"]
    assert observed[0][1] == observed[1][1]
    assert observed[0][1] != configured
    assert observed[0][2] == observed[1][2] == VALID_ROOT_CA
    snapshot_path = observed[0][1]
    assert snapshot_path.exists()

    database.close()
    assert not snapshot_path.exists()


def test_rust_connect_and_lazy_python_driver_share_one_custom_root_snapshot(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    import type_bridge._rust_runtime as rust_runtime
    import type_bridge.session as session

    configured = tmp_path / "configured-root.pem"
    configured.write_bytes(VALID_ROOT_CA)
    created_snapshots: list[tuple[str, Path]] = []
    real_snapshot = session._snapshot_tls_root_ca

    def snapshot_once(path: str) -> tuple[object, str]:
        snapshot, snapshot_path = real_snapshot(path)
        created_snapshots.append((path, Path(snapshot_path)))
        return snapshot, snapshot_path

    monkeypatch.setattr(session, "_snapshot_tls_root_ca", snapshot_once)

    rust_observed: list[tuple[Path, bytes]] = []

    class FakeRustDatabase:
        @staticmethod
        def connect(*args: object, **kwargs: object) -> object:
            del args
            snapshot_path = Path(str(kwargs["tls_root_ca"]))
            rust_observed.append((snapshot_path, snapshot_path.read_bytes()))
            return _CurrentRustDatabaseHandle()

    monkeypatch.setattr(
        rust_runtime,
        "rust_core",
        lambda: SimpleNamespace(PyRustDatabase=FakeRustDatabase),
    )
    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.12.0")
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _installed: 9,
    )

    driver_observed: list[tuple[Path, bytes]] = []

    def options(*, is_tls_enabled: bool, tls_root_ca: str) -> object:
        assert is_tls_enabled
        snapshot_path = Path(tls_root_ca)
        driver_observed.append((snapshot_path, snapshot_path.read_bytes()))
        return object()

    monkeypatch.setattr(session, "create_driver_options", options)
    monkeypatch.setattr(session, "Credentials", lambda username, password: (username, password))
    fake_type_db = MagicMock()
    fake_type_db.driver.return_value = MagicMock()
    monkeypatch.setattr(session, "TypeDB", fake_type_db)

    database = Database(
        address="localhost:1729",
        database="tls_unit",
        server_version="3.12.0",
        tls=True,
        tls_root_ca=configured,
    )
    rust_runtime.rust_database_for(database)
    configured.write_bytes(b"replacement is not a certificate")
    _ = database.driver

    assert len(created_snapshots) == 1
    assert created_snapshots[0][0] == str(configured)
    assert rust_observed == driver_observed
    assert rust_observed[0][0] == created_snapshots[0][1]
    assert rust_observed[0][1] == VALID_ROOT_CA
    snapshot_path = rust_observed[0][0]

    database.close()
    assert not snapshot_path.exists()


def test_python_driver_and_later_rust_connect_share_one_custom_root_snapshot(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    import type_bridge._rust_runtime as rust_runtime
    import type_bridge.session as session

    configured = tmp_path / "configured-root.pem"
    configured.write_bytes(VALID_ROOT_CA)
    python_observed: list[bytes] = []
    rust_observed: list[bytes] = []

    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.12.0")
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _installed: 9,
    )

    def options(*, is_tls_enabled: bool, tls_root_ca: str) -> object:
        assert is_tls_enabled
        python_observed.append(Path(tls_root_ca).read_bytes())
        return object()

    monkeypatch.setattr(session, "create_driver_options", options)
    monkeypatch.setattr(session, "Credentials", lambda username, password: (username, password))
    fake_type_db = MagicMock()
    fake_type_db.driver.return_value = MagicMock()
    monkeypatch.setattr(session, "TypeDB", fake_type_db)

    class FakeRustDatabase:
        @staticmethod
        def connect(*args: object, **kwargs: object) -> object:
            del args
            rust_observed.append(Path(str(kwargs["tls_root_ca"])).read_bytes())
            return _CurrentRustDatabaseHandle()

    monkeypatch.setattr(
        rust_runtime,
        "rust_core",
        lambda: SimpleNamespace(PyRustDatabase=FakeRustDatabase),
    )

    database = Database(
        database="tls_unit",
        server_version="3.12.0",
        tls=True,
        tls_root_ca=configured,
    )
    _ = database.driver
    configured.write_bytes(b"replacement is not a certificate")
    rust_runtime.rust_database_for(database)

    assert python_observed == rust_observed == [VALID_ROOT_CA]
    database.close()


@pytest.mark.parametrize(
    "mutation",
    [
        lambda database, tmp_path: setattr(database, "tls_root_ca", None),
        lambda database, tmp_path: setattr(database, "tls_root_ca", tmp_path / "other.pem"),
        lambda database, tmp_path: setattr(database, "tls", False),
        lambda database, tmp_path: setattr(database, "address", "other.example:1729"),
        lambda database, tmp_path: setattr(database, "username", "other-user"),
    ],
)
def test_second_backend_rejects_connection_identity_mutation(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    mutation: object,
) -> None:
    import type_bridge._rust_runtime as rust_runtime
    import type_bridge.session as session

    root = tmp_path / "root.pem"
    root.write_bytes(VALID_ROOT_CA)

    class FakeRustDatabase:
        @staticmethod
        def connect(*args: object, **kwargs: object) -> object:
            del args
            Path(str(kwargs["tls_root_ca"])).read_bytes()
            return _CurrentRustDatabaseHandle()

    monkeypatch.setattr(
        rust_runtime,
        "rust_core",
        lambda: SimpleNamespace(PyRustDatabase=FakeRustDatabase),
    )
    native_host = MagicMock()
    monkeypatch.setattr(session, "TypeDB", native_host)

    database = Database(
        database="tls_unit",
        username="admin",
        password="password",
        server_version="3.12.0",
        tls=True,
        tls_root_ca=root,
    )
    rust_runtime.rust_database_for(database)
    mutation(database, tmp_path)  # type: ignore[operator]

    with pytest.raises(
        ValueError,
        match="connection settings changed|tls_root_ca contradicts explicit tls=False",
    ):
        _ = database.driver
    native_host.driver.assert_not_called()
    database.close()


def test_failed_first_connect_discards_snapshot_for_corrected_retry(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    import type_bridge.session as session

    configured = tmp_path / "root.pem"
    configured.write_bytes(VALID_ROOT_CA)
    observed: list[bytes] = []

    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.12.0")
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _installed: 9,
    )

    def options(*, is_tls_enabled: bool, tls_root_ca: str) -> object:
        assert is_tls_enabled
        observed.append(Path(tls_root_ca).read_bytes())
        return object()

    monkeypatch.setattr(session, "create_driver_options", options)
    monkeypatch.setattr(session, "Credentials", lambda username, password: (username, password))
    fake_type_db = MagicMock()
    fake_type_db.driver.side_effect = [RuntimeError("first host failed"), MagicMock()]
    monkeypatch.setattr(session, "TypeDB", fake_type_db)

    database = Database(
        database="tls_unit",
        server_version="3.12.0",
        tls=True,
        tls_root_ca=configured,
    )
    with pytest.raises(RuntimeError, match="first host failed"):
        _ = database.driver
    assert database._prepared_connection is None

    corrected = VALID_ROOT_CA + b"\n"
    configured.write_bytes(corrected)
    _ = database.driver
    assert observed == [VALID_ROOT_CA, corrected]
    database.close()


def test_close_waits_for_inflight_rust_connect_and_then_cleans_snapshot(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    import type_bridge._rust_runtime as rust_runtime

    root = tmp_path / "root.pem"
    root.write_bytes(VALID_ROOT_CA)
    connect_started = threading.Event()
    release_connect = threading.Event()
    close_finished = threading.Event()
    failures: list[BaseException] = []

    class FakeRustDatabase:
        @staticmethod
        def connect(*args: object, **kwargs: object) -> object:
            del args
            assert Path(str(kwargs["tls_root_ca"])).read_bytes() == VALID_ROOT_CA
            connect_started.set()
            assert release_connect.wait(timeout=2)
            return _CurrentRustDatabaseHandle()

    monkeypatch.setattr(
        rust_runtime,
        "rust_core",
        lambda: SimpleNamespace(PyRustDatabase=FakeRustDatabase),
    )
    database = Database(
        database="tls_unit",
        server_version="3.12.0",
        tls=True,
        tls_root_ca=root,
    )

    def connect() -> None:
        try:
            rust_runtime.rust_database_for(database)
        except BaseException as error:  # noqa: BLE001 - thread assertion transport
            failures.append(error)

    connector = threading.Thread(target=connect)
    connector.start()
    assert connect_started.wait(timeout=2)

    closer = threading.Thread(target=lambda: (database.close(), close_finished.set()))
    closer.start()
    assert not close_finished.wait(timeout=0.1)
    release_connect.set()
    connector.join(timeout=2)
    closer.join(timeout=2)

    assert not failures
    assert close_finished.is_set()
    assert not hasattr(database, "_rust_backend_database")
    assert database._tls_root_ca_snapshot is None


def test_close_waits_for_inflight_python_host_and_then_closes_it(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    import type_bridge.session as session

    root = tmp_path / "root.pem"
    root.write_bytes(VALID_ROOT_CA)
    host_started = threading.Event()
    release_host = threading.Event()
    close_finished = threading.Event()
    failures: list[BaseException] = []
    driver = MagicMock()

    monkeypatch.setattr(session.typedb_driver, "driver_version", lambda: "3.12.0")
    monkeypatch.setattr(
        session.typedb_driver,
        "_ensure_driver_interpreter_supported",
        lambda _installed: 9,
    )
    monkeypatch.setattr(
        session,
        "create_driver_options",
        lambda *, is_tls_enabled, tls_root_ca: (
            Path(tls_root_ca).read_bytes(),
            is_tls_enabled,
        ),
    )
    monkeypatch.setattr(session, "Credentials", lambda username, password: (username, password))
    fake_type_db = MagicMock()

    def create_host(*args: object) -> object:
        del args
        host_started.set()
        assert release_host.wait(timeout=2)
        return driver

    fake_type_db.driver.side_effect = create_host
    monkeypatch.setattr(session, "TypeDB", fake_type_db)
    database = Database(
        database="tls_unit",
        server_version="3.12.0",
        tls=True,
        tls_root_ca=root,
    )

    def connect() -> None:
        try:
            _ = database.driver
        except BaseException as error:  # noqa: BLE001 - thread assertion transport
            failures.append(error)

    connector = threading.Thread(target=connect)
    connector.start()
    assert host_started.wait(timeout=2)
    closer = threading.Thread(target=lambda: (database.close(), close_finished.set()))
    closer.start()
    assert not close_finished.wait(timeout=0.1)
    release_host.set()
    connector.join(timeout=2)
    closer.join(timeout=2)

    assert not failures
    assert close_finished.is_set()
    driver.close.assert_called_once_with()
    assert database._driver is None
    assert database._tls_root_ca_snapshot is None


def test_pristine_database_pickle_and_legacy_state_restore_tls_defaults() -> None:
    database = Database(address="db.example:1729", database="pickle_unit")
    restored = pickle.loads(pickle.dumps(database))
    assert restored.address == "db.example:1729"
    assert restored.database_name == "pickle_unit"
    assert restored.tls is None
    assert restored._prepared_connection is None

    legacy = Database.__new__(Database)
    legacy.__setstate__(
        {
            "address": "legacy.example:1729",
            "database_name": "legacy_unit",
            "username": None,
            "password": None,
            "http_port": typedb_driver.DEFAULT_HTTP_PORT,
            "server_version": None,
            "_driver": None,
            "_owns_driver": True,
        }
    )
    assert legacy.tls is None
    assert legacy.tls_root_ca is None
    assert legacy._current_connection_identity().address == "legacy.example:1729"
    legacy.close()
