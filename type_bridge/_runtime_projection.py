"""Thin forwarding facade for generated V2 runtime projections."""

from collections.abc import Sequence
from typing import Any

from type_bridge._rust_runtime import rust_core, rust_database_for, rust_transaction_for


def install_runtime_projection(
    projection_json: str,
    semantic_fingerprint_json: str,
    projection_fingerprint_json: str,
    models: Sequence[tuple[type[object], type[object] | None]],
) -> Any:
    """Verify and install one generated package's native projection."""
    return rust_core().PyRuntimeProjection(
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        list(models),
    )


def projected_manager_for(
    projection: Any,
    model: type[object],
    connection: object,
) -> Any:
    """Bind a package projection to an existing native connection handle."""
    transaction = rust_transaction_for(connection)
    if transaction is not None:
        return projection.manager_for_transaction(model, transaction)
    return projection.manager_for_database(model, rust_database_for(connection))
