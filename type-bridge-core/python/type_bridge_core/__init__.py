"""Native TypeBridge engine exports and private binding seams."""

# ruff: noqa: F405  (the explicit public list names native wildcard imports)

from . import type_bridge_core
from ._blocking import blocking as blocking
from .type_bridge_core import *  # noqa: F403  (re-export the compiled extension's public API)
from .write_result import BatchWriteResult as BatchWriteResult
from .write_result import WriteResult as WriteResult

# Private native schema, query, and plan seams used by retained bindings.
# Leading underscores exclude them from wildcard exports.
_ArchivedTypeSchema = type_bridge_core._ArchivedTypeSchema
_CustomRootCaSnapshot = type_bridge_core._CustomRootCaSnapshot
_QueryDescriptorRegistry = type_bridge_core._QueryDescriptorRegistry
_query_plan_round_trip = type_bridge_core._query_plan_round_trip
_archived_classify_schema_diff = type_bridge_core._archived_classify_schema_diff
_archived_compute_schema_diff = type_bridge_core._archived_compute_schema_diff
_archived_generate_define_block = type_bridge_core._archived_generate_define_block
_archived_schema_diff_is_breaking = type_bridge_core._archived_schema_diff_is_breaking
_typeql_queries_equivalent = type_bridge_core._typeql_queries_equivalent
_derived_journal_database_name = type_bridge_core._derived_journal_database_name
# The plan builder's native seam; `type_bridge.plan` is its public surface.
_PlanBuilderCore = type_bridge_core._PlanBuilderCore
_Plan = type_bridge_core._Plan
# The perf drivers' stage recorder; only `--features perf-stages` builds,
# never a shipped wheel, have it.
if hasattr(type_bridge_core, "_perf_stages"):
    _perf_stages = type_bridge_core._perf_stages

__doc__ = type_bridge_core.__doc__
__all__ = [
    "BatchWriteResult",
    "Count",
    "DEFAULT_DIRECT_QUERY_ATTRIBUTE_VALUES",
    "DEFAULT_DIRECT_QUERY_BYTES",
    "DEFAULT_DIRECT_QUERY_COLLECTION_MEMBERS",
    "DEFAULT_DIRECT_QUERY_GRAPH_NODES",
    "DEFAULT_DIRECT_QUERY_ITEMS",
    "DEFAULT_DIRECT_QUERY_ROLE_PLAYERS",
    "DEFAULT_DIRECT_QUERY_STATEMENTS",
    "DEFAULT_DIRECT_QUERY_TIMEOUT_MILLISECONDS",
    "DEFAULT_HTTP_PORT",
    "DEFAULT_PROXY_QUERY_ATTRIBUTE_VALUES",
    "DEFAULT_PROXY_QUERY_BYTES",
    "DEFAULT_PROXY_QUERY_COLLECTION_MEMBERS",
    "DEFAULT_PROXY_QUERY_GRAPH_NODES",
    "DEFAULT_PROXY_QUERY_ITEMS",
    "DEFAULT_PROXY_QUERY_ROLE_PLAYERS",
    "DEFAULT_PROXY_QUERY_STATEMENTS",
    "DEFAULT_PROXY_QUERY_TIMEOUT_MILLISECONDS",
    "Documents",
    "DynamicExpr",
    "DynamicValue",
    "Exists",
    "Iid",
    "InvalidQueryRequestError",
    "MatchBindingHandle",
    "MatchFieldHandle",
    "MatchFunctionArgumentHandle",
    "MatchFunctionCallHandle",
    "MatchFunctionHandle",
    "MatchFunctionValueHandle",
    "MatchOrderHandle",
    "MatchPredicateHandle",
    "MatchQueryHandle",
    "MatchRequestError",
    "MatchRoleHandle",
    "MatchSelectionHandle",
    "MatchSessionHandle",
    "MatchShapeHandle",
    "Matched",
    "MigrationApprovalBuilder",
    "MigrationApprovalSet",
    "MigrationBackfillObservation",
    "MigrationCancellation",
    "MigrationCatalog",
    "MigrationExecutionReport",
    "MigrationExecutionResources",
    "MigrationHistoryEntry",
    "MigrationIdentity",
    "MigrationPlan",
    "MigrationPreview",
    "MigrationPreviewEntry",
    "MigrationVerificationFinding",
    "MigrationVerificationReport",
    "Pending",
    "PendingRemoteModelQuery",
    "PlanAnswer",
    "PlanAuthority",
    "PlanError",
    "ProjectedCardinalityError",
    "ProxyConnection",
    "PyDatabase",
    "PyManagedDatabaseDeletionPlan",
    "PyProjectedManagerFilter",
    "PyProjectedModelManager",
    "PyRuntimeProjection",
    "PyTransactionContext",
    "QueryCancellation",
    "QueryExecutionResourceLimits",
    "QueryLimits",
    "RawQueryError",
    "RawTransaction",
    "RemoteModelQueryContext",
    "Rows",
    "StagedPlan",
    "Transaction",
    "UnknownEnumValueError",
    "ValidatedMatchAttributeHandle",
    "ValidatedMatchResultHandle",
    "ValidatedMatchRoleHandle",
    "ValidatedMatchRolePlayerHandle",
    "ValidatedMatchRowHandle",
    "ValidatedMatchSlotHandle",
    "ValidatedMatchThingHandle",
    "VersionError",
    "WriteResult",
    "applied_migration_entity_label",
    "band",
    "blocking",
    "canonical_enum_spelling",
    "check_server_supported",
    "check_supported",
    "embedded_driver_version",
    "embedded_driver_versions",
    "is_migration_state_type",
    "max_supported_line",
    "migration_state_schema",
    "min_supported_version",
    "open_migration_catalog",
    "plan_authority",
    "plan_authority_from_schema_authority",
    "plan_prepare_remote_model_count",
    "plan_prepare_remote_model_exists",
    "plan_prepare_remote_model_page",
    "plan_prepare_remote_model_reduce",
    "plan_prepare_remote_model_reduce_by_field",
    "plan_prepare_remote_model_reduce_by_fields",
    "plan_prepare_remote_model_rows",
    "plan_remote_model_context_with_resources",
    "run_cli",
    "server_version",
]
