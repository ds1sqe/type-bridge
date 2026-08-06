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
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest as _, Sha256};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{
    OrderDirection, OrderTerm, QueryOutput, QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, SchemaFact, SourceSpan, SourcedSchemaFact,
    SubFact, SubFactId, TypeFact, ValueFact, ValueFactId, encode_declared_schema,
};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_orm::TxType;
use type_bridge_orm::query_v2_prepared::{QueryAuthority, execute_prepared_local};
use type_bridge_orm::session::backend::QueryV2AnswerLimits;
use type_bridge_orm::session::database::Database;
use type_bridge_orm::{ConnectOptions, SecureConnectOptions, TlsMode};

const MANAGED_SCOPE: &str = "e2e-smoke";
const PROFILE: &str = "typedb-3.12.1/v1";

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

fn source_tree_python() -> PathBuf {
    let configured = std::env::var_os("TYPE_BRIDGE_TEST_PYTHON").map(PathBuf::from);
    let path = configured.unwrap_or_else(|| {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .expect("repository root resolves");
        if cfg!(windows) {
            repository.join(".venv/Scripts/python.exe")
        } else {
            repository.join(".venv/bin/python")
        }
    });
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .expect("current directory resolves")
            .join(path)
    };
    assert!(
        absolute.is_file(),
        "sidecar converter Python is absent at {}; run `uv sync` or set \
         TYPE_BRIDGE_TEST_PYTHON to an interpreter with the project installed",
        absolute.display(),
    );
    absolute
}

fn run_sidecar_converter(workspace: &Path, archive_directory: &Path) -> std::process::Output {
    Command::new(source_tree_python())
        .current_dir(workspace)
        .env_remove("PYTHONHOME")
        .env_remove("PYTHONPATH")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONSAFEPATH", "1")
        .env("PYTHONWARNINGS", "error")
        .args(["-m", "type_bridge.migration.sidecar"])
        .arg(archive_directory)
        .output()
        .expect("the shipped Python sidecar converter runs")
}

fn assert_success(output: &std::process::Output, step: &str) {
    assert!(
        output.status.success(),
        "{step} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

async fn seed_legacy_ledger(
    database: &std::sync::Arc<Database>,
    app_label: &str,
    records: &[(&str, &str)],
) {
    use type_bridge_migration::MigrationStateStore as _;

    let store = type_bridge_migration::TypeDbStateStore::new(std::sync::Arc::clone(database));
    store
        .ensure_schema()
        .await
        .expect("legacy ledger schema installs");
    for &(name, checksum) in records {
        store
            .record_applied(type_bridge_migration::AppliedMigrationRecord {
                app_label: app_label.to_owned(),
                name: name.to_owned(),
                checksum: checksum.to_owned(),
                applied_at: None,
            })
            .await
            .expect("legacy ledger record seeds");
    }
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

fn write_tls_manifest(workspace: &Path, address: &str, http_port: u16, database: &str) {
    fs::write(
        workspace.join("typebridge.yaml"),
        format!(
            "format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n  \
             ownership: exclusive\n  managed-scope: {MANAGED_SCOPE}\ncompatibility:\n  \
             semantic-profile: {PROFILE}\nmigrations:\n  directory: migrations/v2\n  \
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
        authority
            .context()
            .managed_state()
            .managed_semantic_schema()
            .clone(),
    )
    .unwrap();
    plan.canonical_bytes().unwrap()
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
    let primary = format!("tb_e2e_smoke_{}", std::process::id());
    let replay = format!("{primary}_replay");
    for database in [&primary, &replay] {
        type_bridge_orm::session::real_driver::ensure_database_exists(
            &address,
            database,
            &username,
            &password,
            connect_options(&http_port),
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

    // Run a typed V2 query against the migrated database.
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
        QueryV2AnswerLimits::default(),
    )
    .await
    .expect("typed query");
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
        PROFILE,
    )
    .expect("evolved authority");
    let evolved_plan = person_name_plan_bytes(&evolved_authority);
    let outcome = execute_prepared_local(
        &database,
        &evolved_authority,
        &evolved_plan,
        invocation,
        QueryV2AnswerLimits::default(),
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
    let outcome = execute_prepared_local(
        &replayed,
        &evolved_authority,
        &evolved_plan,
        invocation,
        QueryV2AnswerLimits::default(),
    )
    .await
    .expect("replayed typed query");
    assert!(outcome.contains("\"eve\""), "{outcome}");

    for managed_database in [&primary, &replay] {
        let journal = Database::connect_with_options(
            &address,
            &format!("{managed_database}__tbv2_journal"),
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

    let database = format!("tb_e2e_tls_{}", std::process::id());
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
    let missing = format!("tb_e2e_verify_missing_{}", std::process::id());

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

fn write_legacy_migration(
    directory: &Path,
    app_label: &str,
    name: &str,
    python_source: &str,
    snapshot_schema: &str,
    write_executable_sidecar: bool,
) -> (String, String) {
    let checksum = type_bridge_migration::migration_file_checksum(python_source);
    let source_sha256 = format!("{:x}", Sha256::digest(python_source.as_bytes()));
    let schema_hash = format!("{:x}", Sha256::digest(snapshot_schema.as_bytes()));
    let schema_source = type_bridge_migration::MigrationDependencySpec {
        app_label: app_label.to_owned(),
        migration_name: name.to_owned(),
    };
    fs::write(directory.join(format!("{name}.py")), python_source)
        .expect("legacy python source writes");
    let adoption_bytes = if write_executable_sidecar {
        let spec = type_bridge_migration::MigrationSpec {
            app_label: app_label.to_owned(),
            name: name.to_owned(),
            dependencies: Vec::new(),
            operations: Vec::new(),
            checksum: Some(checksum.clone()),
            source_sha256: Some(source_sha256.clone()),
            reversible: true,
        };
        let sidecar_bytes = serde_json::to_vec_pretty(&spec).expect("legacy sidecar encodes");
        fs::write(directory.join(format!("{name}.json")), &sidecar_bytes)
            .expect("legacy sidecar writes");
        let adoption = type_bridge_migration::LegacySidecarAdoptionMetadata::new(
            name,
            app_label,
            name,
            spec.dependencies.clone(),
            checksum.clone(),
            spec.checksum.clone(),
            source_sha256.clone(),
            format!("{:x}", Sha256::digest(&sidecar_bytes)),
            type_bridge_migration::LegacySchemaEffect::Snapshot,
            schema_source.clone(),
            schema_hash.clone(),
        )
        .expect("legacy sidecar adoption metadata constructs");
        serde_json::to_vec_pretty(&adoption).expect("legacy sidecar adoption metadata encodes")
    } else {
        // Without a released sidecar, the frozen source metadata carries the
        // graph identity while the snapshot remains schema authority.
        let adoption = type_bridge_migration::LegacyAdoptionMetadata::new(
            app_label,
            name,
            Vec::new(),
            checksum.clone(),
            source_sha256,
            type_bridge_migration::LegacySchemaEffect::Snapshot,
            schema_source,
            schema_hash.clone(),
        )
        .expect("legacy adoption metadata constructs");
        serde_json::to_vec_pretty(&adoption).expect("legacy adoption metadata encodes")
    };
    fs::write(
        directory.join(format!("{name}.adoption.json")),
        adoption_bytes,
    )
    .expect("legacy adoption metadata writes");

    let snapshot = directory.join("snapshots/v0001");
    fs::create_dir_all(&snapshot).expect("legacy snapshot directory writes");
    fs::write(snapshot.join("schema.tql"), snapshot_schema).expect("legacy snapshot schema writes");
    fs::write(
        snapshot.join("snapshot.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "version": "v0001",
            "source_migration": name,
            "schema_hash": schema_hash,
            "file_hashes": {"schema.tql": schema_hash},
            "type_bridge_version": "1.5.11",
            "type_bridge_core_version": "1.5.11"
        }))
        .expect("legacy snapshot manifest encodes"),
    )
    .expect("legacy snapshot manifest writes");
    (checksum, schema_hash)
}

fn write_legacy_run_python_migration(
    directory: &Path,
    app_label: &str,
    name: &str,
    dependency_name: &str,
    python_source: &str,
    snapshot_schema_source: &str,
    snapshot_schema_hash: &str,
) -> String {
    let checksum = type_bridge_migration::migration_file_checksum(python_source);
    fs::write(directory.join(format!("{name}.py")), python_source)
        .expect("legacy RunPython source writes");
    let dependency = type_bridge_migration::MigrationDependencySpec {
        app_label: app_label.to_owned(),
        migration_name: dependency_name.to_owned(),
    };
    let adoption = type_bridge_migration::LegacyAdoptionMetadata::new(
        app_label,
        name,
        vec![dependency],
        checksum.clone(),
        format!("{:x}", Sha256::digest(python_source.as_bytes())),
        type_bridge_migration::LegacySchemaEffect::UnchangedRunPython,
        type_bridge_migration::MigrationDependencySpec {
            app_label: app_label.to_owned(),
            migration_name: snapshot_schema_source.to_owned(),
        },
        snapshot_schema_hash,
    )
    .expect("legacy RunPython adoption metadata constructs");
    fs::write(
        directory.join(format!("{name}.adoption.json")),
        serde_json::to_string_pretty(&adoption)
            .expect("legacy RunPython adoption metadata encodes"),
    )
    .expect("legacy RunPython adoption metadata writes");
    checksum
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB (TYPEDB_ADDRESS / TYPEDB_HTTP_PORT)"]
async fn adopt_legacy_history_then_evolve_live() {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let http_port = std::env::var("TYPEDB_HTTP_PORT").unwrap_or_else(|_| "8000".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    // SAFETY: test process, set before any CLI child spawns.
    unsafe {
        std::env::set_var("TYPEDB_USERNAME", &username);
        std::env::set_var("TYPEDB_PASSWORD", &password);
    }

    // Seed a migrated v1 database: legacy ledger schema, one applied
    // migration with its recorded checksum, and the user schema its history
    // reached — exactly what a 1.5.x deployment carries before cutover.
    let primary = format!("tb_e2e_adopt_{}", std::process::id());
    let replay = format!("{primary}_replay");
    for database in [&primary, &replay] {
        type_bridge_orm::session::real_driver::ensure_database_exists(
            &address,
            database,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("v1 database exists");
    }
    let v1 = std::sync::Arc::new(
        Database::connect_with_options(
            &address,
            &primary,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("v1 database connects"),
    );
    let replay_database = std::sync::Arc::new(
        Database::connect_with_options(
            &address,
            &replay,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("legacy-equivalent replay database connects"),
    );

    let workspace = tempfile::tempdir().expect("workspace directory");
    let root = workspace.path();
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("migration directory");
    fs::create_dir_all(root.join("migrations/smoke")).expect("legacy directory");
    write_manifest(
        root,
        &address,
        &http_port,
        &[("live", &primary), ("replay", &replay)],
    );
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

    let legacy_schema = "define\nattribute name, value string;\nentity person, owns name;\n";
    let (initial_checksum, schema_hash) = write_legacy_migration(
        &root.join("migrations/smoke"),
        "smoke",
        "0001_initial",
        "class Migration:\n    operations = []\n",
        legacy_schema,
        true,
    );
    let backfill_checksum = write_legacy_run_python_migration(
        &root.join("migrations/smoke"),
        "smoke",
        "0002_backfill",
        "0001_initial",
        "def forwards(database):\n    return None\n\nclass Migration:\n    dependencies = [(\"smoke\", \"0001_initial\")]\n    operations = [RunPython(forwards)]\n",
        "0001_initial",
        &schema_hash,
    );
    for database in [&v1, &replay_database] {
        seed_legacy_ledger(
            database,
            "smoke",
            &[
                ("0001_initial", initial_checksum.as_str()),
                ("0002_backfill", backfill_checksum.as_str()),
            ],
        )
        .await;
        database
            .execute_raw(legacy_schema, TxType::Schema)
            .await
            .expect("legacy schema effect replays");
    }

    // Adopt: the reconstructed head becomes the durable workspace genesis
    // and the zero-operation bridge checkpoints the ledger.
    assert_success(
        &run_cli(
            root,
            &[
                "migration",
                "adopt",
                "--environment",
                "live",
                "--archive-directory",
                "migrations/smoke",
            ],
        ),
        "migration adopt",
    );
    assert!(root.join("migrations/v2/adopted-genesis.typeql").exists());
    assert!(
        root.join("migrations/v2/0000_archive_frontier.tbmigration.json")
            .exists(),
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "post-adoption verify",
    );

    // Adoption is idempotent: a re-run reports the bridged ledger current.
    let rerun = run_cli(
        root,
        &[
            "migration",
            "adopt",
            "--environment",
            "live",
            "--archive-directory",
            "migrations/smoke",
        ],
    );
    assert_success(&rerun, "repeated adopt");
    assert!(
        String::from_utf8_lossy(&rerun.stdout).contains("already adopted"),
        "stdout: {}",
        String::from_utf8_lossy(&rerun.stdout),
    );

    // Every database establishes its own ledger-bound cutover anchor through
    // guarded adoption. The second scope reuses the canonical workspace
    // authority without executing either historical Python migration.
    assert_success(
        &run_cli(
            root,
            &[
                "migration",
                "adopt",
                "--environment",
                "replay",
                "--archive-directory",
                "migrations/smoke",
            ],
        ),
        "mixed-history replay adoption",
    );

    // Ordinary work chains onto the adopted genesis: make sees the bridge
    // head as its source and generates only the nickname delta.
    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n  \
         nickname: { value: string }\nentities:\n  person: { owns: [name, nickname] }\n",
    )
    .expect("schema evolves");
    assert_success(
        &run_cli(root, &["migration", "make", "--name", "nickname"]),
        "post-adoption make",
    );
    assert!(
        root.join("migrations/v2/0001_nickname.tbmigration.json")
            .exists(),
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "live"]),
        "post-adoption apply",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "post-adoption evolved verify",
    );
    v1.execute_raw(
        "insert $a isa person, has name \"ada\", has nickname \"lovelace\";",
        TxType::Write,
    )
    .await
    .expect("evolved schema accepts data");

    // Once the replay scope has its own guarded frontier anchor, ordinary
    // apply replays only the post-adoption V2 delta.
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "replay"]),
        "mixed-history canonical replay",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "replay"]),
        "mixed-history replay verify",
    );
    replay_database
        .execute_raw(
            "insert $a isa person, has name \"grace\", has nickname \"hopper\";",
            TxType::Write,
        )
        .await
        .expect("replayed evolved schema accepts data");

    for database in [&primary, &replay] {
        let journal = Database::connect_with_options(
            &address,
            &format!("{database}__tbv2_journal"),
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("journal connects");
        journal.delete_database().await.expect("journal cleanup");
    }
    replay_database
        .delete_database()
        .await
        .expect("replay database cleanup");
    v1.delete_database().await.expect("v1 cleanup");

    // Repeat the product path with an all-Python legacy directory. Neither
    // migration has an executable JSON sidecar: the root snapshot and the
    // RunPython head are represented only by their released Python source and
    // checksum-bound non-executable adoption metadata.
    let python_primary = format!("{primary}_python");
    let python_replay = format!("{python_primary}_replay");
    for database in [&python_primary, &python_replay] {
        type_bridge_orm::session::real_driver::ensure_database_exists(
            &address,
            database,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("all-Python legacy database exists");
    }
    let python_live_database = std::sync::Arc::new(
        Database::connect_with_options(
            &address,
            &python_primary,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("all-Python legacy database connects"),
    );
    let python_replay_database = std::sync::Arc::new(
        Database::connect_with_options(
            &address,
            &python_replay,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("all-Python replay database connects"),
    );
    let python_workspace = tempfile::tempdir().expect("all-Python workspace directory");
    let python_root = python_workspace.path();
    fs::create_dir_all(python_root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(python_root.join("migrations/v2")).expect("migration directory");
    fs::create_dir_all(python_root.join("migrations/smoke")).expect("legacy directory");
    write_manifest(
        python_root,
        &address,
        &http_port,
        &[("live", &python_primary), ("replay", &python_replay)],
    );
    fs::write(
        python_root.join("schema/schema.yaml"),
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
    )
    .expect("schema set writes");
    fs::write(
        python_root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n\
         entities:\n  person: { owns: [name] }\n",
    )
    .expect("schema writes");
    let (python_initial_checksum, python_schema_hash) = write_legacy_migration(
        &python_root.join("migrations/smoke"),
        "smoke",
        "0001_initial",
        "class Migration:\n    operations = []\n",
        legacy_schema,
        false,
    );
    let python_backfill_checksum = write_legacy_run_python_migration(
        &python_root.join("migrations/smoke"),
        "smoke",
        "0002_backfill",
        "0001_initial",
        "def forwards(database):\n    return None\n\nclass Migration:\n    dependencies = [(\"smoke\", \"0001_initial\")]\n    operations = [RunPython(forwards)]\n",
        "0001_initial",
        &python_schema_hash,
    );
    for database in [&python_live_database, &python_replay_database] {
        seed_legacy_ledger(
            database,
            "smoke",
            &[
                ("0001_initial", python_initial_checksum.as_str()),
                ("0002_backfill", python_backfill_checksum.as_str()),
            ],
        )
        .await;
        database
            .execute_raw(legacy_schema, TxType::Schema)
            .await
            .expect("all-Python legacy schema effect replays");
    }
    assert_success(
        &run_cli(
            python_root,
            &[
                "migration",
                "adopt",
                "--environment",
                "live",
                "--archive-directory",
                "migrations/smoke",
            ],
        ),
        "all-Python migration adopt",
    );
    assert_success(
        &run_cli(
            python_root,
            &["migration", "verify", "--environment", "live"],
        ),
        "all-Python post-adoption verify",
    );
    assert_success(
        &run_cli(
            python_root,
            &[
                "migration",
                "adopt",
                "--environment",
                "replay",
                "--archive-directory",
                "migrations/smoke",
            ],
        ),
        "all-Python replay adoption",
    );
    assert_success(
        &run_cli(
            python_root,
            &["migration", "apply", "--environment", "replay"],
        ),
        "all-Python canonical replay",
    );
    assert_success(
        &run_cli(
            python_root,
            &["migration", "verify", "--environment", "replay"],
        ),
        "all-Python replay verify",
    );
    for database in [&python_primary, &python_replay] {
        let journal = Database::connect_with_options(
            &address,
            &format!("{database}__tbv2_journal"),
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("all-Python journal connects");
        journal
            .delete_database()
            .await
            .expect("all-Python journal cleanup");
    }
    python_replay_database
        .delete_database()
        .await
        .expect("all-Python replay database cleanup");
    python_live_database
        .delete_database()
        .await
        .expect("all-Python legacy database cleanup");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a live TypeDB and an installed source-tree Python package"]
async fn shipped_python_converter_to_native_adoption_live() {
    let address = std::env::var("TYPEDB_ADDRESS").unwrap_or_else(|_| "localhost:1730".into());
    let http_port = std::env::var("TYPEDB_HTTP_PORT").unwrap_or_else(|_| "8000".into());
    let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
    // SAFETY: ignored live test process, set before any CLI child spawns.
    unsafe {
        std::env::set_var("TYPEDB_USERNAME", &username);
        std::env::set_var("TYPEDB_PASSWORD", &password);
    }

    let primary = format!("tb_e2e_converter_{}", std::process::id());
    let replay = format!("{primary}_replay");
    for database in [&primary, &replay] {
        type_bridge_orm::session::real_driver::ensure_database_exists(
            &address,
            database,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("converter lifecycle database exists");
    }
    let live_database = std::sync::Arc::new(
        Database::connect_with_options(
            &address,
            &primary,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("converter lifecycle database connects"),
    );
    let replay_database = std::sync::Arc::new(
        Database::connect_with_options(
            &address,
            &replay,
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("converter replay database connects"),
    );

    let workspace = tempfile::tempdir().expect("converter workspace directory");
    let root = workspace.path();
    let archive_directory = root.join("migrations/smoke");
    fs::create_dir_all(root.join("schema/fragments")).expect("schema directory");
    fs::create_dir_all(root.join("migrations/v2")).expect("V2 migration directory");
    fs::create_dir_all(&archive_directory).expect("legacy migration directory");
    write_manifest(
        root,
        &address,
        &http_port,
        &[("live", &primary), ("replay", &replay)],
    );
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
    .expect("base desired schema writes");

    let legacy_schema = "define\nattribute name, value string;\nentity person, owns name;\n";
    let initial_source = r#"from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops


class LegacyInitial(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = []
    operations: ClassVar[list[Operation]] = [
        ops.RunTypeQL(
            forward="define\nattribute name, value string;\nentity person, owns name;\n",
            reverse="undefine\nentity person;\nattribute name;\n",
        ),
    ]
"#;
    let backfill_source = r#"from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation
from type_bridge.migration import operations as ops


def forwards(database):
    return None


class LegacyBackfill(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [("smoke", "0001_initial")]
    operations: ClassVar[list[Operation]] = [ops.RunPython(forwards)]
"#;
    let notes_source =
        "\"\"\"Historical notes retained beside the released migration history.\"\"\"\n";
    let disabled_source = r#"from typing import ClassVar

from type_bridge.migration import Migration
from type_bridge.migration.operations import Operation


class _DisabledMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = []
    operations: ClassVar[list[Operation]] = []
"#;
    fs::write(archive_directory.join("0000_notes.py"), notes_source)
        .expect("ignored notes source writes");
    fs::write(archive_directory.join("0001_initial.py"), initial_source)
        .expect("initial Python migration writes");
    fs::write(archive_directory.join("0002_backfill.py"), backfill_source)
        .expect("RunPython migration writes");
    fs::write(archive_directory.join("0003_disabled.py"), disabled_source)
        .expect("ignored disabled source writes");

    let schema_hash = format!("{:x}", Sha256::digest(legacy_schema.as_bytes()));
    let snapshot = archive_directory.join("snapshots/v0001");
    fs::create_dir_all(&snapshot).expect("legacy snapshot directory writes");
    fs::write(snapshot.join("schema.tql"), legacy_schema).expect("snapshot schema writes");
    fs::write(
        snapshot.join("snapshot.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "version": "v0001",
            "source_migration": "0001_initial",
            "schema_hash": schema_hash,
            "file_hashes": {"schema.tql": schema_hash},
            "type_bridge_version": "1.5.11",
            "type_bridge_core_version": "1.5.11"
        }))
        .expect("snapshot manifest encodes"),
    )
    .expect("snapshot manifest writes");

    let initial_checksum = type_bridge_migration::migration_file_checksum(initial_source);
    let backfill_checksum = type_bridge_migration::migration_file_checksum(backfill_source);
    for database in [&live_database, &replay_database] {
        seed_legacy_ledger(
            database,
            "smoke",
            &[
                ("0001_initial", initial_checksum.as_str()),
                ("0002_backfill", backfill_checksum.as_str()),
            ],
        )
        .await;
        database
            .execute_raw(legacy_schema, TxType::Schema)
            .await
            .expect("legacy schema effect installs");
    }

    // This is the documented product boundary. No Rust test helper constructs
    // either adoption document or the executable sidecar.
    let conversion = run_sidecar_converter(root, Path::new("migrations/smoke"));
    assert_success(&conversion, "shipped Python sidecar conversion");
    assert!(
        conversion.stderr.is_empty(),
        "shipped Python sidecar conversion emitted stderr: {}",
        String::from_utf8_lossy(&conversion.stderr),
    );
    let conversion_stdout = String::from_utf8_lossy(&conversion.stdout);
    for emitted in [
        "0000_notes.adoption.json",
        "0001_initial.adoption.json",
        "0001_initial.json",
        "0002_backfill.adoption.json",
        "0003_disabled.adoption.json",
    ] {
        assert!(
            conversion_stdout.contains(emitted),
            "converter stdout omitted {emitted}: {conversion_stdout}",
        );
        assert!(
            archive_directory.join(emitted).is_file(),
            "missing {emitted}"
        );
    }
    assert!(
        !archive_directory.join("0002_backfill.json").exists(),
        "RunPython must not be represented as a native executable sidecar",
    );
    for ignored_sidecar in ["0000_notes.json", "0003_disabled.json"] {
        assert!(
            !archive_directory.join(ignored_sidecar).exists(),
            "V1-ignored sources must not become native graph nodes",
        );
    }
    let notes_adoption: serde_json::Value = serde_json::from_slice(
        &fs::read(archive_directory.join("0000_notes.adoption.json"))
            .expect("ignored notes metadata reads"),
    )
    .expect("ignored notes metadata parses");
    let initial_adoption: serde_json::Value = serde_json::from_slice(
        &fs::read(archive_directory.join("0001_initial.adoption.json"))
            .expect("initial adoption metadata reads"),
    )
    .expect("initial adoption metadata parses");
    let backfill_adoption: serde_json::Value = serde_json::from_slice(
        &fs::read(archive_directory.join("0002_backfill.adoption.json"))
            .expect("backfill adoption metadata reads"),
    )
    .expect("backfill adoption metadata parses");
    let disabled_adoption: serde_json::Value = serde_json::from_slice(
        &fs::read(archive_directory.join("0003_disabled.adoption.json"))
            .expect("ignored disabled metadata reads"),
    )
    .expect("ignored disabled metadata parses");
    assert_eq!(
        notes_adoption["format"],
        "typebridge.migration-adoption-ignored-source/v1",
    );
    assert_eq!(
        notes_adoption["checksum"],
        type_bridge_migration::migration_file_checksum(notes_source),
    );
    assert_eq!(initial_adoption["checksum"], initial_checksum);
    assert_eq!(initial_adoption["schema_effect"], "snapshot");
    assert_eq!(backfill_adoption["checksum"], backfill_checksum);
    assert_eq!(backfill_adoption["schema_effect"], "unchanged_run_python");
    assert_eq!(
        disabled_adoption["format"],
        "typebridge.migration-adoption-ignored-source/v1",
    );
    assert_eq!(
        disabled_adoption["checksum"],
        type_bridge_migration::migration_file_checksum(disabled_source),
    );

    let repeated_conversion = run_sidecar_converter(root, Path::new("migrations/smoke"));
    assert_success(
        &repeated_conversion,
        "idempotent shipped Python sidecar conversion",
    );
    assert!(
        repeated_conversion.stderr.is_empty(),
        "idempotent shipped Python sidecar conversion emitted stderr: {}",
        String::from_utf8_lossy(&repeated_conversion.stderr),
    );
    assert_eq!(
        String::from_utf8_lossy(&repeated_conversion.stdout).trim(),
        "all migrations carry adoption metadata and every natively executable migration \
         carries an execution sidecar; nothing to do",
    );

    assert_success(
        &run_cli(
            root,
            &[
                "migration",
                "adopt",
                "--environment",
                "live",
                "--archive-directory",
                "migrations/smoke",
            ],
        ),
        "native adoption of converter output",
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "live"]),
        "native post-adoption apply",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "native post-adoption verify",
    );
    assert_success(
        &run_cli(
            root,
            &[
                "migration",
                "adopt",
                "--environment",
                "replay",
                "--archive-directory",
                "migrations/smoke",
            ],
        ),
        "converter replay adoption",
    );

    fs::write(
        root.join("schema/fragments/model.yaml"),
        "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\n  \
         nickname: { value: string }\nentities:\n  person: { owns: [name, nickname] }\n",
    )
    .expect("post-adoption desired schema evolves");
    assert_success(
        &run_cli(root, &["migration", "make", "--name", "converter_nickname"]),
        "post-converter migration make",
    );
    assert!(
        root.join("migrations/v2/0001_converter_nickname.tbmigration.json")
            .is_file(),
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "live"]),
        "post-converter native apply",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "live"]),
        "post-converter native verify",
    );
    assert_success(
        &run_cli(root, &["migration", "apply", "--environment", "replay"]),
        "converter history canonical replay",
    );
    assert_success(
        &run_cli(root, &["migration", "verify", "--environment", "replay"]),
        "converter history replay verify",
    );

    for database in [&live_database, &replay_database] {
        database
            .execute_raw(
                "insert $person isa person, has name \"ada\", has nickname \"lovelace\";",
                TxType::Write,
            )
            .await
            .expect("converted and replayed schema accepts evolved data");
    }

    for database in [&primary, &replay] {
        let journal = Database::connect_with_options(
            &address,
            &format!("{database}__tbv2_journal"),
            &username,
            &password,
            connect_options(&http_port),
        )
        .await
        .expect("converter lifecycle journal connects");
        journal
            .delete_database()
            .await
            .expect("converter lifecycle journal cleanup");
    }
    replay_database
        .delete_database()
        .await
        .expect("converter replay database cleanup");
    live_database
        .delete_database()
        .await
        .expect("converter live database cleanup");
}
