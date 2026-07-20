//! Context-free canonical migration identities, fingerprints, and schema steps.

use std::fmt;

use serde::{Serialize, Serializer};
use sha2::{Digest as _, Sha256};

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::to_canonical_json;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDigest, FingerprintDomain,
};
use crate::migration_assertion::{
    AssertionExpectation, MigrationAssertionPlan, MigrationAssertionPlanFingerprint,
};
use crate::schema_delta::{SchemaDelta, SchemaOperation};
use crate::schema_fingerprint::ManagedSemanticSchemaFingerprint;

/// Exact canonical migration format discriminator.
pub const MIGRATION_FORMAT_V1: &str = "typebridge.migration/v1";
// Pre-release wire ledger: migration/v1 gained assertion steps and the manifest
// lowering-profile binding while legacy schema-only canonical bytes stay unchanged.
/// Fingerprint domain for compound migration ledger keys.
pub const MIGRATION_ID_FINGERPRINT_DOMAIN: &str = "typebridge.migration.id";
/// Canonicalization contract for compound migration IDs.
pub const MIGRATION_ID_CANONICALIZATION: &str = "typebridge.migration-id/v1";
/// Fingerprint domain for trusted schema-delta bytes.
pub const SCHEMA_DELTA_FINGERPRINT_DOMAIN: &str = "typebridge.migration.schema-delta";
/// Canonicalization contract for trusted schema deltas.
pub const SCHEMA_DELTA_FINGERPRINT_CANONICALIZATION: &str = "typebridge.schema-delta/v1";
/// Fingerprint domain for ordered schema-only migration plans.
pub const MIGRATION_PLAN_FINGERPRINT_DOMAIN: &str = "typebridge.migration.plan";
/// Canonicalization contract for ordered schema-only migration plans.
pub const MIGRATION_PLAN_FINGERPRINT_CANONICALIZATION: &str = "typebridge.migration-plan/v1";
/// Capability required by a persisted verifier-derived conditional assertion.
pub const CONDITIONAL_RESOLUTION_CAPABILITY: &str = "migration.conditional-resolution";
/// Maximum ASCII byte length of one portable migration identity component.
pub const MAX_MIGRATION_COMPONENT_BYTES: usize = 255;

fn validate_component(
    value: String,
    kind: &'static str,
    allow_leading_digit: bool,
) -> Result<String, Diagnostic> {
    let mut bytes = value.bytes();
    let valid_first = bytes.next().is_some_and(|byte| {
        byte.is_ascii_lowercase() || (allow_leading_digit && byte.is_ascii_digit())
    });
    let valid_rest = bytes.all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
    });
    if value.len() <= MAX_MIGRATION_COMPONENT_BYTES && valid_first && valid_rest {
        Ok(value)
    } else {
        Err(Diagnostic::stable(
            DiagnosticCategory::InvalidContract,
            "invalid_migration_identity_component",
            "migration identity component must be bounded portable lowercase ASCII",
        )
        .with_detail("component_kind", kind))
    }
}

macro_rules! migration_component {
    ($name:ident, $doc:literal, $kind:literal, $leading_digit:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Validate and construct this portable migration identity component.
            pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
                Ok(Self(validate_component(
                    value.into(),
                    $kind,
                    $leading_digit,
                )?))
            }

            /// Return the canonical component spelling.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

migration_component!(
    MigrationAppLabel,
    "A portable application label forming the first component of migration identity.",
    "app_label",
    false
);
migration_component!(
    MigrationName,
    "A portable filename-stem migration name forming the second identity component.",
    "name",
    true
);
migration_component!(
    MigrationStepId,
    "A stable identity unique within one ordered migration plan.",
    "step_id",
    false
);

/// A typed compound migration identity; it is never delimiter-joined.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct MigrationId {
    app_label: MigrationAppLabel,
    name: MigrationName,
}

impl MigrationId {
    /// Validate and construct a compound migration identity.
    pub fn new(app_label: impl Into<String>, name: impl Into<String>) -> Result<Self, Diagnostic> {
        Ok(Self {
            app_label: MigrationAppLabel::new(app_label)?,
            name: MigrationName::new(name)?,
        })
    }

    /// Construct from already validated components.
    #[must_use]
    pub const fn from_components(app_label: MigrationAppLabel, name: MigrationName) -> Self {
        Self { app_label, name }
    }

    /// Return the application-label component.
    pub const fn app_label(&self) -> &MigrationAppLabel {
        &self.app_label
    }

    /// Return the filename-stem name component.
    pub const fn name(&self) -> &MigrationName {
        &self.name
    }

    /// Return exact canonical compound-ID bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }

    /// Derive the domain-separated ledger key for this compound identity.
    pub fn ledger_key(&self) -> Result<MigrationLedgerKey, Diagnostic> {
        MigrationLedgerKey::compute(self)
    }
}

/// The closed canonical migration format admitted by this contract revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MigrationFormat;

impl MigrationFormat {
    /// The initial canonical migration format.
    pub const V1: Self = Self;

    /// Validate an exact migration format discriminator.
    pub fn new(value: &str) -> Result<Self, Diagnostic> {
        if value == MIGRATION_FORMAT_V1 {
            Ok(Self::V1)
        } else {
            Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "unsupported_migration_format",
                "canonical migration format is not supported",
            )
            .with_detail("actual", value.to_owned())
            .with_detail("supported", MIGRATION_FORMAT_V1))
        }
    }

    /// Return the exact wire discriminator.
    pub const fn as_str(self) -> &'static str {
        MIGRATION_FORMAT_V1
    }
}

impl Serialize for MigrationFormat {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// Retry policy supported by the schema-only contract slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryPolicy {
    /// Never replay without a higher-layer verified recovery decision.
    Never,
}

/// Recovery policy supported by the schema-only contract slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPolicy {
    /// Stop and require an explicit verified operator decision.
    OperatorRequired,
}

/// Raw full SHA-256 of exact canonical manifest bytes, stored outside the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct MigrationManifestDigest(FingerprintDigest);

impl MigrationManifestDigest {
    /// Hash exact canonical manifest bytes without adding a fingerprint domain preimage.
    #[must_use]
    pub fn compute(canonical_manifest_bytes: &[u8]) -> Self {
        let digest = Sha256::digest(canonical_manifest_bytes);
        let hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Self(
            FingerprintDigest::from_hex(&hex)
                .expect("SHA-256 always produces a valid lowercase 32-byte digest"),
        )
    }

    /// Parse exactly 64 lowercase hexadecimal characters.
    pub fn from_hex(value: &str) -> Result<Self, Diagnostic> {
        FingerprintDigest::from_hex(value).map(Self)
    }

    /// Return lowercase hexadecimal text.
    #[must_use]
    pub fn to_hex(self) -> String {
        self.0.to_hex()
    }

    /// Return the raw 32 digest bytes.
    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0.bytes()
    }
}

/// Domain-separated ledger key derived from canonical compound migration-ID bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MigrationLedgerKey(Fingerprint);

impl MigrationLedgerKey {
    /// Compute one stable ledger key without flattening identity components.
    pub fn compute(id: &MigrationId) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(MIGRATION_ID_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(MIGRATION_ID_CANONICALIZATION)?,
            None,
            &id.canonical_bytes()?,
        )))
    }

    /// Return the generic domain-separated fingerprint.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// Fingerprint of one trusted schema delta's exact canonical bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SchemaDeltaFingerprint(Fingerprint);

impl SchemaDeltaFingerprint {
    /// Compute the fingerprint of one already validated schema delta.
    pub fn compute(delta: &SchemaDelta) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(SCHEMA_DELTA_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(SCHEMA_DELTA_FINGERPRINT_CANONICALIZATION)?,
            None,
            &delta.canonical_bytes()?,
        )))
    }

    /// Return the generic domain-separated fingerprint.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// The only context-free migration step kind in the schema-only slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStepKind {
    /// One trusted schema-delta chunk.
    SchemaDelta,
    /// One canonical verifier-derived no-rows assertion.
    Assertion,
}

/// Context-free claims derived exactly from a trusted schema delta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SchemaDeltaStepContract {
    delta_fingerprint: SchemaDeltaFingerprint,
    id: MigrationStepId,
    recovery: RecoveryPolicy,
    required_capabilities: CapabilitySet,
    retry: RetryPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverse: Option<SchemaDelta>,
    source_semantics: ManagedSemanticSchemaFingerprint,
    target_semantics: ManagedSemanticSchemaFingerprint,
}

impl SchemaDeltaStepContract {
    /// Derive all step claims and validate an optional exact operation-wise inverse.
    pub fn new(
        id: MigrationStepId,
        delta: &SchemaDelta,
        reverse: Option<SchemaDelta>,
    ) -> Result<Self, Diagnostic> {
        if let Some(candidate) = &reverse {
            let inverse_operations = delta
                .operations()
                .iter()
                .rev()
                .flat_map(SchemaOperation::inverse)
                .collect();
            let expected = SchemaDelta::new(
                delta.format(),
                delta.target().clone(),
                delta.source().clone(),
                inverse_operations,
            )?;
            if candidate != &expected {
                return Err(Diagnostic::stable(
                    DiagnosticCategory::InvalidContract,
                    "schema_delta_step_inverse_mismatch",
                    "schema step reverse delta is not the exact operation-wise inverse",
                ));
            }
        }
        Ok(Self {
            delta_fingerprint: SchemaDeltaFingerprint::compute(delta)?,
            id,
            recovery: RecoveryPolicy::OperatorRequired,
            required_capabilities: delta.required_capabilities().clone(),
            retry: RetryPolicy::Never,
            reverse,
            source_semantics: delta.source().managed_semantic_schema().clone(),
            target_semantics: delta.target().managed_semantic_schema().clone(),
        })
    }

    /// Return the step-local identity.
    pub const fn id(&self) -> &MigrationStepId {
        &self.id
    }

    /// Return the exact source managed-semantic precondition.
    pub const fn source_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.source_semantics
    }

    /// Return the exact target managed-semantic postcondition.
    pub const fn target_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.target_semantics
    }

    /// Return capabilities derived from the trusted delta.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return the exact schema-delta fingerprint.
    pub const fn delta_fingerprint(&self) -> &SchemaDeltaFingerprint {
        &self.delta_fingerprint
    }

    /// Return the fixed no-replay retry policy.
    pub const fn retry(&self) -> RetryPolicy {
        self.retry
    }

    /// Return the fixed operator-required recovery policy.
    pub const fn recovery(&self) -> RecoveryPolicy {
        self.recovery
    }

    /// Return the optional checked exact inverse.
    pub const fn reverse(&self) -> Option<&SchemaDelta> {
        self.reverse.as_ref()
    }
}

/// One trusted schema-only migration step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SchemaDeltaStep {
    contract: SchemaDeltaStepContract,
    delta: SchemaDelta,
    kind: MigrationStepKind,
}

impl SchemaDeltaStep {
    /// Construct a schema step and derive every context-free contract field.
    pub fn new(
        id: MigrationStepId,
        delta: SchemaDelta,
        reverse: Option<SchemaDelta>,
    ) -> Result<Self, Diagnostic> {
        let contract = SchemaDeltaStepContract::new(id, &delta, reverse)?;
        Ok(Self {
            contract,
            delta,
            kind: MigrationStepKind::SchemaDelta,
        })
    }

    /// Return the fixed schema-delta step kind.
    pub const fn kind(&self) -> MigrationStepKind {
        self.kind
    }

    /// Return the derived step contract.
    pub const fn contract(&self) -> &SchemaDeltaStepContract {
        &self.contract
    }

    /// Return the trusted schema delta.
    pub const fn delta(&self) -> &SchemaDelta {
        &self.delta
    }

    /// Return exact canonical step bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }
}

/// Context-free claims derived exactly from one canonical assertion plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssertionStepContract {
    id: MigrationStepId,
    plan_fingerprint: MigrationAssertionPlanFingerprint,
    recovery: RecoveryPolicy,
    required_capabilities: CapabilitySet,
    retry: RetryPolicy,
    source_semantics: ManagedSemanticSchemaFingerprint,
    target_semantics: ManagedSemanticSchemaFingerprint,
}

impl AssertionStepContract {
    fn derive(
        id: MigrationStepId,
        plan: &MigrationAssertionPlan,
        expected: AssertionExpectation,
    ) -> Result<Self, Diagnostic> {
        if expected != AssertionExpectation::NoRows
            || plan.expectation() != AssertionExpectation::NoRows
        {
            return Err(Diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "migration_assertion_step_expectation_mismatch",
                "persisted migration assertions support only the no-rows expectation",
            ));
        }
        let mut required_capabilities = plan.required_capabilities().clone();
        required_capabilities.insert(
            CapabilityId::new(CONDITIONAL_RESOLUTION_CAPABILITY)
                .expect("the fixed conditional-resolution capability is canonical"),
        );
        Ok(Self {
            id,
            plan_fingerprint: plan.fingerprint()?,
            recovery: RecoveryPolicy::OperatorRequired,
            required_capabilities,
            retry: RetryPolicy::Never,
            source_semantics: plan.managed_semantics().clone(),
            target_semantics: plan.managed_semantics().clone(),
        })
    }

    /// Return the step-local identity.
    pub const fn id(&self) -> &MigrationStepId {
        &self.id
    }

    /// Return the canonical assertion-plan fingerprint.
    pub const fn plan_fingerprint(&self) -> &MigrationAssertionPlanFingerprint {
        &self.plan_fingerprint
    }

    /// Return capabilities derived from the assertion syntax and resolution contract.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return the fixed no-replay retry policy.
    pub const fn retry(&self) -> RetryPolicy {
        self.retry
    }

    /// Return the fixed operator-required recovery policy.
    pub const fn recovery(&self) -> RecoveryPolicy {
        self.recovery
    }

    /// Return the exact source managed-semantic precondition.
    pub const fn source_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.source_semantics
    }

    /// Return the identical target managed-semantic postcondition.
    pub const fn target_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.target_semantics
    }

    /// Assertions never have an independently executable reverse step.
    pub const fn reverse(&self) -> Option<()> {
        None
    }
}

/// Exact ordered migration step algebra. Trusted steps deliberately do not deserialize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationStep {
    /// One state-changing schema delta.
    SchemaDelta(Box<SchemaDeltaStep>),
    /// One state-preserving verifier-derived assertion.
    Assertion {
        /// Constructor-derived assertion contract.
        contract: Box<AssertionStepContract>,
        /// Canonical typed assertion plan.
        plan: Box<MigrationAssertionPlan>,
        /// Closed expected outcome.
        expected: AssertionExpectation,
    },
}

impl MigrationStep {
    /// Construct a trusted assertion step and derive all contract claims.
    pub fn assertion(
        id: MigrationStepId,
        plan: MigrationAssertionPlan,
        expected: AssertionExpectation,
    ) -> Result<Self, Diagnostic> {
        let contract = AssertionStepContract::derive(id, &plan, expected)?;
        Ok(Self::Assertion {
            contract: Box::new(contract),
            plan: Box::new(plan),
            expected,
        })
    }

    /// Return the closed step discriminator.
    pub const fn kind(&self) -> MigrationStepKind {
        match self {
            Self::SchemaDelta(_) => MigrationStepKind::SchemaDelta,
            Self::Assertion { .. } => MigrationStepKind::Assertion,
        }
    }

    /// Return the unique step-local identity.
    pub const fn id(&self) -> &MigrationStepId {
        match self {
            Self::SchemaDelta(step) => step.contract().id(),
            Self::Assertion { contract, .. } => contract.id(),
        }
    }

    /// Return constructor-derived required capabilities.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        match self {
            Self::SchemaDelta(step) => step.contract().required_capabilities(),
            Self::Assertion { contract, .. } => contract.required_capabilities(),
        }
    }

    /// Return a schema-delta step when this is state-changing.
    pub fn as_schema_delta(&self) -> Option<&SchemaDeltaStep> {
        match self {
            Self::SchemaDelta(step) => Some(step),
            Self::Assertion { .. } => None,
        }
    }

    /// Return assertion contract, plan, and expectation when state-preserving.
    pub fn as_assertion(
        &self,
    ) -> Option<(
        &AssertionStepContract,
        &MigrationAssertionPlan,
        AssertionExpectation,
    )> {
        match self {
            Self::SchemaDelta(_) => None,
            Self::Assertion {
                contract,
                plan,
                expected,
            } => Some((contract, plan, *expected)),
        }
    }

    /// Rebuild all derived claims and reject an assembled mismatched step.
    pub fn validate(&self) -> Result<(), Diagnostic> {
        let rebuilt = match self {
            Self::SchemaDelta(step) => Self::SchemaDelta(Box::new(SchemaDeltaStep::new(
                step.contract().id().clone(),
                step.delta().clone(),
                step.contract().reverse().cloned(),
            )?)),
            Self::Assertion {
                contract,
                plan,
                expected,
            } => Self::assertion(contract.id().clone(), plan.as_ref().clone(), *expected)?,
        };
        if &rebuilt != self {
            return Err(Diagnostic::stable(
                DiagnosticCategory::Integrity,
                "migration_step_contract_mismatch",
                "migration step claims differ from constructor-derived claims",
            ));
        }
        Ok(())
    }

    /// Return exact canonical heterogeneous step bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }
}

impl From<SchemaDeltaStep> for MigrationStep {
    fn from(step: SchemaDeltaStep) -> Self {
        Self::SchemaDelta(Box::new(step))
    }
}

#[derive(Serialize)]
struct AssertionStepView<'a> {
    contract: &'a AssertionStepContract,
    expected: AssertionExpectation,
    kind: MigrationStepKind,
    plan: &'a MigrationAssertionPlan,
}

impl Serialize for MigrationStep {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::SchemaDelta(step) => step.serialize(serializer),
            Self::Assertion {
                contract,
                plan,
                expected,
            } => AssertionStepView {
                contract,
                expected: *expected,
                kind: MigrationStepKind::Assertion,
                plan,
            }
            .serialize(serializer),
        }
    }
}

#[derive(Serialize)]
struct MigrationPlanView<'a> {
    steps: &'a [MigrationStep],
}

/// Fingerprint of the exact ordered schema-only step algebra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MigrationPlanFingerprint(Fingerprint);

impl MigrationPlanFingerprint {
    /// Return canonical bytes for the exact ordered schema-only plan.
    pub fn canonical_plan_bytes(steps: &[MigrationStep]) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(&MigrationPlanView { steps })
    }

    /// Compute an ordered plan fingerprint; changing step order changes the digest.
    pub fn compute(steps: &[MigrationStep]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(MIGRATION_PLAN_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(MIGRATION_PLAN_FINGERPRINT_CANONICALIZATION)?,
            None,
            &Self::canonical_plan_bytes(steps)?,
        )))
    }

    /// Return the generic domain-separated fingerprint.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}
