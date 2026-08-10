//! Provider-neutral, pre-I/O verification of one canonical migration apply plan.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::managed_scope::SemanticProfileBinding;
use type_bridge_contract::migration::{MigrationId, MigrationManifestDigest, MigrationStep};
use type_bridge_contract::migration_assertion::AssertionExpectation;
use type_bridge_contract::schema::{DeclaredSchema, ManagedSchemaState};
use type_bridge_query::ValidatedMigrationAssertionPlan;
use type_bridge_schema::{DeltaError, ManagedDeltaContext, SafetyDerivationProfile, apply_delta};

use crate::history::MigrationHistoryGraph;
use crate::lowering::{
    SchemaFactCatalog, SchemaLoweringBinding, SchemaLoweringDiagnostic, SchemaLoweringPlan,
    lower_schema_delta_with_verified_assertions,
};
use crate::manifest::{
    VerifiedSchemaMigrationManifest, verified_manifest_digest, verify_assertion_coverage,
};
use crate::policy::{MigrationApplyApproval, MigrationSafetyPolicy, SafetyPolicyDecision};

/// Select the target closure of a migration apply operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MigrationApplyTarget {
    /// Apply to the sole verified history head, rejecting an ambiguous history.
    DefaultHead,
    /// Apply the complete ancestor closure of these explicit targets.
    Explicit(BTreeSet<MigrationId>),
}

/// One trusted step aligned to its independently verified execution evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerifiedMigrationApplyStep {
    /// A state-preserving typed assertion retained at its exact manifest position.
    Assertion {
        /// The constructor-validated binding-neutral migration step.
        step: MigrationStep,
        /// Schema-aware validation rederived against the exact replayed source state.
        validated: Box<ValidatedMigrationAssertionPlan>,
    },
    /// A state-changing schema step with freshly derived provider lowering.
    SchemaDelta {
        /// The constructor-validated binding-neutral migration step.
        step: MigrationStep,
        /// The exact lowering derived from the replayed source and target catalogs.
        lowering: Box<SchemaLoweringPlan>,
    },
}

impl VerifiedMigrationApplyStep {
    /// Return the exact binding-neutral manifest step.
    pub const fn step(&self) -> &MigrationStep {
        match self {
            Self::Assertion { step, .. } | Self::SchemaDelta { step, .. } => step,
        }
    }

    /// Return schema-aware execution evidence only for an assertion step.
    pub fn validated_assertion(&self) -> Option<&ValidatedMigrationAssertionPlan> {
        match self {
            Self::Assertion { validated, .. } => Some(validated),
            Self::SchemaDelta { .. } => None,
        }
    }

    /// Return provider lowering only for a schema-delta step.
    pub fn lowering(&self) -> Option<&SchemaLoweringPlan> {
        match self {
            Self::Assertion { .. } => None,
            Self::SchemaDelta { lowering, .. } => Some(lowering),
        }
    }
}

/// One verifier-owned transaction group ending in exactly one schema delta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMigrationTransactionGroup {
    ordinal: usize,
    first_step_index: usize,
    schema_delta_step_index: usize,
}

impl VerifiedMigrationTransactionGroup {
    /// Return this group's zero-based position in its manifest.
    pub const fn ordinal(&self) -> usize {
        self.ordinal
    }

    /// Return the first assertion index, or the delta index when there are none.
    pub const fn first_step_index(&self) -> usize {
        self.first_step_index
    }

    /// Return the index of the group's single terminal schema delta.
    pub const fn schema_delta_step_index(&self) -> usize {
        self.schema_delta_step_index
    }

    /// Return the number of assertions executed before the schema delta.
    pub const fn assertion_count(&self) -> usize {
        self.schema_delta_step_index - self.first_step_index
    }

    /// Return the exclusive end of this group's step range.
    pub const fn end_step_index(&self) -> usize {
        self.schema_delta_step_index + 1
    }
}

/// Partition trusted apply evidence into exact assertion-plus-delta transactions.
pub fn partition_transaction_groups(
    steps: &[VerifiedMigrationApplyStep],
) -> Result<Vec<VerifiedMigrationTransactionGroup>, Diagnostic> {
    let mut groups = Vec::new();
    let mut first_step_index = 0;
    for (step_index, step) in steps.iter().enumerate() {
        if matches!(step, VerifiedMigrationApplyStep::Assertion { .. }) {
            continue;
        }
        let schema_step = step.step().as_schema_delta().ok_or_else(|| {
            contract_failure(
                DiagnosticCategory::Integrity,
                "migration_apply_group_step_kind_mismatch",
                "schema-delta apply evidence does not contain a schema-delta step",
            )
        })?;
        for assertion in &steps[first_step_index..step_index] {
            let Some(validated) = assertion.validated_assertion() else {
                return Err(contract_failure(
                    DiagnosticCategory::Integrity,
                    "migration_apply_group_step_kind_mismatch",
                    "a pre-delta transaction step is not validated assertion evidence",
                ));
            };
            let (contract, persisted, expectation) =
                assertion.step().as_assertion().ok_or_else(|| {
                    contract_failure(
                        DiagnosticCategory::Integrity,
                        "migration_apply_group_step_kind_mismatch",
                        "assertion apply evidence does not contain an assertion step",
                    )
                })?;
            if expectation != AssertionExpectation::NoRows
                || validated.plan().canonical_bytes()? != persisted.canonical_bytes()?
                || validated.plan().fingerprint()? != *contract.plan_fingerprint()
            {
                return Err(contract_failure(
                    DiagnosticCategory::Integrity,
                    "migration_apply_group_assertion_mismatch",
                    "validated assertion evidence differs from its persisted step",
                ));
            }
            if validated.source_state() != schema_step.delta().source() {
                return Err(contract_failure(
                    DiagnosticCategory::Integrity,
                    "migration_apply_group_source_state_mismatch",
                    "validated assertion source differs from the following delta source",
                ));
            }
        }
        groups.push(VerifiedMigrationTransactionGroup {
            ordinal: groups.len(),
            first_step_index,
            schema_delta_step_index: step_index,
        });
        first_step_index = step_index + 1;
    }
    if first_step_index != steps.len() {
        return Err(contract_failure(
            DiagnosticCategory::InvalidContract,
            "migration_apply_group_orphan_assertion",
            "transaction grouping ended with assertions that have no schema delta",
        ));
    }
    Ok(groups)
}

/// One verified manifest plus its external digest and rederived step evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMigrationApplyManifest {
    digest: MigrationManifestDigest,
    manifest: VerifiedSchemaMigrationManifest,
    steps: Vec<VerifiedMigrationApplyStep>,
    transaction_groups: Vec<VerifiedMigrationTransactionGroup>,
}

impl VerifiedMigrationApplyManifest {
    /// Return the replay-verified canonical manifest.
    pub const fn manifest(&self) -> &VerifiedSchemaMigrationManifest {
        &self.manifest
    }

    /// Return the raw SHA-256 digest of exact canonical manifest bytes.
    pub const fn digest(&self) -> MigrationManifestDigest {
        self.digest
    }

    /// Return exact ordered steps and their independently derived execution evidence.
    pub fn steps(&self) -> &[VerifiedMigrationApplyStep] {
        &self.steps
    }

    /// Return verifier-owned transaction grouping for these exact steps.
    pub fn transaction_groups(&self) -> &[VerifiedMigrationTransactionGroup] {
        &self.transaction_groups
    }
}

/// An opaque, deterministic, pre-I/O migration apply plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedMigrationApplyPlan {
    applied_migrations: Vec<MigrationId>,
    applied_frontier: Vec<MigrationId>,
    migrations: Vec<VerifiedMigrationApplyManifest>,
    required_capabilities: CapabilitySet,
    source_schema: Option<DeclaredSchema>,
    source_state: Option<ManagedSchemaState>,
    target_frontier: Vec<MigrationId>,
    target_schema: Option<DeclaredSchema>,
    target_state: Option<ManagedSchemaState>,
}

impl VerifiedMigrationApplyPlan {
    /// Return every migration identity supplied as applied, in canonical order.
    pub fn applied_migrations(&self) -> &[MigrationId] {
        &self.applied_migrations
    }

    /// Return the maximal applied identities observed by planning.
    pub fn applied_frontier(&self) -> &[MigrationId] {
        &self.applied_frontier
    }

    /// Return manifests in deterministic execution order.
    pub fn migrations(&self) -> &[VerifiedMigrationApplyManifest] {
        &self.migrations
    }

    /// Return the exact union of capabilities required by the planned manifests.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return the exact declared schema before the first planned migration, if any.
    pub const fn source_schema(&self) -> Option<&DeclaredSchema> {
        self.source_schema.as_ref()
    }

    /// Return the exact managed state before the first planned migration, if any.
    pub const fn source_state(&self) -> Option<&ManagedSchemaState> {
        self.source_state.as_ref()
    }

    /// Return the resulting maximal applied identities.
    pub fn target_frontier(&self) -> &[MigrationId] {
        &self.target_frontier
    }

    /// Return the exact declared schema after the last planned migration, if any.
    pub const fn target_schema(&self) -> Option<&DeclaredSchema> {
        self.target_schema.as_ref()
    }

    /// Return the exact managed state after the last planned migration, if any.
    pub const fn target_state(&self) -> Option<&ManagedSchemaState> {
        self.target_state.as_ref()
    }
}

/// Failure while deriving a provider-neutral apply plan before any I/O.
#[derive(Debug)]
pub enum MigrationApplyPlanError {
    /// A canonical contract, history, capability, or policy check failed.
    Contract(Diagnostic),
    /// Pure schema replay failed.
    Schema(DeltaError),
    /// Provider-profile lowering failed.
    Lowering(SchemaLoweringDiagnostic),
}

impl fmt::Display for MigrationApplyPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "{error}"),
            Self::Schema(_) => formatter.write_str("schema delta replay failed"),
            Self::Lowering(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for MigrationApplyPlanError {}

impl From<Diagnostic> for MigrationApplyPlanError {
    fn from(value: Diagnostic) -> Self {
        Self::Contract(value)
    }
}

impl From<DeltaError> for MigrationApplyPlanError {
    fn from(value: DeltaError) -> Self {
        Self::Schema(value)
    }
}

impl From<SchemaLoweringDiagnostic> for MigrationApplyPlanError {
    fn from(value: SchemaLoweringDiagnostic) -> Self {
        Self::Lowering(value)
    }
}

/// Build a complete apply plan from verified history and explicit execution policy.
///
/// Every check completes before lease acquisition or provider I/O. The function
/// independently replays and lowers every state-changing step rather than trusting
/// manifest-adjacent rendered statements.
/// A manifest whose class the policy gates behind approval executes only when an
/// approval binds its exact verified transition; the binding admits the manifest's
/// destructive statement units through the lowering gate.
pub fn build_verified_migration_apply_plan(
    graph: &MigrationHistoryGraph,
    applied: &BTreeSet<MigrationId>,
    target: &MigrationApplyTarget,
    delta_context: &ManagedDeltaContext,
    lowering_binding: &SchemaLoweringBinding,
    policy: &MigrationSafetyPolicy,
    approvals: &[MigrationApplyApproval],
) -> Result<VerifiedMigrationApplyPlan, MigrationApplyPlanError> {
    if lowering_binding.available_capabilities() != delta_context.available_capabilities() {
        return Err(contract_failure(
            DiagnosticCategory::InvalidContract,
            "migration_apply_capability_context_mismatch",
            "schema and lowering contexts must expose the same capability set",
        )
        .into());
    }
    let ordered_ids = match target {
        MigrationApplyTarget::DefaultHead => graph.plan_apply_to_default_head(applied)?,
        MigrationApplyTarget::Explicit(targets) => graph.plan_apply(applied, targets)?,
    };
    let applied_frontier = graph.applied_frontier(applied)?;
    let (mut current_schema, mut current_state) =
        coherent_frontier_state(graph, &applied_frontier)?;
    let source_from_applied = current_schema.clone();
    let state_from_applied = current_state.clone();
    let mut migrations = Vec::with_capacity(ordered_ids.len());
    let mut required_capabilities = CapabilitySet::new();

    for id in ordered_ids {
        let manifest = graph.manifest(&id).ok_or_else(|| {
            contract_failure(
                DiagnosticCategory::Integrity,
                "migration_apply_missing_verified_manifest",
                "history planning returned an identity without a verified manifest",
            )
        })?;
        if manifest.lowering_profile().id() != lowering_binding.profile_id()
            || manifest.lowering_profile().fingerprint() != lowering_binding.profile_fingerprint()
        {
            return Err(contract_failure(
                DiagnosticCategory::InvalidContract,
                "migration_apply_lowering_profile_mismatch",
                "manifest lowering profile differs from the explicit execution binding",
            )
            .into());
        }
        let approved = match policy.decision(manifest.safety()) {
            SafetyPolicyDecision::Allow => false,
            SafetyPolicyDecision::Reject => {
                return Err(contract_failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_apply_safety_policy_rejected",
                    "explicit apply policy rejects a manifest safety classification",
                )
                .into());
            }
            SafetyPolicyDecision::RequireApproval => {
                let mut bound = false;
                for approval in approvals {
                    if approval.binds(manifest)? {
                        bound = true;
                        break;
                    }
                }
                if !bound {
                    return Err(contract_failure(
                        DiagnosticCategory::InvalidContract,
                        "migration_apply_approval_required",
                        "manifest safety requires an approval bound to this exact transition",
                    )
                    .into());
                }
                true
            }
        };
        manifest
            .required_capabilities()
            .ensure_supported_by(lowering_binding.available_capabilities())?;
        for capability in manifest.required_capabilities().iter().cloned() {
            required_capabilities.insert(capability);
        }
        let safety_profile = SafetyDerivationProfile::new(
            SemanticProfileBinding::resolve(delta_context.semantic_profile().clone())?,
            manifest.lowering_profile().clone(),
        )?;

        match &current_schema {
            Some(schema)
                if schema.declared_identity_fingerprint()
                    != manifest.source_schema().declared_identity_fingerprint() =>
            {
                return Err(contract_failure(
                    DiagnosticCategory::Integrity,
                    "migration_apply_schema_chain_mismatch",
                    "planned manifest source schema does not continue the applied chain",
                )
                .into());
            }
            None => {
                current_schema = Some(manifest.source_schema().clone());
                current_state = Some(manifest.source_state().clone());
            }
            Some(_) => {}
        }
        if current_state.as_ref() != Some(manifest.source_state()) {
            return Err(contract_failure(
                DiagnosticCategory::Integrity,
                "migration_apply_state_chain_mismatch",
                "planned manifest source state does not continue the applied chain",
            )
            .into());
        }

        let mut replayed = current_schema
            .clone()
            .expect("a planned manifest establishes a source schema");
        let mut verified_steps = Vec::with_capacity(manifest.steps().len());
        let mut pending_assertions = Vec::new();
        for step in manifest.steps() {
            step.validate()?;
            step.required_capabilities()
                .ensure_supported_by(lowering_binding.available_capabilities())?;
            let Some(schema_step) = step.as_schema_delta() else {
                pending_assertions.push(step);
                continue;
            };
            let target_schema = apply_delta(&replayed, schema_step.delta(), delta_context)?;
            let coverage = verify_assertion_coverage(
                &pending_assertions,
                schema_step.delta(),
                &replayed,
                &target_schema,
                &safety_profile,
            )?;
            for (assertion, validated) in pending_assertions.iter().zip(coverage.validated()) {
                verified_steps.push(VerifiedMigrationApplyStep::Assertion {
                    step: (*assertion).clone(),
                    validated: Box::new(validated.clone()),
                });
            }
            pending_assertions.clear();
            let source_catalog = SchemaFactCatalog::new(replayed.facts().cloned())?;
            let target_catalog = SchemaFactCatalog::new(target_schema.facts().cloned())?;
            let lowering = lower_schema_delta_with_verified_assertions(
                schema_step.delta(),
                &source_catalog,
                &target_catalog,
                lowering_binding,
                coverage.discharged_operation_indices(),
                approved,
            )?;
            verified_steps.push(VerifiedMigrationApplyStep::SchemaDelta {
                step: step.clone(),
                lowering: Box::new(lowering),
            });
            replayed = target_schema;
        }
        if !pending_assertions.is_empty() {
            return Err(contract_failure(
                DiagnosticCategory::InvalidContract,
                "migration_apply_orphan_assertion",
                "assertion steps must be immediately followed by a schema delta",
            )
            .into());
        }
        if replayed.declared_identity_fingerprint()
            != manifest.target_schema().declared_identity_fingerprint()
        {
            return Err(contract_failure(
                DiagnosticCategory::Integrity,
                "migration_apply_manifest_replay_mismatch",
                "independent apply-plan replay differs from the verified manifest target",
            )
            .into());
        }
        let transaction_groups = partition_transaction_groups(&verified_steps)?;
        migrations.push(VerifiedMigrationApplyManifest {
            digest: verified_manifest_digest(manifest)?,
            manifest: manifest.clone(),
            steps: verified_steps,
            transaction_groups,
        });
        current_schema = Some(manifest.target_schema().clone());
        current_state = Some(manifest.target_state().clone());
    }

    let source_schema = source_from_applied.or_else(|| {
        migrations
            .first()
            .map(|entry| entry.manifest().source_schema().clone())
    });
    let source_state = state_from_applied.or_else(|| {
        migrations
            .first()
            .map(|entry| entry.manifest().source_state().clone())
    });
    let mut resulting_applied = applied.clone();
    resulting_applied.extend(migrations.iter().map(|entry| entry.manifest().id().clone()));
    let target_frontier = graph.applied_frontier(&resulting_applied)?;
    Ok(VerifiedMigrationApplyPlan {
        applied_migrations: applied.iter().cloned().collect(),
        applied_frontier,
        migrations,
        required_capabilities,
        source_schema,
        source_state,
        target_frontier,
        target_schema: current_schema,
        target_state: current_state,
    })
}

pub(crate) fn coherent_frontier_state(
    graph: &MigrationHistoryGraph,
    frontier: &[MigrationId],
) -> Result<(Option<DeclaredSchema>, Option<ManagedSchemaState>), MigrationApplyPlanError> {
    let mut schema: Option<DeclaredSchema> = None;
    let mut state: Option<ManagedSchemaState> = None;
    for id in frontier {
        let manifest = graph.manifest(id).ok_or_else(|| {
            contract_failure(
                DiagnosticCategory::Integrity,
                "migration_apply_missing_frontier_manifest",
                "applied frontier identity has no verified manifest",
            )
        })?;
        if schema.as_ref().is_some_and(|current| {
            current.declared_identity_fingerprint()
                != manifest.target_schema().declared_identity_fingerprint()
        }) || state
            .as_ref()
            .is_some_and(|current| current != manifest.target_state())
        {
            return Err(contract_failure(
                DiagnosticCategory::Integrity,
                "migration_apply_divergent_frontier",
                "applied frontier does not identify one exact managed schema state",
            )
            .into());
        }
        schema = Some(manifest.target_schema().clone());
        state = Some(manifest.target_state().clone());
    }
    Ok((schema, state))
}

pub(crate) fn contract_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static migration apply diagnostic code"),
        message,
    )
}
