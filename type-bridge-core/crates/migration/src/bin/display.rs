//! Colored display helpers for the type-bridge-migration CLI.
//!
//! All ANSI coloring is confined to this module — the `crates/orm`
//! `ChangeCategory` type and the `crates/migration` library remain
//! color-agnostic (invariant 7).
//!
//! `anstream::stdout()` auto-strips ANSI codes when stdout is not a TTY
//! (so subprocess capture in tests gets clean text) and respects `NO_COLOR` /
//! `CLICOLOR`.

use std::io::Write as _;

use anstream::stdout;
use anstyle::{AnsiColor, Effects, Style};
use type_bridge_migration::plan::{ExecutionStep, StepKind};
use type_bridge_migration::{MigrationAction, MigrationResult};
use type_bridge_orm::TxType;

// ── Color palette ────────────────────────────────────────────────────────────

/// Green — `ChangeCategory::Safe` and schema DDL steps.
const STYLE_SAFE: Style = Style::new()
    .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green)))
    .effects(Effects::BOLD);

/// Yellow — `ChangeCategory::Warning` and write-typed steps.
const STYLE_WARNING: Style = Style::new()
    .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow)))
    .effects(Effects::BOLD);

/// Red — `ChangeCategory::Breaking` and backfill steps.
const STYLE_BREAKING: Style = Style::new()
    .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red)))
    .effects(Effects::BOLD);

/// Dim — neutral / headers.
const STYLE_DIM: Style = Style::new().effects(Effects::DIMMED);

// ── Public helpers ───────────────────────────────────────────────────────────

/// Print a brief "No pending migrations." notice.
pub fn print_no_pending() {
    let mut out = stdout();
    writeln!(out, "No pending migrations.").unwrap_or_default();
}

/// Print the migration header line (name + action).
pub fn print_migration_header(name: &str, action: &str) {
    let mut out = stdout();
    writeln!(
        out,
        "\n{STYLE_DIM}Migration:{STYLE_DIM:#}  {name}  [{action}]"
    )
    .unwrap_or_default();
}

/// Print a single [`ExecutionStep`] in the `plan` verb output.
///
/// Each step shows its zero-based index, `tx_type`, `kind`, and a short
/// excerpt of the forward TypeQL.  The step kind label is colored by
/// category: schema=green, write/backfill=yellow or red.
pub fn print_step(index: usize, step: &ExecutionStep) {
    let mut out = stdout();

    let kind_label = step_kind_label(step.kind);
    let kind_style = step_kind_style(step.kind, step.tx_type);

    // Truncate the forward TypeQL for readability in the plan view.
    let preview = truncate_typeql(&step.forward, 80);

    writeln!(
        out,
        "  Step {index}: {kind_style}{kind_label}{kind_style:#}  tx={tx_type}",
        tx_type = tx_type_label(step.tx_type),
    )
    .unwrap_or_default();

    writeln!(out, "    {STYLE_DIM}{preview}{STYLE_DIM:#}").unwrap_or_default();
}

/// Print the forward TypeQL for the `sqlmigrate` verb.
pub fn print_sqlmigrate_forward(index: usize, step: &ExecutionStep) {
    let mut out = stdout();
    writeln!(
        out,
        "-- Step {index} (forward, tx={tx_type})",
        tx_type = tx_type_label(step.tx_type)
    )
    .unwrap_or_default();
    writeln!(out, "{}", step.forward).unwrap_or_default();
}

/// Print the reverse TypeQL for `sqlmigrate --reverse`.
pub fn print_sqlmigrate_reverse(index: usize, step: &ExecutionStep) {
    let mut out = stdout();
    match &step.reverse {
        Some(reverse_typeql) => {
            writeln!(
                out,
                "-- Step {index} (reverse, tx={tx_type})",
                tx_type = tx_type_label(step.tx_type)
            )
            .unwrap_or_default();
            writeln!(out, "{reverse_typeql}").unwrap_or_default();
        }
        None => {
            writeln!(out, "-- Step {index}: no reverse (non-reversible step)").unwrap_or_default();
        }
    }
}

/// Print the result of executing a single migration (apply or rollback).
///
/// On success: `  Applied: <name> ... OK` / `  Rolled back: <name> ... OK`.
/// On failure: prints the action, name, and error message; returns after
/// printing so the caller can decide to exit non-zero.
/// Backfill counts (when present) are printed on a subsequent indented line.
pub fn print_result(result: &MigrationResult) {
    let mut out = stdout();
    let action_label = match result.action {
        MigrationAction::Apply => "Applied",
        MigrationAction::Rollback => "Rolled back",
    };
    let status_label = if result.success { "OK" } else { "FAILED" };
    let status_style = if result.success {
        STYLE_SAFE
    } else {
        STYLE_BREAKING
    };
    writeln!(
        out,
        "  {action_label}: {name} ... {status_style}{status_label}{status_style:#}",
        name = result.name,
    )
    .unwrap_or_default();

    if let Some(error) = &result.error {
        writeln!(out, "    Error: {error}").unwrap_or_default();
    }

    if let Some(backfill_steps) = &result.backfill {
        for bf in backfill_steps {
            writeln!(
                out,
                "    {STYLE_DIM}backfill step {idx}: matched={matched} inserted={inserted} skipped={skipped}{STYLE_DIM:#}",
                idx = bf.step_index,
                matched = bf.matched,
                inserted = bf.inserted,
                skipped = bf.skipped,
            )
            .unwrap_or_default();
        }
    }
}

/// Print an app-label header for `showmigrations` output.
pub fn print_app_label(app_label: &str) {
    let mut out = stdout();
    writeln!(out, "\n{app_label}").unwrap_or_default();
}

/// Print a single migration's applied/pending status for `showmigrations`.
///
/// Applied migrations are shown in green; pending in dim.
pub fn print_migration_status(name: &str, is_applied: bool) {
    let mut out = stdout();
    if is_applied {
        writeln!(out, " {STYLE_SAFE}[X]{STYLE_SAFE:#} {name}").unwrap_or_default();
    } else {
        writeln!(out, " {STYLE_DIM}[ ]{STYLE_DIM:#} {name}").unwrap_or_default();
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

fn step_kind_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Schema => "schema",
        StepKind::Write => "write",
        StepKind::Backfill => "backfill",
    }
}

/// Map a step kind + tx_type to a display style.
///
/// - Schema DDL → green (safe, additive schema change).
/// - Write (non-backfill) → yellow (data mutation, review recommended).
/// - Backfill → red (bulk data migration, potentially long-running).
fn step_kind_style(kind: StepKind, tx_type: TxType) -> Style {
    match kind {
        StepKind::Schema => STYLE_SAFE,
        StepKind::Backfill => STYLE_BREAKING,
        StepKind::Write => match tx_type {
            TxType::Write => STYLE_WARNING,
            TxType::Schema => STYLE_SAFE,
            TxType::Read => STYLE_DIM,
        },
    }
}

fn tx_type_label(tx_type: TxType) -> &'static str {
    match tx_type {
        TxType::Schema => "schema",
        TxType::Write => "write",
        TxType::Read => "read",
    }
}

/// Return the first `max_chars` characters of `typeql`, appending `…` if
/// truncated, after collapsing newlines to spaces for single-line display.
fn truncate_typeql(typeql: &str, max_chars: usize) -> String {
    let single_line: String = typeql.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= max_chars {
        single_line
    } else {
        let truncated: String = single_line.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}
