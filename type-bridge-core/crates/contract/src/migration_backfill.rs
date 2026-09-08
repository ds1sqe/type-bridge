//! Closed binding-neutral data plans used by canonical migration steps.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::{FormatVersion, from_canonical_json, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::Fingerprint as GenericFingerprint;
use crate::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use crate::id::{AttributeId, TypeId, TypeKind};
use crate::schema_fingerprint::ManagedSemanticSchemaFingerprint;

/// Capability required to execute the first closed copy-attribute backfill.
pub const COPY_ATTRIBUTE_BACKFILL_CAPABILITY: &str = "migration.backfill.copy-attribute";
/// Fingerprint domain for a canonical binding-neutral backfill plan.
pub const MIGRATION_BACKFILL_FINGERPRINT_DOMAIN: &str = "typebridge.migration.backfill";
/// Canonicalization identity for the first backfill-plan wire.
pub const MIGRATION_BACKFILL_CANONICALIZATION: &str = "typebridge.migration-backfill/v1";
/// Maximum rows admitted into one deterministic backfill transaction group.
pub const MAX_BACKFILL_BATCH_ROWS: u32 = 10_000;

/// Closed value transformation applied while copying an attribute.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillValueTransform {
    /// Copy the exact canonical source value without coercion.
    Identity,
}

/// Closed conflict behavior for a destination value that already exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillConflictPolicy {
    /// Skip an equal value and reject a different value before reporting success.
    SkipEqualRejectDifferent,
}

/// Required terminal predicate for the first backfill plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillPostcondition {
    /// Every selected source value has one equal destination value.
    SourceValueCopiedExactly,
}

/// Optional independently verifiable reverse program.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillReverseProgram {
    /// Remove only a destination value that still equals its source value.
    RemoveEqualCopiedDestination,
}

/// A deterministic partition contract bound to a stable historical attribute.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BackfillPartition {
    batch_rows: u32,
    stable_attribute: AttributeId,
}

impl BackfillPartition {
    /// Construct a bounded deterministic partition contract.
    pub fn new(batch_rows: u32, stable_attribute: AttributeId) -> Result<Self, Diagnostic> {
        if batch_rows == 0 || batch_rows > MAX_BACKFILL_BATCH_ROWS {
            return Err(backfill_failure(
                DiagnosticCategory::ResourceLimit,
                "migration_backfill_batch_rows_out_of_range",
                "backfill batch rows must be nonzero and within the common ceiling",
            ));
        }
        Ok(Self {
            batch_rows,
            stable_attribute,
        })
    }

    /// Return the maximum rows in one transaction group.
    pub const fn batch_rows(&self) -> u32 {
        self.batch_rows
    }

    /// Return the exact historical attribute used for stable partition order.
    pub const fn stable_attribute(&self) -> &AttributeId {
        &self.stable_attribute
    }
}

/// First closed binding-neutral data plan: copy one historical attribute.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AttributeBackfillPlan {
    conflict: BackfillConflictPolicy,
    destination: AttributeId,
    format: FormatVersion,
    managed_semantics: ManagedSemanticSchemaFingerprint,
    owner: TypeId,
    partition: BackfillPartition,
    postcondition: BackfillPostcondition,
    required_capabilities: CapabilitySet,
    #[serde(skip_serializing_if = "Option::is_none")]
    reverse: Option<BackfillReverseProgram>,
    source: AttributeId,
    transform: BackfillValueTransform,
}

impl AttributeBackfillPlan {
    /// Construct and validate an exact copy-attribute data plan.
    pub fn new(
        owner: TypeId,
        source: AttributeId,
        destination: AttributeId,
        partition: BackfillPartition,
        managed_semantics: ManagedSemanticSchemaFingerprint,
        reverse: Option<BackfillReverseProgram>,
    ) -> Result<Self, Diagnostic> {
        if !matches!(owner.kind(), TypeKind::Entity | TypeKind::Relation) {
            return Err(backfill_failure(
                DiagnosticCategory::InvalidContract,
                "migration_backfill_owner_kind_invalid",
                "backfill owner must be an entity or relation type",
            ));
        }
        if source == destination {
            return Err(backfill_failure(
                DiagnosticCategory::InvalidContract,
                "migration_backfill_same_attribute",
                "backfill source and destination attributes must differ",
            ));
        }
        let mut required_capabilities = CapabilitySet::new();
        required_capabilities.insert(
            CapabilityId::new(COPY_ATTRIBUTE_BACKFILL_CAPABILITY)
                .expect("the fixed backfill capability is canonical"),
        );
        Ok(Self {
            conflict: BackfillConflictPolicy::SkipEqualRejectDifferent,
            destination,
            format: FormatVersion::V1,
            managed_semantics,
            owner,
            partition,
            postcondition: BackfillPostcondition::SourceValueCopiedExactly,
            required_capabilities,
            reverse,
            source,
            transform: BackfillValueTransform::Identity,
        })
    }

    /// Return the historical owner type selected by this plan.
    pub const fn owner(&self) -> &TypeId {
        &self.owner
    }

    /// Return the historical source attribute.
    pub const fn source(&self) -> &AttributeId {
        &self.source
    }

    /// Return the historical destination attribute.
    pub const fn destination(&self) -> &AttributeId {
        &self.destination
    }

    /// Return deterministic transaction partitioning.
    pub const fn partition(&self) -> &BackfillPartition {
        &self.partition
    }

    /// Return the exact historical intermediate-schema fingerprint.
    pub const fn managed_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.managed_semantics
    }

    /// Return the fixed conflict policy.
    pub const fn conflict(&self) -> BackfillConflictPolicy {
        self.conflict
    }

    /// Return the fixed value transformation.
    pub const fn transform(&self) -> BackfillValueTransform {
        self.transform
    }

    /// Return the required terminal postcondition.
    pub const fn postcondition(&self) -> BackfillPostcondition {
        self.postcondition
    }

    /// Return the optional checked reverse program.
    pub const fn reverse(&self) -> Option<BackfillReverseProgram> {
        self.reverse
    }

    /// Return capabilities derived from the closed plan variant.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return exact canonical plan bytes.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }

    /// Compute the domain-separated plan fingerprint.
    pub fn fingerprint(&self) -> Result<Fingerprint, Diagnostic> {
        Ok(Fingerprint::compute(
            FingerprintDomain::new(MIGRATION_BACKFILL_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(MIGRATION_BACKFILL_CANONICALIZATION)?,
            self.managed_semantics
                .as_fingerprint()
                .semantic_profile()
                .cloned(),
            &self.canonical_bytes()?,
        ))
    }
}

/// Decode exact canonical bytes and rebuild every derived backfill claim.
pub fn decode_attribute_backfill_plan(bytes: &[u8]) -> Result<AttributeBackfillPlan, Diagnostic> {
    let candidate = from_canonical_json::<AttributeBackfillCandidate>(bytes)?;
    let format = match candidate.format {
        1 => FormatVersion::V1,
        _ => {
            return Err(backfill_failure(
                DiagnosticCategory::InvalidContract,
                "migration_backfill_format_unsupported",
                "backfill plan format is not supported",
            ));
        }
    };
    let _ = format;
    let semantics =
        ManagedSemanticSchemaFingerprint::from_wire(from_canonical_json::<GenericFingerprint>(
            &to_canonical_json(&candidate.managed_semantics)?,
        )?)?;
    let reverse = match candidate.reverse.as_deref() {
        None => None,
        Some("remove_equal_copied_destination") => {
            Some(BackfillReverseProgram::RemoveEqualCopiedDestination)
        }
        Some(_) => {
            return Err(backfill_failure(
                DiagnosticCategory::InvalidContract,
                "migration_backfill_reverse_unsupported",
                "backfill reverse program is not supported",
            ));
        }
    };
    let plan = AttributeBackfillPlan::new(
        candidate.owner,
        candidate.source,
        candidate.destination,
        BackfillPartition::new(
            candidate.partition.batch_rows,
            candidate.partition.stable_attribute,
        )?,
        semantics,
        reverse,
    )?;
    if plan.canonical_bytes()? != bytes {
        return Err(backfill_failure(
            DiagnosticCategory::Integrity,
            "migration_backfill_contract_mismatch",
            "backfill plan claims differ from constructor-derived canonical claims",
        ));
    }
    Ok(plan)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttributeBackfillCandidate {
    conflict: String,
    destination: AttributeId,
    format: u32,
    managed_semantics: Value,
    owner: TypeId,
    partition: BackfillPartitionCandidate,
    postcondition: String,
    required_capabilities: CapabilitySet,
    #[serde(default)]
    reverse: Option<String>,
    source: AttributeId,
    transform: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BackfillPartitionCandidate {
    batch_rows: u32,
    stable_attribute: AttributeId,
}

fn backfill_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::stable(category, code, message)
}
