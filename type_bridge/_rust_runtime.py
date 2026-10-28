"""Native connection helpers for generated Python applications."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge_core import PyRustDatabase, PyRustTransactionContext


def rust_core() -> Any:
    """Import the optional Rust extension or raise a backend-specific error."""
    try:
        import type_bridge_core
    except ImportError as exc:
        raise RuntimeError(
            "TYPE_BRIDGE_BACKEND=rust requires the type_bridge_core Rust extension"
        ) from exc
    return type_bridge_core


def migration_state_schema() -> dict[str, Any]:
    """Return the canonical Rust-owned migration-state schema descriptor."""
    return rust_core().migration_state_schema()


def applied_migration_entity_label() -> str:
    """Return the canonical applied-migration entity label."""
    return str(rust_core().applied_migration_entity_label())


def is_migration_state_type(kind: str, label: str) -> bool:
    """Return whether a kind-and-label pair belongs to migration state."""
    return bool(rust_core().is_migration_state_type(kind, label))


def schema_text(connection: Any) -> str:
    """Export the live TypeDB schema as TypeQL text through the Rust driver."""
    return rust_database_for(connection).schema_text()


def rust_database_for(connection: Any) -> PyRustDatabase:
    """Return or create a Rust database handle for a Python Database object."""
    from type_bridge.session import Database, TransactionContext

    if isinstance(connection, TransactionContext):
        return rust_database_for(connection.database)
    if not isinstance(connection, Database):
        raise NotImplementedError(
            "TYPE_BRIDGE_BACKEND=rust supports Database and TransactionContext connections; "
            "raw Python Transaction handles cannot be shared with the Rust backend"
        )

    with connection._transport_lock:
        cached = getattr(connection, "_rust_backend_database", None)
        if cached is not None:
            return cached

        try:
            prepared = connection._prepared_connection_options()
            identity = prepared.identity
            connect_kwargs: dict[str, object] = {"http_port": identity.http_port}
            if identity.server_version is not None:
                connect_kwargs["server_version"] = identity.server_version
            if identity.tls_argument is not None:
                connect_kwargs["tls"] = identity.tls_argument
            if prepared.tls_root_ca_snapshot is not None:
                connect_kwargs["tls_root_ca"] = prepared.tls_root_ca_snapshot

            rust_db = rust_core().PyRustDatabase.connect(
                identity.address,
                identity.database,
                identity.username or "admin",
                identity.password or "password",
                **connect_kwargs,
            )
        except BaseException:
            connection._discard_uncommitted_transport()
            raise
        setattr(connection, "_rust_backend_database", rust_db)
        connection._transport_committed = True
        return rust_db


def database_from_rust(
    endpoint: str,
    database_name: str,
    rust_database: PyRustDatabase,
) -> Any:
    """Wrap one package-owned native database without reconnecting."""
    from type_bridge.session import Database

    database = Database(address=endpoint, database=database_name)
    setattr(database, "_rust_backend_database", rust_database)
    database._transport_committed = True
    return database


def rust_transaction_for(connection: Any) -> PyRustTransactionContext | None:
    """Return the Rust transaction adapter for a Python TransactionContext."""
    from type_bridge.session import TransactionContext

    if not isinstance(connection, TransactionContext):
        return None

    rust_tx = getattr(connection, "_rust_tx", None)
    if rust_tx is None:
        raise RuntimeError("Rust transaction context is not active")
    return rust_tx
