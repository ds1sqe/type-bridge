//! Explicit apply-side safety policy and identity-bound operator approvals.
//!
//! Policy may reject a safety class or require an explicit approval; it can
//! never relabel the verifier's classification or grant a standing bypass. An
//! approval is bound to one exact verified transition — the manifest digest,
//! plan fingerprint, profiles, and source/target managed states — so applying
//! the transition consumes it structurally: the frontier moves and the same
//! binding can never match again.

use std::collections::BTreeMap;

use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::managed_scope::SemanticProfileBinding;
use type_bridge_contract::migration::{MigrationId, MigrationManifestDigest};
use type_bridge_contract::migration::MigrationPlanFingerprint;
use type_bridge_contract::schema::ManagedSchemaState;
use type_bridge_contract::schema_lowering::SchemaLoweringProfileBinding;
use type_bridge_schema::SafetyClass;

use crate::manifest::{VerifiedSchemaMigrationManifest, verified_manifest_digest};

/// How the apply gate treats one manifest safety classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafetyPolicyDecision {
    /// The class executes without operator involvement.
    Allow,
    /// The class executes only under a matching identity-bound approval.
    RequireApproval,
    /// The class never executes under this policy.
    Reject,
}

/// Explicit per-class apply policy.
///
/// The verifier's classification is a floor the policy can only tighten:
/// destructive and opaque work can be rejected or gated behind approval but
/// never permanently allowed (a standing `Allow` is the invalid permanent
/// `force = true` shape), and classes the manifest verifier refuses to carry
/// stay rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationSafetyPolicy {
    decisions: BTreeMap<SafetyClass, SafetyPolicyDecision>,
}

impl MigrationSafetyPolicy {
    /// Return the default policy: verified-safe classes execute, destructive
    /// and opaque work requires approval, unresolved work stays rejected.
    pub fn default_policy() -> Self {
        Self {
            decisions: BTreeMap::from([
                (SafetyClass::FormalOnly, SafetyPolicyDecision::Allow),
                (SafetyClass::SchemaMetadata, SafetyPolicyDecision::Allow),
                (SafetyClass::Additive, SafetyPolicyDecision::Allow),
                (SafetyClass::Conditional, SafetyPolicyDecision::Allow),
                (SafetyClass::Destructive, SafetyPolicyDecision::RequireApproval),
                (SafetyClass::Opaque, SafetyPolicyDecision::RequireApproval),
                (SafetyClass::BackfillRequired, SafetyPolicyDecision::Reject),
                (SafetyClass::Unsupported, SafetyPolicyDecision::Reject),
            ]),
        }
    }

    /// Override one class decision, refusing any loosening of the floor.
    pub fn with_decision(
        mut self,
        class: SafetyClass,
        decision: SafetyPolicyDecision,
    ) -> Result<Self, Diagnostic> {
        match (class, decision) {
            (
                SafetyClass::Destructive | SafetyClass::Opaque,
                SafetyPolicyDecision::Allow,
            ) => {
                return Err(failure(
                    "migration_policy_forbidden_allow",
                    "destructive and opaque work cannot carry a standing allowance",
                ));
            }
            (SafetyClass::BackfillRequired | SafetyClass::Unsupported, decision)
                if decision != SafetyPolicyDecision::Reject =>
            {
                return Err(failure(
                    "migration_policy_unresolvable_class",
                    "classes the manifest verifier refuses cannot be admitted by policy",
                ));
            }
            _ => {}
        }
        self.decisions.insert(class, decision);
        Ok(self)
    }

    /// Return the decision for one manifest safety classification.
    pub fn decision(&self, class: SafetyClass) -> SafetyPolicyDecision {
        self.decisions
            .get(&class)
            .copied()
            .unwrap_or(SafetyPolicyDecision::Reject)
    }
}

/// A one-time operator approval bound to one exact verified transition.
///
/// Every element the plan executes under is captured: any change to the
/// manifest bytes, ordered steps, profiles, or the source/target managed
/// states breaks the binding and the approval no longer matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationApplyApproval {
    id: MigrationId,
    lowering_profile: SchemaLoweringProfileBinding,
    manifest_digest: MigrationManifestDigest,
    plan_fingerprint: MigrationPlanFingerprint,
    safety: SafetyClass,
    semantic_profile: SemanticProfileBinding,
    source_state: ManagedSchemaState,
    target_state: ManagedSchemaState,
}

impl MigrationApplyApproval {
    /// Record an approval for the exact transition a verified manifest claims.
    pub fn for_manifest(
        manifest: &VerifiedSchemaMigrationManifest,
    ) -> Result<Self, Diagnostic> {
        Ok(Self {
            id: manifest.id().clone(),
            lowering_profile: manifest.lowering_profile().clone(),
            manifest_digest: verified_manifest_digest(manifest)?,
            plan_fingerprint: manifest.plan_fingerprint().clone(),
            safety: manifest.safety(),
            semantic_profile: manifest.semantic_profile().clone(),
            source_state: manifest.source_state().clone(),
            target_state: manifest.target_state().clone(),
        })
    }

    /// Return the approved compound migration identity.
    pub const fn id(&self) -> &MigrationId {
        &self.id
    }

    /// Return the approved safety classification.
    pub const fn safety(&self) -> SafetyClass {
        self.safety
    }

    /// Return whether this approval binds the exact verified manifest.
    pub fn binds(
        &self,
        manifest: &VerifiedSchemaMigrationManifest,
    ) -> Result<bool, Diagnostic> {
        Ok(self.id == *manifest.id()
            && self.safety == manifest.safety()
            && self.plan_fingerprint == *manifest.plan_fingerprint()
            && self.semantic_profile == *manifest.semantic_profile()
            && self.lowering_profile == *manifest.lowering_profile()
            && self.source_state == *manifest.source_state()
            && self.target_state == *manifest.target_state()
            && self.manifest_digest == verified_manifest_digest(manifest)?)
    }
}

fn failure(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static policy diagnostic code is canonical"),
        message,
    )
}
