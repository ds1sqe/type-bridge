//! Fully self-contained CLI for type-bridge migrations.
//!
//! Five verbs:
//!
//! * `plan`            — preview the ordered pending migrations (no DB).
//! * `sqlmigrate`      — print the forward/reverse TypeQL for a migration (no DB).
//! * `migrate`         — apply or roll back migrations (connected).
//! * `showmigrations`  — list migrations with applied/pending status (connected).
//! * `makemigrations`  — diff model vs live schema, write .py+.json (shells to `_generate`).
//!
//! Pure verbs (`plan`, `sqlmigrate`) run entirely from the sidecar files on
//! disk via [`load_dir_checked`].  The three connected verbs open their own
//! `Database::connect` + Tokio runtime; the shared `TypeDbStateStore` and
//! `execute_plan` from `crates/migration` are reused without re-implementation
//! (invariant 2: one engine, two bootstraps).

mod display;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anstream::ColorChoice;
use clap::{Parser, Subcommand, ValueEnum};
use thiserror::Error;
use tokio::runtime::Runtime;
use type_bridge_migration::{
    AppliedMigrationRecord, MigrationAction, MigrationError, MigrationStateStore, TypeDbStateStore,
    execute_plan, load_dir, load_dir_checked, plan,
};
use type_bridge_orm::Database;

/// CLI-orchestration errors that are not migration-library concerns.
///
/// The migration library's [`MigrationError`] covers planning, execution, and
/// state. These variants cover the bin's own orchestration — standing up a
/// runtime, opening a connection, and shelling out to Python — and keep those
/// process-local failures out of the library's public error surface.
#[derive(Debug, Error)]
enum CliError {
    /// A migration-library error surfaced while running a command.
    #[error(transparent)]
    Migration(#[from] MigrationError),
    /// The Tokio runtime could not be created.
    #[error("failed to create Tokio runtime: {0}")]
    Runtime(String),
    /// Connecting to TypeDB failed.
    #[error("failed to connect to TypeDB: {0}")]
    Connect(String),
    /// Shelling out to a Python entrypoint failed.
    #[error("{0}")]
    Subprocess(String),
}

type CliResult<T> = std::result::Result<T, CliError>;

/// Default server address, matching the test fixtures (`typedb_lifecycle.py:33`).
const DEFAULT_ADDRESS: &str = "localhost:1730";

/// Environment variable consulted for the server address.
const TYPEDB_ADDRESS_ENV: &str = "TYPEDB_ADDRESS";

/// Default username matching `PyRustDatabase::connect` defaults.
const DEFAULT_USERNAME: &str = "admin";

/// Default password matching `PyRustDatabase::connect` defaults.
const DEFAULT_PASSWORD: &str = "password";

// ── Color mode arg ───────────────────────────────────────────────────────────

/// When to emit ANSI color codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ColorMode {
    /// Auto-detect based on TTY status and `NO_COLOR` / `CLICOLOR` env vars.
    Auto,
    /// Always emit ANSI codes (even when piped).
    Always,
    /// Never emit ANSI codes.
    Never,
}

// ── Top-level CLI ────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(version, about = "type-bridge migration CLI")]
struct Cli {
    /// Control ANSI color output.
    #[arg(long, default_value = "auto", global = true)]
    color: ColorMode,

    #[command(subcommand)]
    command: Command,
}

// ── Subcommands ──────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
enum Command {
    /// Preview the ordered set of pending migrations without connecting to
    /// TypeDB.  Reads `.json` sidecars from the migrations directory and runs
    /// the pure Rust planner with an empty applied-state list.
    Plan {
        /// Directory containing the `.json` sidecar files.
        #[arg(long, default_value = "migrations")]
        migrations_dir: PathBuf,

        /// Limit the plan to migrations up to (and including) this name.
        #[arg(long)]
        target: Option<String>,
    },

    /// Print the carried forward (and optional reverse) TypeQL for a named
    /// migration without connecting to TypeDB.
    Sqlmigrate {
        /// Migration name to inspect (e.g. `0001_initial`).
        name: String,

        /// Directory containing the `.json` sidecar files.
        #[arg(long, default_value = "migrations")]
        migrations_dir: PathBuf,

        /// Print the reverse (rollback) TypeQL instead of the forward TypeQL.
        #[arg(short, long)]
        reverse: bool,
    },

    /// Apply pending migrations (or roll back to a target).  Requires a live
    /// TypeDB connection.
    Migrate {
        /// Server address (e.g. `localhost:1730`).  Falls back to the
        /// `TYPEDB_ADDRESS` environment variable, then `localhost:1730`.
        #[arg(short = 'a', long)]
        address: Option<String>,

        /// Database name (required).
        #[arg(short = 'd', long)]
        database: String,

        /// TypeDB username.
        #[arg(long, default_value = DEFAULT_USERNAME)]
        username: String,

        /// TypeDB password.
        #[arg(long, default_value = DEFAULT_PASSWORD)]
        password: String,

        /// Directory containing the `.json` sidecar files.
        #[arg(long, default_value = "migrations")]
        migrations_dir: PathBuf,

        /// Roll back to this migration (inclusive); all later applied
        /// migrations are rolled back.
        #[arg(long)]
        target: Option<String>,
    },

    /// List migrations and their applied/pending status.  Requires a live
    /// TypeDB connection.
    Showmigrations {
        /// Server address.  Falls back to `TYPEDB_ADDRESS` env, then
        /// `localhost:1730`.
        #[arg(short = 'a', long)]
        address: Option<String>,

        /// Database name (required).
        #[arg(short = 'd', long)]
        database: String,

        /// TypeDB username.
        #[arg(long, default_value = DEFAULT_USERNAME)]
        username: String,

        /// TypeDB password.
        #[arg(long, default_value = DEFAULT_PASSWORD)]
        password: String,

        /// Directory containing the `.json` sidecar files.
        #[arg(long, default_value = "migrations")]
        migrations_dir: PathBuf,
    },

    /// Diff the model IR against the live schema and write a new `.py`+`.json`
    /// migration pair.
    ///
    /// makemigrations renders Python authoring source, which requires the live
    /// Python model objects — see `_generate` (invariant 2: no parallel .py
    /// generator).  This verb shells to `python -m type_bridge.migration._generate`.
    Makemigrations {
        /// Server address.  Falls back to `TYPEDB_ADDRESS` env, then
        /// `localhost:1730`.
        #[arg(short = 'a', long)]
        address: Option<String>,

        /// Database name (required).
        #[arg(short = 'd', long)]
        database: String,

        /// TypeDB username.
        #[arg(long, default_value = DEFAULT_USERNAME)]
        username: String,

        /// TypeDB password.
        #[arg(long, default_value = DEFAULT_PASSWORD)]
        password: String,

        /// Directory to write the generated migration files.
        #[arg(long, default_value = "migrations")]
        migrations_dir: PathBuf,

        /// Dotted Python module path whose descriptors supply the model IR
        /// (e.g. `myapp.models`).
        #[arg(long)]
        models: String,

        /// Optional migration name (default: auto-generated timestamp slug).
        #[arg(long)]
        name: Option<String>,

        /// Create an empty migration for manual editing.
        #[arg(long)]
        empty: bool,

        /// Python interpreter to use for the `_generate` shell-out.
        /// Defaults to the `python3` found on PATH, or the venv python when
        /// the binary is installed in a venv.
        #[arg(long)]
        python: Option<String>,
    },
}

// ── Entry point ──────────────────────────────────────────────────────────────

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(2);
    }
}

fn run() -> CliResult<()> {
    let cli = Cli::parse();

    // Apply the requested color mode globally so anstream respects it.
    match cli.color {
        ColorMode::Always => ColorChoice::AlwaysAnsi.write_global(),
        ColorMode::Never => ColorChoice::Never.write_global(),
        ColorMode::Auto => {} // anstream's default auto-detection applies
    }

    match cli.command {
        Command::Plan {
            migrations_dir,
            target,
        } => cmd_plan(&migrations_dir, target.as_deref()),

        Command::Sqlmigrate {
            name,
            migrations_dir,
            reverse,
        } => cmd_sqlmigrate(&migrations_dir, &name, reverse),

        Command::Migrate {
            address,
            database,
            username,
            password,
            migrations_dir,
            target,
        } => {
            let address = resolve_address(address);
            cmd_migrate(
                &address,
                &database,
                &username,
                &password,
                &migrations_dir,
                target.as_deref(),
            )
        }

        Command::Showmigrations {
            address,
            database,
            username,
            password,
            migrations_dir,
        } => {
            let address = resolve_address(address);
            cmd_showmigrations(&address, &database, &username, &password, &migrations_dir)
        }

        Command::Makemigrations {
            address,
            database,
            username,
            password,
            migrations_dir,
            models,
            name,
            empty,
            python,
        } => {
            let address = resolve_address(address);
            cmd_makemigrations(
                &address,
                &database,
                &username,
                &password,
                &migrations_dir,
                &models,
                name.as_deref(),
                empty,
                python.as_deref(),
            )
        }
    }
}

// ── Pure verb implementations ─────────────────────────────────────────────────

/// Resolve the effective server address: CLI flag → env var → default.
fn resolve_address(flag: Option<String>) -> String {
    flag.or_else(|| std::env::var(TYPEDB_ADDRESS_ENV).ok())
        .unwrap_or_else(|| DEFAULT_ADDRESS.to_string())
}

/// `plan` — load sidecars, plan with empty applied state, print per-step info.
fn cmd_plan(migrations_dir: &Path, target: Option<&str>) -> CliResult<()> {
    let graph = load_dir_checked(migrations_dir)?;
    let execution_plan = plan(&graph, &[], target)?;

    if execution_plan.to_apply.is_empty() && execution_plan.to_rollback.is_empty() {
        display::print_no_pending();
        return Ok(());
    }

    for migration_exec in &execution_plan.to_apply {
        display::print_migration_header(&migration_exec.name, "apply");
        for (i, step) in migration_exec.steps.iter().enumerate() {
            display::print_step(i, step);
        }
    }

    for migration_exec in &execution_plan.to_rollback {
        display::print_migration_header(&migration_exec.name, "rollback");
        for (i, step) in migration_exec.steps.iter().enumerate() {
            display::print_step(i, step);
        }
    }

    Ok(())
}

/// `sqlmigrate` — load sidecars, locate the named migration, print TypeQL.
fn cmd_sqlmigrate(migrations_dir: &Path, name: &str, show_reverse: bool) -> CliResult<()> {
    let graph = load_dir_checked(migrations_dir)?;
    // Use plan() with the target set to `name` and empty applied state so we
    // get the step assembly for exactly that migration.
    let execution_plan = plan(&graph, &[], Some(name))?;

    // plan() returns to_apply for migrations up to and including the target.
    // We want the step(s) from the migration whose name matches exactly.
    let migration_exec = execution_plan
        .to_apply
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| MigrationError::TargetNotFound {
            target: name.to_string(),
        })?;

    for (i, step) in migration_exec.steps.iter().enumerate() {
        if show_reverse {
            display::print_sqlmigrate_reverse(i, step);
        } else {
            display::print_sqlmigrate_forward(i, step);
        }
    }

    Ok(())
}

// ── Connected helpers ─────────────────────────────────────────────────────────

/// Open a new Tokio runtime and `Database::connect`, mirroring
/// `PyRustDatabase::connect` in `orm_runtime.rs:445-470`.
///
/// Returns `(Arc<Database>, Arc<Runtime>)`.  The runtime must outlive every
/// `block_on` call on the database; callers keep both handles for the
/// duration of the command.
fn connect(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
) -> CliResult<(Arc<Database>, Arc<Runtime>)> {
    let runtime = Runtime::new()
        .map(Arc::new)
        .map_err(|error| CliError::Runtime(error.to_string()))?;
    let db = runtime
        .block_on(Database::connect(address, database, username, password))
        .map_err(|error| CliError::Connect(format!("{address}/{database}: {error}")))?;
    Ok((Arc::new(db), runtime))
}

// ── Connected verb implementations ────────────────────────────────────────────

/// `migrate` — connect, load state, plan, execute, record state.
///
/// Mirrors the Python `executor.py` coordination:
///   load_applied → plan (filters out applied) → execute_plan
///   → for each successful to_apply: record_applied
///   → for each to_rollback: record_unapplied
#[allow(clippy::too_many_arguments)]
fn cmd_migrate(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    migrations_dir: &Path,
    target: Option<&str>,
) -> CliResult<()> {
    let (db, runtime) = connect(address, database, username, password)?;

    let store = TypeDbStateStore::new(Arc::clone(&db));
    runtime.block_on(store.ensure_schema())?;

    let applied = runtime.block_on(store.load_applied())?;

    let graph = load_dir_checked(migrations_dir)?;
    let execution_plan = plan(&graph, &applied, target)?;

    if execution_plan.to_apply.is_empty() && execution_plan.to_rollback.is_empty() {
        display::print_no_pending();
        return Ok(());
    }

    let results = runtime.block_on(execute_plan(&db, execution_plan));

    let mut any_failure = false;
    for result in &results {
        display::print_result(result);
        if result.success {
            match result.action {
                MigrationAction::Apply => {
                    // Build the checksum from the spec in the loaded graph.
                    let checksum = graph
                        .migrations
                        .iter()
                        .find(|spec| spec.app_label == result.app_label && spec.name == result.name)
                        .and_then(|spec| spec.checksum.clone())
                        .unwrap_or_default();
                    let record = AppliedMigrationRecord {
                        app_label: result.app_label.clone(),
                        name: result.name.clone(),
                        checksum,
                        applied_at: None,
                    };
                    runtime.block_on(store.record_applied(record))?;
                }
                MigrationAction::Rollback => {
                    runtime.block_on(store.record_unapplied(&result.app_label, &result.name))?;
                }
            }
        } else {
            any_failure = true;
        }
    }

    if any_failure {
        std::process::exit(1);
    }

    Ok(())
}

/// `showmigrations` — connect, load applied state, cross-reference sidecar
/// list, print applied/pending per migration.
fn cmd_showmigrations(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    migrations_dir: &Path,
) -> CliResult<()> {
    let (db, runtime) = connect(address, database, username, password)?;

    let store = TypeDbStateStore::new(Arc::clone(&db));
    runtime.block_on(store.ensure_schema())?;

    let applied = runtime.block_on(store.load_applied())?;

    // `showmigrations` only reports applied/pending status — it executes no
    // TypeQL — so it uses the unchecked `load_dir`. A drifted sidecar must not
    // hide the status listing; the drift guard (`load_dir_checked`) gates the
    // execution paths (`plan`, `sqlmigrate`, `migrate`) instead.
    let graph = load_dir(migrations_dir)?;

    // Build a lookup set of (app_label, name) pairs for applied migrations.
    let applied_keys: std::collections::BTreeSet<(&str, &str)> = applied
        .iter()
        .map(|r| (r.app_label.as_str(), r.name.as_str()))
        .collect();

    if graph.migrations.is_empty() {
        display::print_no_pending();
        return Ok(());
    }

    // Group by app_label for readability, matching the Python `showmigrations`
    // output style (app_label header, then per-migration status lines).
    let mut current_app: Option<&str> = None;
    for spec in &graph.migrations {
        if current_app != Some(spec.app_label.as_str()) {
            display::print_app_label(&spec.app_label);
            current_app = Some(spec.app_label.as_str());
        }
        let is_applied = applied_keys.contains(&(spec.app_label.as_str(), spec.name.as_str()));
        display::print_migration_status(&spec.name, is_applied);
    }

    Ok(())
}

/// `makemigrations` — shell to `python -m type_bridge.migration._generate`
/// forwarding all relevant flags.
///
/// makemigrations renders Python authoring source, which requires the live
/// Python model objects — see `_generate` (invariant 2: no parallel .py
/// generator).
#[allow(clippy::too_many_arguments)]
fn cmd_makemigrations(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    migrations_dir: &Path,
    models: &str,
    name: Option<&str>,
    empty: bool,
    python: Option<&str>,
) -> CliResult<()> {
    // Locate the Python interpreter.  Priority:
    //   1. Explicit --python flag.
    //   2. Venv python: executable in the same bin/ dir as this binary.
    //   3. `python3` on PATH.
    let python_exe = if let Some(py) = python {
        py.to_string()
    } else {
        // Try to find a python3 sibling in the same directory as this binary.
        let self_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        if let Some(dir) = self_dir {
            let candidate = dir.join("python3");
            if candidate.exists() {
                candidate.to_string_lossy().into_owned()
            } else {
                let candidate_py = dir.join("python");
                if candidate_py.exists() {
                    candidate_py.to_string_lossy().into_owned()
                } else {
                    "python3".to_string()
                }
            }
        } else {
            "python3".to_string()
        }
    };

    let mut cmd = std::process::Command::new(&python_exe);
    cmd.args(["-m", "type_bridge.migration._generate"]);
    cmd.args(["--address", address]);
    cmd.args(["--database", database]);
    cmd.args(["--username", username]);
    cmd.args(["--password", password]);
    cmd.args(["--migrations-dir", &migrations_dir.to_string_lossy()]);
    cmd.args(["--models", models]);
    // --name is required by _generate; pass "auto" as the default when unset,
    // matching the old Python Typer default.
    cmd.args(["--name", name.unwrap_or("auto")]);
    if empty {
        cmd.arg("--empty");
    }

    let status = cmd.status().map_err(|error| {
        CliError::Subprocess(format!(
            "failed to launch python interpreter '{python_exe}': {error}. \
             Ensure Python is installed and type_bridge is importable."
        ))
    })?;

    let code = status.code().unwrap_or(1);
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}
