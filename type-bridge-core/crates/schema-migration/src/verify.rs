//! Read-only drift verification across the migration state triad.
//!
//! `verify` checks the three authorities against each other — committed
//! immutable manifests, the applied ledger, and the desired schema — plus
//! the observed live managed semantics. It reports drift; it never repairs,
//! never generates work from production state, and never infers intent. A
//! schema mismatch is drift, not an invitation to reconcile automatically.

use std::collections::BTreeSet;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::migration::MigrationId;
use type_bridge_contract::schema::{DeclaredSchema, ManagedSchemaState};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_schema::{ManagedDeltaContext, managed_schema_state};

use type_bridge_contract::diagnostic::{DiagnosticCategory, DiagnosticCode};

use crate::apply_plan::MigrationApplyPlanError;
use crate::history::MigrationHistoryGraph;
use crate::manifest::delta_diagnostic;

/// One verified drift finding.
///
/// Every variant names the exact authorities that disagree; none carries a
/// proposed repair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MigrationDriftFinding {
    /// The applied ledger is not a valid downward-closed prefix/DAG frontier
    /// of verified history, or its frontier identifies no single state.
    AppliedLedger {
        /// The exact ledger validation failure.
        diagnostic: Diagnostic,
    },
    /// The live managed semantics differ from the recorded frontier target.
    LiveSemantics {
        /// Semantics the applied frontier's verified manifests recorded.
        recorded: ManagedSemanticSchemaFingerprint,
        /// Semantics observed live under the same profile and scope.
        observed: ManagedSemanticSchemaFingerprint,
    },
    /// The desired schema semantics differ from the selected head target.
    DesiredDivergence {
        /// Semantics the selected migration head reaches.
        head: ManagedSemanticSchemaFingerprint,
        /// Semantics the desired schema declares.
        desired: ManagedSemanticSchemaFingerprint,
    },
    /// Verified history extends beyond the applied frontier.
    PendingMigrations {
        /// Dependency-ordered identities not yet applied.
        pending: Vec<MigrationId>,
    },
    /// A verified manifest requires capabilities the context cannot meet.
    Capabilities {
        /// The exact capability negotiation failure.
        diagnostic: Diagnostic,
    },
}

/// The complete read-only verification report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationVerifyReport {
    findings: Vec<MigrationDriftFinding>,
    applied_frontier: Vec<MigrationId>,
    frontier_semantics: Option<ManagedSemanticSchemaFingerprint>,
    observed_semantics: Option<ManagedSemanticSchemaFingerprint>,
}

impl MigrationVerifyReport {
    /// Return whether every check passed with no drift.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty()
    }

    /// Return all findings in the canonical check order.
    pub fn findings(&self) -> &[MigrationDriftFinding] {
        &self.findings
    }

    /// Return the maximal applied identities, when the ledger is valid.
    pub fn applied_frontier(&self) -> &[MigrationId] {
        &self.applied_frontier
    }

    /// Return the semantics the applied frontier records, when valid.
    pub const fn frontier_semantics(&self) -> Option<&ManagedSemanticSchemaFingerprint> {
        self.frontier_semantics.as_ref()
    }

    /// Return the observed live managed semantics, when supplied.
    pub const fn observed_semantics(&self) -> Option<&ManagedSemanticSchemaFingerprint> {
        self.observed_semantics.as_ref()
    }

    /// Prepend one independently observed applied-ledger drift.
    ///
    /// Backend adapters use this for read-only legacy cutover bindings that
    /// are outside the canonical V2 journal but remain part of its permanent
    /// lineage authority. Inserting at the front preserves the documented
    /// ordering in which applied-ledger defects precede semantic drift.
    pub fn prepend_applied_ledger_drift(&mut self, diagnostic: Diagnostic) {
        self.findings
            .insert(0, MigrationDriftFinding::AppliedLedger { diagnostic });
    }
}

/// Verify the migration state triad and report every drift finding.
///
/// Checks run in the canonical order — applied-ledger validity, live
/// semantics against the recorded frontier target, desired semantics against
/// the selected head, pending work, and capability negotiation. Structural
/// failures of the history graph itself (an ambiguous default head, a
/// missing manifest) are errors, not findings: they mean there is no single
/// authority to verify against.
pub fn verify_migration_state(
    graph: &MigrationHistoryGraph,
    applied: &BTreeSet<MigrationId>,
    genesis_source: &DeclaredSchema,
    desired: Option<&DeclaredSchema>,
    observed_live: Option<&ManagedSchemaState>,
    context: &ManagedDeltaContext,
) -> Result<MigrationVerifyReport, Diagnostic> {
    let mut findings = Vec::new();

    let genesis_state = managed_schema_state(genesis_source, context).map_err(delta_diagnostic)?;
    let mut applied_frontier = Vec::new();
    let mut frontier_state = None;
    match graph.applied_frontier(applied) {
        Ok(frontier) => match crate::apply_plan::coherent_frontier_state(graph, &frontier) {
            Ok((_, state)) => {
                applied_frontier = frontier;
                frontier_state = Some(state.unwrap_or(genesis_state));
            }
            Err(error) => findings.push(MigrationDriftFinding::AppliedLedger {
                diagnostic: plan_error_diagnostic(error),
            }),
        },
        Err(diagnostic) => {
            findings.push(MigrationDriftFinding::AppliedLedger { diagnostic });
        }
    }

    let frontier_semantics = frontier_state
        .as_ref()
        .map(|state| state.managed_semantic_schema().clone());
    let observed_semantics = observed_live.map(|state| state.managed_semantic_schema().clone());
    if let (Some(recorded), Some(observed)) =
        (frontier_semantics.as_ref(), observed_semantics.as_ref())
        && recorded != observed
    {
        findings.push(MigrationDriftFinding::LiveSemantics {
            recorded: recorded.clone(),
            observed: observed.clone(),
        });
    }

    let head_state = match graph.default_head()? {
        Some(head) => graph
            .manifest(head)
            .map(|manifest| manifest.target_state().clone()),
        None => Some(managed_schema_state(genesis_source, context).map_err(delta_diagnostic)?),
    };
    if let (Some(desired), Some(head_state)) = (desired, head_state.as_ref()) {
        let desired_state = managed_schema_state(desired, context).map_err(delta_diagnostic)?;
        if desired_state.managed_semantic_schema() != head_state.managed_semantic_schema() {
            findings.push(MigrationDriftFinding::DesiredDivergence {
                head: head_state.managed_semantic_schema().clone(),
                desired: desired_state.managed_semantic_schema().clone(),
            });
        }
    }

    if graph.applied_frontier(applied).is_ok() {
        let pending = graph.plan_apply_to_default_head(applied)?;
        if !pending.is_empty() {
            findings.push(MigrationDriftFinding::PendingMigrations { pending });
        }
    }

    for (_, manifest) in graph.manifests() {
        if let Err(diagnostic) = manifest
            .required_capabilities()
            .ensure_supported_by(context.available_capabilities())
        {
            findings.push(MigrationDriftFinding::Capabilities { diagnostic });
            break;
        }
    }

    Ok(MigrationVerifyReport {
        findings,
        applied_frontier,
        frontier_semantics,
        observed_semantics,
    })
}

fn plan_error_diagnostic(error: MigrationApplyPlanError) -> Diagnostic {
    match error {
        MigrationApplyPlanError::Contract(diagnostic) => diagnostic,
        MigrationApplyPlanError::Schema(delta) => delta_diagnostic(delta),
        MigrationApplyPlanError::Lowering(lowering) => Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new(lowering.code()).expect("lowering diagnostic codes are canonical"),
            "frontier verification failed in provider lowering",
        ),
    }
}
