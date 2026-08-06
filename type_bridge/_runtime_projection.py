"""Thin forwarding facade for generated V2 runtime projections."""

from __future__ import annotations

from collections.abc import Sequence
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_bridge_core import PyProjectedModelManager, PyRuntimeProjection

from type_bridge._rust_runtime import rust_core, rust_database_for, rust_transaction_for


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
    installed = rust_core().PyRuntimeProjection(
        projection_json,
        semantic_fingerprint_json,
        projection_fingerprint_json,
        list(models),
    )
    for model, _reference in models:
        _installed_projection_by_model[model] = installed
    return installed


def _installed_projection_for(model: type[object]) -> PyRuntimeProjection:
    try:
        return _installed_projection_by_model[model]
    except KeyError:
        raise TypeError("query builder requires an exact installed generated model class") from None


def projected_query_builder_match_entity_for(
    model: type[GeneratedEntityProjection],
    variable: str,
    filters: dict[str, object],
) -> str:
    """Compile a raw-query entity match from verified generated projection facts."""
    projection = _installed_projection_for(model)
    return projection.query_builder_match_entity(model, variable, filters)


def projected_query_builder_insert_entity_for(
    instance: GeneratedEntityProjection,
    variable: str,
) -> str:
    """Compile a raw-query entity insert from one exact generated value."""
    projection = _installed_projection_for(type(instance))
    return projection.query_builder_insert_entity(instance, variable)


def projected_query_builder_match_relation_for(
    model: type[GeneratedRelationProjection],
    variable: str,
    role_players: dict[str, str] | None,
) -> str:
    """Compile a raw-query relation match from verified generated role tokens."""
    projection = _installed_projection_for(model)
    return projection.query_builder_match_relation(model, variable, role_players)


def projected_manager_for(
    projection: PyRuntimeProjection,
    model: type[object],
    connection: object,
) -> PyProjectedModelManager:
    """Bind a package projection to an existing native connection handle."""
    transaction = rust_transaction_for(connection)
    if transaction is not None:
        return projection.manager_for_transaction(model, transaction)
    return projection.manager_for_database(model, rust_database_for(connection))
