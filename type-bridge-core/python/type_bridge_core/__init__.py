from . import type_bridge_core
from ._blocking import blocking as blocking
from .type_bridge_core import *  # noqa: F403  (re-export the compiled extension's public API)

# Private compatibility seams used only by retained V1 query execution and
# frozen-history readers. Leading underscores keep them out of wildcard and
# public inventory exports while allowing the Python facade to import them.
_ArchivedTypeSchema = type_bridge_core._ArchivedTypeSchema
_QueryCrudQueryBuilder = type_bridge_core._QueryCrudQueryBuilder
_QueryDescriptorRegistry = type_bridge_core._QueryDescriptorRegistry
_QueryDynamicEntityManager = type_bridge_core._QueryDynamicEntityManager
_QueryDynamicRelationManager = type_bridge_core._QueryDynamicRelationManager
_query_plan_v3_round_trip = type_bridge_core._query_plan_v3_round_trip
_archived_classify_schema_diff = type_bridge_core._archived_classify_schema_diff
_archived_compute_schema_diff = type_bridge_core._archived_compute_schema_diff
_archived_generate_define_block = type_bridge_core._archived_generate_define_block
_archived_schema_diff_is_breaking = type_bridge_core._archived_schema_diff_is_breaking
_typeql_queries_equivalent = type_bridge_core._typeql_queries_equivalent
_generated_declared_descriptors_json = type_bridge_core._generated_declared_descriptors_json
_query_build_has_lookup_query = type_bridge_core._query_build_has_lookup_query
_render_models_json = type_bridge_core._render_models_json
# The plan builder's native seam; `type_bridge.plan` is its public surface.
_PlanBuilderCore = type_bridge_core._PlanBuilderCore
_Plan = type_bridge_core._Plan
# The perf drivers' stage recorder; only `--features perf-stages` builds,
# never a shipped wheel, have it.
if hasattr(type_bridge_core, "_perf_stages"):
    _perf_stages = type_bridge_core._perf_stages

__doc__ = type_bridge_core.__doc__
if hasattr(type_bridge_core, "__all__"):
    __all__ = [*type_bridge_core.__all__, "blocking"]
