"""Native TypeBridge engine exports and private binding seams."""

from . import type_bridge_core
from ._blocking import blocking as blocking
from .type_bridge_core import *  # noqa: F403  (re-export the compiled extension's public API)
from .write_result import BatchWriteResult as BatchWriteResult
from .write_result import WriteResult as WriteResult

# Private native schema, query, and plan seams used by retained bindings.
# Leading underscores exclude them from wildcard exports.
_ArchivedTypeSchema = type_bridge_core._ArchivedTypeSchema
_QueryDescriptorRegistry = type_bridge_core._QueryDescriptorRegistry
_query_plan_round_trip = type_bridge_core._query_plan_round_trip
_archived_classify_schema_diff = type_bridge_core._archived_classify_schema_diff
_archived_compute_schema_diff = type_bridge_core._archived_compute_schema_diff
_archived_generate_define_block = type_bridge_core._archived_generate_define_block
_archived_schema_diff_is_breaking = type_bridge_core._archived_schema_diff_is_breaking
_typeql_queries_equivalent = type_bridge_core._typeql_queries_equivalent
# The plan builder's native seam; `type_bridge.plan` is its public surface.
_PlanBuilderCore = type_bridge_core._PlanBuilderCore
_Plan = type_bridge_core._Plan
# The perf drivers' stage recorder; only `--features perf-stages` builds,
# never a shipped wheel, have it.
if hasattr(type_bridge_core, "_perf_stages"):
    _perf_stages = type_bridge_core._perf_stages

__doc__ = type_bridge_core.__doc__
if hasattr(type_bridge_core, "__all__"):
    __all__ = [*type_bridge_core.__all__, "blocking", "BatchWriteResult", "WriteResult"]
