//! The Phase-8 integration smoke: from an empty workspace, author schema,
//! generate and apply migrations, run typed queries, evolve the schema, and
//! replay the whole history into an empty database.
//!
//! Model/code generation for the binding targets is exercised by the
//! generator parity suites (Python `render_models_json`, Node
//! `renderModelsJson`, and the dts-parity lane); this smoke covers the
//! workspace-to-database journey through the real `type-bridge` binary.
//!
//! Requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT); run with
//! `-- --ignored`.

use std::fs;
use std::path::Path;
use std::process::Command;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{Operation, PlanSpec, QueryPlan, Stage};
use type_bridge_contract::query_plan::{OrderDirection, OrderTerm, QueryOutput, QueryPattern};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact,
    SubFact, SubFactId, TypeFact, ValueFact, ValueFactId, encode_declared_schema,
};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_orm::TxType;
use type_bridge_orm::plan_authority::QueryAuthority;
use type_bridge_orm::session::database::Database;
use type_bridge_orm::session::staged::TransactionOptions;
use type_bridge_orm::{ConnectOptions, SecureConnectOptions, TlsMode};

#[path = "../../../../tests/support/typedb_database_move.rs"]
mod database_move;
mod support;

use database_move::move_database;

const MANAGED_SCOPE: &str = "e2e-smoke";
/// Environment variable naming the workspace semantic profile the live suite
/// runs under; the CLI requires the TypeDB server that profile pairs with.
const PROFILE_ENV: &str = "TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE";

fn live_profile() -> String {
    let profile = std::env::var(PROFILE_ENV).unwrap_or_else(|_| "typedb-3.12.1/v1".to_owned());
    let id = type_bridge_contract::fingerprint::SemanticProfileId::new(&profile)
        .unwrap_or_else(|error| panic!("{PROFILE_ENV}={profile:?} is invalid: {error}"));
    assert!(
        type_bridge_schema_migration_typedb::migration_server_for_profile(&id).is_some(),
        "{PROFILE_ENV}={profile} cannot run connected migrations"
    );
    profile
}

fn connect_options(http_port: &str) -> ConnectOptions {
    let http_port = http_port.parse::<u16>().unwrap_or_else(|error| {
        panic!(
            "TYPEDB_HTTP_PORT must be an integer from 1 through 65535, got {http_port:?}: {error}"
        )
    });
    assert_ne!(
        http_port, 0,
        "TYPEDB_HTTP_PORT must be an integer from 1 through 65535, got \"0\""
    );
    ConnectOptions {
        http_port,
        ..ConnectOptions::default()
    }
}

fn run_cli(workspace: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_type-bridge"))
        .current_dir(workspace)
        .args(arguments)
        .output()
        .expect("the type-bridge binary runs")
}

fn assert_success(output: &std::process::Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn write_manifest(workspace: &Path, address: &str, http_port: &str, databases: &[(&str, &str)]) {
    let mut environments = String::new();
    for (name, database) in databases {
        environments.push_str(&format!(
            "  {name}:\n    database: {database}\n    uri: {address}\n    \
             http-port: '{http_port}'\n    migrate: 'true'\n    credential:\n      \
             username: env:TYPEDB_USERNAME\n      password: env:TYPEDB_PASSWORD\n",
        ));
    }
    let profile = live_profile();
    fs::write(
        workspace.join("typebridge.yaml"),
        format!(
            "format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  \
             ownership: exclusive\n  managed-scope: {MANAGED_SCOPE}\ncompatibility:\n  \
             semantic-profile: {profile}\nmigrations:\n  directory: migrations/v2\n  \
             app-label: smoke\nenvironments:\n{environments}",
        ),
    )
    .expect("manifest writes");
}

fn write_tls_manifest(workspace: &Path, address: &str, http_port: u16, database: &str) {
    let profile = live_profile();
    fs::write(
        workspace.join("typebridge.yaml"),
        format!(
            "format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  \
             ownership: exclusive\n  managed-scope: {MANAGED_SCOPE}\ncompatibility:\n  \
             semantic-profile: {profile}\nmigrations:\n  directory: migrations/v2\n  \
             app-label: smoke\nenvironments:\n  tls-live:\n    database: {database}\n    \
             uri: {address}\n    http-port: '{http_port}'\n    tls: 'true'\n    \
             tls-root-ca: certs/root-ca.pem\n    migrate: 'true'\n    credential:\n      \
             username: env:TYPEDB_USERNAME\n      password: env:TYPEDB_PASSWORD\n",
        ),
    )
    .expect("TLS manifest writes");
}

fn declared_bytes_with_nickname(include_nickname: bool) -> Vec<u8> {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let name = AttributeId::new("name").unwrap();
    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(TypeFact::new(TypeId::new(TypeKind::Attribute, "name").unwrap()).unwrap()),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(name.clone()),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), name.clone()).unwrap(),
        )),
    ];
    if include_nickname {
        let nickname = AttributeId::new("nickname").unwrap();
        let employee = TypeId::new(TypeKind::Entity, "employee").unwrap();
        facts.push(SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, "nickname").unwrap()).unwrap(),
        ));
        facts.push(SchemaFact::Value(ValueFact::new(
            ValueFactId::new(nickname.clone()),
            ValueTypeTag::String,
        )));
        facts.push(SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), nickname).unwrap(),
        )));
        facts.push(SchemaFact::Type(TypeFact::new(employee.clone()).unwrap()));
        facts.push(SchemaFact::Sub(SubFact::new(
            SubFactId::new(employee, person).unwrap(),
        )));
    }
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).unwrap();
        let line = u32::try_from(index + 1).unwrap();
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("e2e-smoke").unwrap(),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .unwrap(),
        )
    });
    let declared =
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced).unwrap();
    encode_declared_schema(&declared).unwrap()
}

fn person_name_plan(authority: &QueryAuthority) -> QueryPlan {
    let binding = |id: u16, variable: &str| {
        AssertionBinding::new(
            BindingId::new(id).unwrap(),
            QueryVariable::new(variable).unwrap(),
        )
    };
    QueryPlan::new(
        PlanSpec {
            bindings: vec![binding(0, "person"), binding(1, "name")],
            inputs: vec![],
            functions: vec![],
            given_rows: None,
            operation: Operation::Rows,
            pipeline: vec![
                Stage::Match {
                    patterns: vec![
                        QueryPattern::Isa {
                            binding: BindingId::new(0).unwrap(),
                            include_subtypes: true,
                            type_id: TypeId::new(TypeKind::Entity, "person").unwrap(),
                        },
                        QueryPattern::Has {
                            attribute: BindingId::new(1).unwrap(),
                            attribute_id: AttributeId::new("name").unwrap(),
                            owner: BindingId::new(0).unwrap(),
                        },
                    ]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                },
                Stage::Sort {
                    terms: vec![OrderTerm::new(
                        BindingId::new(1).unwrap(),
                        OrderDirection::Ascending,
                    )],
                },
            ],
            output: QueryOutput::Rows {
                columns: vec![BindingId::new(0).unwrap(), BindingId::new(1).unwrap()],
            },
            managed_semantics: authority
                .context()
                .managed_state()
                .managed_semantic_schema()
                .clone(),
        },
        authority.declared(),
    )
    .unwrap()
}

async fn execute_plan(
    database: &Database,
    plan: &QueryPlan,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut transaction = database.transaction(TransactionOptions::default());
    let answer = transaction.stage_plan(plan)?;
    transaction.commit().await?;
    Ok(format!("{:?}", answer.get()?))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn empty_workspace_to_replayed_history_live() {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let http_port = std::env::var("TYPEDB_HTTP_PORT").unwrap_or_else(|_| "8000".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    // SAFETY: test process, set before any CLI child spawns.
    unsafe {
        std::env::set_var("TYPEDB_USERNAME", &username);
        std::env::set_var("TYPEDB_PASSWORD", &password);
    }
    let primary = support::live_database_name("tb_e2e_smoke");
    let replay = format!("{primary}_replay");

    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    write_manifest(
        root,
        &address,
        &http_port,
        &[("live", &primary), ("replay", &replay)],
    );

    // Author the first schema and check it offline. The schema root is a
    // schema-set manifest listing fragment documents.
    fs::write(
        root.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
    )
    .expect("schema set writes");
    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n\
         entities:\n  person: { owns: [name] }\n",
    )
    .expect("schema writes");
    assert_success(&run_cli(root, &["schema", "check"]), "schema check");

    // Generate and apply the first migration.
    assert_success(
        &run_cli(root, &["migration", "make", "--name", "init"]),
        "migration make init",
    );
    assert!(
        root.join("migrations/v2/0001_init.tbmigration.json")
            .exists()
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "live"]),
        "migration apply init",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "migration verify init",
    );

    // Run a typed query against the migrated database.
    let database = Database::connect_with_options(
        &address,
        &primary,
        &username,
        &password,
        connect_options(&http_port),
    )
    .await
    .expect("primary connects");
    database
        .execute_raw(
            "insert $a isa person, has name \"ada\"; \
             $b isa person, has name \"bob\";",
            TxType::Write,
        )
        .await
        .expect("data inserts");
    let authority = QueryAuthority::from_declared_bytes(
        &declared_bytes_with_nickname(false),
        MANAGED_SCOPE,
        &live_profile(),
    )
    .expect("first authority");
    let plan = person_name_plan(&authority);
    let outcome = execute_plan(&database, &plan).await.expect("typed query");
    assert!(outcome.contains("\"ada\""), "{outcome}");
    assert!(outcome.contains("\"bob\""), "{outcome}");

    // Evolve the schema and migrate forward.
    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n  \
         nickname: { value: string }\nentities:\n  person: { owns: [name, nickname] }\n  \
         employee: { sub: { type: person } }\n",
    )
    .expect("schema evolves");
    assert_success(
        &run_cli(root, &["migration", "make", "--name", "nickname"]),
        "migration make nickname",
    );
    assert!(
        root.join("migrations/v2/0002_nickname.tbmigration.json")
            .exists(),
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "live"]),
        "migration apply nickname",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "migration verify nickname",
    );

    // The evolved schema serves the same typed query plus the new attribute.
    database
        .execute_raw(
            "match $a isa person, has name \"ada\"; \
             insert $a has nickname \"lovelace\";",
            TxType::Write,
        )
        .await
        .expect("evolved data inserts");
    let evolved_authority = QueryAuthority::from_declared_bytes(
        &declared_bytes_with_nickname(true),
        MANAGED_SCOPE,
        &live_profile(),
    )
    .expect("evolved authority");
    let evolved_plan = person_name_plan(&evolved_authority);
    let outcome = execute_plan(&database, &evolved_plan)
        .await
        .expect("evolved typed query");
    assert!(outcome.contains("\"ada\""), "{outcome}");

    // Replay the complete history from an empty database.
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "replay"]),
        "migration replay",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "replay"]),
        "migration replay verify",
    );
    let replayed = Database::connect_with_options(
        &address,
        &replay,
        &username,
        &password,
        connect_options(&http_port),
    )
    .await
    .expect("replay connects");
    replayed
        .execute_raw(
            "insert $a isa person, has name \"eve\", has nickname \"ev\";",
            TxType::Write,
        )
        .await
        .expect("replayed schema accepts data");
    let outcome = execute_plan(&replayed, &evolved_plan)
        .await
        .expect("replayed typed query");
    assert!(outcome.contains("\"eve\""), "{outcome}");

    for managed_database in [&primary, &replay] {
        let journal = Database::connect_with_options(
            &address,
            &format!(
                "{managed_database}{}",
                type_bridge_contract::reserved::TYPEBRIDGE_JOURNAL_DATABASE_SUFFIX
            ),
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("journal connects");
        journal.delete_database().await.expect("journal cleanup");
    }
    database.delete_database().await.expect("primary cleanup");
    replayed.delete_database().await.expect("replay cleanup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated TypeDB 3.13.6 server (the documented examples' profile)"]
async fn documented_examples_initial_constraints_apply_and_verify_live() {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let http_port = std::env::var("TYPEDB_HTTP_PORT").unwrap_or_else(|_| "8000".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    // SAFETY: ignored live test process, set before any CLI child spawns.
    unsafe {
        std::env::set_var("TYPEDB_USERNAME", &username);
        std::env::set_var("TYPEDB_PASSWORD", &password);
    }

    let database_name = support::live_database_name("tb_e2e_documented_examples");
    let journal_name =
        type_bridge_schema_migration_typedb::derived_journal_database_name(&database_name);
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../examples");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir(root.join("schema")).expect("schema directory");
    for relative in [
        "typebridge.yaml",
        "schema/schema.yaml",
        "schema/application.yaml",
    ] {
        fs::copy(source.join(relative), root.join(relative))
            .unwrap_or_else(|error| panic!("copy documented {relative}: {error}"));
        assert_eq!(
            fs::read(source.join(relative)).expect("source example reads"),
            fs::read(root.join(relative)).expect("copied example reads"),
            "documented {relative} changed while staging live acceptance",
        );
    }

    // Preserve the copied public manifest byte-for-byte. The derived live
    // overlay changes only the endpoint and unique database required for an
    // isolated test; schema, migration, and binding authority remain exact.
    let documented_manifest =
        fs::read_to_string(root.join("typebridge.yaml")).expect("documented manifest reads");
    let documented_environment = "    database: typebridge-examples\n    uri: localhost:1729\n";
    assert_eq!(
        documented_manifest.matches(documented_environment).count(),
        1,
        "documented environment contract drifted",
    );
    let live_manifest = documented_manifest.replacen(
        documented_environment,
        &format!(
            "    database: {database_name}\n    uri: {address}\n    http-port: '{http_port}'\n"
        ),
        1,
    );
    fs::write(root.join("typebridge-live.yaml"), live_manifest)
        .expect("live manifest overlay writes");

    let run_example = |arguments: &[&str]| {
        let mut cli_arguments = vec!["--manifest", "typebridge-live.yaml"];
        cli_arguments.extend_from_slice(arguments);
        run_cli(root, &cli_arguments)
    };
    for (step, arguments) in [
        ("documented schema check", &["schema", "check"][..]),
        ("documented schema generate", &["schema", "generate"][..]),
        (
            "documented migration make",
            &["migration", "make", "--name", "initial"][..],
        ),
    ] {
        assert_success(&run_example(arguments), step);
    }
    let plan = run_example(&["migration", "plan"]);
    assert_success(&plan, "documented migration plan");
    assert!(
        String::from_utf8_lossy(&plan.stdout).contains("examples/0001_initial"),
        "documented migration plan omitted its initial migration: {}",
        String::from_utf8_lossy(&plan.stdout),
    );

    assert_success(
        &run_example(&["migration", "apply", "--environment", "development"]),
        "documented initial-constraint apply",
    );
    assert_success(
        &run_example(&["migration", "apply", "--environment", "development"]),
        "documented initial-constraint reapply",
    );
    assert_success(
        &run_example(&["migration", "verify", "--environment", "development"]),
        "documented initial-constraint verify",
    );

    for relative in [
        "typebridge.yaml",
        "schema/schema.yaml",
        "schema/application.yaml",
    ] {
        assert_eq!(
            fs::read(source.join(relative)).expect("source example reads"),
            fs::read(root.join(relative)).expect("copied example reads"),
            "live acceptance mutated documented {relative}",
        );
    }

    let managed = Database::connect_with_options(
        &address,
        &database_name,
        &username,
        &password,
        connect_options(&http_port),
    )
    .await
    .expect("documented managed database connects");
    assert!(
        managed
            .database_exists()
            .await
            .expect("managed database exists"),
        "documented apply did not create the managed database",
    );
    let journal = Database::connect_with_options(
        &address,
        &journal_name,
        &username,
        &password,
        connect_options(&http_port),
    )
    .await
    .expect("documented journal database connects");
    assert!(
        journal
            .database_exists()
            .await
            .expect("journal database exists"),
        "documented apply did not create its journal database",
    );
    journal.delete_database().await.expect("journal cleanup");
    managed.delete_database().await.expect("managed cleanup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires TYPEDB_TLS_ADDRESS / TYPEDB_TLS_HTTP_PORT / TYPEDB_TLS_ROOT_CA"]
async fn tls_workspace_apply_and_verify_live() {
    let (Ok(address), Ok(http_port), Ok(source_root_ca)) = (
        std::env::var("TYPEDB_TLS_ADDRESS"),
        std::env::var("TYPEDB_TLS_HTTP_PORT"),
        std::env::var("TYPEDB_TLS_ROOT_CA"),
    ) else {
        eprintln!(
            "skipping TLS live smoke: TYPEDB_TLS_ADDRESS, TYPEDB_TLS_HTTP_PORT, \
             and TYPEDB_TLS_ROOT_CA must all be set"
        );
        return;
    };
    let http_port = http_port
        .parse::<u16>()
        .expect("TYPEDB_TLS_HTTP_PORT is a u16");
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    // SAFETY: ignored live test process, set before any CLI child spawns.
    unsafe {
        std::env::set_var("TYPEDB_USERNAME", &username);
        std::env::set_var("TYPEDB_PASSWORD", &password);
    }

    let database = support::live_database_name("tb_e2e_tls");
    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::create_dir_all(root.join("certs")).expect("certificate directory");
    fs::copy(&source_root_ca, root.join("certs/root-ca.pem"))
        .expect("TLS root CA copies into the workspace");
    write_tls_manifest(root, &address, http_port, &database);
    fs::write(
        root.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
    )
    .expect("schema set writes");
    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n\
         entities:\n  person: { owns: [name] }\n",
    )
    .expect("schema writes");

    assert_success(
        &run_cli(root, &["migration", "make", "--name", "tls_init"]),
        "TLS migration make",
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "tls-live"]),
        "TLS migration apply",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "tls-live"]),
        "TLS migration verify",
    );

    let options = SecureConnectOptions {
        http_port,
        tls_mode: TlsMode::CustomRootCa(root.join("certs/root-ca.pem")),
        server_version: None,
    };
    let journal = type_bridge_schema_migration_typedb::derived_journal_database_name(&database);
    type_bridge_orm::delete_database_secure(
        &address,
        &journal,
        &username,
        &password,
        options.clone(),
    )
    .await
    .expect("TLS journal cleanup");
    type_bridge_orm::delete_database_secure(&address, &database, &username, &password, options)
        .await
        .expect("TLS managed database cleanup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn verify_never_creates_databases_live() {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let http_port = std::env::var("TYPEDB_HTTP_PORT").unwrap_or_else(|_| "8000".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    // SAFETY: test process, set before any CLI child spawns.
    unsafe {
        std::env::set_var("TYPEDB_USERNAME", &username);
        std::env::set_var("TYPEDB_PASSWORD", &password);
    }
    let missing = support::live_database_name("tb_e2e_verify_missing");

    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    write_manifest(root, &address, &http_port, &[("live", &missing)]);
    fs::write(
        root.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
    )
    .expect("schema set writes");
    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n\
         entities:\n  person: { owns: [name] }\n",
    )
    .expect("schema writes");

    // Two identical refusals prove verify created nothing on the first run.
    for round in 1..=2 {
        let output = run_cli(root, &["migration", "verify", "--environment", "live"]);
        assert!(
            !output.status.success(),
            "verify round {round} must refuse a missing database"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("does not exist"),
            "verify round {round} must name the missing database; stderr: {stderr}"
        );
    }
    let exists = type_bridge_orm::session::real_driver::database_exists(
        &address,
        &missing,
        &username,
        &password,
        connect_options(&http_port),
    )
    .await
    .expect("existence check");
    assert!(!exists, "verify must never create the managed database");
}

/// Address of the TypeDB 3.12.3 server the history is first applied on.
const UPGRADE_FROM_ENV: &str = "TYPE_BRIDGE_MIGRATION_UPGRADE_FROM";
/// Address of the TypeDB 3.13.6 server the history moves to.
const UPGRADE_TO_ENV: &str = "TYPE_BRIDGE_MIGRATION_UPGRADE_TO";
/// Optional HTTP ports of the two servers; both default to TypeDB's 8000,
/// which two servers on one host cannot share.
const UPGRADE_FROM_HTTP_PORT_ENV: &str = "TYPE_BRIDGE_MIGRATION_UPGRADE_FROM_HTTP_PORT";
const UPGRADE_TO_HTTP_PORT_ENV: &str = "TYPE_BRIDGE_MIGRATION_UPGRADE_TO_HTTP_PORT";

/// The HTTP port set for the upgrade endpoint at `address`, if any.
fn upgrade_http_port(address: &str) -> Option<String> {
    [
        (UPGRADE_FROM_ENV, UPGRADE_FROM_HTTP_PORT_ENV),
        (UPGRADE_TO_ENV, UPGRADE_TO_HTTP_PORT_ENV),
    ]
    .into_iter()
    .find(|(endpoint, _)| std::env::var(endpoint).as_deref() == Ok(address))
    .and_then(|(_, port)| std::env::var(port).ok())
}

fn upgrade_connect_options(address: &str) -> ConnectOptions {
    upgrade_http_port(address).map_or_else(ConnectOptions::default, |port| connect_options(&port))
}

fn write_upgrade_manifest(workspace: &Path, profile: &str, from: &str, to: &str, database: &str) {
    let mut environments = String::new();
    for (name, address) in [("old", from), ("new", to)] {
        environments.push_str(&format!(
            "  {name}:\n    database: {database}\n    uri: {address}\n    migrate: 'true'\n    \
             credential:\n      username: env:TYPEDB_USERNAME\n      password: env:TYPEDB_PASSWORD\n",
        ));
        if let Some(port) = upgrade_http_port(address) {
            environments.push_str(&format!("    http-port: '{port}'\n"));
        }
    }
    fs::write(
        workspace.join("typebridge.yaml"),
        format!(
            "format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  \
             ownership: exclusive\n  managed-scope: {MANAGED_SCOPE}\ncompatibility:\n  \
             semantic-profile: {profile}\nmigrations:\n  directory: migrations/v2\n  \
             app-label: smoke\nenvironments:\n{environments}",
        ),
    )
    .expect("manifest writes");
}

fn assert_failure_contains(output: &std::process::Output, step: &str, needle: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && stderr.contains(needle),
        "{step} must fail with {needle}:\nstdout: {}\nstderr: {stderr}",
        String::from_utf8_lossy(&output.stdout),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires TYPE_BRIDGE_MIGRATION_UPGRADE_FROM (TypeDB 3.12.3) and \
            TYPE_BRIDGE_MIGRATION_UPGRADE_TO (TypeDB 3.13.6)"]
async fn upgrade_profile_moves_a_3_12_history_to_3_13_live() {
    let (Ok(from), Ok(to)) = (
        std::env::var(UPGRADE_FROM_ENV),
        std::env::var(UPGRADE_TO_ENV),
    ) else {
        eprintln!("{UPGRADE_FROM_ENV} and {UPGRADE_TO_ENV} are unset; skipping");
        return;
    };
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    // SAFETY: test process, set before any CLI child spawns.
    unsafe {
        std::env::set_var("TYPEDB_USERNAME", &username);
        std::env::set_var("TYPEDB_PASSWORD", &password);
    }
    let database_name = support::live_database_name("tb_e2e_upgrade_profile");
    let journal_name =
        type_bridge_schema_migration_typedb::derived_journal_database_name(&database_name);
    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::write(
        root.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
    )
    .expect("schema set writes");
    let model = root.join("schema/fragments/model.yaml");
    fs::write(
        &model,
        "format: typebridge.schema/v2\nentities: {person: {}}\n",
    )
    .expect("schema writes");

    // 1. Author and apply a history on the 3.12.3 server.
    write_upgrade_manifest(root, "typedb-3.12.1/v1", &from, &to, &database_name);
    assert_success(
        &run_cli(root, &["migration", "make", "--name", "initial"]),
        "make initial",
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "old"]),
        "apply on 3.12.3",
    );

    // 2. Move both databases to the 3.13.6 server.
    let files = tempfile::tempdir().expect("export directory");
    for name in [&database_name, &journal_name] {
        move_database(name, &from, &to, &username, &password, files.path()).await;
    }

    // 3. The workspace moves to 3.13.6; ordinary commands name the upgrade.
    write_upgrade_manifest(root, "typedb-3.13.6/v1", &from, &to, &database_name);
    let refused = run_cli(root, &["migration", "apply", "--environment", "new"]);
    assert_failure_contains(
        &refused,
        "apply before the upgrade",
        "migration_history_profile_upgrade_required",
    );
    assert_failure_contains(
        &refused,
        "apply before the upgrade",
        "hint: run `migration upgrade-profile --to typedb-3.13.6/v1`",
    );
    let upgrade_new = [
        "migration",
        "upgrade-profile",
        "--environment",
        "new",
        "--to",
        "typedb-3.13.6/v1",
    ];

    // 4. Live drift refuses the upgrade; the bridge file is already published.
    let managed = Database::connect_with_options(
        &to,
        &database_name,
        &username,
        &password,
        upgrade_connect_options(&to),
    )
    .await
    .expect("moved managed database connects");
    let mut drift = managed.schema_transaction().await.expect("drift tx");
    drift
        .query("define entity intruder;")
        .await
        .expect("add out-of-band type");
    drift.commit().await.expect("commit out-of-band type");
    assert_failure_contains(
        &run_cli(root, &upgrade_new),
        "upgrade over drift",
        "migration_upgrade_profile_live_drift",
    );
    assert!(
        root.join("migrations/v2/0002_profile_upgrade_typedb_3_13_6.tbmigration.json")
            .is_file()
    );
    let mut repair = managed.schema_transaction().await.expect("repair tx");
    repair
        .query("undefine intruder;")
        .await
        .expect("remove out-of-band type");
    repair.commit().await.expect("commit repair");

    // 5. Record the bridge, then again as a no-op.
    let recorded = run_cli(root, &upgrade_new);
    assert_success(&recorded, "upgrade-profile");
    assert!(
        String::from_utf8_lossy(&recorded.stdout).contains("recorded profile bridge"),
        "{}",
        String::from_utf8_lossy(&recorded.stdout)
    );
    let repeated = run_cli(root, &upgrade_new);
    assert_success(&repeated, "repeated upgrade-profile");
    assert!(
        String::from_utf8_lossy(&repeated.stdout).contains("already recorded"),
        "{}",
        String::from_utf8_lossy(&repeated.stdout)
    );

    // 6. The history continues on 3.13.6 and verifies clean.
    fs::write(
        &model,
        "format: typebridge.schema/v2\nentities: {person: {}, company: {}}\n",
    )
    .expect("schema evolves");
    assert_success(
        &run_cli(root, &["migration", "make", "--name", "company"]),
        "make after the bridge",
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "new"]),
        "apply after the bridge",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "new"]),
        "verify after the bridge",
    );

    for address in [&from, &to] {
        for name in [&database_name, &journal_name] {
            let database = Database::connect_with_options(
                address,
                name,
                &username,
                &password,
                upgrade_connect_options(address),
            )
            .await
            .expect("isolated database connects");
            database
                .delete_database()
                .await
                .expect("delete isolated database");
        }
    }
}

#[path = "../../../../tests/support/adopted_history.rs"]
mod adopted_history;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn completed_legacy_history_then_evolve_live() {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap();
    let http_port = std::env::var("TYPEDB_HTTP_PORT").unwrap();
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    let primary = support::live_database_name("tb270_06b_adopted_cli");
    let journal_name = type_bridge_schema_migration_typedb::derived_journal_database_name(&primary);
    let options = connect_options(&http_port);
    for name in [&primary, &journal_name] {
        type_bridge_orm::ensure_database_exists(&address, name, &username, &password, options)
            .await
            .unwrap();
    }
    let managed = std::sync::Arc::new(
        Database::connect_with_options(&address, &primary, &username, &password, options)
            .await
            .unwrap(),
    );
    let journal = std::sync::Arc::new(
        Database::connect_with_options(&address, &journal_name, &username, &password, options)
            .await
            .unwrap(),
    );
    let context = type_bridge_schema::ManagedDeltaContext::new(
        type_bridge_contract::managed_scope::ManagedScopeId::new(MANAGED_SCOPE).unwrap(),
        type_bridge_contract::fingerprint::SemanticProfileId::new(live_profile()).unwrap(),
        type_bridge_schema_migration_typedb::execution_capability_vocabulary().unwrap(),
    );
    let source = "define\nattribute name, value string;\nentity person, owns name;\n";
    let bridge = adopted_history::bridge(&context, source);
    adopted_history::seed(&managed, &journal, &context, &bridge, source).await;
    adopted_history::assert_writer_closed(&managed).await;
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).unwrap();
    fs::create_dir_all(root.join("migrations/v2")).unwrap();
    write_manifest(root, &address, &http_port, &[("live", &primary)]);
    fs::write(
        root.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
    )
    .unwrap();
    fs::write(root.join("schema/fragments/model.yaml"), "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\nentities:\n  person: { owns: [name] }\n").unwrap();
    fs::write(root.join("migrations/v2/adopted-genesis.typeql"), source).unwrap();
    fs::write(
        root.join("migrations/v2/0000_legacy_frontier.tbmigration.json"),
        type_bridge_schema_migration::encode_verified_manifest(&bridge).unwrap(),
    )
    .unwrap();
    let untouched_journal_schema = journal.schema_text().await.unwrap();
    let pending = run_cli(root, &["migration", "verify", "--environment", "live"]);
    assert!(!pending.status.success());
    assert!(String::from_utf8_lossy(&pending.stderr).contains("migration_journal_import_pending"));
    assert_eq!(
        journal.schema_text().await.unwrap(),
        untouched_journal_schema
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "live"]),
        "import completed adoption",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "verify imported adoption",
    );
    fs::write(root.join("schema/fragments/model.yaml"), "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n  nickname: { value: string }\nentities:\n  person: { owns: [name, nickname] }\n").unwrap();
    assert_success(
        &run_cli(root, &["migration", "make", "--name", "nickname"]),
        "post-adoption make",
    );
    assert!(
        root.join("migrations/v2/0001_nickname.tbmigration.json")
            .exists()
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "live"]),
        "post-adoption apply",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "post-adoption evolved verify",
    );
    managed
        .execute_raw(
            "insert $a isa person, has name \"ada\", has nickname \"lovelace\";",
            TxType::Write,
        )
        .await
        .unwrap();
    adopted_history::assert_writer_closed(&managed).await;
    managed
        .derived_legacy_journal_database()
        .delete_database()
        .await
        .unwrap();
    managed.delete_database().await.unwrap();
    journal.delete_database().await.unwrap();
}
