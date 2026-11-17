"""Pin the supported surface and audit repo consumers, including benchmark/CI scripts.

The accompanying whole-repository import grep covers workflow snippets and SDK
names in other languages; this AST check also catches multiline Python imports.
"""

import type_bridge_core

PUBLIC_EXPORTS = (
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
)


def test_public_exports_are_explicit_and_exact() -> None:
    assert tuple(sorted(type_bridge_core.__all__)) == PUBLIC_EXPORTS
    wildcard: dict[str, object] = {}
    exec("from type_bridge_core import *", wildcard)
    assert set(wildcard) - {"__builtins__"} == set(PUBLIC_EXPORTS)
    # README and the typed-query guide document this helper as public.
    assert callable(wildcard["blocking"])
    for removed in (
        "ValidationEngine",
        "ValueCoercer",
        "coerce_value",
        "format_value",
        "_clear_attribute_query",
        "_render_schema_annotations",
        "_render_schema_definition",
        "_render_schema_owns",
    ):
        assert not hasattr(type_bridge_core, removed), removed


def test_repository_python_consumers_do_not_import_retired_native_names():
    import ast
    import subprocess
    from pathlib import Path

    root = Path(__file__).resolve().parents[3]
    files = subprocess.check_output(["git", "ls-files", "--", "*.py"], cwd=root, text=True)
    removed = {
        "ValidationEngine",
        "ValueCoercer",
        "coerce_value",
        "format_value",
        "_clear_attribute_query",
        "_render_schema_annotations",
        "_render_schema_definition",
        "_render_schema_owns",
    }
    imports = []
    for relative in files.splitlines():
        source = root / relative
        if not source.exists():
            continue
        for node in ast.walk(ast.parse(source.read_text(encoding="utf-8"))):
            if isinstance(node, ast.ImportFrom) and node.module in {
                "type_bridge_core",
                "type_bridge_core.type_bridge_core",
            }:
                imports.extend(
                    f"{relative}:{node.lineno}: {name.name}"
                    for name in node.names
                    if name.name in removed
                )
    assert not imports, "Retired native imports: " + ", ".join(imports)
