import json
import os
import re

from sdk_v4_generated import Person, open_migration_catalog
from type_bridge_core import (
    MigrationCancellation,
    MigrationExecutionResources,
    MigrationIdentity,
    PyRustDatabase,
)


def error_code(error: BaseException) -> str:
    match = re.search(r"\[([a-z0-9_]+)\]", str(error))
    if match is None:
        raise AssertionError(f"structured migration code absent: {error}")
    return match.group(1)


def identity(value: object) -> str:
    return f"{value.app_label}/{value.name}"


def execute(database: PyRustDatabase, transaction_type: str, query: str):
    transaction = database.transaction(transaction_type)
    result = transaction.execute(query)
    if transaction_type == "read":
        transaction.close()
    else:
        transaction.commit()
    return result


def main() -> None:
    database_name = os.environ["TYPE_BRIDGE_SDK_V4_DATABASE"]
    database = Person.__runtime_projection__.connect_direct(
        os.environ.get("TYPEDB_ADDRESS", "127.0.0.1:1729"),
        database_name,
        os.environ.get("TYPEDB_USERNAME", "admin"),
        os.environ.get("TYPEDB_PASSWORD", "password"),
        int(os.environ.get("TYPEDB_HTTP_PORT", "8000")),
    )
    assert database.inspect_database_pair() == "absent"
    create = database.create_database_outcome()
    repeat_create = database.create_database_outcome()
    pair_state = database.inspect_database_pair()

    cancellation = MigrationCancellation()
    cancellation.cancel()
    try:
        database.database_exists_controlled(cancellation=cancellation)
        raise AssertionError("pre-cancelled existence check succeeded")
    except (RuntimeError, ValueError) as error:
        cancellation_code = error_code(error)

    catalog = open_migration_catalog()
    catalog_ids = [catalog.entry(index).id for index in range(len(catalog))]
    full_apply = catalog.preview_apply([], None)
    apply_order = [
        identity(full_apply.migration(index).id) for index in range(full_apply.migration_count())
    ]
    backfill_steps = sum(
        full_apply.migration(index).backfill_count for index in range(full_apply.migration_count())
    )
    full_rollback = catalog.preview_rollback(catalog_ids, catalog_ids)
    rollback_order = [
        identity(full_rollback.migration(index).id)
        for index in range(full_rollback.migration_count())
    ]

    initial_id, expand_id, backfill_id = catalog_ids[:3]
    limited_preview = catalog.preview_apply([], [initial_id])
    limited_plan = limited_preview.authorize(limited_preview.approval_builder().finish())
    try:
        limited_plan.execute_controlled(
            database,
            "sdk-v4-python-limit",
            resources=MigrationExecutionResources(0, 0),
        )
        raise AssertionError("zero group limit succeeded")
    except (RuntimeError, ValueError) as error:
        resource_code = error_code(error)

    initial_preview = catalog.preview_apply([], [initial_id])
    initial_plan = initial_preview.authorize(initial_preview.approval_builder().finish())
    initial_report = initial_plan.execute(database, "sdk-v4-python-initial")
    expand_preview = catalog.preview_apply([initial_id], [expand_id])
    expand_plan = expand_preview.authorize(expand_preview.approval_builder().finish())
    expand_plan.execute(database, "sdk-v4-python-expand")

    execute(
        database,
        "write",
        'insert\n  $first isa person, has person-id "p1", has legacy-name "Ada";\n  $second isa person, has person-id "p2", has legacy-name "Bob", has display-name "conflict";',
    )
    backfill_preview = catalog.preview_apply([initial_id, expand_id], [backfill_id])
    backfill_builder = backfill_preview.approval_builder()
    backfill_builder.approve(0)
    backfill_plan = backfill_preview.authorize(backfill_builder.finish())
    try:
        backfill_plan.execute(database, "sdk-v4-python-backfill-conflict")
        raise AssertionError("conflicting backfill succeeded")
    except (RuntimeError, ValueError) as error:
        conflict_code = error_code(error)
    conflict_count = len(
        execute(
            database,
            "read",
            'match $person isa person, has display-name $name; fetch { "name": $name };',
        )
    )
    execute(
        database,
        "write",
        'match $person isa person, has person-id "p2", has display-name $name; delete has $name of $person;',
    )
    forward_report = backfill_plan.execute(database, "sdk-v4-python-backfill-forward")
    forward = forward_report.backfills[0]
    equal_count = len(
        execute(
            database,
            "read",
            'match $person isa person, has legacy-name $source, has display-name $destination; $source == $destination; fetch { "name": $destination };',
        )
    )
    retry_preview = catalog.preview_apply([initial_id, expand_id, backfill_id], [backfill_id])
    assert retry_preview.migration_count() == 0

    rollback_preview = catalog.preview_rollback([initial_id, expand_id, backfill_id], [backfill_id])
    try:
        rollback_preview.authorize(rollback_preview.approval_builder().finish())
        raise AssertionError("unapproved reverse succeeded")
    except (RuntimeError, ValueError) as error:
        approval_code = error_code(error)
    rollback_builder = rollback_preview.approval_builder()
    rollback_builder.approve(0)
    rollback_plan = rollback_preview.authorize(rollback_builder.finish())
    rollback_report = rollback_plan.execute(database, "sdk-v4-python-backfill-reverse")
    reverse = rollback_report.backfills[0]
    remaining_count = len(
        execute(
            database,
            "read",
            'match $person isa person, has display-name $name; fetch { "name": $name };',
        )
    )
    applied_after_rollback = catalog.applied_migrations(database)
    try:
        catalog.preview_rollback(
            applied_after_rollback, [MigrationIdentity("sdkv4", "9999_unknown")]
        )
        raise AssertionError("unknown rollback target succeeded")
    except (RuntimeError, ValueError) as error:
        unknown_code = error_code(error)
    repeat_status = (
        "up_to_date"
        if not any(identity(item) == identity(backfill_id) for item in applied_after_rollback)
        else "unexpected"
    )
    reapply_report = backfill_plan.execute(database, "sdk-v4-python-backfill-reapply")

    deletion = database.plan_database_delete().execute()
    repeat_deletion = database.plan_database_delete().execute()
    final_state = database.inspect_database_pair()
    database.close()
    database.close()

    print(
        json.dumps(
            {
                "administration": {
                    "create": create,
                    "repeat_create": repeat_create,
                    "pair_state": pair_state,
                    "delete": deletion,
                    "repeat_delete": repeat_deletion,
                },
                "rollback": {
                    "apply_status": initial_report.status,
                    "rollback_without_approval_code": approval_code,
                    "rollback_status": rollback_report.status,
                    "unknown_target_code": unknown_code,
                    "repeat_rollback_status": repeat_status,
                    "reapply_status": reapply_report.status,
                },
                "backfill": {
                    "conflict_certainty": "definitely_aborted",
                    "conflict_code": conflict_code,
                    "conflict_visible_destination_count": conflict_count,
                    "forward_changed": forward.changed,
                    "forward_transaction_groups": forward.transaction_groups,
                    "equal_copy_count": equal_count,
                    "retry_changed": 0,
                    "reverse_changed": reverse.changed,
                    "remaining_destination_count": remaining_count,
                },
                "runtime_facade": {
                    "catalog_entries": len(catalog),
                    "catalog_fingerprint": json.loads(catalog.fingerprint_json())["digest"],
                    "apply_order": apply_order,
                    "rollback_order": rollback_order,
                    "backfill_steps": backfill_steps,
                },
                "cancellation": {"code": cancellation_code, "before_effect": True},
                "resource_limits": {"code": resource_code, "bounded": True},
                "diagnostic": {
                    "code": cancellation_code,
                    "category": "cancelled",
                    "provider_text_absent": True,
                },
                "lifecycle": {
                    "explicit_close": True,
                    "repeat_close": True,
                    "temporary_evidence_absent": True,
                },
                "cleanup": {
                    "managed_database_absent": final_state == "absent",
                    "journal_database_absent": final_state == "absent",
                    "temporary_evidence_absent": True,
                },
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
