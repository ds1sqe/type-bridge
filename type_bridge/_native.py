"""Native connection helpers for generated Python applications."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge_core import PyDatabase, PyTransactionContext


def core_module() -> Any:
    """Import the native extension or report that it is missing."""
    try:
        import type_bridge_core
    except ImportError as exc:
        raise RuntimeError("The type_bridge_core native extension is required") from exc
    return type_bridge_core


def migration_state_schema() -> dict[str, Any]:
    """Return the canonical Rust-owned migration-state schema descriptor."""
    return core_module().migration_state_schema()


def applied_migration_entity_label() -> str:
    """Return the canonical applied-migration entity label."""
    return str(core_module().applied_migration_entity_label())


def is_migration_state_type(kind: str, label: str) -> bool:
    """Return whether a kind-and-label pair belongs to migration state."""
    return bool(core_module().is_migration_state_type(kind, label))


def schema_text(connection: Any) -> str:
    """Export the live TypeDB schema as TypeQL text through the Rust driver."""
    return core_database_for(connection).schema_text()


def core_database_for(connection: Any) -> PyDatabase:
    """Return or create a Rust database handle for a Python Database object."""
    from type_bridge.session import Database, TransactionContext

    if isinstance(connection, TransactionContext):
        return core_database_for(connection.database)
    if not isinstance(connection, Database):
        raise NotImplementedError(
            "Native connections require Database or TransactionContext; "
            "raw Python transaction handles cannot be shared with the native engine"
        )

    with connection._transport_lock:
        cached = getattr(connection, "_native_database", None)
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

            native_db = core_module().PyDatabase.connect(
                identity.address,
                identity.database,
                identity.username or "admin",
                identity.password or "password",
                **connect_kwargs,
            )
        except BaseException:
            connection._discard_uncommitted_transport()
            raise
        setattr(connection, "_native_database", native_db)
        connection._transport_committed = True
        return native_db


def database_from_core(
    endpoint: str,
    database_name: str,
    native_database: PyDatabase,
) -> Any:
    """Wrap one package-owned native database without reconnecting."""
    from type_bridge.session import Database

    database = Database(address=endpoint, database=database_name)
    setattr(database, "_native_database", native_database)
    database._transport_committed = True
    return database


def core_transaction_for(connection: Any) -> PyTransactionContext | None:
    """Return the Rust transaction adapter for a Python TransactionContext."""
    from type_bridge.session import TransactionContext

    if not isinstance(connection, TransactionContext):
        return None

    native_tx = getattr(connection, "_native_tx", None)
    if native_tx is None:
        raise RuntimeError("Native transaction context is not active")
    return native_tx
