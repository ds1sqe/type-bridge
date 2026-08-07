//! `type-bridge` — the V2 workspace command-line interface.
//!
//! `schema check`, `schema generate`, `migration make`, and `migration
//! plan` run without any network I/O. `migration apply`, `migration
//! verify`, and `migration adopt` connect through one named workspace
//! environment: credentials stay symbolic environment references resolved
//! only at command time, and application requires the environment's
//! explicit `migrate: true` opt-in.
//!
//! The crate is a library plus a thin binary so the exact same command
//! surface ships both as the standalone `type-bridge` executable and
//! in-process inside the Python wheel via [`run_cli`].

#![deny(missing_docs)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

use clap::{Parser, Subcommand};
#[cfg(test)]
use type_bridge_schema::SystemSchemaSourceService;
use type_bridge_schema_migration::MigrationGenerationOutcome;
use type_bridge_schema_migration_typedb::execution_capability_vocabulary;
use type_bridge_workspace::{
    ConfigOrigin, ExtensionRegistryService, ExtensionRequirement, SecretReference,
    SecretReferenceService, TypeBridgeConfigSpec, TypeBridgeWorkspace, TypeBridgeWorkspaceServices,
    WorkspaceDirectoryAuthority, WorkspaceEnvironment, WorkspaceRoot, WorkspaceServiceError,
    WorkspaceTransportPolicy,
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
    /// Generate the configured binding projections from the canonical schema.
    Generate,
    /// Export canonical declared-schema bytes for low-level V2 tooling.
    ExportDeclared {
        /// Workspace-relative destination for the canonical JSON artifact.
        #[arg(long, default_value = "declared-schema.json")]
        output: PathBuf,
    },
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
    /// Adopt a completed archived V1 history as the canonical genesis.
    Adopt {
        /// The manifest environment holding the migrated v1 database.
        #[arg(long)]
        environment: String,
        /// Directory containing the completed archived migration files.
        #[arg(long)]
        archive_directory: PathBuf,
        /// Migration name recorded for the zero-operation bridge manifest.
        #[arg(long, default_value = "0000_archive_frontier")]
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

/// Run the CLI over process-style arguments (`argv[0]` included).
///
/// Returns the process exit code. All output goes to the process stdout
/// and stderr exactly as the standalone binary would print it: `--help`
/// and `--version` exit 0, argument errors exit 2, command failures
/// print `error: ...` and exit 1.
pub fn run_cli<I, T>(arguments: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(arguments) {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return if error.use_stderr() { 2 } else { 0 };
        }
    };
    match run(&cli) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("error: {message}");
            1
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
        Command::Schema {
            command: SchemaCommand::Generate,
        } => run_schema_generate(&workspace),
        Command::Schema {
            command: SchemaCommand::ExportDeclared { output },
        } => run_schema_export_declared(&workspace, output),
        Command::Migration { command } => match command {
            MigrationCommand::Make { name } => {
                let directory = workspace.open_migration_directory().map_err(display)?;
                match workspace
                    .migration_make_in(&directory, name)
                    .map_err(display)?
                {
                    MigrationGenerationOutcome::UpToDate => {
                        println!("history already reaches the desired schema");
                    }
                    MigrationGenerationOutcome::Generated(generated) => {
                        workspace
                            .write_generated_migration_in(&directory, &generated)
                            .map_err(display)?;
                        let path = directory.display_path().join(generated.file_name());
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
                archive_directory,
                name,
            } => run_connected(
                &workspace,
                environment,
                ConnectedAction::Adopt {
                    archive_directory: archive_directory.clone(),
                    name: name.clone(),
                },
            ),
            MigrationCommand::Plan => {
                let directory = workspace.open_migration_directory().map_err(display)?;
                let plan = workspace
                    .migration_plan_in(&directory, &BTreeSet::new())
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
    let source = WorkspaceDirectoryAuthority::open(root.clone()).map_err(display)?;
    let origin = ConfigOrigin::new(root, file_name, "type-bridge cli").map_err(display)?;
    // Read at most the canonical document ceiling plus one byte: an
    // oversized manifest fails with a stable message before its full
    // content is ever allocated.
    let limit = type_bridge_contract::limits::MAX_CANONICAL_BYTES;
    let captured = source
        .capture_relative_file(Path::new(file_name), limit)
        .map_err(|error| format!("cannot read {}: {error}", manifest.display()))?;
    let bytes = captured.bytes();
    if bytes.len() > limit {
        return Err(format!(
            "{} exceeds the 16 MiB manifest ceiling",
            manifest.display()
        ));
    }
    let located = TypeBridgeConfigSpec::from_yaml_bytes(bytes, origin).map_err(display)?;

    let available = execution_capability_vocabulary().map_err(display)?;
    let secrets = DeferSecrets;
    let extensions = NoExtensions;
    let services = TypeBridgeWorkspaceServices::new(&source, &secrets, &extensions, &available);
    // Services borrow locally, so the workspace is constructed in this scope.
    TypeBridgeWorkspace::from_located_config(located, &services).map_err(display)
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Render one secure lifecycle failure after credentials have been resolved.
///
/// Provider errors are allowed to echo request metadata, including
/// credentials. Retain only the runtime's structurally credential-safe
/// TLS/version projection; every other error collapses to an operation code
/// and drops its source.
fn sanitize_connected_error(
    context: String,
    code: &'static str,
    error: type_bridge_orm::SecureConnectError,
) -> String {
    match error.credential_safe_diagnostic() {
        Some(diagnostic) => format!("{context}: {diagnostic}"),
        None => format!("{context} [{code}]; inspect provider logs"),
    }
}

/// Schema export happens after credentials have been resolved, so no raw ORM
/// error or source chain may cross the CLI boundary.
fn sanitize_schema_export_error(_error: type_bridge_orm::OrmError) -> String {
    "cannot export the managed schema [typedb_schema_export_failed]; inspect provider logs"
        .to_owned()
}

/// Render a non-success migration outcome without exposing diagnostic details.
///
/// TypeDB adapters retain provider text in `Diagnostic` details for trusted
/// programmatic inspection. Its `Display` surface intentionally omits those
/// details, while derived `Debug` includes them, so every post-credential CLI
/// path must use this closed projection.
fn sanitize_migration_execution_outcome(
    context: &str,
    outcome: type_bridge_schema_migration::MigrationExecutionOutcome,
) -> String {
    use type_bridge_schema_migration::{
        MigrationExecutionOutcome as Outcome, MigrationExecutionPosition as Position,
    };

    let render_position = |position| match position {
        Position::TransactionGroup(ordinal) => format!("transaction group {ordinal}"),
        Position::ManifestCheckpoint => "manifest checkpoint".to_owned(),
    };
    match outcome {
        Outcome::Applied => format!("{context}: applied"),
        Outcome::RetrySafe {
            migration_id,
            position,
            diagnostic,
        } => format!(
            "{context}: retry-safe at {}/{} ({position}): {diagnostic}",
            migration_id.app_label().as_str(),
            migration_id.name().as_str(),
            position = render_position(position),
        ),
        Outcome::RequiresExplicitRecovery {
            migration_id,
            position,
            diagnostic,
        } => format!(
            "{context}: explicit recovery required at {}/{} ({position}): {diagnostic}",
            migration_id.app_label().as_str(),
            migration_id.name().as_str(),
            position = render_position(position),
        ),
    }
}

/// Generate every configured binding projection from the canonical schema.
///
/// The resolved workspace schema is projected per configured target with
/// each shipped emitter's handler and code-resource evidence — the same
/// path the codegen acceptance fixtures pin — and emitted
/// deterministically. The complete generation is prepared beneath the
/// retained workspace authority and committed as one rollback-verified batch,
/// with schema authority last; files not produced by an emitter are never
/// touched or deleted.
fn run_schema_generate(workspace: &TypeBridgeWorkspace) -> Result<(), String> {
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig};
    use type_bridge_schema::{build_schema_authority, encode_schema_authority, project};
    use type_bridge_schema_codegen::{PythonEmitter, RustEmitter, TypeScriptEmitter};

    let outputs = workspace.config().outputs();
    let authority_output = workspace.config().schema_authority_output();
    if outputs.is_empty() && authority_output.is_none() {
        return Err(
            "no generated outputs configured; add bindings.<target>.output or \
             artifacts.schema-authority.output to the manifest"
                .into(),
        );
    }

    let resolved = workspace.resolved_schema();
    let authority = build_schema_authority(
        workspace.declared_schema(),
        workspace.required_capabilities(),
        workspace.delta_context(),
    )
    .map_err(display)?;
    let authority_bytes = encode_schema_authority(&authority);

    // Finish every pure projection before mutating any output. A target-level
    // generation failure therefore cannot publish an earlier language from a
    // different semantic attempt.
    let mut packages = Vec::with_capacity(outputs.len());
    for (&target, directory) in outputs {
        let package = match target {
            BindingTarget::Python => {
                let emitter = PythonEmitter::new();
                let projection = project(
                    resolved,
                    BindingTarget::Python,
                    &ProjectionConfig::python(),
                    &emitter.generator_handlers(),
                    &emitter.code_resources().map_err(display)?,
                )
                .map_err(display)?;
                emitter.emit(&projection, &authority)
            }
            BindingTarget::TypeScript => {
                let emitter = TypeScriptEmitter::new();
                let projection = project(
                    resolved,
                    BindingTarget::TypeScript,
                    &ProjectionConfig::typescript(),
                    &emitter.generator_handlers(),
                    &emitter.code_resources().map_err(display)?,
                )
                .map_err(display)?;
                emitter.emit(&projection, &authority)
            }
            BindingTarget::Rust => {
                let emitter = RustEmitter::new();
                let projection = project(
                    resolved,
                    BindingTarget::Rust,
                    &ProjectionConfig::rust(),
                    &emitter.generator_handlers(),
                    &emitter.code_resources().map_err(display)?,
                )
                .map_err(display)?;
                emitter.emit(&projection, &authority)
            }
        }
        .map_err(display)?;
        packages.push((target, directory, package));
    }

    // Build one workspace-relative batch without touching the filesystem. The
    // workspace authority prevalidates every destination and prepares every
    // flushed same-directory temporary before it publishes in this order. The
    // final server authority is deliberately appended last.
    let workspace_root = workspace.output_root()?;
    let mut generated_files = Vec::new();
    let mut generated_packages = Vec::with_capacity(packages.len());
    for (target, directory, package) in &packages {
        let display_root = workspace_root.display_path().join(directory.as_path());
        let file_count = package.files().len();
        for (path, bytes) in package.files() {
            let relative = std::path::Path::new(path);
            validate_generated_relative_path(relative)?;
            generated_files.push((directory.as_path().join(relative), bytes.as_slice()));
        }
        generated_packages.push((*target, display_root, file_count));
    }
    let prepared_authority = authority_output
        .map(|output| {
            let path = output.as_path();
            let _file_name = path
                .file_name()
                .ok_or_else(|| "schema-authority output has no file name".to_owned())?;
            Ok::<_, String>(path.to_path_buf())
        })
        .transpose()?;

    if let Some(relative) = &prepared_authority {
        generated_files.push((relative.clone(), authority_bytes.as_slice()));
    }
    workspace_root.write_atomic_batch(
        generated_files
            .iter()
            .map(|(path, bytes)| (path.as_path(), *bytes)),
    )?;

    for (target, display_root, file_count) in generated_packages {
        println!(
            "generated {} file(s) for {} into {}",
            file_count,
            match target {
                BindingTarget::Python => "python",
                BindingTarget::TypeScript => "typescript",
                BindingTarget::Rust => "rust",
            },
            display_root.display(),
        );
    }
    if let Some(relative) = prepared_authority {
        println!(
            "generated schema authority at {}\n  authority identity: {}",
            workspace_root.display_path().join(relative).display(),
            authority.authority_fingerprint().digest().to_hex(),
        );
    }
    Ok(())
}

/// Export canonical declared bytes for explicitly low-level V2 tooling.
fn run_schema_export_declared(
    workspace: &TypeBridgeWorkspace,
    output: &Path,
) -> Result<(), String> {
    use type_bridge_contract::schema::encode_declared_schema;

    validate_declared_output_path(output)?;
    let root = workspace.output_root()?;
    let parent = root.open_beneath(output.parent().unwrap_or_else(|| std::path::Path::new("")))?;
    let file_name = output
        .file_name()
        .ok_or_else(|| "declared-schema output has no file name".to_owned())?;
    let destination = parent.display_path().join(file_name);
    let bytes = encode_declared_schema(workspace.declared_schema()).map_err(display)?;
    parent.write_atomic(file_name, &bytes)?;
    println!(
        "wrote canonical declared schema to {}\n  declared identity: {}",
        destination.display(),
        workspace
            .declared_schema()
            .declared_identity_fingerprint()
            .as_fingerprint()
            .digest()
            .to_hex(),
    );
    Ok(())
}

fn validate_declared_output_path(output: &Path) -> Result<(), String> {
    let Some(portable) = output.to_str() else {
        return Err("declared-schema output must be valid UTF-8".into());
    };
    let invalid_spelling = portable.is_empty()
        || portable.contains(['\\', ':', '\0'])
        || portable.bytes().any(|byte| byte.is_ascii_control())
        || portable
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."));
    let invalid_components = output.is_absolute()
        || output
            .components()
            .any(|component| !matches!(component, Component::Normal(_)));
    if invalid_spelling || invalid_components {
        return Err("declared-schema output must be a confined portable workspace path".into());
    }
    if output.extension().and_then(|extension| extension.to_str()) != Some("json") {
        return Err("declared-schema output must end in lowercase .json".into());
    }
    Ok(())
}

fn validate_generated_relative_path(path: &std::path::Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "generated output path {:?} is not a confined relative file",
            path
        ));
    }
    Ok(())
}

enum ConnectedAction {
    Apply {
        approvals: Vec<String>,
    },
    Verify,
    Adopt {
        archive_directory: PathBuf,
        name: String,
    },
}

fn secure_connect_options(
    environment: &WorkspaceEnvironment,
) -> type_bridge_orm::SecureConnectOptions {
    let tls_mode = match environment.transport_policy() {
        WorkspaceTransportPolicy::Disabled => type_bridge_orm::TlsMode::Disabled,
        WorkspaceTransportPolicy::NativeRoots => type_bridge_orm::TlsMode::NativeRoots,
        WorkspaceTransportPolicy::CustomRootCa(root_ca) => {
            type_bridge_orm::TlsMode::CustomRootCa(root_ca.as_path().to_path_buf())
        }
    };
    let mut options = type_bridge_orm::SecureConnectOptions {
        tls_mode,
        ..type_bridge_orm::SecureConnectOptions::default()
    };
    if let Some(port) = environment.http_port() {
        options.http_port = port;
    }
    options
}

fn preflight_secure_connect_options(
    workspace: &TypeBridgeWorkspace,
    environment_name: &str,
) -> Result<type_bridge_orm::PreparedSecureConnectOptions, String> {
    let environment = workspace
        .config()
        .environment(environment_name)
        .ok_or_else(|| {
            format!("environment {environment_name:?} is not owned by this workspace")
        })?;
    let options = secure_connect_options(environment);
    match workspace
        .capture_environment_custom_root_ca(environment_name)
        .map_err(display)?
    {
        Some(bytes) => options
            .prepare_transport_from_captured_custom_root(bytes)
            .map_err(display),
        None => options.prepare_transport().map_err(display),
    }
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
        &action,
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

    // Validate the name and capture one immutable archive-history authority
    // before creating the canonical directory or resolving credentials.
    let prepared_adoption = match &action {
        ConnectedAction::Adopt {
            archive_directory,
            name,
        } => Some(prepare_archive_adoption(
            workspace,
            archive_directory,
            name,
        )?),
        ConnectedAction::Apply { .. } | ConnectedAction::Verify => None,
    };

    // Retain one descriptor-backed authority for the whole connected action.
    // Adoption alone may create missing real directory components; apply and
    // verify remain fail-closed and non-creating here.
    let migration_directory = if matches!(&action, ConnectedAction::Adopt { .. }) {
        workspace.ensure_migration_directory().map_err(display)?
    } else {
        workspace.open_migration_directory().map_err(display)?
    };
    // Ordinary connected operations reject an incomplete adoption pair before
    // credentials, network I/O, or database creation. Adoption itself is the
    // sole recovery path permitted to observe and complete an exact orphan.
    let ordinary_graph = if prepared_adoption.is_none() {
        Some(
            workspace
                .discover_migrations_in(&migration_directory)
                .map_err(display)?,
        )
    } else {
        None
    };
    // Approval syntax, membership, safety, and digest binding are local
    // authority checks. Resolve them before credentials, network I/O, or
    // database creation; the runner re-discovers and rechecks the bound
    // manifest at execution time.
    let prepared_approvals = match &action {
        ConnectedAction::Apply { approvals } => Some(bind_approvals(
            ordinary_graph
                .as_ref()
                .ok_or_else(|| "internal apply history was not retained".to_owned())?,
            approvals,
        )?),
        ConnectedAction::Verify | ConnectedAction::Adopt { .. } => None,
    };

    // Resolve and snapshot the complete transport policy before reading either
    // credential. Every later lifecycle/connect call clones this prepared
    // handle, so no custom-root path is reopened after secret resolution.
    let options = preflight_secure_connect_options(workspace, environment_name)?;
    let username = resolve_credential(environment.username())?;
    let password = resolve_credential(environment.password())?;
    let journal_name =
        type_bridge_schema_migration_typedb::derived_journal_database_name(environment.database());
    // `verify` is observational: it must never create the managed or
    // journal database (a typoed environment name would otherwise
    // materialize two databases). `adopt` requires the migrated v1 managed
    // database to already exist — bootstrapping an empty one would
    // guarantee a broken adoption — while its journal companion is new by
    // definition. Only migration-gated actions may bootstrap anything.
    let managed_requires_existing = match &action {
        ConnectedAction::Verify => Some(
            "`migration verify` is read-only and never creates databases \
             — apply migrations to this environment first",
        ),
        ConnectedAction::Adopt { .. } => {
            Some("`migration adopt` cutover requires the migrated v1 database to already exist")
        }
        ConnectedAction::Apply { .. } => None,
    };
    if let Some(reason) = managed_requires_existing {
        let exists = type_bridge_orm::database_exists_prepared_secure(
            environment.uri(),
            environment.database(),
            &username,
            &password,
            options.clone(),
        )
        .await
        .map_err(|error| {
            sanitize_connected_error(
                format!("cannot check database {:?}", environment.database()),
                "typedb_database_exists_failed",
                error,
            )
        })?;
        if !exists {
            return Err(format!(
                "database {:?} does not exist; {reason}",
                environment.database()
            ));
        }
    } else {
        type_bridge_orm::ensure_database_exists_prepared_secure(
            environment.uri(),
            environment.database(),
            &username,
            &password,
            options.clone(),
        )
        .await
        .map_err(|error| {
            sanitize_connected_error(
                format!("cannot ensure database {:?}", environment.database()),
                "typedb_database_ensure_failed",
                error,
            )
        })?;
    }
    let managed = std::sync::Arc::new(
        type_bridge_orm::Database::connect_prepared_secure_with_options(
            environment.uri(),
            environment.database(),
            &username,
            &password,
            options.clone(),
        )
        .await
        .map_err(|error| {
            sanitize_connected_error(
                "cannot connect the managed database".to_owned(),
                "typedb_database_connect_failed",
                error,
            )
        })?,
    );

    // Adoption's live-schema comparison and complete pair publication precede
    // journal creation. Publication is bridge-first under the canonical
    // authoring lock, rolls back files created by a failed attempt, and accepts
    // exact orphan pieces so interrupted attempts remain adopt-only resumable.
    let adoption_files = if let Some(prepared) = prepared_adoption.as_ref() {
        verify_prepared_adoption_live(&managed, prepared).await?;
        Some(publish_prepared_adoption(
            workspace,
            &migration_directory,
            prepared,
        )?)
    } else {
        None
    };

    if matches!(&action, ConnectedAction::Verify) {
        let exists = type_bridge_orm::database_exists_prepared_secure(
            environment.uri(),
            &journal_name,
            &username,
            &password,
            options.clone(),
        )
        .await
        .map_err(|error| {
            sanitize_connected_error(
                format!("cannot check database {journal_name:?}"),
                "typedb_database_exists_failed",
                error,
            )
        })?;
        if !exists {
            return Err(format!(
                "database {journal_name:?} does not exist; `migration verify` is read-only and never creates databases"
            ));
        }
    } else {
        type_bridge_orm::ensure_database_exists_prepared_secure(
            environment.uri(),
            &journal_name,
            &username,
            &password,
            options.clone(),
        )
        .await
        .map_err(|error| {
            sanitize_connected_error(
                format!("cannot ensure database {journal_name:?}"),
                "typedb_database_ensure_failed",
                error,
            )
        })?;
    }
    let journal = std::sync::Arc::new(
        type_bridge_orm::Database::connect_prepared_secure_with_options(
            environment.uri(),
            &journal_name,
            &username,
            &password,
            options,
        )
        .await
        .map_err(|error| {
            sanitize_connected_error(
                "cannot connect the journal database".to_owned(),
                "typedb_database_connect_failed",
                error,
            )
        })?,
    );

    let genesis = workspace
        .migration_genesis_in(&migration_directory)
        .map_err(display)?;
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
    let directory = migration_directory.directory();

    match action {
        ConnectedAction::Apply { .. } => {
            let approvals = prepared_approvals
                .as_deref()
                .ok_or_else(|| "internal apply approvals were not retained".to_owned())?;
            let outcome = runner
                .apply_in(
                    directory,
                    &type_bridge_schema_migration::MigrationApplyTarget::DefaultHead,
                    &holder,
                    approvals,
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
                ) => Err(sanitize_migration_execution_outcome(
                    "apply did not complete",
                    outcome,
                )),
            }
        }
        ConnectedAction::Verify => {
            let report = runner
                .verify_in(directory, Some(workspace.declared_schema()))
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
        ConnectedAction::Adopt { .. } => {
            let bridge_display_path = adoption_files
                .ok_or_else(|| "internal adoption preflight state was not retained".to_owned())?;
            let prepared = prepared_adoption
                .as_ref()
                .ok_or_else(|| "internal adoption authority was not retained".to_owned())?;
            let outcome = runner
                .import_verified_legacy_frontier_in(
                    &prepared.history,
                    &prepared.reconstructed,
                    directory,
                    &holder,
                )
                .await;
            match outcome {
                Ok(
                    type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::UpToDate,
                ) => {
                    println!("archive history is already adopted; the bridged ledger is current");
                    Ok(())
                }
                Ok(
                    type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::Executed(
                        type_bridge_schema_migration::MigrationExecutionOutcome::Applied,
                    ),
                ) => {
                    println!(
                        "adopted the archive history\n  genesis: {}\n  bridge: {}",
                        migration_directory
                            .display_path()
                            .join(type_bridge_schema_compat::ADOPTED_GENESIS_FILE_NAME)
                            .display(),
                        bridge_display_path.display(),
                    );
                    Ok(())
                }
                Ok(
                    type_bridge_schema_migration_typedb::MigrationDirectoryApplyOutcome::Executed(
                        outcome,
                    ),
                ) => Err(sanitize_migration_execution_outcome(
                    "adoption checkpoint did not complete",
                    outcome,
                )),
                Err(error) => Err(display(error)),
            }
        }
    }
}

struct PreparedArchiveAdoption {
    history: type_bridge_migration::LegacyAdoptionHistory,
    reconstructed: type_bridge_migration::VerifiedLegacyHead,
    authority: type_bridge_schema_compat::AdoptedGenesisAuthority,
    bridge: type_bridge_schema_migration::VerifiedSchemaMigrationManifest,
    bridge_name: String,
    bridge_bytes: Vec<u8>,
}

/// Validate and derive every filesystem authority from one retained archive
/// history capture. This function performs no canonical-directory writes.
fn prepare_archive_adoption(
    workspace: &TypeBridgeWorkspace,
    archive_directory: &std::path::Path,
    name: &str,
) -> Result<PreparedArchiveAdoption, String> {
    // Validate the caller-controlled name before loading history or creating
    // the configured canonical directory.
    let migration_name =
        type_bridge_contract::migration::MigrationName::new(name.to_owned()).map_err(display)?;
    let bridge_name = format!("{}.tbmigration.json", migration_name.as_str());
    let history =
        type_bridge_migration::load_adoption_history(archive_directory).map_err(|error| {
            format!("archive migration directory failed the checked adoption loader: {error}")
        })?;
    let reconstructed = type_bridge_migration::reconstruct_legacy_head(&history)
        .map_err(|error| format!("archive head reconstruction failed: {error}"))?;
    let authority = type_bridge_schema_compat::parse_adopted_genesis_authority(
        type_bridge_contract::schema::DocumentId::new("legacy-head-snapshot.typeql")
            .map_err(display)?,
        reconstructed.schema_typeql(),
    )
    .map_err(display)?;
    let frontier = type_bridge_schema_migration_typedb::extract_legacy_frontier(history.graph())
        .map_err(display)?;
    let applied_set =
        type_bridge_schema_migration_typedb::extract_legacy_applied_set_digest(history.graph())
            .map_err(display)?;
    let id = type_bridge_contract::migration::MigrationId::from_components(
        type_bridge_contract::migration::MigrationAppLabel::new(
            workspace.config().app_label().as_str().to_owned(),
        )
        .map_err(display)?,
        migration_name,
    );
    let bridge = type_bridge_schema_migration::build_legacy_frontier_bridge(
        id,
        frontier,
        applied_set,
        authority.declared(),
        workspace.delta_context(),
    )
    .map_err(display)?;
    let bridge_bytes =
        type_bridge_schema_migration::encode_verified_manifest(&bridge).map_err(display)?;
    history
        .require_unchanged_head(&reconstructed)
        .map_err(|error| {
            format!("archive migration directory changed during adoption preparation: {error}")
        })?;
    Ok(PreparedArchiveAdoption {
        history,
        reconstructed,
        authority,
        bridge,
        bridge_name,
        bridge_bytes,
    })
}

/// Compare the live managed schema with the prepared immutable head without
/// using live state as publication authority.
async fn verify_prepared_adoption_live(
    managed: &type_bridge_orm::Database,
    prepared: &PreparedArchiveAdoption,
) -> Result<(), String> {
    let export = managed
        .schema_text()
        .await
        .map_err(sanitize_schema_export_error)?;
    prepared
        .history
        .require_unchanged_head(&prepared.reconstructed)
        .map_err(|error| format!("archive adoption history changed during live export: {error}"))?;
    let expected_internal = type_bridge_schema_compat::released_typeql_to_declared_projection(
        type_bridge_contract::schema::DocumentId::new("managed-fence-schema.typeql")
            .map_err(display)?,
        type_bridge_schema_migration_typedb::MANAGED_FENCE_SCHEMA_TYPEQL,
    )
    .map_err(display)?;
    let live = type_bridge_schema_compat::parse_adopted_genesis_authority_with_internal(
        type_bridge_contract::schema::DocumentId::new("legacy-live-head.typeql")
            .map_err(display)?,
        &export,
        Some(&expected_internal),
    )
    .map_err(display)?;
    if live.legacy_identity() != prepared.authority.legacy_identity()
        || live.declared().declared_identity_fingerprint()
            != prepared
                .authority
                .declared()
                .declared_identity_fingerprint()
        || live.released_extension_identity() != prepared.authority.released_extension_identity()
    {
        return Err(
            "live managed schema differs from the independently verified archive-head snapshot"
                .to_owned(),
        );
    }
    Ok(())
}

/// Publish the bridge/genesis pair under the shared authoring lock.
///
/// The prospective complete graph is replay-verified before publication. The
/// bridge is made visible first, so ordinary readers fail closed during the
/// short incomplete interval. Exact pre-existing orphan pieces are retained
/// and completed; only files created by this attempt are rolled back.
fn publish_prepared_adoption(
    workspace: &TypeBridgeWorkspace,
    migration_directory: &type_bridge_workspace::MigrationDirectoryAuthority,
    prepared: &PreparedArchiveAdoption,
) -> Result<PathBuf, String> {
    publish_prepared_adoption_with_after_bridge(workspace, migration_directory, prepared, || {})
}

fn publish_prepared_adoption_with_after_bridge<F>(
    workspace: &TypeBridgeWorkspace,
    migration_directory: &type_bridge_workspace::MigrationDirectoryAuthority,
    prepared: &PreparedArchiveAdoption,
    after_bridge: F,
) -> Result<PathBuf, String>
where
    F: FnOnce(),
{
    let directory = migration_directory.directory();
    let _lock = directory.try_acquire_authoring_lock().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            "migration adoption conflicts with another canonical history publisher".to_owned()
        } else {
            format!("cannot lock canonical migration publication: {error}")
        }
    })?;
    let genesis_name = type_bridge_schema_compat::ADOPTED_GENESIS_FILE_NAME;
    let genesis_bytes = prepared.reconstructed.schema_typeql().as_bytes();
    let mut bridge_created = false;
    let mut genesis_created = false;
    let mut after_bridge = Some(after_bridge);

    let publication = (|| -> Result<(), String> {
        if let Some(existing) = read_existing_authority(directory, genesis_name)?
            && existing != genesis_bytes
        {
            return Err(format!(
                "{genesis_name} already exists but differs from the verified archive-head snapshot"
            ));
        }
        let bridge_already_published =
            if let Some(existing) = read_existing_authority(directory, &prepared.bridge_name)? {
                if existing != prepared.bridge_bytes {
                    return Err(format!(
                        "{} already exists with different authority bytes",
                        prepared.bridge_name
                    ));
                }
                true
            } else {
                false
            };

        let (current, evidence) =
            type_bridge_schema_migration::discover_verified_migration_chain_with_evidence_in(
                directory,
                prepared.authority.declared(),
                workspace.delta_context(),
            )
            .map_err(display)?;
        let prospective = if current.manifest(prepared.bridge.id()).is_some() {
            current
        } else {
            let manifests = current
                .manifests()
                .map(|(_, manifest)| manifest.clone())
                .chain(std::iter::once(prepared.bridge.clone()))
                .collect::<Vec<_>>();
            type_bridge_schema_migration::MigrationHistoryGraph::from_verified(manifests)
                .map_err(display)?
        };
        type_bridge_schema_migration::require_adoption_authority_pair(&prospective, true)
            .map_err(display)?;
        evidence.require_unchanged(directory).map_err(display)?;
        prepared
            .history
            .require_unchanged_head(&prepared.reconstructed)
            .map_err(|error| {
                format!("archive adoption history changed before pair publication: {error}")
            })?;

        if !bridge_already_published {
            bridge_created =
                publish_authority(directory, &prepared.bridge_name, &prepared.bridge_bytes)?;
        }
        if let Some(after_bridge) = after_bridge.take() {
            after_bridge();
        }
        prepared
            .history
            .require_unchanged_head(&prepared.reconstructed)
            .map_err(|error| {
                format!("archive adoption history changed before genesis publication: {error}")
            })?;
        genesis_created = publish_authority(directory, genesis_name, genesis_bytes)?;
        prepared
            .history
            .require_unchanged_head(&prepared.reconstructed)
            .map_err(|error| {
                format!("archive adoption history changed after pair publication: {error}")
            })?;
        workspace
            .discover_migrations_in(migration_directory)
            .map_err(display)?;
        Ok(())
    })();

    if let Err(error) = publication {
        return Err(rollback_adoption_publication(
            directory,
            &prepared.bridge_name,
            bridge_created,
            genesis_created,
            error,
        ));
    }
    Ok(migration_directory
        .display_path()
        .join(&prepared.bridge_name))
}

fn rollback_adoption_publication(
    directory: &type_bridge_schema_migration::MigrationDirectory,
    bridge_name: &str,
    bridge_created: bool,
    genesis_created: bool,
    primary: String,
) -> String {
    let mut cleanup_errors = Vec::new();
    if genesis_created
        && let Err(error) =
            directory.remove_file(type_bridge_schema_compat::ADOPTED_GENESIS_FILE_NAME.as_ref())
    {
        cleanup_errors.push(format!("cannot remove newly published genesis: {error}"));
    }
    if bridge_created && let Err(error) = directory.remove_file(bridge_name.as_ref()) {
        cleanup_errors.push(format!("cannot remove newly published bridge: {error}"));
    }
    if (bridge_created || genesis_created)
        && let Err(error) = directory.sync_all()
    {
        cleanup_errors.push(format!("cannot flush adoption rollback: {error}"));
    }
    if cleanup_errors.is_empty() {
        primary
    } else {
        format!(
            "{primary}; adoption publication rollback failed: {}",
            cleanup_errors.join("; ")
        )
    }
}

/// Publish immutable authority from a unique, flushed same-directory temp.
///
/// Hard-link publication is atomic and no-replace. An existing final name is
/// accepted only when its bounded bytes are identical, allowing a retry to
/// recover after publication succeeded but the caller did not observe it. In
/// particular, a directory-sync error may be reported after the final link is
/// already durable; that exact orphan is intentionally left for the same
/// adoption command to recognize and complete on retry.
fn publish_authority(
    directory: &type_bridge_schema_migration::MigrationDirectory,
    name: &str,
    bytes: &[u8],
) -> Result<bool, String> {
    use std::io::Write;
    if let Some(existing) = read_existing_authority(directory, name)? {
        if existing == bytes {
            return Ok(false);
        }
        return Err(format!(
            "{name} already exists with different authority bytes"
        ));
    }
    let mut temporary = None;
    for attempt in 0..128_u64 {
        let candidate = unique_authority_temporary_name(name, attempt);
        match directory.create_new(candidate.as_ref()) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                    let _ = directory.remove_file(candidate.as_ref());
                    return Err(format!("cannot write {candidate}: {error}"));
                }
                temporary = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!("cannot create {candidate}: {error}"));
            }
        }
    }
    let temporary = temporary.ok_or_else(|| {
        format!("cannot allocate a unique temporary authority file beside {name}")
    })?;
    let publication = directory.hard_link(temporary.as_ref(), name.as_ref());
    match publication {
        Ok(()) => {
            if let Err(error) = directory.sync_all() {
                let _ = directory.remove_file(temporary.as_ref());
                return Err(format!("cannot flush migration directory: {error}"));
            }
            let _ = directory.remove_file(temporary.as_ref());
            directory
                .sync_all()
                .map_err(|error| format!("cannot flush migration directory: {error}"))?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = directory.remove_file(temporary.as_ref());
            let existing = read_existing_authority(directory, name)?
                .ok_or_else(|| format!("{name} disappeared during no-replace publication"))?;
            if existing == bytes {
                Ok(false)
            } else {
                Err(format!(
                    "{name} was concurrently published with different authority bytes"
                ))
            }
        }
        Err(error) => {
            let _ = directory.remove_file(temporary.as_ref());
            Err(format!("cannot publish {name}: {error}"))
        }
    }
}

fn read_existing_authority(
    directory: &type_bridge_schema_migration::MigrationDirectory,
    name: &str,
) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    let file = match directory.open_regular_readonly(name.as_ref()) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {name}: {error}")),
    };
    let limit = type_bridge_contract::limits::MAX_CANONICAL_BYTES;
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read {name}: {error}"))?;
    if bytes.len() > limit {
        return Err(format!("{name} exceeds the 16 MiB authority ceiling"));
    }
    Ok(Some(bytes))
}

fn unique_authority_temporary_name(name: &str, attempt: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_AUTHORITY_TEMPORARY: AtomicU64 = AtomicU64::new(1);
    let nonce = NEXT_AUTHORITY_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    format!(".{name}.{}.{}.{}.tmp", std::process::id(), nonce, attempt)
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

#[cfg(test)]
mod credential_error_redaction_tests {
    use super::*;
    use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
    use type_bridge_typedb_runtime::RuntimeError;

    const PROVIDER_TEXT: &str =
        "TB_ADDRESS_SECRET TB_USERNAME_SECRET TB_PASSWORD_SECRET TB_PROVIDER_SECRET";
    const SECRETS: [&str; 4] = [
        "TB_ADDRESS_SECRET",
        "TB_USERNAME_SECRET",
        "TB_PASSWORD_SECRET",
        "TB_PROVIDER_SECRET",
    ];

    fn hostile_secure_error() -> type_bridge_orm::SecureConnectError {
        type_bridge_orm::SecureConnectError::Runtime(RuntimeError::Connection(
            PROVIDER_TEXT.to_owned(),
        ))
    }

    #[test]
    fn connected_lifecycle_contexts_drop_hostile_provider_text() {
        for (context, code) in [
            (
                "cannot check database \"managed\"",
                "typedb_database_exists_failed",
            ),
            (
                "cannot ensure database \"managed\"",
                "typedb_database_ensure_failed",
            ),
            (
                "cannot connect the managed database",
                "typedb_database_connect_failed",
            ),
        ] {
            let sanitized =
                sanitize_connected_error(context.to_owned(), code, hostile_secure_error());
            let rendered = format!("{sanitized}\n{sanitized:?}");
            for secret in SECRETS {
                assert!(!rendered.contains(secret), "{secret}: {rendered}");
            }
            assert!(rendered.contains(context), "{rendered}");
            assert!(rendered.contains(code), "{rendered}");
        }
    }

    #[test]
    fn connected_lifecycle_preserves_only_typed_safe_diagnostics() {
        let sanitized = sanitize_connected_error(
            "cannot connect the managed database".to_owned(),
            "typedb_database_connect_failed",
            type_bridge_orm::SecureConnectError::DriverTlsConfiguration { band: 9 },
        );
        assert!(
            sanitized.contains("tls_driver_lowering_failed"),
            "{sanitized}"
        );
        assert!(sanitized.contains("driver band 9"), "{sanitized}");
        assert!(
            !sanitized.contains("typedb_database_connect_failed"),
            "{sanitized}"
        );
    }

    #[test]
    fn schema_export_drops_hostile_orm_text_and_source() {
        let sanitized = sanitize_schema_export_error(type_bridge_orm::OrmError::Connection(
            PROVIDER_TEXT.into(),
        ));
        let rendered = format!("{sanitized}\n{sanitized:?}");
        for secret in SECRETS {
            assert!(!rendered.contains(secret), "{secret}: {rendered}");
        }
        assert!(
            rendered.contains("typedb_schema_export_failed"),
            "{rendered}"
        );
    }

    #[test]
    fn migration_runner_display_omits_provider_details() {
        let diagnostic = Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new("migration_provider_test_failed").expect("static code"),
            "migration provider operation failed",
        )
        .with_detail("provider", PROVIDER_TEXT);
        let error = type_bridge_schema_migration_typedb::MigrationDirectoryApplyError::Diagnostic(
            diagnostic,
        );

        let rendered = display(error);
        for secret in SECRETS {
            assert!(!rendered.contains(secret), "{secret}: {rendered}");
        }
        assert!(
            rendered.contains("migration_provider_test_failed"),
            "{rendered}"
        );
    }

    #[test]
    fn migration_outcome_projection_omits_provider_details_for_apply_and_adopt() {
        use type_bridge_contract::migration::MigrationId;
        use type_bridge_schema_migration::{MigrationExecutionOutcome, MigrationExecutionPosition};

        let diagnostic = || {
            Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                DiagnosticCode::new("migration_provider_test_failed").expect("static code"),
                "migration provider operation failed",
            )
            .with_detail("provider", PROVIDER_TEXT)
        };
        for context in [
            "apply did not complete",
            "adoption checkpoint did not complete",
        ] {
            for (outcome, expected_state, expected_position) in [
                (
                    MigrationExecutionOutcome::RetrySafe {
                        migration_id: MigrationId::new("example", "0001_initial")
                            .expect("migration id"),
                        position: MigrationExecutionPosition::TransactionGroup(7),
                        diagnostic: diagnostic(),
                    },
                    "retry-safe",
                    "transaction group 7",
                ),
                (
                    MigrationExecutionOutcome::RequiresExplicitRecovery {
                        migration_id: MigrationId::new("example", "0001_initial")
                            .expect("migration id"),
                        position: MigrationExecutionPosition::ManifestCheckpoint,
                        diagnostic: diagnostic(),
                    },
                    "explicit recovery required",
                    "manifest checkpoint",
                ),
            ] {
                let rendered = sanitize_migration_execution_outcome(context, outcome);
                for secret in SECRETS {
                    assert!(!rendered.contains(secret), "{secret}: {rendered}");
                }
                for expected in [
                    context,
                    expected_state,
                    "example/0001_initial",
                    expected_position,
                    "migration_provider_test_failed",
                    "migration provider operation failed",
                ] {
                    assert!(rendered.contains(expected), "{expected}: {rendered}");
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod output_authority_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn retained_output_authority_survives_component_swap_without_redirecting() {
        let workspace = tempfile::tempdir().expect("workspace directory");
        let outside = tempfile::tempdir().expect("outside directory");
        fs::create_dir_all(workspace.path().join("generated/python")).expect("output directory");
        let authority = WorkspaceDirectoryAuthority::open(
            WorkspaceRoot::new(fs::canonicalize(workspace.path()).expect("canonical workspace"))
                .expect("workspace root"),
        )
        .expect("workspace authority");
        let root = authority.output_root().expect("output authority");
        let output = root
            .open_beneath(Path::new("generated/python"))
            .expect("output authority");

        let held = workspace.path().join("generated/python-held");
        fs::rename(workspace.path().join("generated/python"), &held)
            .expect("move retained output directory");
        symlink(outside.path(), workspace.path().join("generated/python"))
            .expect("redirect configured output path");

        output
            .write_atomic("_models.py".as_ref(), b"retained authority")
            .expect("publication remains handle-relative");
        assert_eq!(
            fs::read(held.join("_models.py")).expect("retained output reads"),
            b"retained authority"
        );
        assert!(
            !outside.path().join("_models.py").exists(),
            "component replacement redirected output outside the workspace"
        );
    }

    #[test]
    fn retained_output_root_survives_root_entry_swap_without_redirecting() {
        let workspace = tempfile::tempdir().expect("workspace directory");
        let outside = tempfile::tempdir().expect("outside directory");
        fs::create_dir_all(workspace.path().join("generated/python")).expect("output directory");
        let authority = WorkspaceDirectoryAuthority::open(
            WorkspaceRoot::new(fs::canonicalize(workspace.path()).expect("canonical workspace"))
                .expect("workspace root"),
        )
        .expect("workspace authority");
        let root = authority.output_root().expect("output authority");
        let held = workspace
            .path()
            .parent()
            .expect("temporary parent")
            .join(format!(
                "{}-retained",
                workspace
                    .path()
                    .file_name()
                    .expect("temporary name")
                    .to_string_lossy()
            ));
        fs::rename(workspace.path(), &held).expect("workspace root moves after validation");
        symlink(outside.path(), workspace.path()).expect("workspace name redirects outside");

        let output = root
            .open_beneath(Path::new("generated/python"))
            .expect("output opens through retained root");
        output
            .write_atomic("_models.py".as_ref(), b"retained root authority")
            .expect("publication remains rooted in the retained handle");
        assert_eq!(
            fs::read(held.join("generated/python/_models.py")).expect("retained output reads"),
            b"retained root authority"
        );
        assert!(
            !outside.path().join("generated/python/_models.py").exists(),
            "root replacement redirected output outside the workspace"
        );

        fs::remove_file(workspace.path()).expect("replacement symlink removes");
        fs::rename(&held, workspace.path()).expect("workspace restores for cleanup");
    }
}

fn bind_approvals(
    graph: &type_bridge_schema_migration::MigrationHistoryGraph,
    approvals: &[String],
) -> Result<Vec<type_bridge_schema_migration::MigrationApplyApproval>, String> {
    if approvals.is_empty() {
        return Ok(Vec::new());
    }
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

#[cfg(test)]
mod transport_option_tests {
    use super::*;

    fn environment(policy: WorkspaceTransportPolicy) -> WorkspaceEnvironment {
        WorkspaceEnvironment::new(
            "typedb.example:1729",
            "example",
            SecretReference::environment("TYPEBRIDGE_TEST_USERNAME").expect("username reference"),
            SecretReference::environment("TYPEBRIDGE_TEST_PASSWORD").expect("password reference"),
        )
        .expect("environment")
        .with_transport_policy(policy)
    }

    fn custom_root_workspace(root_bytes: &[u8]) -> (tempfile::TempDir, TypeBridgeWorkspace) {
        let directory = tempfile::tempdir().expect("workspace directory");
        fs::create_dir_all(directory.path().join("schema/fragments")).expect("schema directory");
        fs::create_dir_all(directory.path().join("migrations/v2")).expect("migration directory");
        fs::create_dir_all(directory.path().join("certs")).expect("certificate directory");
        fs::write(
            directory.path().join("schema/schema.yaml"),
            "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
        )
        .expect("schema set writes");
        fs::write(
            directory.path().join("schema/fragments/model.yaml"),
            "format: typebridge.schema/v2\nentities: {person: {}}\n",
        )
        .expect("schema writes");
        fs::write(directory.path().join("certs/root.pem"), root_bytes).expect("certificate writes");
        let manifest = directory.path().join("typebridge.yaml");
        fs::write(
            &manifest,
            "format: typebridge.workspace/v1\n\
             schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: tls-test\n\
             compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
             migrations:\n  directory: migrations/v2\n  app-label: tlstest\n\
             environments:\n  dev:\n    database: tls_test\n    uri: never-contact.invalid:1729\n    \
             tls: 'true'\n    tls-root-ca: certs/root.pem\n    credential:\n      username: \
             env:TYPEBRIDGE_TEST_USERNAME\n      password: env:TYPEBRIDGE_TEST_PASSWORD\n",
        )
        .expect("manifest writes");
        let workspace = load_workspace(&manifest).expect("custom-root workspace loads");
        (directory, workspace)
    }

    #[test]
    fn workspace_transport_policy_maps_without_changing_plaintext_defaults() {
        let defaults = type_bridge_orm::SecureConnectOptions::default();
        let disabled = secure_connect_options(&environment(WorkspaceTransportPolicy::Disabled));
        assert_eq!(disabled.tls_mode, type_bridge_orm::TlsMode::Disabled);
        assert_eq!(disabled.http_port, defaults.http_port);
        assert_eq!(disabled.server_version, defaults.server_version);

        let native = secure_connect_options(
            &environment(WorkspaceTransportPolicy::NativeRoots).with_http_port(9443),
        );
        assert_eq!(native.tls_mode, type_bridge_orm::TlsMode::NativeRoots);
        assert_eq!(native.http_port, 9443);
        assert_eq!(native.server_version, defaults.server_version);
    }

    #[test]
    fn custom_root_mapping_preserves_the_validated_canonical_path() {
        let directory = tempfile::tempdir().expect("workspace directory");
        let canonical = fs::canonicalize(directory.path()).expect("canonical workspace");
        fs::create_dir_all(canonical.join("certs")).expect("certificate directory");
        fs::write(
            canonical.join("certs/root.pem"),
            b"not parsed at workspace boundary\n",
        )
        .expect("certificate writes");
        let root = WorkspaceRoot::new(canonical.clone()).expect("workspace root");
        let root_ca = type_bridge_workspace::WorkspaceRootCa::new(
            &root,
            "certs/root.pem",
            &SystemSchemaSourceService,
        )
        .expect("confined root CA");

        let options = secure_connect_options(
            &environment(WorkspaceTransportPolicy::CustomRootCa(root_ca)).with_http_port(8443),
        );
        assert_eq!(
            options.tls_mode,
            type_bridge_orm::TlsMode::CustomRootCa(canonical.join("certs/root.pem"))
        );
        assert_eq!(options.http_port, 8443);
    }

    #[test]
    fn malformed_custom_root_fails_transport_preflight_before_credentials_are_needed() {
        let (_directory, workspace) = custom_root_workspace(b"definitely not a certificate\n");

        let error = preflight_secure_connect_options(&workspace, "dev")
            .expect_err("PEM parsing must happen before credential resolution");
        assert!(error.contains("tls_custom_root_ca_invalid_pem"), "{error}");
        assert!(!error.contains("TYPEBRIDGE_TEST_USERNAME"), "{error}");
        assert!(!error.contains("TYPEBRIDGE_TEST_PASSWORD"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn workspace_root_swap_to_outside_symlink_is_rejected_at_transport_preflight() {
        use std::os::unix::fs::symlink;

        let (directory, workspace) = custom_root_workspace(b"initial regular root\n");
        let outside = tempfile::tempdir().expect("outside directory");
        let configured = directory.path().join("certs/root.pem");

        let outside_root = outside.path().join("malicious.pem");
        fs::write(
            &outside_root,
            include_bytes!("../../core/tests/fixtures/valid-root.pem"),
        )
        .expect("write outside replacement root");
        fs::remove_file(&configured).expect("remove validated confined root");
        symlink(&outside_root, &configured).expect("install outside symlink after validation");

        let error = preflight_secure_connect_options(&workspace, "dev")
            .expect_err("retained workspace paths must never follow a replacement symlink");
        assert!(error.contains("tls_custom_root_ca_unreadable"), "{error}");
        assert!(!error.contains("tls_custom_root_ca_invalid_pem"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn real_directory_root_replacement_cannot_substitute_custom_trust() {
        let (directory, workspace) =
            custom_root_workspace(include_bytes!("../../core/tests/fixtures/valid-root.pem"));
        let configured_root = directory.path().to_path_buf();
        let held_root = configured_root.with_extension("retained-custom-root-ca");
        fs::rename(&configured_root, &held_root).expect("move retained workspace root");
        fs::create_dir_all(configured_root.join("certs")).expect("replacement root creates");
        fs::write(
            configured_root.join("certs/root.pem"),
            b"attacker-controlled replacement is not a certificate\n",
        )
        .expect("replacement root writes");

        let preflight = preflight_secure_connect_options(&workspace, "dev");

        fs::remove_dir_all(&configured_root).expect("replacement root removes");
        fs::rename(&held_root, &configured_root).expect("retained root restores");
        preflight.expect("transport must use the CA under the retained original root");
    }
}

#[cfg(test)]
mod adoption_file_tests {
    use super::*;
    use sha2::{Digest as _, Sha256};

    const LEGACY_SCHEMA: &str = "define\nentity person;\n";

    fn adoption_workspace() -> (tempfile::TempDir, TypeBridgeWorkspace) {
        let directory = tempfile::tempdir().expect("workspace directory");
        fs::create_dir_all(directory.path().join("schema/fragments")).expect("schema directory");
        fs::write(
            directory.path().join("schema/schema.yaml"),
            "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n",
        )
        .expect("schema set writes");
        fs::write(
            directory.path().join("schema/fragments/model.yaml"),
            "format: typebridge.schema/v2\nentities: {person: {}}\n",
        )
        .expect("schema writes");
        let manifest = directory.path().join("typebridge.yaml");
        fs::write(
            &manifest,
            "format: typebridge.workspace/v1\n\
             schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n  managed-scope: adoption-test\n\
             compatibility:\n  semantic-profile: typedb-3.12.1/v1\n\
             migrations:\n  directory: migrations/v2\n  app-label: smoke\n\
             environments:\n  dev:\n    database: adoption_test\n    uri: never-contact.invalid:1729\n    migrate: 'true'\n    credential:\n      username: env:TYPEBRIDGE_TEST_USERNAME\n      password: env:TYPEBRIDGE_TEST_PASSWORD\n",
        )
        .expect("manifest writes");
        let workspace = load_workspace(&manifest).expect("workspace loads");
        (directory, workspace)
    }

    fn write_legacy_fixture(root: &Path) -> PathBuf {
        let directory = root.join("migrations/legacy");
        fs::create_dir_all(&directory).expect("legacy directory");
        let name = "0001_initial";
        let python_source = "class Migration:\n    operations = []\n";
        let checksum = type_bridge_migration::migration_file_checksum(python_source);
        let source_sha256 = format!("{:x}", Sha256::digest(python_source.as_bytes()));
        let schema_hash = format!("{:x}", Sha256::digest(LEGACY_SCHEMA.as_bytes()));
        fs::write(directory.join(format!("{name}.py")), python_source)
            .expect("legacy source writes");
        let adoption = type_bridge_migration::LegacyAdoptionMetadata::new(
            "legacy",
            name,
            Vec::new(),
            checksum,
            source_sha256,
            type_bridge_migration::LegacySchemaEffect::Snapshot,
            type_bridge_migration::MigrationDependencySpec {
                app_label: "legacy".to_owned(),
                migration_name: name.to_owned(),
            },
            schema_hash.clone(),
        )
        .expect("legacy adoption metadata");
        fs::write(
            directory.join(format!("{name}.adoption.json")),
            serde_json::to_vec_pretty(&adoption).expect("metadata encodes"),
        )
        .expect("metadata writes");
        let snapshot = directory.join("snapshots/v0001");
        fs::create_dir_all(&snapshot).expect("snapshot directory");
        fs::write(snapshot.join("schema.tql"), LEGACY_SCHEMA).expect("snapshot schema writes");
        fs::write(
            snapshot.join("snapshot.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": "v0001",
                "source_migration": name,
                "schema_hash": schema_hash,
                "file_hashes": {"schema.tql": schema_hash},
                "type_bridge_version": "1.5.11",
                "type_bridge_core_version": "1.5.11"
            }))
            .expect("snapshot manifest encodes"),
        )
        .expect("snapshot manifest writes");
        directory
    }

    #[test]
    fn authority_publication_is_atomic_no_replace_and_resumable() {
        let directory = tempfile::tempdir().expect("directory");
        let authority =
            type_bridge_schema_migration::MigrationDirectory::open_ambient(directory.path())
                .expect("directory authority");
        let name = "adopted-genesis.typeql";
        let path = directory.path().join("adopted-genesis.typeql");
        assert!(
            publish_authority(&authority, name, b"define\nentity person;\n")
                .expect("first publish")
        );
        assert!(
            !publish_authority(&authority, name, b"define\nentity person;\n")
                .expect("identical recovery")
        );
        assert!(publish_authority(&authority, name, b"define\nentity company;\n").is_err());
        assert_eq!(
            fs::read(&path).expect("authority reads"),
            b"define\nentity person;\n"
        );
        assert!(
            fs::read_dir(directory.path())
                .expect("directory reads")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn authority_publication_rejects_final_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("directory");
        let outside = directory.path().join("outside");
        fs::write(&outside, b"untouched").expect("outside writes");
        let path = directory.path().join("adopted-genesis.typeql");
        symlink(&outside, &path).expect("symlink");
        let authority =
            type_bridge_schema_migration::MigrationDirectory::open_ambient(directory.path())
                .expect("directory authority");

        assert!(publish_authority(&authority, "adopted-genesis.typeql", b"replacement").is_err());
        assert_eq!(fs::read(&outside).expect("outside reads"), b"untouched");
    }

    #[test]
    fn invalid_adoption_name_creates_no_canonical_directory() {
        let (directory, workspace) = adoption_workspace();
        let missing_archive = directory.path().join("missing-legacy");
        let error = run_connected(
            &workspace,
            "dev",
            ConnectedAction::Adopt {
                archive_directory: missing_archive,
                name: String::new(),
            },
        )
        .expect_err("invalid name fails before history or network access");
        assert!(error.contains("migration"), "{error}");
        assert!(
            !directory.path().join("migrations/v2").exists(),
            "bad-name validation must not create canonical filesystem state"
        );
    }

    #[test]
    fn invalid_apply_approvals_fail_before_credentials_or_network() {
        let (directory, workspace) = adoption_workspace();
        fs::create_dir_all(directory.path().join("migrations/v2")).expect("canonical directory");

        for (approval, expected) in [
            ("not-a-compound-id", "must be app-label/name"),
            ("smoke/0001_missing", "is not in the committed history"),
        ] {
            let error = run_connected(
                &workspace,
                "dev",
                ConnectedAction::Apply {
                    approvals: vec![approval.to_owned()],
                },
            )
            .expect_err("invalid approval is rejected by local authority");
            assert!(error.contains(expected), "{approval}: {error}");
            assert!(
                !error.contains("credential")
                    && !error.contains("connect")
                    && !error.contains("database"),
                "approval validation ran after external setup: {error}"
            );
        }
    }

    #[test]
    fn adoption_retry_completes_either_exact_orphan_direction() {
        for orphan in ["genesis", "bridge"] {
            let (directory, workspace) = adoption_workspace();
            let legacy = write_legacy_fixture(directory.path());
            let prepared = prepare_archive_adoption(&workspace, &legacy, "0000_archive_frontier")
                .expect("adoption prepares");
            let migration_directory = workspace
                .ensure_migration_directory()
                .expect("canonical directory");
            match orphan {
                "genesis" => {
                    publish_authority(
                        migration_directory.directory(),
                        type_bridge_schema_compat::ADOPTED_GENESIS_FILE_NAME,
                        prepared.reconstructed.schema_typeql().as_bytes(),
                    )
                    .expect("genesis orphan publishes");
                }
                "bridge" => {
                    publish_authority(
                        migration_directory.directory(),
                        &prepared.bridge_name,
                        &prepared.bridge_bytes,
                    )
                    .expect("bridge orphan publishes");
                }
                _ => unreachable!(),
            }

            publish_prepared_adoption(&workspace, &migration_directory, &prepared)
                .expect("adoption retry completes the exact orphan");
            workspace
                .discover_migrations_in(&migration_directory)
                .expect("completed adoption pair discovers");
            assert!(
                migration_directory
                    .display_path()
                    .join(type_bridge_schema_compat::ADOPTED_GENESIS_FILE_NAME)
                    .is_file()
            );
            assert!(
                migration_directory
                    .display_path()
                    .join(&prepared.bridge_name)
                    .is_file()
            );
        }
    }

    #[test]
    fn legacy_history_race_after_bridge_rolls_back_new_publication() {
        let (directory, workspace) = adoption_workspace();
        let legacy = write_legacy_fixture(directory.path());
        let prepared = prepare_archive_adoption(&workspace, &legacy, "0000_archive_frontier")
            .expect("adoption prepares");
        let migration_directory = workspace
            .ensure_migration_directory()
            .expect("canonical directory");
        let legacy_source = legacy.join("0001_initial.py");

        let error = publish_prepared_adoption_with_after_bridge(
            &workspace,
            &migration_directory,
            &prepared,
            || {
                fs::write(
                    &legacy_source,
                    "class Migration:\n    operations = ['changed']\n",
                )
                .expect("race mutation writes");
            },
        )
        .expect_err("legacy authority race aborts pair publication");
        assert!(error.contains("changed"), "{error}");
        assert!(
            !migration_directory
                .display_path()
                .join(&prepared.bridge_name)
                .exists(),
            "new bridge is rolled back"
        );
        assert!(
            !migration_directory
                .display_path()
                .join(type_bridge_schema_compat::ADOPTED_GENESIS_FILE_NAME)
                .exists(),
            "genesis is never published after the race"
        );
    }
}
