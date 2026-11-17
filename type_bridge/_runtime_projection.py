"""Thin forwarding facade for generated runtime projections."""

from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_bridge_core import PyProjectedModelManager, PyRuntimeProjection

from type_bridge._native import core_database_for, core_module, core_transaction_for


class GeneratedEntityProjection:
    """Nominal marker implemented only by generated entity model bases."""

    __slots__ = ()


class GeneratedRelationProjection:
    """Nominal marker implemented only by generated relation model bases."""

    __slots__ = ()


_installed_projection_by_model: dict[type[object], PyRuntimeProjection] = {}


def install_runtime_projection(
    projection_json: str,
    semantic_fingerprint_json: str,
    projection_fingerprint_json: str,
    models: Sequence[tuple[type[object], type[object] | None]],
) -> PyRuntimeProjection:
    """Verify and install one generated package's native projection."""
    installed = core_module().PyRuntimeProjection(
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        list(models),
    )
    for model, _reference in models:
        _installed_projection_by_model[model] = installed
    return installed


def install_runtime_projection_with_authority(
    projection_json: str,
    semantic_fingerprint_json: str,
    projection_fingerprint_json: str,
    models: Sequence[tuple[type[object], type[object] | None]],
    schema_authority: bytes,
    *,
    structs: Sequence[type[object]] = (),
) -> PyRuntimeProjection:
    """Verify and install one ordered generated package with compiled authority."""
    installed = core_module().PyRuntimeProjection(
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        list(models),
        schema_authority,
        list(structs),
    )
    for model, _reference in models:
        _installed_projection_by_model[model] = installed
    return installed


def _installed_projection_for(model: type[object]) -> PyRuntimeProjection:
    try:
        return _installed_projection_by_model[model]
    except KeyError:
        raise TypeError("query builder requires an exact installed generated model class") from None


def projected_manager_for(
    projection: PyRuntimeProjection,
    model: type[object],
    connection: object,
) -> PyProjectedModelManager:
    """Bind a package projection to an existing native connection handle."""
    from type_bridge_core import ProxyConnection

    if isinstance(connection, ProxyConnection):
        return projection.manager_for_proxy(model, connection)
    transaction = core_transaction_for(connection)
    if transaction is not None:
        return projection.manager_for_transaction(model, transaction)
    return projection.manager_for_database(model, core_database_for(connection))
