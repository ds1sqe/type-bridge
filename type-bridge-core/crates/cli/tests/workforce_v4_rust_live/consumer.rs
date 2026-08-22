use std::env;

use serde_json::json;
use type_bridge_generated_schema::{SCHEMA, open_migration_catalog};

#[tokio::main]
async fn main() {
    let database_name = env::var("TYPE_BRIDGE_WORKFORCE_V4_DATABASE")
        .expect("isolated database name is provided");
    let options = type_bridge::ConnectionOptions::new(
        env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "127.0.0.1:1729".to_owned()),
        database_name,
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
            "lifecycle": {"explicit_close": true, "repeat_close": true},
            "cleanup": {
                "managed_database_absent": final_state == type_bridge::ManagedDatabasePairState::Absent,
                "journal_database_absent": final_state == type_bridge::ManagedDatabasePairState::Absent
            }
        })
    );
}
