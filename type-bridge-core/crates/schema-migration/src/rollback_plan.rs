//! Provider-neutral, pre-I/O verification of one reverse-topological rollback plan.
//!
//! Rollback executes the reverse programs the verified manifests already
//! carry: each schema step's recorded reverse delta is independently
//! replayed, honestly re-classified, policy-gated, and lowered before any
//! provider I/O. A manifest without a real semantic inverse cannot be rolled
//! back through this path — it stays manually reversible.

use std::collections::BTreeSet;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::migration::{MigrationId, MigrationManifestDigest, MigrationStepId};
use type_bridge_contract::schema::{DeclaredSchema, ManagedSchemaState, SchemaDelta};
use type_bridge_schema::{ManagedDeltaContext, SafetyClass, SafetyDerivationProfile, apply_delta};

use crate::apply_plan::{MigrationApplyPlanError, coherent_frontier_state, contract_failure};
use crate::history::MigrationHistoryGraph;
use crate::lowering::{
    SchemaFactCatalog, SchemaLoweringBinding, SchemaLoweringPlan,
    lower_schema_delta_with_verified_assertions,
};
use crate::manifest::{
    VerifiedSchemaMigrationManifest, verified_manifest_digest, verify_assertion_coverage,
};
use crate::policy::{MigrationApplyApproval, MigrationSafetyPolicy, SafetyPolicyDecision};

/// One lowered reverse program for one forward schema step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMigrationRollbackStep {
    forward_step_id: MigrationStepId,
    lowering: SchemaLoweringPlan,
}

impl VerifiedMigrationRollbackStep {
    /// Return the identity of the forward step this program reverses.
    pub const fn forward_step_id(&self) -> &MigrationStepId {
        &self.forward_step_id
    }

    /// Return the lowered reverse statement plan.
    pub const fn lowering(&self) -> &SchemaLoweringPlan {
        &self.lowering
    }
}

/// One verified manifest with its complete reverse program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMigrationRollbackManifest {
    digest: MigrationManifestDigest,
    manifest: VerifiedSchemaMigrationManifest,
    rollback_safety: SafetyClass,
    steps: Vec<VerifiedMigrationRollbackStep>,
}

impl VerifiedMigrationRollbackManifest {
    /// Return the exact canonical digest of the manifest being rolled back.
    pub const fn digest(&self) -> &MigrationManifestDigest {
        &self.digest
    }

    /// Return the verified manifest being rolled back.
    pub const fn manifest(&self) -> &VerifiedSchemaMigrationManifest {
        &self.manifest
    }

    /// Return the honest classification of the complete reverse program.
    pub const fn rollback_safety(&self) -> SafetyClass {
        self.rollback_safety
    }

    /// Return lowered reverse programs in execution order (last step first).
    pub fn steps(&self) -> &[VerifiedMigrationRollbackStep] {
        &self.steps
    }

    /// Return the verified reverse delta one rollback step executes.
    ///
    /// The manifest stays the trust anchor: the reverse program is read back
    /// from the forward step this rollback step names, never from a copy.
    pub fn reverse_delta(
        &self,
        step: &VerifiedMigrationRollbackStep,
    ) -> Result<&SchemaDelta, Diagnostic> {
        self.manifest
            .steps()
            .iter()
            .filter_map(|candidate| candidate.as_schema_delta())
            .find(|candidate| candidate.contract().id() == step.forward_step_id())
            .and_then(|candidate| candidate.contract().reverse())
            .ok_or_else(|| {
                contract_failure(
                    DiagnosticCategory::Integrity,
                    "migration_rollback_missing_reverse_step",
                    "rollback step names a forward step without a verified reverse program",
                )
            })
    }
}

/// A complete pre-verified rollback plan in reverse-topological order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMigrationRollbackPlan {
    rollbacks: Vec<VerifiedMigrationRollbackManifest>,
    remaining_applied: Vec<MigrationId>,
    source_state: ManagedSchemaState,
    target_schema: DeclaredSchema,
    target_state: ManagedSchemaState,
}

impl VerifiedMigrationRollbackPlan {
    /// Return rollback manifests in execution order.
    pub fn rollbacks(&self) -> &[VerifiedMigrationRollbackManifest] {
        &self.rollbacks
    }

    /// Return the applied identities that survive the rollback, in order.
    pub fn remaining_applied(&self) -> &[MigrationId] {
        &self.remaining_applied
    }

    /// Return the exact managed state the rollback starts from.
    pub const fn source_state(&self) -> &ManagedSchemaState {
        &self.source_state
    }

    /// Return the exact declared schema the rollback restores.
    pub const fn target_schema(&self) -> &DeclaredSchema {
        &self.target_schema
    }

    /// Return the exact managed state the rollback restores.
    pub const fn target_state(&self) -> &ManagedSchemaState {
        &self.target_state
    }

    /// Return the complete canonically ordered applied basis this plan assumed.
    ///
    /// The basis is the union of the rolled-back identities and the surviving
    /// ones; execution compares it against the live applied ledger so a plan
    /// built from a stale ledger fails closed before provider I/O.
    pub fn applied_basis(&self) -> BTreeSet<MigrationId> {
        self.rollbacks
            .iter()
            .map(|rollback| rollback.manifest().id().clone())
            .chain(self.remaining_applied.iter().cloned())
            .collect()
    }
}

/// Build a complete rollback plan from verified history and explicit policy.
///
/// Ordering, downward-closure, and remaining-descendant rejection come from
/// the history graph. Every reverse program is independently replayed against
/// the walked chain state and honestly re-classified: rolling back additive
/// work destroys what it added, so the reverse classification — not the
/// forward one — meets the policy, and an approval must bind the rollback
/// transition (the manifest's states swapped).
pub fn build_verified_migration_rollback_plan(
    graph: &MigrationHistoryGraph,
    applied: &BTreeSet<MigrationId>,
    removals: &BTreeSet<MigrationId>,
    delta_context: &ManagedDeltaContext,
    lowering_binding: &SchemaLoweringBinding,
    policy: &MigrationSafetyPolicy,
    approvals: &[MigrationApplyApproval],
) -> Result<VerifiedMigrationRollbackPlan, MigrationApplyPlanError> {
    if lowering_binding.available_capabilities() != delta_context.available_capabilities() {
        return Err(contract_failure(
            DiagnosticCategory::InvalidContract,
            "migration_rollback_capability_context_mismatch",
            "schema and lowering contexts must expose the same capability set",
        )
        .into());
    }
    let ordered_ids = graph.plan_rollback(applied, removals)?;
    let applied_frontier = graph.applied_frontier(applied)?;
    let (frontier_schema, frontier_state) = coherent_frontier_state(graph, &applied_frontier)?;
    let (Some(mut current_schema), Some(mut current_state)) = (frontier_schema, frontier_state)
    else {
        return Err(contract_failure(
            DiagnosticCategory::InvalidContract,
            "migration_rollback_empty_applied",
            "rollback requires at least one applied migration",
        )
        .into());
    };
    let source_state = current_state.clone();

    let mut rollbacks = Vec::with_capacity(ordered_ids.len());
    for id in &ordered_ids {
        let manifest = graph.manifest(id).ok_or_else(|| {
            contract_failure(
                DiagnosticCategory::Integrity,
                "migration_rollback_missing_verified_manifest",
                "rollback planning returned an identity without a verified manifest",
            )
        })?;
        if manifest.is_legacy_bridge() {
            return Err(contract_failure(
                DiagnosticCategory::InvalidContract,
                "migration_rollback_legacy_bridge_permanent",
                "the legacy cutover bridge is a permanent lineage root and cannot be rolled back",
            )
            .into());
        }
        if manifest.lowering_profile().id() != lowering_binding.profile_id()
            || manifest.lowering_profile().fingerprint() != lowering_binding.profile_fingerprint()
        {
            return Err(contract_failure(
                DiagnosticCategory::InvalidContract,
                "migration_rollback_lowering_profile_mismatch",
                "manifest lowering profile differs from the explicit execution binding",
            )
            .into());
        }
        if !manifest.reversible() {
            return Err(contract_failure(
                DiagnosticCategory::InvalidContract,
                "migration_rollback_irreversible",
                "manifest carries no verified reverse program and stays manually reversible",
            )
            .into());
        }
        if current_state != *manifest.target_state()
            || current_schema.declared_identity_fingerprint()
                != manifest.target_schema().declared_identity_fingerprint()
        {
            return Err(contract_failure(
                DiagnosticCategory::Integrity,
                "migration_rollback_state_chain_mismatch",
                "manifest target state does not continue the rollback chain",
            )
            .into());
        }

        let safety_profile = SafetyDerivationProfile::new(
            manifest.semantic_profile().clone(),
            manifest.lowering_profile().clone(),
        )?;

        // Replay the forward steps to recover each step's exact intermediate
        // target, then reverse them back-to-front through the recorded
        // reverse programs.
        let mut boundaries = vec![manifest.source_schema().clone()];
        for step in manifest.steps() {
            if let Some(schema_step) = step.as_schema_delta() {
                let source = boundaries
                    .last()
                    .expect("the boundary walk always starts from the source");
                boundaries.push(apply_delta(source, schema_step.delta(), delta_context)?);
            }
        }

        let mut rollback_safety = SafetyClass::FormalOnly;
        let mut steps = Vec::new();
        let mut boundary_index = boundaries.len() - 1;
        for step in manifest.steps().iter().rev() {
            let Some(schema_step) = step.as_schema_delta() else {
                // Assertions guard forward execution; the reverse program has
                // no data precondition by manifest-verification guarantee.
                continue;
            };
            let reverse = schema_step.contract().reverse().ok_or_else(|| {
                contract_failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_rollback_irreversible",
                    "schema step carries no verified reverse program",
                )
            })?;
            reverse
                .required_capabilities()
                .ensure_supported_by(lowering_binding.available_capabilities())?;
            let step_target = &boundaries[boundary_index];
            let step_source = &boundaries[boundary_index - 1];
            boundary_index -= 1;
            let restored = apply_delta(step_target, reverse, delta_context)?;
            if restored.declared_identity_fingerprint()
                != step_source.declared_identity_fingerprint()
            {
                return Err(contract_failure(
                    DiagnosticCategory::Integrity,
                    "migration_rollback_reverse_replay_mismatch",
                    "independent reverse replay does not restore the exact step source",
                )
                .into());
            }
            let coverage =
                verify_assertion_coverage(&[], reverse, step_target, &restored, &safety_profile)?;
            rollback_safety = rollback_safety.max(coverage.effective_safety());
            let source_catalog = SchemaFactCatalog::new(step_target.facts().cloned())?;
            let target_catalog = SchemaFactCatalog::new(restored.facts().cloned())?;
            steps.push((
                schema_step.contract().id().clone(),
                reverse.clone(),
                source_catalog,
                target_catalog,
                coverage.discharged_operation_indices().to_vec(),
            ));
        }

        let approved = match policy.decision(rollback_safety) {
            SafetyPolicyDecision::Allow => false,
            SafetyPolicyDecision::Reject => {
                return Err(contract_failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_rollback_safety_policy_rejected",
                    "explicit policy rejects the reverse program classification",
                )
                .into());
            }
            SafetyPolicyDecision::RequireApproval => {
                let mut bound = false;
                for approval in approvals {
                    if approval.binds_rollback(manifest, rollback_safety)? {
                        bound = true;
                        break;
                    }
                }
                if !bound {
                    return Err(contract_failure(
                        DiagnosticCategory::InvalidContract,
                        "migration_rollback_approval_required",
                        "reverse program requires an approval bound to this exact rollback",
                    )
                    .into());
                }
                true
            }
        };

        let lowered_steps = steps
            .into_iter()
            .map(
                |(forward_step_id, reverse, source_catalog, target_catalog, indices)| {
                    Ok(VerifiedMigrationRollbackStep {
                        forward_step_id,
                        lowering: lower_schema_delta_with_verified_assertions(
                            &reverse,
                            &source_catalog,
                            &target_catalog,
                            lowering_binding,
                            &indices,
                            approved,
                        )?,
                    })
                },
            )
            .collect::<Result<Vec<_>, MigrationApplyPlanError>>()?;

        rollbacks.push(VerifiedMigrationRollbackManifest {
            digest: verified_manifest_digest(manifest)?,
            manifest: manifest.clone(),
            rollback_safety,
            steps: lowered_steps,
        });
        current_schema = manifest.source_schema().clone();
        current_state = manifest.source_state().clone();
    }

    let remaining_applied = applied
        .iter()
        .filter(|id| !removals.contains(*id))
        .cloned()
        .collect();
    Ok(VerifiedMigrationRollbackPlan {
        rollbacks,
        remaining_applied,
        source_state,
        target_schema: current_schema,
        target_state: current_state,
    })
}
