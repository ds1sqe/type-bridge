//! CLI skeleton for future Rust-native migration commands.

use clap::{Parser, Subcommand};
use type_bridge_migration::{MigrationError, Result};

#[derive(Debug, Parser)]
#[command(version, about = "type-bridge migration IR CLI skeleton")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Reserved for sub-plan 08.
    Plan,
    /// Reserved for sub-plan 08.
    Apply,
    /// Reserved for sub-plan 08.
    Status,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => Ok(()),
        Some(Command::Plan) => Err(MigrationError::Unsupported {
            feature: "plan",
            sub_plan: 8,
        }),
        Some(Command::Apply) => Err(MigrationError::Unsupported {
            feature: "apply",
            sub_plan: 8,
        }),
        Some(Command::Status) => Err(MigrationError::Unsupported {
            feature: "status",
            sub_plan: 8,
        }),
    }
}
