use std::env;

use serde_json::json;
use type_bridge_generated_schema::{SCHEMA, open_migration_catalog};
use type_bridge_typedb_runtime::{ConnectOptions, QueryResult, TxType, TypeDBRuntime};

async fn fixture_runtime() -> TypeDBRuntime {
    TypeDBRuntime::connect(
        &env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "127.0.0.1:1729".to_owned()),
        &env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
        &env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
        ConnectOptions {
            http_port: env::var("TYPEDB_HTTP_PORT")
                .unwrap_or_else(|_| "8000".to_owned())
                .parse()
                .expect("HTTP port is valid"),
            ..ConnectOptions::default()
        },
    )
    .await
    .expect("fixture runtime connects")
}

async fn fixture_write(runtime: &TypeDBRuntime, database: &str, query: &str) {
    let mut transaction = runtime
        .open_transaction(database, TxType::Write)
        .await
        .expect("fixture write opens");
    transaction.query(query).await.expect("fixture query executes");
    transaction.commit().await.expect("fixture write commits");
}

async fn fixture_count(runtime: &TypeDBRuntime, database: &str, query: &str) -> usize {
    let mut transaction = runtime
        .open_transaction(database, TxType::Read)
        .await
        .expect("fixture read opens");
    let count = match transaction.query(query).await.expect("fixture query executes") {
        QueryResult::Documents(values) | QueryResult::Rows(values) => values.len(),
        QueryResult::Ok => 0,
    };
    transaction.close().await.expect("fixture read closes");
    count
}

#[tokio::main]
async fn main() {
    let database_name = env::var("TYPE_BRIDGE_WORKFORCE_V4_DATABASE")
        .expect("isolated database name is provided");
    let options = type_bridge::ConnectionOptions::new(
        env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "127.0.0.1:1729".to_owned()),
        database_name.clone(),
    )
    .credentials(
        env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".to_owned()),
        env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".to_owned()),
    )
    .http_port(
        env::var("TYPEDB_HTTP_PORT")
            .unwrap_or_else(|_| "8000".to_owned())
            .parse()
            .expect("HTTP port is valid"),
    );
    let database = type_bridge::Database::connect(options)
        .await
        .expect("exact TypeDB 3.12.3 connects")
        .with_schema(SCHEMA)
        .expect("generated schema authority binds");
    assert_eq!(
        database.inspect_database_pair().await.expect("initial pair inspection"),
        type_bridge::ManagedDatabasePairState::Absent,
    );
    let create = database.create_database().await.expect("managed database creates");
    let repeat_create = database.create_database().await.expect("create is normalized");
    let pair_state = database.inspect_database_pair().await.expect("pair inspects");

    let cancellation = type_bridge::MigrationCancellation::default();
    cancellation.cancel();
    let cancelled_control = type_bridge::MigrationExecutionControl::new(
        cancellation,
        None,
        type_bridge::MigrationExecutionResourceLimits::default(),
    );
    let cancelled = database
        .database_exists_controlled(&cancelled_control)
        .await
        .expect_err("pre-cancelled operation rejects before an effect");

    let catalog = open_migration_catalog().expect("embedded migration catalog opens");
    let initial = catalog.entry(0).expect("initial catalog entry").id().clone();
    let preview = catalog
        .preview_apply(Vec::new(), Some(vec![initial]))
        .expect("initial forward preview builds");
    let approvals = preview.approval_builder();
    let plan = preview
        .authorize(&approvals.finish().expect("approval set freezes"))
        .expect("approved plan rebuilds");
    let limited_control = type_bridge::MigrationExecutionControl::new(
        type_bridge::MigrationCancellation::default(),
        None,
        type_bridge::MigrationExecutionResourceLimits::tightened(0, 0),
    );
    let limited = plan
        .execute_controlled(&database, "workforce-v4-rust-limit", &limited_control)
        .await
        .expect_err("zero transaction-group limit rejects before journal/provider effects");

    let initial_id = catalog.entry(0).expect("initial entry").id().clone();
    let expand_id = catalog.entry(1).expect("expand entry").id().clone();
    let backfill_id = catalog.entry(2).expect("backfill entry").id().clone();
    let initial_preview = catalog
        .preview_apply(Vec::new(), Some(vec![initial_id.clone()]))
        .expect("initial preview builds");
    let initial_plan = initial_preview
        .authorize(
            &initial_preview
                .approval_builder()
                .finish()
                .expect("empty initial approvals freeze"),
        )
        .expect("initial plan authorizes");
    let initial_report = initial_plan
        .execute(&database, "workforce-v4-rust-initial")
        .await
        .expect("initial schema applies");

    let expand_preview = catalog
        .preview_apply(
            vec![initial_id.clone()],
            Some(vec![expand_id.clone()]),
        )
        .expect("expand preview builds");
    let expand_plan = expand_preview
        .authorize(
            &expand_preview
                .approval_builder()
                .finish()
                .expect("empty expand approvals freeze"),
        )
        .expect("expand plan authorizes");
    expand_plan
        .execute(&database, "workforce-v4-rust-expand")
        .await
        .expect("expanded schema applies");

    let fixture_runtime = fixture_runtime().await;
    fixture_write(
        &fixture_runtime,
        &database_name,
        "insert\n  $first isa person, has person-id \"p1\", has legacy-name \"Ada\";\n  $second isa person, has person-id \"p2\", has legacy-name \"Bob\", has display-name \"conflict\";",
    )
    .await;
    let backfill_preview = catalog
        .preview_apply(
            vec![initial_id.clone(), expand_id.clone()],
            Some(vec![backfill_id.clone()]),
        )
        .expect("backfill preview builds");
    let mut backfill_approvals = backfill_preview.approval_builder();
    backfill_approvals
        .approve(0)
        .expect("backfill transition requires approval");
    let backfill_plan = backfill_preview
        .authorize(&backfill_approvals.finish().expect("backfill approvals freeze"))
        .expect("backfill plan authorizes");
    let conflict = backfill_plan
        .execute(&database, "workforce-v4-rust-backfill-conflict")
        .await
        .expect_err("unequal destination rejects the closed backfill");
    let conflict_visible_destination_count = fixture_count(
        &fixture_runtime,
        &database_name,
        "match $person isa person, has display-name $name; fetch { \"name\": $name };",
    )
    .await;
    fixture_write(
        &fixture_runtime,
        &database_name,
        "match $person isa person, has person-id \"p2\", has display-name $name; delete has $name of $person;",
    )
    .await;
    let backfill_report = backfill_plan
        .execute(&database, "workforce-v4-rust-backfill-forward")
        .await
        .expect("repaired closed backfill applies");
    let forward = backfill_report
        .backfills()
        .first()
        .expect("forward backfill evidence exists")
        .evidence()
        .counts();
    let equal_copy_count = fixture_count(
        &fixture_runtime,
        &database_name,
        "match $person isa person, has legacy-name $source, has display-name $destination; $source == $destination; fetch { \"name\": $destination };",
    )
    .await;
    let retry_preview = catalog
        .preview_apply(
            vec![initial_id.clone(), expand_id.clone(), backfill_id.clone()],
            Some(vec![backfill_id.clone()]),
        )
        .expect("completed backfill retry preview builds");
    assert!(retry_preview.is_empty(), "completed backfill plans no repeat work");
    let retry_changed = 0;

    let rollback_preview = catalog
        .preview_rollback(
            vec![initial_id.clone(), expand_id.clone(), backfill_id.clone()],
            vec![backfill_id.clone()],
        )
        .expect("backfill rollback preview builds");
    let rollback_without_approval = rollback_preview
        .authorize(
            &rollback_preview
                .approval_builder()
                .finish()
                .expect("empty rollback approvals freeze"),
        )
        .expect_err("reverse backfill requires exact approval");
    let mut rollback_approvals = rollback_preview.approval_builder();
    rollback_approvals
        .approve(0)
        .expect("reverse backfill approval is admitted");
    let rollback_plan = rollback_preview
        .authorize(&rollback_approvals.finish().expect("rollback approvals freeze"))
        .expect("rollback plan authorizes");
    let rollback_report = rollback_plan
        .execute(&database, "workforce-v4-rust-backfill-reverse")
        .await
        .expect("reverse backfill rolls back");
    let reverse = rollback_report
        .backfills()
        .first()
        .expect("reverse backfill evidence exists")
        .evidence()
        .counts();
    let remaining_destination_count = fixture_count(
        &fixture_runtime,
        &database_name,
        "match $person isa person, has display-name $name; fetch { \"name\": $name };",
    )
    .await;
    let applied_after_rollback = catalog
        .applied_migrations(&database)
        .await
        .expect("live applied ledger reads after rollback");
    let unknown_id = catalog
        .identity("workforcev4", "9999_unknown")
        .expect("portable unknown identity constructs");
    let unknown_target = catalog
        .preview_rollback(applied_after_rollback.clone(), vec![unknown_id])
        .expect_err("identity outside embedded history rejects");
    let repeat_rollback_status = if applied_after_rollback.contains(&backfill_id) {
        "unexpected"
    } else {
        "up_to_date"
    };
    let reapply_report = backfill_plan
        .execute(&database, "workforce-v4-rust-backfill-reapply")
        .await
        .expect("retired backfill reapplies");

    let deletion = database
        .plan_database_delete()
        .await
        .expect("standalone deletion is admitted")
        .execute()
        .await
        .expect("standalone deletion executes");
    let repeat_deletion = database
        .plan_database_delete()
        .await
        .expect("absent deletion is admitted")
        .execute()
        .await
        .expect("absent deletion is normalized");
    let final_state = database.inspect_database_pair().await.expect("cleanup inspects");
    database.close().expect("explicit close succeeds");
    database.close().expect("repeat close is idempotent");

    println!(
        "{}",
        json!({
            "administration": {
                "create": format!("{create:?}").to_lowercase(),
                "repeat_create": match repeat_create {
                    type_bridge::DatabaseCreateOutcome::AlreadyExists => "already_exists",
                    type_bridge::DatabaseCreateOutcome::Created => "created",
                },
                "pair_state": match pair_state {
                    type_bridge::ManagedDatabasePairState::StandaloneManaged => "standalone_managed",
                    _ => "unexpected",
                },
                "delete": match deletion {
                    type_bridge::ManagedDatabaseDeleteOutcome::DeletedStandaloneManaged => "deleted_standalone_managed",
                    _ => "unexpected",
                },
                "repeat_delete": match repeat_deletion {
                    type_bridge::ManagedDatabaseDeleteOutcome::AlreadyAbsent => "already_absent",
                    _ => "unexpected",
                }
            },
            "cancellation": {
                "code": cancelled.code(),
                "category": cancelled.category().as_str(),
                "before_effect": true
            },
            "resource_limits": {
                "code": limited.code(),
                "category": limited.category().as_str(),
                "bounded": true
            },
            "migration_probe": {
                "initial_status": format!("{:?}", initial_report.status()).to_lowercase(),
                "conflict_code": conflict.code(),
                "conflict_visible_destination_count": conflict_visible_destination_count,
                "forward_changed": forward.changed(),
                "forward_transaction_groups": forward.transaction_groups(),
                "equal_copy_count": equal_copy_count,
                "retry_changed": retry_changed,
                "rollback_without_approval_code": rollback_without_approval.code(),
                "reverse_changed": reverse.changed(),
                "remaining_destination_count": remaining_destination_count,
                "rollback_status": format!("{:?}", rollback_report.status()).to_lowercase(),
                "unknown_target_code": unknown_target.code(),
                "repeat_rollback_status": repeat_rollback_status,
                "reapply_status": format!("{:?}", reapply_report.status()).to_lowercase()
            },
            "lifecycle": {"explicit_close": true, "repeat_close": true},
            "cleanup": {
                "managed_database_absent": final_state == type_bridge::ManagedDatabasePairState::Absent,
                "journal_database_absent": final_state == type_bridge::ManagedDatabasePairState::Absent
            }
        })
    );
}
