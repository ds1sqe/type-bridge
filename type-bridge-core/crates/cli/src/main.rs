//! `type-bridge` — the V2 workspace command-line interface.
//!
//! Offline commands only: `schema check` parses and resolves without network
//! I/O, `migration make` authors against the committed head, and
//! `migration plan` orders the chain and names each manifest's verified
//! safety class. Database-bearing commands (`migration apply`,
//! `migration verify --environment`) arrive with workspace environments;
//! until then their library equivalents live on `TypeDbMigrationRunner`.

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
    SecretReferenceService, TypeBridgeConfigSpec, TypeBridgeWorkspace,
    TypeBridgeWorkspaceServices, WorkspaceRoot, WorkspaceServiceError,
};

#[derive(Parser)]
#[command(name = "type-bridge", version, about = "TypeBridge V2 workspace commands")]
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
        Command::Schema { command: SchemaCommand::Check } => {
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
                            path.with_file_name(generated.preview_file_name())
                                .display(),
                        );
                    }
                }
                Ok(())
            }
            MigrationCommand::Plan => {
                let plan =
                    workspace.migration_plan(&BTreeSet::new()).map_err(display)?;
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
    let origin =
        ConfigOrigin::new(root, file_name, "type-bridge cli").map_err(display)?;
    let bytes = fs::read(&manifest)
        .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
    let located = TypeBridgeConfigSpec::from_yaml_bytes(&bytes, origin).map_err(display)?;

    let available = execution_capability_vocabulary().map_err(display)?;
    let source = SystemSchemaSourceService;
    let secrets = DeferSecrets;
    let extensions = NoExtensions;
    let services =
        TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available);
    // Services borrow locally, so the workspace is constructed in this scope.
    TypeBridgeWorkspace::from_located_config(located, &services).map_err(display)
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}
