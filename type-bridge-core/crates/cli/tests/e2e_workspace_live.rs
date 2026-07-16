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
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable,
};
use type_bridge_contract::query_plan::{
    OrderDirection, OrderTerm, QueryOutput, QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan,
    SourcedSchemaFact, TypeFact, ValueFact, ValueFactId, encode_declared_schema,
};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_orm::TxType;
use type_bridge_orm::query_v2_prepared::{QueryAuthority, execute_prepared_local};
use type_bridge_orm::session::backend::BoundedAnswerLimits;
use type_bridge_orm::session::database::Database;

const MANAGED_SCOPE: &str = "e2e-smoke";
const PROFILE: &str = "typedb-3.12.1/v1";

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
    fs::write(
        workspace.join("typebridge.yaml"),
        format!(
            "format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  \
             ownership: exclusive\n  managed-scope: {MANAGED_SCOPE}\ncompatibility:\n  \
             semantic-profile: {PROFILE}\nmigrations:\n  directory: migrations/v2\n  \
             app-label: smoke\nenvironments:\n{environments}",
        ),
    )
    .expect("manifest writes");
}

fn declared_bytes_with_nickname(include_nickname: bool) -> Vec<u8> {
    let person = TypeId::new(TypeKind::Entity, "person").unwrap();
    let name = AttributeId::new("name").unwrap();
    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).unwrap()),
        SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, "name").unwrap()).unwrap(),
        ),
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
        facts.push(SchemaFact::Type(
            TypeFact::new(TypeId::new(TypeKind::Attribute, "nickname").unwrap())
                .unwrap(),
        ));
        facts.push(SchemaFact::Value(ValueFact::new(
            ValueFactId::new(nickname.clone()),
            ValueTypeTag::String,
        )));
        facts.push(SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person, nickname).unwrap(),
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
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .unwrap();
    encode_declared_schema(&declared).unwrap()
}

fn person_name_plan_bytes(authority: &QueryAuthority) -> Vec<u8> {
    let binding = |id: u16, variable: &str| {
        AssertionBinding::new(
            BindingId::new(id).unwrap(),
            QueryVariable::new(variable).unwrap(),
        )
    };
    let plan = QueryPlan::new(
        vec![binding(0, "person"), binding(1, "name")],
        Vec::new(),
        vec![
            ReadStage::Match {
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
                ],
            },
            ReadStage::Sort {
                terms: vec![OrderTerm::new(
                    BindingId::new(1).unwrap(),
                    OrderDirection::Ascending,
                )],
            },
        ],
        QueryOutput::Rows {
            columns: vec![BindingId::new(0).unwrap(), BindingId::new(1).unwrap()],
        },
        authority.context().managed_state().managed_semantic_schema().clone(),
    )
    .unwrap();
    plan.canonical_bytes().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn empty_workspace_to_replayed_history_live() {
    let address =
        std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let http_port =
        std::env::var("TYPEDB_HTTP_PORT").unwrap_or_else(|_| "8000".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password =
        std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    // SAFETY: test process, set before any CLI child spawns.
    unsafe {
        std::env::set_var("TYPEDB_USERNAME", &username);
        std::env::set_var("TYPEDB_PASSWORD", &password);
    }
    let primary = format!("tb_e2e_smoke_{}", std::process::id());
    let replay = format!("{primary}_replay");
    for database in [&primary, &replay] {
        type_bridge_orm::session::real_driver::ensure_database_exists(
            &address,
            database,
            &username,
            &password,
            type_bridge_orm::session::real_driver::ConnectOptions::default(),
        )
        .await
        .expect("database exists");
    }

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
    assert!(root.join("migrations/v2/0001_init.tbmigration.json").exists());
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "live"]),
        "migration apply init",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "migration verify init",
    );

    // Run a typed V2 query against the migrated database.
    let database =
        Database::connect(&address, &primary, &username, &password)
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
        PROFILE,
    )
    .expect("first authority");
    let plan = person_name_plan_bytes(&authority);
    let invocation = r#"{"operation":"rows","rows":[]}"#;
    let outcome = execute_prepared_local(
        &database,
        &authority,
        &plan,
        invocation,
        BoundedAnswerLimits::default(),
    )
    .await
    .expect("typed query");
    assert!(outcome.contains("\"ada\""), "{outcome}");
    assert!(outcome.contains("\"bob\""), "{outcome}");

    // Evolve the schema and migrate forward.
    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n  \
         nickname: { value: string }\nentities:\n  person: { owns: [name, nickname] }\n",
    )
    .expect("schema evolves");
    assert_success(
        &run_cli(root, &["migration", "make", "--name", "nickname"]),
        "migration make nickname",
    );
    assert!(
        root.join("migrations/v2/0002_nickname.tbmigration.json").exists(),
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
        PROFILE,
    )
    .expect("evolved authority");
    let evolved_plan = person_name_plan_bytes(&evolved_authority);
    let outcome = execute_prepared_local(
        &database,
        &evolved_authority,
        &evolved_plan,
        invocation,
        BoundedAnswerLimits::default(),
    )
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
    let replayed =
        Database::connect(&address, &replay, &username, &password)
            .await
            .expect("replay connects");
    replayed
        .execute_raw(
            "insert $a isa person, has name \"eve\", has nickname \"ev\";",
            TxType::Write,
        )
        .await
        .expect("replayed schema accepts data");
    let outcome = execute_prepared_local(
        &replayed,
        &evolved_authority,
        &evolved_plan,
        invocation,
        BoundedAnswerLimits::default(),
    )
    .await
    .expect("replayed typed query");
    assert!(outcome.contains("\"eve\""), "{outcome}");

    database.delete_database().await.expect("primary cleanup");
    replayed.delete_database().await.expect("replay cleanup");
}
