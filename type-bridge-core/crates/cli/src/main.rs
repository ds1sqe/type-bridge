//! `type-bridge` — the V2 workspace command-line interface.
//!
//! `schema check`, `migration make`, and `migration plan` run without any
//! network I/O. `migration apply` and `migration verify` connect through one
//! named workspace environment: credentials stay symbolic environment
//! references resolved only at command time, and application requires the
//! environment's explicit `migrate: true` opt-in.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use type_bridge_schema::SystemSchemaSourceService;
use type_bridge_schema_migration::MigrationGenerationOutcome;
use type_bridge_schema_migration_typedb::execution_capability_vocabulary;
use type_bridge_workspace::{
    ConfigOrigin, ExtensionRegistryService, ExtensionRequirement, SecretReference,
    SecretReferenceService, TypeBridgeConfigSpec, TypeBridgeWorkspace, TypeBridgeWorkspaceServices,
    WorkspaceRoot, WorkspaceServiceError,
};

#[derive(Parser)]
#[command(
    name = "type-bridge",
    version,
    about = "TypeBridge V2 workspace commands"
)]
struct Cli {
    /// Path to the workspace manifest.
    #[arg(long, global = true, default_value = "typebridge.yaml")]
    manifest: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Schema-source commands.
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Canonical migration commands.
    Migration {
        #[command(subcommand)]
        command: MigrationCommand,
    },
}

#[derive(Subcommand)]
enum SchemaCommand {
    /// Parse and resolve the schema sources without network I/O.
    Check,
}

#[derive(Subcommand)]
enum MigrationCommand {
    /// Author the next canonical migration toward the schema sources.
    Make {
        /// Descriptive migration name; the ordinal prefix is allocated.
        #[arg(long)]
        name: String,
    },
    /// Order the committed chain and report each manifest's safety class.
    Plan,
    /// Apply the committed chain to one named environment.
    Apply {
        /// The manifest environment to apply against.
        #[arg(long)]
        environment: String,
        /// Approve one destructive migration by compound id (app/name).
        #[arg(long = "approve")]
        approvals: Vec<String>,
    },
    /// Verify the migration state triad against one named environment.
    Verify {
        /// The manifest environment to verify against.
        #[arg(long)]
        environment: String,
    },
    /// Adopt a completed legacy (v1) history as the canonical genesis.
    Adopt {
        /// The manifest environment holding the migrated v1 database.
        #[arg(long)]
        environment: String,
        /// Directory containing the completed legacy migration files.
        #[arg(long)]
        legacy_directory: PathBuf,
        /// Migration name recorded for the zero-operation bridge manifest.
        #[arg(long, default_value = "0000_legacy_frontier")]
        name: String,
    },
}

/// Symbolic secret references stay unresolved during offline commands.
struct DeferSecrets;

impl SecretReferenceService for DeferSecrets {
    fn validate_reference(
        &self,
        _reference: &SecretReference,
    ) -> Result<(), WorkspaceServiceError> {
        Ok(())
    }
}

/// No extension handlers ship with the CLI yet; requirements fail closed.
struct NoExtensions;

impl ExtensionRegistryService for NoExtensions {
    fn validate_requirement(
        &self,
        _requirement: &ExtensionRequirement,
    ) -> Result<(), WorkspaceServiceError> {
        Err(WorkspaceServiceError::new(
            "extension_handlers_unavailable_in_cli",
        ))
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let workspace = load_workspace(&cli.manifest)?;
    match &cli.command {
        Command::Schema {
            command: SchemaCommand::Check,
        } => {
            println!(
                "schema sources are valid\n  declared identity: {}\n  managed semantics: {}",
                workspace
                    .declared_schema()
                    .declared_identity_fingerprint()
                    .as_fingerprint()
                    .digest()
                    .to_hex(),
                workspace
                    .managed_state()
                    .managed_semantic_schema()
                    .as_fingerprint()
                    .digest()
                    .to_hex(),
            );
            Ok(())
        }
        Command::Migration { command } => match command {
            MigrationCommand::Make { name } => {
                match workspace.migration_make(name).map_err(display)? {
                    MigrationGenerationOutcome::UpToDate => {
                        println!("history already reaches the desired schema");
                    }
                    MigrationGenerationOutcome::Generated(generated) => {
                        let path = workspace
                            .write_generated_migration(&generated)
                            .map_err(display)?;
                        println!(
                            "wrote {}\n  safety: {:?}\n  preview: {}",
                            path.display(),
                            generated.manifest().safety(),
                            path.with_file_name(generated.preview_file_name()).display(),
                        );
                    }
                }
                Ok(())
            }
            MigrationCommand::Apply {
                environment,
                approvals,
            } => run_connected(
                &workspace,
                environment,
                ConnectedAction::Apply {
                    approvals: approvals.clone(),
                },
            ),
            MigrationCommand::Verify { environment } => {
                run_connected(&workspace, environment, ConnectedAction::Verify)
            }
            MigrationCommand::Adopt {
                environment,
                legacy_directory,
                name,
            } => run_connected(
                &workspace,
                environment,
                ConnectedAction::Adopt {
                    legacy_directory: legacy_directory.clone(),
                    name: name.clone(),
                },
            ),
            MigrationCommand::Plan => {
                let plan = workspace
                    .migration_plan(&BTreeSet::new())
                    .map_err(display)?;
                if plan.is_empty() {
                    println!("no committed migrations");
                    return Ok(());
                }
                for entry in plan {
                    println!(
                        "{}/{}  safety={:?}  reversible={}",
                        entry.id().app_label().as_str(),
                        entry.id().name().as_str(),
                        entry.safety(),
                        entry.reversible(),
                    );
                }
                Ok(())
            }
        },
    }
}

fn load_workspace(manifest: &PathBuf) -> Result<TypeBridgeWorkspace, String> {
    let manifest = fs::canonicalize(manifest)
        .map_err(|error| format!("cannot resolve {}: {error}", manifest.display()))?;
    let root = manifest
        .parent()
        .ok_or_else(|| "workspace manifest has no parent directory".to_owned())?;
    let file_name = manifest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "workspace manifest has no UTF-8 file name".to_owned())?;
    let root = WorkspaceRoot::new(root).map_err(display)?;
    let origin = ConfigOrigin::new(root, file_name, "type-bridge cli").map_err(display)?;
    let bytes = fs::read(&manifest)
        .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
    let located = TypeBridgeConfigSpec::from_yaml_bytes(&bytes, origin).map_err(display)?;

    let available = execution_capability_vocabulary().map_err(display)?;
    let source = SystemSchemaSourceService;
    let secrets = DeferSecrets;
    let extensions = NoExtensions;
    let services = TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available);
    // Services borrow locally, so the workspace is constructed in this scope.
    TypeBridgeWorkspace::from_located_config(located, &services).map_err(display)
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

enum ConnectedAction {
    Apply {
        approvals: Vec<String>,
    },
    Verify,
    Adopt {
        legacy_directory: PathBuf,
        name: String,
    },
}

fn run_connected(
    workspace: &TypeBridgeWorkspace,
    environment: &str,
    action: ConnectedAction,
) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| format!("cannot start the async runtime: {error}"))?;
    runtime.block_on(run_connected_async(workspace, environment, action))
}

async fn run_connected_async(
    workspace: &TypeBridgeWorkspace,
    environment_name: &str,
    action: ConnectedAction,
) -> Result<(), String> {
    let config = workspace.config();
    let Some(environment) = config.environment(environment_name) else {
        let known = config
            .environments()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "unknown environment {environment_name:?}; the manifest declares: [{known}]"
        ));
    };
    if matches!(
        action,
        ConnectedAction::Apply { .. } | ConnectedAction::Adopt { .. }
    ) && !environment.migrate()
    {
        return Err(format!(
            "environment {environment_name:?} is not opted into migration \
             application; set `migrate: true` in the manifest to allow it"
        ));
    }
    environment
        .requirements()
        .ensure_supported_by(&execution_capability_vocabulary().map_err(display)?)
        .map_err(display)?;

    let username = resolve_credential(environment.username())?;
    let password = resolve_credential(environment.password())?;
    let mut options = type_bridge_orm::ConnectOptions::default();
    if let Some(port) = environment.http_port() {
        options.http_port = port;
    }
    let journal_name =
        type_bridge_schema_migration_typedb::derived_journal_database_name(environment.database());
    // `verify` is observational: it must never create the managed or
    // journal database (a typoed environment name would otherwise
    // materialize two databases). `adopt` requires the migrated v1 managed
    // database to already exist — bootstrapping an empty one would
    // guarantee a broken adoption — while its journal companion is new by
    // definition. Only migration-gated actions may bootstrap anything.
    for database in [environment.database(), journal_name.as_str()] {
        let requires_existing = match &action {
            ConnectedAction::Verify => Some(
                "`migration verify` is read-only and never creates databases \
                 — apply migrations to this environment first",
            ),
            ConnectedAction::Adopt { .. } if database == environment.database() => Some(
                "`migration adopt` cutover requires the migrated v1 \
                     database to already exist",
            ),
            ConnectedAction::Apply { .. } | ConnectedAction::Adopt { .. } => None,
        };
        if let Some(reason) = requires_existing {
            let exists = type_bridge_orm::database_exists(
                environment.uri(),
                database,
                &username,
                &password,
                options,
            )
            .await
            .map_err(|error| format!("cannot check database {database:?}: {error}"))?;
            if !exists {
                return Err(format!("database {database:?} does not exist; {reason}"));
            }
        } else {
            type_bridge_orm::ensure_database_exists(
                environment.uri(),
                database,
                &username,
                &password,
                options,
            )
            .await
            .map_err(|error| format!("cannot ensure database {database:?}: {error}"))?;
        }
    }
    let managed = std::sync::Arc::new(
        type_bridge_orm::Database::connect_with_options(
            environment.uri(),
            environment.database(),
            &username,
            &password,
            options,
        )
        .await
        .map_err(|error| format!("cannot connect the managed database: {error}"))?,
    );
    let journal = std::sync::Arc::new(
        type_bridge_orm::Database::connect_with_options(
            environment.uri(),
            &journal_name,
            &username,
            &password,
            options,
        )
        .await
        .map_err(|error| format!("cannot connect the journal database: {error}"))?,
    );

    // Adoption records the pre-adoption managed export as the durable
    // genesis artifact before genesis resolution runs; every action then
    // resolves the workspace genesis the same way — the adopted head when
    // the artifact exists, the empty schema otherwise.
    let adopted_artifact_created = if matches!(action, ConnectedAction::Adopt { .. }) {
        ensure_adopted_genesis_artifact(workspace, &managed).await?
    } else {
        false
    };
    let genesis = workspace.migration_genesis().map_err(display)?;
    let lowering = type_bridge_schema_migration::SchemaLoweringBinding::current(
        workspace.delta_context().available_capabilities().clone(),
    )
    .map_err(display)?;
    let runner = type_bridge_schema_migration_typedb::TypeDbMigrationRunner::new(
        managed,
        journal,
        genesis.clone(),
        workspace.delta_context().clone(),
        lowering,
        config.migration_policy().clone(),
    );
    let holder =
        type_bridge_schema_migration::LeaseHolderId::new("type-bridge-cli").map_err(display)?;
    let directory = workspace.migration_directory_absolute_path();

    match action {
        ConnectedAction::Apply { approvals } => {
            let approvals = bind_approvals(&runner, &directory, &approvals)?;
            let outcome = runner
                .apply(
                    &directory,
                    &type_bridge_schema_migration::MigrationApplyTarget::DefaultHead,
                    &holder,
                    &approvals,
                )
                .await
                .map_err(display)?;
            match outcome {
                type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::UpToDate => {
                    println!("applied ledger already reaches the committed head");
                    Ok(())
                }
                type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::Executed(
                    type_bridge_schema_migration::MigrationExecutionOutcome::Applied,
                ) => {
                    println!("applied the committed chain");
                    Ok(())
                }
                type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::Executed(
                    outcome,
                ) => Err(format!("apply did not complete: {outcome:?}")),
            }
        }
        ConnectedAction::Verify => {
            let report = runner
                .verify(&directory, Some(workspace.declared_schema()))
                .await
                .map_err(display)?;
            if report.is_clean() {
                println!(
                    "migration state is coherent\n  applied frontier: {}",
                    report
                        .applied_frontier()
                        .iter()
                        .map(|id| format!("{}/{}", id.app_label().as_str(), id.name().as_str()))
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                Ok(())
            } else {
                for finding in report.findings() {
                    eprintln!("drift: {finding:?}");
                }
                Err(format!("{} drift finding(s)", report.findings().len()))
            }
        }
        ConnectedAction::Adopt {
            legacy_directory,
            name,
        } => {
            let bridge_path = directory.join(format!("{name}.tbmigration.json"));
            let bridge_created = if bridge_path.exists() {
                false
            } else {
                match author_bridge_manifest(
                    workspace,
                    &legacy_directory,
                    &name,
                    &genesis,
                    &bridge_path,
                ) {
                    Ok(()) => true,
                    Err(error) => {
                        cleanup_adoption_files(
                            adopted_artifact_created,
                            workspace,
                            false,
                            &bridge_path,
                        );
                        return Err(error);
                    }
                }
            };
            let outcome = runner
                .import_legacy_frontier(&legacy_directory, &directory, &holder)
                .await;
            match outcome {
                Ok(
                    type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::UpToDate,
                ) => {
                    println!("legacy history is already adopted; the bridged ledger is current");
                    Ok(())
                }
                Ok(
                    type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::Executed(
                        type_bridge_schema_migration::MigrationExecutionOutcome::Applied,
                    ),
                ) => {
                    println!(
                        "adopted the legacy history\n  genesis: {}\n  bridge: {}",
                        workspace.adopted_genesis_absolute_path().display(),
                        bridge_path.display(),
                    );
                    Ok(())
                }
                Ok(
                    type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::Executed(
                        outcome,
                    ),
                ) => {
                    cleanup_adoption_files(
                        adopted_artifact_created,
                        workspace,
                        bridge_created,
                        &bridge_path,
                    );
                    Err(format!("adoption checkpoint did not complete: {outcome:?}"))
                }
                Err(error) => {
                    cleanup_adoption_files(
                        adopted_artifact_created,
                        workspace,
                        bridge_created,
                        &bridge_path,
                    );
                    Err(display(error))
                }
            }
        }
    }
}

/// Record the pre-adoption managed export as the durable genesis artifact.
///
/// The export is validated through the same parse the workspace genesis
/// resolution uses before a byte is written, so an already-adopted or
/// otherwise contaminated database fails closed here instead of persisting
/// an artifact the workspace would later reject. Returns whether this call
/// created the artifact.
async fn ensure_adopted_genesis_artifact(
    workspace: &TypeBridgeWorkspace,
    managed: &type_bridge_orm::Database,
) -> Result<bool, String> {
    let path = workspace.adopted_genesis_absolute_path();
    if path.exists() {
        return Ok(false);
    }
    let export = managed
        .schema_text()
        .await
        .map_err(|error| format!("cannot export the managed schema: {error}"))?;
    let document = type_bridge_contract::schema::DocumentId::new(
        type_bridge_schema_compat::ADOPTED_GENESIS_FILE_NAME,
    )
    .map_err(display)?;
    type_bridge_schema_compat::parse_adopted_genesis(document, &export).map_err(display)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create the migration directory: {error}"))?;
    }
    write_exclusive(&path, export.as_bytes())?;
    Ok(true)
}

/// Author and persist the zero-operation legacy-frontier bridge manifest.
fn author_bridge_manifest(
    workspace: &TypeBridgeWorkspace,
    legacy_directory: &std::path::Path,
    name: &str,
    genesis: &type_bridge_contract::schema::DeclaredSchema,
    bridge_path: &std::path::Path,
) -> Result<(), String> {
    let legacy_graph =
        type_bridge_migration::load_dir_checked(legacy_directory).map_err(|error| {
            format!("legacy migration directory failed the checked v1 loader: {error}")
        })?;
    let frontier = type_bridge_schema_migration_typedb::extract_legacy_frontier(&legacy_graph)
        .map_err(display)?;
    let id = type_bridge_contract::migration::MigrationId::from_components(
        type_bridge_contract::migration::MigrationAppLabel::new(
            workspace.config().app_label().as_str().to_owned(),
        )
        .map_err(display)?,
        type_bridge_contract::migration::MigrationName::new(name.to_owned()).map_err(display)?,
    );
    let bridge = type_bridge_schema_migration::build_legacy_frontier_bridge(
        id,
        frontier,
        genesis,
        workspace.delta_context(),
    )
    .map_err(display)?;
    let bytes = type_bridge_schema_migration::encode_verified_manifest(&bridge).map_err(display)?;
    write_exclusive(bridge_path, &bytes)
}

fn write_exclusive(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

/// Best-effort removal of files this adoption run created before it failed.
///
/// Pre-existing files are never touched: a re-run over an already-adopted
/// workspace must not delete the durable artifacts a previous successful
/// adoption recorded.
fn cleanup_adoption_files(
    artifact_created: bool,
    workspace: &TypeBridgeWorkspace,
    bridge_created: bool,
    bridge_path: &std::path::Path,
) {
    if bridge_created {
        let _ = fs::remove_file(bridge_path);
    }
    if artifact_created {
        let _ = fs::remove_file(workspace.adopted_genesis_absolute_path());
    }
}

fn resolve_credential(
    reference: &type_bridge_workspace::SecretReference,
) -> Result<String, String> {
    std::env::var(reference.environment_variable()).map_err(|_| {
        format!(
            "credential environment variable {:?} is not set",
            reference.environment_variable()
        )
    })
}

fn bind_approvals(
    runner: &type_bridge_schema_migration_typedb::TypeDbMigrationRunner,
    directory: &std::path::Path,
    approvals: &[String],
) -> Result<Vec<type_bridge_schema_migration::MigrationApplyApproval>, String> {
    if approvals.is_empty() {
        return Ok(Vec::new());
    }
    let graph = runner.discover(directory).map_err(display)?;
    approvals
        .iter()
        .map(|compound| {
            let (app_label, name) = compound
                .split_once('/')
                .ok_or_else(|| format!("approval {compound:?} must be app-label/name"))?;
            let id = type_bridge_contract::migration::MigrationId::from_components(
                type_bridge_contract::migration::MigrationAppLabel::new(app_label.to_owned())
                    .map_err(display)?,
                type_bridge_contract::migration::MigrationName::new(name.to_owned())
                    .map_err(display)?,
            );
            let manifest = graph.manifest(&id).ok_or_else(|| {
                format!("approval target {compound:?} is not in the committed history")
            })?;
            type_bridge_schema_migration::MigrationApplyApproval::for_manifest(manifest)
                .map_err(display)
        })
        .collect()
}
