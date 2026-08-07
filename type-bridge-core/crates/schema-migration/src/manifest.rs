//! Closed, replay-verified canonical schema migration manifests.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{from_canonical_json, to_canonical_json};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::limits::{MAX_CANONICAL_COLLECTION_LEN, StructuralLimits};
use type_bridge_contract::managed_scope::{ManagedScopeBinding, SemanticProfileBinding};
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationFormat, MigrationId, MigrationManifestDigest, MigrationName,
    MigrationPlanFingerprint, MigrationStep, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::{
    AssertionExpectation, decode_migration_assertion_plan,
};
use type_bridge_contract::schema::{
    DeclaredIdentityFingerprint, DeclaredSchema, decode_schema_delta,
};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::schema_fingerprint::{
    ManagedDeclaredIdentityFingerprint, ManagedSemanticSchemaFingerprint,
};
use type_bridge_contract::schema_lowering::SchemaLoweringProfileBinding;
use type_bridge_query::{
    MigrationAssertionValidationContext, ValidatedMigrationAssertionPlan, lower_condition_to_plan,
};
use type_bridge_schema::{
    DeltaError, ManagedDeltaContext, RequiredSafetyCondition, SafetyClass, SafetyCondition,
    SafetyDerivationProfile, apply_delta, classify_delta_safety, derive_safety_conditions,
    managed_schema_state, plan_schema_operations, resolve,
};

use crate::legacy::{
    LEGACY_APPLIED_SET_ALGORITHM, LEGACY_APPLIED_SET_CANONICALIZATION, LEGACY_CHECKSUM_ALGORITHM,
    LegacyAppliedSetDigest, LegacyMigrationChecksum, LegacyMigrationId, LegacyMigrationReference,
};
use crate::profile::schema_lowering_profile_binding;

const MANIFEST_SCHEMA_CANONICALIZATION: &str = "typebridge.schema-c14n/v2";
const MANIFEST_CODEC: &str = "typebridge.canonical-json/v1";
const MANIFEST_DELTA_IR: &str = "typebridge.schema-delta/v1";

/// Trusted authoring input containing only context-free schema steps.
///
/// This type deliberately has no serialization implementation. Persisted bytes
/// can only be produced after replay by [`build_verified_manifest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaMigrationDraft {
    id: MigrationId,
    parents: Vec<MigrationId>,
    steps: Vec<MigrationStep>,
    legacy_parents: Vec<LegacyMigrationReference>,
    legacy_applied_set: Option<LegacyAppliedSetDigest>,
}

impl SchemaMigrationDraft {
    /// Construct a draft, canonicalizing its set-like parent order.
    pub fn new<S>(
        id: MigrationId,
        mut parents: Vec<MigrationId>,
        steps: Vec<S>,
    ) -> Result<Self, Diagnostic>
    where
        S: Into<MigrationStep>,
    {
        let steps = steps.into_iter().map(Into::into).collect::<Vec<_>>();
        if steps.len() > MAX_CANONICAL_COLLECTION_LEN {
            return Err(failure(
                DiagnosticCategory::ResourceLimit,
                "migration_manifest_step_limit",
                "migration draft exceeds the canonical step-count ceiling",
            ));
        }
        parents.sort();
        if parents.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_duplicate_parent",
                "migration draft contains a duplicate parent identity",
            ));
        }
        if parents.iter().any(|parent| parent == &id) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_self_parent",
                "migration draft cannot name itself as a parent",
            ));
        }
        let mut step_ids = BTreeSet::new();
        for step in &steps {
            step.validate()?;
            if !step_ids.insert(step.id().clone()) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_manifest_duplicate_step_id",
                    "migration draft contains a duplicate step identity",
                ));
            }
        }
        Ok(Self {
            id,
            parents,
            steps,
            legacy_parents: Vec::new(),
            legacy_applied_set: None,
        })
    }

    /// Construct the zero-operation legacy-frontier bridge draft.
    ///
    /// The bridge is the only manifest that names legacy parents; it carries
    /// no steps and no canonical parents, so its verified source and target
    /// are the identical reconstructed legacy head.
    pub fn legacy_bridge(
        id: MigrationId,
        mut legacy_parents: Vec<LegacyMigrationReference>,
        legacy_applied_set: LegacyAppliedSetDigest,
    ) -> Result<Self, Diagnostic> {
        if legacy_parents.is_empty() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_empty_legacy_frontier",
                "a legacy-frontier bridge must name at least one legacy parent",
            ));
        }
        if legacy_parents.len() > MAX_CANONICAL_COLLECTION_LEN {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_legacy_frontier_too_large",
                "legacy-frontier bridge exceeds the canonical collection ceiling",
            ));
        }
        legacy_parents.sort();
        if legacy_parents
            .windows(2)
            .any(|pair| pair[0].id() == pair[1].id())
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_duplicate_legacy_parent",
                "legacy-frontier bridge names a legacy identity twice",
            ));
        }
        Ok(Self {
            id,
            parents: Vec::new(),
            steps: Vec::new(),
            legacy_parents,
            legacy_applied_set: Some(legacy_applied_set),
        })
    }

    /// Return the compound migration identity.
    pub const fn id(&self) -> &MigrationId {
        &self.id
    }

    /// Return canonical-sorted parent identities.
    pub fn parents(&self) -> &[MigrationId] {
        &self.parents
    }

    /// Return ordered trusted schema steps.
    pub fn steps(&self) -> &[MigrationStep] {
        &self.steps
    }

    /// Return canonical-sorted legacy frontier references (bridge only).
    pub fn legacy_parents(&self) -> &[LegacyMigrationReference] {
        &self.legacy_parents
    }

    /// Return the complete released applied-set digest (bridge only).
    pub const fn legacy_applied_set(&self) -> Option<&LegacyAppliedSetDigest> {
        self.legacy_applied_set.as_ref()
    }
}

/// A manifest whose complete schema program has been replayed and recomputed.
///
/// The type is Serialize-free by design; use [`encode_verified_manifest`] so
/// canonical limits and the closed wire shape cannot be bypassed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSchemaMigrationManifest {
    format: MigrationFormat,
    id: MigrationId,
    legacy_parents: Vec<LegacyMigrationReference>,
    legacy_applied_set: Option<LegacyAppliedSetDigest>,
    lowering_profile: SchemaLoweringProfileBinding,
    managed_scope: ManagedScopeBinding,
    parents: Vec<MigrationId>,
    plan_fingerprint: MigrationPlanFingerprint,
    required_capabilities: CapabilitySet,
    reversible: bool,
    safety: SafetyClass,
    semantic_profile: SemanticProfileBinding,
    source_schema: DeclaredSchema,
    source_state: ManagedSchemaState,
    steps: Vec<MigrationStep>,
    target_schema: DeclaredSchema,
    target_state: ManagedSchemaState,
}

impl VerifiedSchemaMigrationManifest {
    /// Return the closed migration manifest format identifier.
    pub const fn format(&self) -> &MigrationFormat {
        &self.format
    }

    /// Return the content-derived migration identity.
    pub const fn id(&self) -> &MigrationId {
        &self.id
    }

    /// Return canonical parent migration identities.
    pub fn parents(&self) -> &[MigrationId] {
        &self.parents
    }

    /// Return canonical-sorted legacy frontier references (bridge only).
    pub fn legacy_parents(&self) -> &[LegacyMigrationReference] {
        &self.legacy_parents
    }

    /// Return the complete released applied-set digest (bridge only).
    pub const fn legacy_applied_set(&self) -> Option<&LegacyAppliedSetDigest> {
        self.legacy_applied_set.as_ref()
    }

    /// Return whether this manifest is the zero-operation legacy bridge.
    pub fn is_legacy_bridge(&self) -> bool {
        !self.legacy_parents.is_empty()
    }

    /// Return the verified migration steps in execution order.
    pub fn steps(&self) -> &[MigrationStep] {
        &self.steps
    }

    /// Return the managed-scope binding verified during replay.
    pub const fn managed_scope(&self) -> &ManagedScopeBinding {
        &self.managed_scope
    }

    /// Return the semantic-profile binding verified during replay.
    pub const fn semantic_profile(&self) -> &SemanticProfileBinding {
        &self.semantic_profile
    }

    /// Return the provider lowering-profile binding verified during replay.
    pub const fn lowering_profile(&self) -> &SchemaLoweringProfileBinding {
        &self.lowering_profile
    }

    /// Return the capabilities required to execute every step.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return the highest safety class in the migration plan.
    pub const fn safety(&self) -> SafetyClass {
        self.safety
    }

    /// Return whether the verified migration has a complete inverse plan.
    pub const fn reversible(&self) -> bool {
        self.reversible
    }

    /// Return the canonical execution-plan fingerprint.
    pub const fn plan_fingerprint(&self) -> &MigrationPlanFingerprint {
        &self.plan_fingerprint
    }

    /// Return the exact managed schema state expected before execution.
    pub const fn source_state(&self) -> &ManagedSchemaState {
        &self.source_state
    }

    /// Return the replay-authoritative declared schema at this migration's source.
    ///
    /// This is verified runtime state and is deliberately absent from the
    /// canonical manifest wire, whose source identity claims remain unchanged.
    pub const fn source_schema(&self) -> &DeclaredSchema {
        &self.source_schema
    }

    /// Return the exact managed schema state expected after execution.
    pub const fn target_state(&self) -> &ManagedSchemaState {
        &self.target_state
    }

    /// Return the replay-authoritative declared target schema.
    pub const fn target_schema(&self) -> &DeclaredSchema {
        &self.target_schema
    }
}

/// Replay a trusted draft against an exact declared source and managed context.
pub fn build_verified_manifest(
    draft: SchemaMigrationDraft,
    context: (&DeclaredSchema, &ManagedDeltaContext),
) -> Result<VerifiedSchemaMigrationManifest, Diagnostic> {
    let (source_schema, delta_context) = context;
    let source_state =
        managed_schema_state(source_schema, delta_context).map_err(delta_diagnostic)?;
    let managed_scope = ManagedScopeBinding::exclusive(delta_context.scope_id().clone())?;
    if source_state.scope() != &managed_scope {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_manifest_scope_mismatch",
            "managed source state does not match the verification scope binding",
        ));
    }
    let semantic_profile =
        SemanticProfileBinding::resolve(delta_context.semantic_profile().clone())?;
    let lowering_profile = schema_lowering_profile_binding()?;
    let safety_profile =
        SafetyDerivationProfile::new(semantic_profile.clone(), lowering_profile.clone())?;

    let SchemaMigrationDraft {
        id,
        parents,
        steps,
        legacy_parents,
        legacy_applied_set,
    } = draft;
    if legacy_parents.is_empty() {
        if legacy_applied_set.is_some() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_legacy_applied_set_without_bridge",
                "an ordinary migration cannot carry a legacy applied-set digest",
            ));
        }
        if steps.is_empty() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_empty_program",
                "a migration without schema steps is valid only as a legacy-frontier bridge",
            ));
        }
    } else {
        if legacy_applied_set.is_none() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_legacy_applied_set_missing",
                "a legacy-frontier bridge requires its complete applied-set digest",
            ));
        }
        if !steps.is_empty() || !parents.is_empty() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_bridge_not_zero_operation",
                "a legacy-frontier bridge carries no steps and no canonical parents",
            ));
        }
    }
    let mut current_schema = source_schema.clone();
    let mut required_capabilities = source_schema.required_capabilities().clone();
    let mut safety = SafetyClass::FormalOnly;
    let mut reversible = true;
    let mut pending_assertions = Vec::new();

    for step in &steps {
        let Some(schema_step) = step.as_schema_delta() else {
            step.validate()?;
            step.required_capabilities()
                .ensure_supported_by(delta_context.available_capabilities())?;
            pending_assertions.push(step);
            continue;
        };
        let delta = schema_step.delta();
        if delta.source().scope() != &managed_scope || delta.target().scope() != &managed_scope {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_manifest_scope_mismatch",
                "schema step crosses the verified managed scope lineage",
            ));
        }
        delta
            .required_capabilities()
            .ensure_supported_by(delta_context.available_capabilities())?;

        let target_schema = apply_delta(&current_schema, delta, delta_context).map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_manifest_step_chain_mismatch",
                "schema step source does not chain from the preceding verified target",
            )
        })?;
        let planned = plan_schema_operations(&current_schema, &target_schema).map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_manifest_dependency_plan_invalid",
                "schema step cannot be reproduced by the dependency planner",
            )
        })?;
        if planned != delta.operations() {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_manifest_dependency_plan_mismatch",
                "schema step operations are not in the canonical dependency plan",
            ));
        }

        verify_assertion_coverage(
            &pending_assertions,
            delta,
            &current_schema,
            &target_schema,
            &safety_profile,
        )?;
        for assertion in &pending_assertions {
            for capability in assertion.required_capabilities().iter().cloned() {
                required_capabilities.insert(capability);
            }
        }
        pending_assertions.clear();

        let report = classify_delta_safety(delta);
        for reason in report.reasons() {
            reject_forward_safety(reason.classification())?;
        }
        reject_forward_safety(report.classification())?;
        safety = safety.max(report.classification());

        for capability in delta.required_capabilities().iter().cloned() {
            required_capabilities.insert(capability);
        }

        if let Some(reverse) = schema_step.contract().reverse() {
            let restored = apply_delta(&target_schema, reverse, delta_context).map_err(|_| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_manifest_inverse_replay_mismatch",
                    "schema step inverse does not replay from its verified target",
                )
            })?;
            let planned_reverse =
                plan_schema_operations(&target_schema, &restored).map_err(|_| {
                    failure(
                        DiagnosticCategory::Integrity,
                        "migration_manifest_inverse_plan_invalid",
                        "schema step inverse has no dependency-safe plan",
                    )
                })?;
            if planned_reverse != reverse.operations()
                || restored.canonical_identity_bytes()?
                    != current_schema.canonical_identity_bytes()?
            {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_manifest_inverse_replay_mismatch",
                    "schema step inverse does not restore the exact declared source",
                ));
            }
            reject_reverse_assertion_requirement(
                reverse,
                &target_schema,
                &restored,
                &safety_profile,
            )?;
        } else {
            reversible = false;
        }
        current_schema = target_schema;
    }

    if !pending_assertions.is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_manifest_orphan_assertion",
            "assertion steps must be immediately followed by a schema delta",
        ));
    }

    let target_state =
        managed_schema_state(&current_schema, delta_context).map_err(delta_diagnostic)?;
    let plan_fingerprint = MigrationPlanFingerprint::compute(&steps)?;
    Ok(VerifiedSchemaMigrationManifest {
        format: MigrationFormat::V1,
        id,
        legacy_parents,
        legacy_applied_set,
        lowering_profile,
        managed_scope,
        parents,
        plan_fingerprint,
        required_capabilities,
        reversible,
        safety,
        semantic_profile,
        source_schema: source_schema.clone(),
        source_state,
        steps,
        target_schema: current_schema,
        target_state,
    })
}

/// Decode canonical bytes and return only a fully replay-verified manifest.
pub fn decode_verified_manifest(
    bytes: &[u8],
    context: (&DeclaredSchema, &ManagedDeltaContext),
) -> Result<VerifiedSchemaMigrationManifest, Diagnostic> {
    let candidate = from_canonical_json::<ManifestCandidate>(bytes)?;
    candidate.validate_header()?;
    let draft = candidate.to_draft()?;
    let verified = build_verified_manifest(draft, context)?;
    if encode_verified_manifest(&verified)? != bytes {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_manifest_verification_mismatch",
            "manifest claims do not equal the replay-derived verified encoding",
        ));
    }
    Ok(verified)
}

/// Read only the untrusted identity header needed to order chain decoding.
///
/// The returned identities carry no verification authority: callers must still
/// decode the same bytes through `decode_verified_manifest` before any graph,
/// planning, or execution use.
pub(crate) fn peek_manifest_identity(
    bytes: &[u8],
) -> Result<(MigrationId, Vec<MigrationId>), Diagnostic> {
    let candidate = from_canonical_json::<ManifestCandidate>(bytes)?;
    candidate.validate_header()?;
    let id = candidate.id.rebuild()?;
    let parents = candidate
        .parents
        .iter()
        .map(MigrationIdCandidate::rebuild)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((id, parents))
}

/// Inspect only whether an untrusted canonical candidate declares the legacy
/// bridge shape needed to select its genesis authority.
///
/// This is deliberately not verification authority. Callers use it only to
/// reject a bridge whose companion adopted-genesis artifact is absent before
/// replay decoding would otherwise use the wrong (empty) genesis.
pub(crate) fn peek_manifest_declares_legacy_bridge(bytes: &[u8]) -> Result<bool, Diagnostic> {
    let candidate = from_canonical_json::<ManifestCandidate>(bytes)?;
    candidate.validate_header()?;
    Ok(!candidate.legacy_parents.is_empty())
}

/// Encode a verified manifest under the bounded canonical JSON contract.
pub fn encode_verified_manifest(
    manifest: &VerifiedSchemaMigrationManifest,
) -> Result<Vec<u8>, Diagnostic> {
    to_canonical_json(&ManifestWire::from_verified(manifest))
}

/// Compute the external raw full-SHA256 digest of exact canonical manifest bytes.
pub fn verified_manifest_digest(
    manifest: &VerifiedSchemaMigrationManifest,
) -> Result<MigrationManifestDigest, Diagnostic> {
    Ok(MigrationManifestDigest::compute(&encode_verified_manifest(
        manifest,
    )?))
}

fn reject_forward_safety(safety: SafetyClass) -> Result<(), Diagnostic> {
    match safety {
        SafetyClass::FormalOnly
        | SafetyClass::SchemaMetadata
        | SafetyClass::Additive
        | SafetyClass::Conditional
        | SafetyClass::Destructive => Ok(()),
        SafetyClass::BackfillRequired | SafetyClass::Opaque | SafetyClass::Unsupported => {
            Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_unresolved_safety",
                "migration manifest cannot carry unresolved backfill, opaque, or unsupported work",
            ))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VerifiedAssertionCoverage {
    conditional_operation_indices: Vec<usize>,
    validated: Vec<ValidatedMigrationAssertionPlan>,
}

impl VerifiedAssertionCoverage {
    pub(crate) fn conditional_operation_indices(&self) -> &[usize] {
        &self.conditional_operation_indices
    }

    pub(crate) fn validated(&self) -> &[ValidatedMigrationAssertionPlan] {
        &self.validated
    }
}

pub(crate) fn verify_assertion_coverage(
    assertions: &[&MigrationStep],
    delta: &type_bridge_contract::schema::SchemaDelta,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
) -> Result<VerifiedAssertionCoverage, Diagnostic> {
    let mut required = Vec::new();
    let mut with_destructive_guards = Vec::new();
    let mut conditional_operation_indices = Vec::new();
    for (operation_index, operation) in delta.operations().iter().enumerate() {
        let mut operation_requires_assertion = false;
        let derived =
            derive_safety_conditions(operation_index, operation, source, target, profile)?;
        for condition in derived.conditions() {
            match condition.policy() {
                SafetyClass::Conditional => {
                    operation_requires_assertion = true;
                    if matches!(condition.condition(), SafetyCondition::Unresolvable { .. }) {
                        return Err(failure(
                            DiagnosticCategory::InvalidContract,
                            "migration_manifest_unresolvable_conditional_assertion",
                            "conditional schema work has no canonical assertion representation",
                        ));
                    }
                    required.push(condition.clone());
                    with_destructive_guards.push(condition.clone());
                }
                SafetyClass::Destructive if condition.condition().is_resolvable() => {
                    with_destructive_guards.push(condition.clone());
                }
                _ => {}
            }
        }
        // A conditional operation is discharged either by assertion coverage
        // or by the verifier's condition-free proof (zero derived conditions
        // survive derivation only when the transition is proven safe).
        if operation_requires_assertion || derived.policy() == SafetyClass::Conditional {
            conditional_operation_indices.push(operation_index);
        }
    }

    let expected: &[RequiredSafetyCondition] = if assertions.is_empty() {
        if !required.is_empty() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_missing_assertion",
                "conditional schema work is missing verifier-derived assertions",
            ));
        }
        &[]
    } else if assertions.len() == required.len() {
        &required
    } else if assertions.len() == with_destructive_guards.len() {
        &with_destructive_guards
    } else {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            if assertions.len() < required.len() {
                "migration_manifest_missing_assertion"
            } else {
                "migration_manifest_extra_assertion"
            },
            "assertion count does not equal canonical verifier-derived coverage",
        ));
    };
    if expected.is_empty() && !assertions.is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_manifest_extra_assertion",
            "schema delta has no verifier-derived assertion requirement",
        ));
    }

    let resolved = resolve(source, profile.semantic().id()).map_err(|diagnostics| {
        diagnostics
            .iter()
            .next()
            .map(|diagnostic| diagnostic.diagnostic().clone())
            .unwrap_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_manifest_assertion_resolution_failed",
                    "assertion source resolution failed without a diagnostic",
                )
            })
    })?;
    let context = MigrationAssertionValidationContext::new(&resolved, delta.source());
    let mut validated_plans = Vec::with_capacity(expected.len());
    for (actual, condition) in assertions.iter().zip(expected) {
        let validated = lower_condition_to_plan(condition, &context, StructuralLimits::CANONICAL)?;
        let (contract, plan, expected) = actual.as_assertion().ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_assertion_order_mismatch",
                "assertion coverage contains a non-assertion step",
            )
        })?;
        if expected != AssertionExpectation::NoRows
            || plan.canonical_bytes()? != validated.plan().canonical_bytes()?
            || plan.fingerprint()? != validated.plan().fingerprint()?
        {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_manifest_assertion_plan_mismatch",
                "persisted assertion does not equal verifier-derived canonical plan",
            ));
        }
        let rebuilt = MigrationStep::assertion(
            contract.id().clone(),
            validated.plan().clone(),
            AssertionExpectation::NoRows,
        )?;
        if &rebuilt != *actual {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_manifest_assertion_contract_mismatch",
                "persisted assertion contract differs from verifier-derived claims",
            ));
        }
        validated_plans.push(validated);
    }
    Ok(VerifiedAssertionCoverage {
        conditional_operation_indices,
        validated: validated_plans,
    })
}

fn reject_reverse_assertion_requirement(
    reverse: &type_bridge_contract::schema::SchemaDelta,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
) -> Result<(), Diagnostic> {
    let report = classify_delta_safety(reverse);
    match report.classification() {
        SafetyClass::BackfillRequired | SafetyClass::Opaque | SafetyClass::Unsupported => {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_reverse_unresolved_safety",
                "claimed reverse has unresolved non-assertion migration work",
            ));
        }
        _ => {}
    }
    for (operation_index, operation) in reverse.operations().iter().enumerate() {
        let derived =
            derive_safety_conditions(operation_index, operation, source, target, profile)?;
        if derived.policy() == SafetyClass::Conditional && !derived.conditions().is_empty() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_reverse_requires_assertions",
                "claimed reverse requires assertions that are not represented",
            ));
        }
    }
    Ok(())
}

pub(crate) fn delta_diagnostic(error: DeltaError) -> Diagnostic {
    match error {
        DeltaError::Contract(diagnostic) => diagnostic,
        DeltaError::Schema(diagnostics) => diagnostics
            .iter()
            .next()
            .map(|diagnostic| diagnostic.diagnostic().clone())
            .unwrap_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_manifest_schema_verification_failed",
                    "schema verification failed without a diagnostic",
                )
            }),
    }
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static manifest diagnostic code is canonical"),
        message,
    )
}

#[derive(Serialize)]
struct ManifestWire<'a> {
    contract: ManifestContractWire<'a>,
    fingerprints: ManifestFingerprintsWire<'a>,
    format: &'a MigrationFormat,
    id: &'a MigrationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    legacy_applied_set: Option<LegacyAppliedSetWire<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    legacy_parents: Vec<LegacyParentWire<'a>>,
    managed_scope: &'a ManagedScopeBinding,
    parents: &'a [MigrationId],
    required_capabilities: &'a CapabilitySet,
    resources: &'static [()],
    safety: ManifestSafetyWire,
    steps: &'a [MigrationStep],
}

impl<'a> ManifestWire<'a> {
    fn from_verified(manifest: &'a VerifiedSchemaMigrationManifest) -> Self {
        Self {
            contract: ManifestContractWire {
                canonicalization: MANIFEST_SCHEMA_CANONICALIZATION,
                codec: MANIFEST_CODEC,
                delta_ir: MANIFEST_DELTA_IR,
                lowering_profile: &manifest.lowering_profile,
                semantic_profile: &manifest.semantic_profile,
            },
            fingerprints: ManifestFingerprintsWire {
                plan: &manifest.plan_fingerprint,
                source: ManifestEndpointFingerprintsWire {
                    declared_identity: manifest.source_state.managed_declared_identity(),
                    resolution_identity: manifest.source_state.declared_identity(),
                    semantics: manifest.source_state.managed_semantic_schema(),
                },
                target: ManifestEndpointFingerprintsWire {
                    declared_identity: manifest.target_state.managed_declared_identity(),
                    resolution_identity: manifest.target_state.declared_identity(),
                    semantics: manifest.target_state.managed_semantic_schema(),
                },
            },
            format: &manifest.format,
            id: &manifest.id,
            legacy_applied_set: manifest.legacy_applied_set.as_ref().map(|digest| {
                LegacyAppliedSetWire {
                    algorithm: digest.algorithm(),
                    canonicalization: digest.canonicalization(),
                    digest: digest.as_str(),
                }
            }),
            legacy_parents: manifest
                .legacy_parents
                .iter()
                .map(|reference| LegacyParentWire {
                    app_label: reference.id().app_label().as_str(),
                    checksum: LegacyChecksumWire {
                        algorithm: reference.checksum().algorithm(),
                        value: reference.checksum().as_str(),
                    },
                    name: reference.id().name().as_str(),
                })
                .collect(),
            managed_scope: &manifest.managed_scope,
            parents: &manifest.parents,
            required_capabilities: &manifest.required_capabilities,
            resources: &[],
            safety: ManifestSafetyWire {
                classification: manifest.safety,
                reversible: manifest.reversible,
            },
            steps: &manifest.steps,
        }
    }
}

#[derive(Serialize)]
struct ManifestContractWire<'a> {
    canonicalization: &'static str,
    codec: &'static str,
    delta_ir: &'static str,
    lowering_profile: &'a SchemaLoweringProfileBinding,
    semantic_profile: &'a SemanticProfileBinding,
}

#[derive(Serialize)]
struct ManifestFingerprintsWire<'a> {
    plan: &'a MigrationPlanFingerprint,
    source: ManifestEndpointFingerprintsWire<'a>,
    target: ManifestEndpointFingerprintsWire<'a>,
}

#[derive(Serialize)]
struct ManifestEndpointFingerprintsWire<'a> {
    declared_identity: &'a ManagedDeclaredIdentityFingerprint,
    resolution_identity: &'a DeclaredIdentityFingerprint,
    semantics: &'a ManagedSemanticSchemaFingerprint,
}

#[derive(Serialize)]
struct ManifestSafetyWire {
    classification: SafetyClass,
    reversible: bool,
}

#[derive(Serialize)]
struct LegacyParentWire<'a> {
    app_label: &'a str,
    checksum: LegacyChecksumWire<'a>,
    name: &'a str,
}

#[derive(Serialize)]
struct LegacyChecksumWire<'a> {
    algorithm: &'static str,
    value: &'a str,
}

#[derive(Serialize)]
struct LegacyAppliedSetWire<'a> {
    algorithm: &'static str,
    canonicalization: &'static str,
    digest: &'a str,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestCandidate {
    contract: ManifestContractCandidate,
    fingerprints: ManifestFingerprintsCandidate,
    format: String,
    id: MigrationIdCandidate,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_applied_set: Option<LegacyAppliedSetCandidate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    legacy_parents: Vec<LegacyParentCandidate>,
    managed_scope: ManagedScopeCandidate,
    parents: Vec<MigrationIdCandidate>,
    required_capabilities: CapabilitySet,
    resources: Vec<Value>,
    safety: ManifestSafetyCandidate,
    steps: Vec<Value>,
}

impl ManifestCandidate {
    fn validate_header(&self) -> Result<(), Diagnostic> {
        MigrationFormat::new(&self.format)?;
        if self.contract.canonicalization != MANIFEST_SCHEMA_CANONICALIZATION
            || self.contract.codec != MANIFEST_CODEC
            || self.contract.delta_ir != MANIFEST_DELTA_IR
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_contract_mismatch",
                "manifest contract metadata is not supported",
            ));
        }
        if !self.resources.is_empty() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_resources_not_empty",
                "schema-only manifest resources must be exactly empty",
            ));
        }
        parse_safety(&self.safety.classification)?;
        Ok(())
    }

    fn to_draft(&self) -> Result<SchemaMigrationDraft, Diagnostic> {
        if !self.legacy_parents.is_empty() {
            if !self.parents.is_empty() || !self.steps.is_empty() {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_manifest_bridge_not_zero_operation",
                    "a legacy-frontier bridge carries no steps and no canonical parents",
                ));
            }
            return SchemaMigrationDraft::legacy_bridge(
                self.id.rebuild()?,
                self.legacy_parents
                    .iter()
                    .map(LegacyParentCandidate::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
                self.legacy_applied_set
                    .as_ref()
                    .ok_or_else(|| {
                        failure(
                            DiagnosticCategory::InvalidContract,
                            "migration_manifest_legacy_applied_set_missing",
                            "a legacy-frontier bridge requires its complete applied-set digest",
                        )
                    })?
                    .rebuild()?,
            );
        }
        if self.legacy_applied_set.is_some() {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_legacy_applied_set_without_bridge",
                "an ordinary migration cannot carry a legacy applied-set digest",
            ));
        }
        SchemaMigrationDraft::new(
            self.id.rebuild()?,
            self.parents
                .iter()
                .map(MigrationIdCandidate::rebuild)
                .collect::<Result<Vec<_>, _>>()?,
            self.steps
                .iter()
                .map(rebuild_step_candidate)
                .collect::<Result<Vec<_>, _>>()?,
        )
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestContractCandidate {
    canonicalization: String,
    codec: String,
    delta_ir: String,
    lowering_profile: ProfileBindingCandidate,
    semantic_profile: ProfileBindingCandidate,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProfileBindingCandidate {
    fingerprint: Value,
    id: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MigrationIdCandidate {
    app_label: String,
    name: String,
}

impl MigrationIdCandidate {
    fn rebuild(&self) -> Result<MigrationId, Diagnostic> {
        Ok(MigrationId::from_components(
            MigrationAppLabel::new(self.app_label.clone())?,
            MigrationName::new(self.name.clone())?,
        ))
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyParentCandidate {
    app_label: String,
    checksum: LegacyChecksumCandidate,
    name: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyChecksumCandidate {
    algorithm: String,
    value: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LegacyAppliedSetCandidate {
    algorithm: String,
    canonicalization: String,
    digest: String,
}

impl LegacyAppliedSetCandidate {
    fn rebuild(&self) -> Result<LegacyAppliedSetDigest, Diagnostic> {
        if self.algorithm != LEGACY_APPLIED_SET_ALGORITHM
            || self.canonicalization != LEGACY_APPLIED_SET_CANONICALIZATION
        {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_legacy_applied_set_contract",
                "legacy applied-set binding carries unsupported digest vocabulary",
            ));
        }
        LegacyAppliedSetDigest::new(self.digest.clone())
    }
}

impl LegacyParentCandidate {
    fn rebuild(&self) -> Result<LegacyMigrationReference, Diagnostic> {
        if self.checksum.algorithm != LEGACY_CHECKSUM_ALGORITHM {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_legacy_checksum_algorithm",
                "legacy parent checksum carries an unsupported algorithm tag",
            ));
        }
        Ok(LegacyMigrationReference::new(
            LegacyMigrationId::new(self.app_label.clone(), self.name.clone())?,
            LegacyMigrationChecksum::new(self.checksum.value.clone())?,
        ))
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedScopeCandidate {
    id: String,
    profile: ProfileBindingCandidate,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestFingerprintsCandidate {
    plan: Value,
    source: ManifestEndpointFingerprintsCandidate,
    target: ManifestEndpointFingerprintsCandidate,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestEndpointFingerprintsCandidate {
    declared_identity: Value,
    resolution_identity: Value,
    semantics: Value,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManifestSafetyCandidate {
    classification: String,
    reversible: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SchemaStepCandidate {
    contract: SchemaStepContractCandidate,
    delta: Value,
    kind: String,
}

impl SchemaStepCandidate {
    fn rebuild(&self) -> Result<MigrationStep, Diagnostic> {
        let delta = decode_schema_delta(&to_canonical_json(&self.delta)?)?;
        let reverse = self
            .contract
            .reverse
            .as_ref()
            .map(|reverse| decode_schema_delta(&to_canonical_json(reverse)?))
            .transpose()?;
        let trusted = SchemaDeltaStep::new(
            MigrationStepId::new(self.contract.id.clone())?,
            delta,
            reverse,
        )?;
        if to_canonical_json(self)? != trusted.canonical_bytes()? {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_manifest_step_contract_mismatch",
                "schema step claims do not match the trusted delta-derived contract",
            ));
        }
        Ok(MigrationStep::from(trusted))
    }
}

fn rebuild_step_candidate(value: &Value) -> Result<MigrationStep, Diagnostic> {
    let kind = value
        .as_object()
        .and_then(|object| object.get("kind"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_missing_step_kind",
                "migration step requires a closed kind discriminator",
            )
        })?;
    let bytes = to_canonical_json(value)?;
    match kind {
        "schema_delta" => from_canonical_json::<SchemaStepCandidate>(&bytes)?.rebuild(),
        "assertion" => from_canonical_json::<AssertionStepCandidate>(&bytes)?.rebuild(),
        _ => Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_manifest_unknown_step_kind",
            "migration step kind is not in the closed step vocabulary",
        )),
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AssertionStepCandidate {
    contract: AssertionStepContractCandidate,
    expected: String,
    kind: String,
    plan: Value,
}

impl AssertionStepCandidate {
    fn rebuild(&self) -> Result<MigrationStep, Diagnostic> {
        if self.kind != "assertion" || self.expected != "no_rows" {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_manifest_assertion_kind_mismatch",
                "persisted assertion kind or expectation is not supported",
            ));
        }
        let plan = decode_migration_assertion_plan(&to_canonical_json(&self.plan)?)?;
        let trusted = MigrationStep::assertion(
            MigrationStepId::new(self.contract.id.clone())?,
            plan,
            AssertionExpectation::NoRows,
        )?;
        if to_canonical_json(self)? != trusted.canonical_bytes()? {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "migration_manifest_assertion_contract_mismatch",
                "assertion step claims do not match the trusted plan-derived contract",
            ));
        }
        Ok(trusted)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AssertionStepContractCandidate {
    id: String,
    plan_fingerprint: Value,
    recovery: String,
    required_capabilities: CapabilitySet,
    retry: String,
    source_semantics: Value,
    target_semantics: Value,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SchemaStepContractCandidate {
    delta_fingerprint: Value,
    id: String,
    recovery: String,
    required_capabilities: CapabilitySet,
    retry: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverse: Option<Value>,
    source_semantics: Value,
    target_semantics: Value,
}

fn parse_safety(value: &str) -> Result<SafetyClass, Diagnostic> {
    match value {
        "formal_only" => Ok(SafetyClass::FormalOnly),
        "schema_metadata" => Ok(SafetyClass::SchemaMetadata),
        "additive" => Ok(SafetyClass::Additive),
        "conditional" => Ok(SafetyClass::Conditional),
        "backfill_required" => Ok(SafetyClass::BackfillRequired),
        "destructive" => Ok(SafetyClass::Destructive),
        "opaque" => Ok(SafetyClass::Opaque),
        "unsupported" => Ok(SafetyClass::Unsupported),
        _ => Err(failure(
            DiagnosticCategory::InvalidContract,
            "migration_manifest_unknown_safety",
            "manifest safety classification is not in the closed eight-class vocabulary",
        )),
    }
}
