//! Verifier-derived safety conditions for exact schema transitions.

use std::cmp::Ordering;

use serde::Serialize;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode,
};
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain,
};
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::managed_scope::SemanticProfileBinding;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationKindId, AnnotationSubjectId, CanonicalValueRange,
    CanonicalValueSet, DeclaredIdentityFingerprint, DeclaredSchema, OwnsFactId,
    SchemaAnnotationValue, SchemaFact, SchemaFactId, SchemaOperation,
    SchemaOperationKind, ValueFactId,
};
use type_bridge_contract::schema_lowering::SchemaLoweringProfileBinding;
use type_bridge_contract::semantic_profile::{InterfaceKind, SemanticProfile};
use type_bridge_contract::value::{CanonicalValue, Cardinality};

use crate::{SafetyClass, classify_schema_operation_safety};

/// Fingerprint domain for verifier-derived safety-condition identities.
pub const SAFETY_CONDITION_FINGERPRINT_DOMAIN: &str =
    "typebridge.schema.safety-condition";
/// Canonicalization identifier for verifier-derived safety conditions.
pub const SAFETY_CONDITION_CANONICALIZATION: &str =
    "typebridge.safety-condition/v1";

/// Registry-owned profiles which affect safety derivation and lowering identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafetyDerivationProfile {
    semantic: SemanticProfileBinding,
    lowering: SchemaLoweringProfileBinding,
}

impl SafetyDerivationProfile {
    /// Bind already registry-resolved semantic and lowering profiles.
    pub fn new(
        semantic: SemanticProfileBinding,
        lowering: SchemaLoweringProfileBinding,
    ) -> Result<Self, Diagnostic> {
        SemanticProfile::resolve(semantic.id())?;
        Ok(Self { semantic, lowering })
    }

    /// Return the exact semantic-profile binding.
    pub const fn semantic(&self) -> &SemanticProfileBinding {
        &self.semantic
    }

    /// Return the exact schema-lowering-profile binding.
    pub const fn lowering(&self) -> &SchemaLoweringProfileBinding {
        &self.lowering
    }
}

/// Stable identity of canonical verifier-derived condition bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SafetyConditionId(Fingerprint);

impl SafetyConditionId {
    fn compute(bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(SAFETY_CONDITION_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(SAFETY_CONDITION_CANONICALIZATION)?,
            None,
            bytes,
        )))
    }

    /// Return the generic domain-separated fingerprint.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// Scalar annotation subject that can be represented by the assertion algebra.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ScalarSafetySubject {
    /// Every instance of one attribute value declaration.
    Value(ValueFactId),
    /// Values reached through one exact effective ownership.
    Owns(OwnsFactId),
}

/// Missing feature or workflow required to express a safety condition honestly.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyConditionUnlock {
    /// Canonical inequality over two thing bindings.
    BindingDistinct,
    /// Canonical regular-expression predicate over an attribute value.
    ValueRegex,
    /// An explicit data backfill or owner-approved transformation.
    Backfill,
}

impl SafetyConditionUnlock {
    /// Return the stable diagnostic spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BindingDistinct => "binding_distinct",
            Self::ValueRegex => "value_regex",
            Self::Backfill => "backfill",
        }
    }
}

/// Closed reason vocabulary for conditions the current query algebra cannot express.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvableSafetyReason {
    /// `@key` needs distinct owner-thing comparison.
    KeyRequiresDistinctOwners,
    /// `@unique` needs distinct owner-thing comparison.
    UniqueRequiresDistinctOwners,
    /// Relates cardinality needs distinct player-thing comparison.
    RelatesCardinalityRequiresDistinctPlayers,
    /// Plays cardinality needs distinct relation-thing comparison.
    PlaysCardinalityRequiresDistinctRelations,
    /// A minimum greater than one needs distinct attribute-thing comparison.
    OwnsMinimumRequiresDistinctAttributes,
    /// Regex narrowing needs a canonical regex value predicate.
    RegexNarrowingRequiresValueRegex,
    /// Value-domain conversion requires an explicit backfill.
    ValueTypeConversionRequiresBackfill,
    /// Subtype-edge changes require an explicit data-policy decision.
    SubtypeTransitionRequiresBackfill,
    /// Role-specialization changes require an explicit data-policy decision.
    RoleSpecializationRequiresBackfill,
    /// A future conditional transition has no assertion-algebra representation.
    ConditionalTransitionRequiresBackfill,
}

impl UnresolvableSafetyReason {
    /// Return the stable diagnostic spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::KeyRequiresDistinctOwners => "key_requires_distinct_owners",
            Self::UniqueRequiresDistinctOwners => "unique_requires_distinct_owners",
            Self::RelatesCardinalityRequiresDistinctPlayers => {
                "relates_cardinality_requires_distinct_players"
            }
            Self::PlaysCardinalityRequiresDistinctRelations => {
                "plays_cardinality_requires_distinct_relations"
            }
            Self::OwnsMinimumRequiresDistinctAttributes => {
                "owns_minimum_requires_distinct_attributes"
            }
            Self::RegexNarrowingRequiresValueRegex => {
                "regex_narrowing_requires_value_regex"
            }
            Self::ValueTypeConversionRequiresBackfill => {
                "value_type_conversion_requires_backfill"
            }
            Self::SubtypeTransitionRequiresBackfill => {
                "subtype_transition_requires_backfill"
            }
            Self::RoleSpecializationRequiresBackfill => {
                "role_specialization_requires_backfill"
            }
            Self::ConditionalTransitionRequiresBackfill => {
                "conditional_transition_requires_backfill"
            }
        }
    }
}

/// Closed verifier-derived safety-condition vocabulary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SafetyCondition {
    /// No instance of a type may exist.
    NoInstances {
        /// Type whose instances would violate the transition.
        type_id: TypeId,
        /// Whether instances of subtypes also violate the transition.
        include_subtypes: bool,
    },
    /// Every owner must have at least the target number of attributes.
    OwnsMinimum {
        /// Exact effective ownership being tightened.
        owns: OwnsFactId,
        /// Target minimum. Revision one can lower only the value one.
        minimum: u64,
    },
    /// No owner may have more than the target number of distinct attribute values.
    OwnsMaximum {
        /// Exact effective ownership being tightened.
        owns: OwnsFactId,
        /// Finite target maximum.
        maximum: u64,
    },
    /// Existing values must not be below a target lower bound.
    RangeLower {
        /// Exact scalar subject being constrained.
        subject: ScalarSafetySubject,
        /// Inclusive target lower bound.
        lower: CanonicalValue,
    },
    /// Existing values must not be above a target upper bound.
    RangeUpper {
        /// Exact scalar subject being constrained.
        subject: ScalarSafetySubject,
        /// Inclusive target upper bound.
        upper: CanonicalValue,
    },
    /// Existing values must belong to the target allowed-value set.
    ValuesNarrowed {
        /// Exact scalar subject being constrained.
        subject: ScalarSafetySubject,
        /// Canonically ordered, non-empty target allowed values.
        allowed: Vec<CanonicalValue>,
    },
    /// No attribute instance may be orphaned when `@independent` is removed.
    NoOrphanAttributes {
        /// Attribute type losing independent existence.
        attribute: AttributeId,
    },
    /// The verifier identified a requirement that cannot be silently weakened.
    Unresolvable {
        /// Stable explanation of the missing representation.
        reason: UnresolvableSafetyReason,
        /// Feature or workflow which can unlock the transition.
        unlock: SafetyConditionUnlock,
    },
}

impl SafetyCondition {
    /// Return whether the current assertion algebra can lower this condition.
    pub const fn is_resolvable(&self) -> bool {
        !matches!(self, Self::Unresolvable { .. })
    }

    /// Return an explicit missing feature or workflow, when gated.
    pub const fn unlock(&self) -> Option<SafetyConditionUnlock> {
        match self {
            Self::Unresolvable { unlock, .. } => Some(*unlock),
            _ => None,
        }
    }
}

#[derive(Serialize)]
struct SafetyConditionIdentityMaterial<'a> {
    canonicalization: &'static str,
    condition: &'a SafetyCondition,
    lowering_profile: &'a SchemaLoweringProfileBinding,
    operation_index: u32,
    policy: SafetyClass,
    semantic_profile: &'a SemanticProfileBinding,
    source_declared: &'a DeclaredIdentityFingerprint,
    target_declared: &'a DeclaredIdentityFingerprint,
}

/// One exact verifier-derived requirement bound to its transition and profiles.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RequiredSafetyCondition {
    condition: SafetyCondition,
    id: SafetyConditionId,
    lowering_profile: SchemaLoweringProfileBinding,
    operation_index: u32,
    policy: SafetyClass,
    semantic_profile: SemanticProfileBinding,
    source_declared: DeclaredIdentityFingerprint,
    target_declared: DeclaredIdentityFingerprint,
}

impl RequiredSafetyCondition {
    fn derive(
        operation_index: u32,
        policy: SafetyClass,
        condition: SafetyCondition,
        source_declared: &DeclaredSchema,
        target_declared: &DeclaredSchema,
        profile: &SafetyDerivationProfile,
    ) -> Result<Self, Diagnostic> {
        let source_declared = source_declared.declared_identity_fingerprint().clone();
        let target_declared = target_declared.declared_identity_fingerprint().clone();
        let identity = SafetyConditionIdentityMaterial {
            canonicalization: SAFETY_CONDITION_CANONICALIZATION,
            condition: &condition,
            lowering_profile: profile.lowering(),
            operation_index,
            policy,
            semantic_profile: profile.semantic(),
            source_declared: &source_declared,
            target_declared: &target_declared,
        };
        let id = SafetyConditionId::compute(&to_canonical_json(&identity)?)?;
        Ok(Self {
            condition,
            id,
            lowering_profile: profile.lowering().clone(),
            operation_index,
            policy,
            semantic_profile: profile.semantic().clone(),
            source_declared,
            target_declared,
        })
    }

    /// Return the stable canonical condition identity.
    pub const fn id(&self) -> &SafetyConditionId {
        &self.id
    }

    /// Return the operation ordinal that produced this requirement.
    pub const fn operation_index(&self) -> u32 {
        self.operation_index
    }

    /// Return the original eight-class policy; guards never rewrite it.
    pub const fn policy(&self) -> SafetyClass {
        self.policy
    }

    /// Return the closed derived condition.
    pub const fn condition(&self) -> &SafetyCondition {
        &self.condition
    }

    /// Return the exact source declaration identity.
    pub const fn source_declared_identity(&self) -> &DeclaredIdentityFingerprint {
        &self.source_declared
    }

    /// Return the exact target declaration identity.
    pub const fn target_declared_identity(&self) -> &DeclaredIdentityFingerprint {
        &self.target_declared
    }

    /// Return the exact semantic-profile binding used by the verifier.
    pub const fn semantic_profile(&self) -> &SemanticProfileBinding {
        &self.semantic_profile
    }

    /// Return the exact lowering-profile binding used by the verifier.
    pub const fn lowering_profile(&self) -> &SchemaLoweringProfileBinding {
        &self.lowering_profile
    }

    /// Return whether this assertion can resolve a Conditional requirement.
    pub fn resolves_conditional_requirement(&self) -> bool {
        self.policy == SafetyClass::Conditional && self.condition.is_resolvable()
    }

    /// Encode identity material without the self-referential identity field.
    pub fn canonical_identity_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(&SafetyConditionIdentityMaterial {
            canonicalization: SAFETY_CONDITION_CANONICALIZATION,
            condition: &self.condition,
            lowering_profile: &self.lowering_profile,
            operation_index: self.operation_index,
            policy: self.policy,
            semantic_profile: &self.semantic_profile,
            source_declared: &self.source_declared,
            target_declared: &self.target_declared,
        })
    }

    /// Encode the complete trusted condition as canonical JSON.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json(self)
    }
}

/// Ordered verifier output for one schema operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DerivedSafetyConditions {
    conditions: Vec<RequiredSafetyCondition>,
    operation_index: u32,
    policy: SafetyClass,
}

impl DerivedSafetyConditions {
    /// Return the operation ordinal shared by every condition.
    pub const fn operation_index(&self) -> u32 {
        self.operation_index
    }

    /// Return the unchanged eight-class operation policy.
    pub const fn policy(&self) -> SafetyClass {
        self.policy
    }

    /// Return conditions in deterministic fact and bound order.
    pub fn conditions(&self) -> &[RequiredSafetyCondition] {
        &self.conditions
    }

    /// Return whether every Conditional requirement has an expressible assertion.
    pub fn resolves_conditional_requirements(&self) -> bool {
        self.policy == SafetyClass::Conditional
            && !self.conditions.is_empty()
            && self
                .conditions
                .iter()
                .all(RequiredSafetyCondition::resolves_conditional_requirement)
    }
}

/// Derive safety conditions from an exact trusted source/operation/target transition.
pub fn derive_safety_conditions(
    operation_index: usize,
    operation: &SchemaOperation,
    source_declared: &DeclaredSchema,
    target_declared: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
) -> Result<DerivedSafetyConditions, Diagnostic> {
    let operation_index = u32::try_from(operation_index).map_err(|_| {
        failure(
            DiagnosticCategory::ResourceLimit,
            "safety_condition_operation_index_limit",
            "operation index exceeds the canonical safety-condition range",
        )
    })?;
    validate_exact_transition(operation, source_declared, target_declared)?;
    let semantic = SemanticProfile::resolve(profile.semantic().id())?;
    let policy = classify_schema_operation_safety(operation);
    let mut conditions = Vec::new();

    match operation.kind() {
        SchemaOperationKind::Define => {
            for fact in operation.defined_facts().expect("define exposes facts") {
                derive_defined_fact(
                    fact,
                    operation_index,
                    policy,
                    source_declared,
                    target_declared,
                    profile,
                    &semantic,
                    &mut conditions,
                )?;
            }
        }
        SchemaOperationKind::Redefine => derive_redefinition(
            operation.expected_fact().expect("redefine exposes expected fact"),
            operation
                .replacement_fact()
                .expect("redefine exposes replacement fact"),
            operation_index,
            policy,
            source_declared,
            target_declared,
            profile,
            &semantic,
            &mut conditions,
        )?,
        SchemaOperationKind::Undefine => derive_undefined_fact(
            operation.undefined_fact().expect("undefine exposes fact"),
            operation_index,
            policy,
            source_declared,
            target_declared,
            profile,
            &semantic,
            &mut conditions,
        )?,
    }

    if policy == SafetyClass::Conditional
        && conditions.is_empty()
        && !is_proven_condition_free_constraint_transition(operation)?
    {
        push_condition(
            &mut conditions,
            operation_index,
            policy,
            SafetyCondition::Unresolvable {
                reason: UnresolvableSafetyReason::ConditionalTransitionRequiresBackfill,
                unlock: SafetyConditionUnlock::Backfill,
            },
            source_declared,
            target_declared,
            profile,
        )?;
    }

    Ok(DerivedSafetyConditions {
        conditions,
        operation_index,
        policy,
    })
}

#[allow(clippy::too_many_arguments)]
fn derive_defined_fact(
    fact: &SchemaFact,
    operation_index: u32,
    policy: SafetyClass,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
    semantic: &SemanticProfile,
    conditions: &mut Vec<RequiredSafetyCondition>,
) -> Result<(), Diagnostic> {
    match fact {
        SchemaFact::Annotation(annotation) => derive_annotation_transition(
            None,
            Some(annotation),
            operation_index,
            policy,
            source,
            target,
            profile,
            semantic,
            conditions,
        ),
        SchemaFact::Sub(_) => push_unresolvable(
            conditions,
            operation_index,
            policy,
            UnresolvableSafetyReason::SubtypeTransitionRequiresBackfill,
            SafetyConditionUnlock::Backfill,
            source,
            target,
            profile,
        ),
        SchemaFact::Relates(relates) if relates.specializes().is_some() => {
            push_unresolvable(
                conditions,
                operation_index,
                policy,
                UnresolvableSafetyReason::RoleSpecializationRequiresBackfill,
                SafetyConditionUnlock::Backfill,
                source,
                target,
                profile,
            )
        }
        _ => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_redefinition(
    old: &SchemaFact,
    new: &SchemaFact,
    operation_index: u32,
    policy: SafetyClass,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
    semantic: &SemanticProfile,
    conditions: &mut Vec<RequiredSafetyCondition>,
) -> Result<(), Diagnostic> {
    match (old, new) {
        (SchemaFact::Annotation(old), SchemaFact::Annotation(new)) => {
            derive_annotation_transition(
                Some(old),
                Some(new),
                operation_index,
                policy,
                source,
                target,
                profile,
                semantic,
                conditions,
            )
        }
        (SchemaFact::Value(old), SchemaFact::Value(new))
            if old.value_type() != new.value_type() =>
        {
            push_unresolvable(
                conditions,
                operation_index,
                policy,
                UnresolvableSafetyReason::ValueTypeConversionRequiresBackfill,
                SafetyConditionUnlock::Backfill,
                source,
                target,
                profile,
            )
        }
        (SchemaFact::Sub(_), SchemaFact::Sub(_)) => push_unresolvable(
            conditions,
            operation_index,
            policy,
            UnresolvableSafetyReason::SubtypeTransitionRequiresBackfill,
            SafetyConditionUnlock::Backfill,
            source,
            target,
            profile,
        ),
        (SchemaFact::Relates(_), SchemaFact::Relates(_)) => push_unresolvable(
            conditions,
            operation_index,
            policy,
            UnresolvableSafetyReason::RoleSpecializationRequiresBackfill,
            SafetyConditionUnlock::Backfill,
            source,
            target,
            profile,
        ),
        _ => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_undefined_fact(
    fact: &SchemaFact,
    operation_index: u32,
    policy: SafetyClass,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
    semantic: &SemanticProfile,
    conditions: &mut Vec<RequiredSafetyCondition>,
) -> Result<(), Diagnostic> {
    match fact {
        SchemaFact::Type(type_fact) => push_condition(
            conditions,
            operation_index,
            policy,
            SafetyCondition::NoInstances {
                type_id: type_fact.id().clone(),
                include_subtypes: true,
            },
            source,
            target,
            profile,
        ),
        SchemaFact::Annotation(annotation) => derive_annotation_transition(
            Some(annotation),
            None,
            operation_index,
            policy,
            source,
            target,
            profile,
            semantic,
            conditions,
        ),
        SchemaFact::Relates(relates) if relates.specializes().is_some() => {
            push_unresolvable(
                conditions,
                operation_index,
                policy,
                UnresolvableSafetyReason::RoleSpecializationRequiresBackfill,
                SafetyConditionUnlock::Backfill,
                source,
                target,
                profile,
            )
        }
        _ => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn derive_annotation_transition(
    old: Option<&AnnotationFact>,
    new: Option<&AnnotationFact>,
    operation_index: u32,
    policy: SafetyClass,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
    semantic: &SemanticProfile,
    conditions: &mut Vec<RequiredSafetyCondition>,
) -> Result<(), Diagnostic> {
    let annotation = new.or(old).expect("an annotation transition has one side");
    let subject = annotation.id().subject();
    match annotation.id().kind() {
        AnnotationKindId::Abstract if old.is_none() => {
            if let AnnotationSubjectId::Type(type_id) = subject {
                push_condition(
                    conditions,
                    operation_index,
                    policy,
                    SafetyCondition::NoInstances {
                        type_id: type_id.clone(),
                        include_subtypes: false,
                    },
                    source,
                    target,
                    profile,
                )?;
            }
            // Abstract-on-relates intentionally reaches the operation-level explicit
            // Backfill/unresolvable fallback until a live-pinned condition exists.
        }
        AnnotationKindId::Independent if new.is_none() => {
            if let AnnotationSubjectId::Type(type_id) = subject
                && type_id.kind() == TypeKind::Attribute
            {
                push_condition(
                    conditions,
                    operation_index,
                    policy,
                    SafetyCondition::NoOrphanAttributes {
                        attribute: AttributeId::new(type_id.label().as_str())?,
                    },
                    source,
                    target,
                    profile,
                )?;
            }
        }
        AnnotationKindId::Key if old.is_none() => push_unresolvable(
            conditions,
            operation_index,
            policy,
            UnresolvableSafetyReason::KeyRequiresDistinctOwners,
            SafetyConditionUnlock::BindingDistinct,
            source,
            target,
            profile,
        )?,
        AnnotationKindId::Unique if old.is_none() => push_unresolvable(
            conditions,
            operation_index,
            policy,
            UnresolvableSafetyReason::UniqueRequiresDistinctOwners,
            SafetyConditionUnlock::BindingDistinct,
            source,
            target,
            profile,
        )?,
        AnnotationKindId::Card => derive_cardinality_conditions(
            subject,
            operation_index,
            policy,
            source,
            target,
            profile,
            semantic,
            conditions,
        )?,
        AnnotationKindId::Range => {
            if let Some(new) = new {
                let range = range_payload(new)?;
                let old_range = old.map(range_payload).transpose()?;
                let (lower_narrows, upper_narrows) =
                    range_narrowing(old_range, range)?;
                let subject = scalar_subject(subject)?;
                if lower_narrows {
                    let lower = range
                        .lower()
                        .expect("a narrowing lower bound is present");
                    push_condition(
                        conditions,
                        operation_index,
                        policy,
                        SafetyCondition::RangeLower {
                            subject: subject.clone(),
                            lower: lower.clone(),
                        },
                        source,
                        target,
                        profile,
                    )?;
                }
                if upper_narrows {
                    let upper = range
                        .upper()
                        .expect("a narrowing upper bound is present");
                    push_condition(
                        conditions,
                        operation_index,
                        policy,
                        SafetyCondition::RangeUpper {
                            subject,
                            upper: upper.clone(),
                        },
                        source,
                        target,
                        profile,
                    )?;
                }
            }
        }
        AnnotationKindId::Values => {
            if let Some(new) = new {
                let values = values_payload(new)?;
                let old_values = old.map(values_payload).transpose()?;
                if values_narrow(old_values, values)? {
                    push_condition(
                        conditions,
                        operation_index,
                        policy,
                        SafetyCondition::ValuesNarrowed {
                            subject: scalar_subject(subject)?,
                            allowed: values.iter().cloned().collect(),
                        },
                        source,
                        target,
                        profile,
                    )?;
                }
            }
        }
        AnnotationKindId::Regex if new.is_some() => push_unresolvable(
            conditions,
            operation_index,
            policy,
            UnresolvableSafetyReason::RegexNarrowingRequiresValueRegex,
            SafetyConditionUnlock::ValueRegex,
            source,
            target,
            profile,
        )?,
        _ => {}
    }
    Ok(())
}

fn is_proven_condition_free_constraint_transition(
    operation: &SchemaOperation,
) -> Result<bool, Diagnostic> {
    if operation.kind() != SchemaOperationKind::Redefine {
        return Ok(false);
    }
    let (SchemaFact::Annotation(old), SchemaFact::Annotation(new)) = (
        operation.expected_fact().expect("redefine exposes expected fact"),
        operation
            .replacement_fact()
            .expect("redefine exposes replacement fact"),
    ) else {
        return Ok(false);
    };
    match new.id().kind() {
        AnnotationKindId::Range => {
            let (lower_narrows, upper_narrows) =
                range_narrowing(Some(range_payload(old)?), range_payload(new)?)?;
            Ok(!lower_narrows && !upper_narrows)
        }
        AnnotationKindId::Values => Ok(!values_narrow(
            Some(values_payload(old)?),
            values_payload(new)?,
        )?),
        _ => Ok(false),
    }
}

fn range_payload(annotation: &AnnotationFact) -> Result<&CanonicalValueRange, Diagnostic> {
    match annotation.value() {
        SchemaAnnotationValue::Range(range) => Ok(range),
        _ => Err(failure(
            DiagnosticCategory::InvalidContract,
            "safety_condition_malformed_range_transition",
            "range transition does not contain range payloads on both sides",
        )),
    }
}

fn range_narrowing(
    old: Option<&CanonicalValueRange>,
    new: &CanonicalValueRange,
) -> Result<(bool, bool), Diagnostic> {
    if let Some(old) = old {
        let old_domain = old
            .lower()
            .or_else(|| old.upper())
            .expect("canonical ranges are non-empty")
            .value_type();
        let new_domain = new
            .lower()
            .or_else(|| new.upper())
            .expect("canonical ranges are non-empty")
            .value_type();
        if old_domain != new_domain {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "safety_condition_incomparable_range_domain",
                "range transition bounds use incomparable scalar domains",
            ));
        }
    }

    let lower_narrows = match (old.and_then(CanonicalValueRange::lower), new.lower()) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(old), Some(new)) => compare_same_domain(new, old)? == Ordering::Greater,
    };
    let upper_narrows = match (old.and_then(CanonicalValueRange::upper), new.upper()) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(old), Some(new)) => compare_same_domain(new, old)? == Ordering::Less,
    };
    Ok((lower_narrows, upper_narrows))
}

fn values_payload(annotation: &AnnotationFact) -> Result<&CanonicalValueSet, Diagnostic> {
    match annotation.value() {
        SchemaAnnotationValue::Values(values) => Ok(values),
        _ => Err(failure(
            DiagnosticCategory::InvalidContract,
            "safety_condition_malformed_values_transition",
            "values transition does not contain values payloads on both sides",
        )),
    }
}

fn values_narrow(
    old: Option<&CanonicalValueSet>,
    new: &CanonicalValueSet,
) -> Result<bool, Diagnostic> {
    let Some(old) = old else {
        return Ok(true);
    };
    let old_domain = old
        .iter()
        .next()
        .expect("canonical value sets are non-empty")
        .value_type();
    let new_domain = new
        .iter()
        .next()
        .expect("canonical value sets are non-empty")
        .value_type();
    if old_domain != new_domain {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "safety_condition_incomparable_values_domain",
            "values transition members use incomparable scalar domains",
        ));
    }
    Ok(old
        .iter()
        .any(|old_value| !new.iter().any(|new_value| new_value == old_value)))
}

fn compare_same_domain(
    left: &CanonicalValue,
    right: &CanonicalValue,
) -> Result<Ordering, Diagnostic> {
    left.semantic_cmp_same_domain(right).ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "safety_condition_incomparable_range_domain",
            "range transition bounds use incomparable scalar domains",
        )
    })
}

#[allow(clippy::too_many_arguments)]
fn derive_cardinality_conditions(
    subject: &AnnotationSubjectId,
    operation_index: u32,
    policy: SafetyClass,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
    semantic: &SemanticProfile,
    conditions: &mut Vec<RequiredSafetyCondition>,
) -> Result<(), Diagnostic> {
    let old = effective_cardinality(source, subject, semantic)?;
    let new = effective_cardinality(target, subject, semantic)?;
    let minimum_narrows = new.min() > old.min();
    let maximum_narrows = match (old.max(), new.max()) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(old), Some(new)) => new < old,
    };
    if !minimum_narrows && !maximum_narrows {
        return Ok(());
    }

    match subject {
        AnnotationSubjectId::Owns(owns) => {
            if minimum_narrows {
                if new.min() == 1 {
                    push_condition(
                        conditions,
                        operation_index,
                        policy,
                        SafetyCondition::OwnsMinimum {
                            owns: owns.clone(),
                            minimum: 1,
                        },
                        source,
                        target,
                        profile,
                    )?;
                } else {
                    push_unresolvable(
                        conditions,
                        operation_index,
                        policy,
                        UnresolvableSafetyReason::OwnsMinimumRequiresDistinctAttributes,
                        SafetyConditionUnlock::BindingDistinct,
                        source,
                        target,
                        profile,
                    )?;
                }
            }
            if maximum_narrows {
                push_condition(
                    conditions,
                    operation_index,
                    policy,
                    SafetyCondition::OwnsMaximum {
                        owns: owns.clone(),
                        maximum: new.max().expect("a narrowing maximum is finite"),
                    },
                    source,
                    target,
                    profile,
                )?;
            }
        }
        AnnotationSubjectId::Relates(_) => push_unresolvable(
            conditions,
            operation_index,
            policy,
            UnresolvableSafetyReason::RelatesCardinalityRequiresDistinctPlayers,
            SafetyConditionUnlock::BindingDistinct,
            source,
            target,
            profile,
        )?,
        AnnotationSubjectId::Plays(_) => push_unresolvable(
            conditions,
            operation_index,
            policy,
            UnresolvableSafetyReason::PlaysCardinalityRequiresDistinctRelations,
            SafetyConditionUnlock::BindingDistinct,
            source,
            target,
            profile,
        )?,
        _ => {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "safety_condition_invalid_cardinality_subject",
                "cardinality safety derivation requires an interface subject",
            ));
        }
    }
    Ok(())
}

fn effective_cardinality(
    declared: &DeclaredSchema,
    subject: &AnnotationSubjectId,
    semantic: &SemanticProfile,
) -> Result<Cardinality, Diagnostic> {
    let kind = match subject {
        AnnotationSubjectId::Owns(_) => InterfaceKind::Owns,
        AnnotationSubjectId::Relates(_) => InterfaceKind::Relates,
        AnnotationSubjectId::Plays(_) => InterfaceKind::Plays,
        _ => {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "safety_condition_invalid_cardinality_subject",
                "cardinality safety derivation requires an interface subject",
            ));
        }
    };
    let explicit = annotation(declared, subject, &AnnotationKindId::Card)
        .and_then(|annotation| match annotation.value() {
            SchemaAnnotationValue::Cardinality(cardinality) => Some(*cardinality),
            _ => None,
        });
    let key = matches!(subject, AnnotationSubjectId::Owns(_))
        && annotation(declared, subject, &AnnotationKindId::Key).is_some();
    Ok(semantic.effective_cardinality(kind, explicit, key))
}

fn annotation<'a>(
    declared: &'a DeclaredSchema,
    subject: &AnnotationSubjectId,
    kind: &AnnotationKindId,
) -> Option<&'a AnnotationFact> {
    declared.facts().find_map(|fact| match fact {
        SchemaFact::Annotation(annotation)
            if annotation.id().subject() == subject && annotation.id().kind() == kind =>
        {
            Some(annotation)
        }
        _ => None,
    })
}

fn scalar_subject(subject: &AnnotationSubjectId) -> Result<ScalarSafetySubject, Diagnostic> {
    match subject {
        AnnotationSubjectId::Value(value) => Ok(ScalarSafetySubject::Value(value.clone())),
        AnnotationSubjectId::Owns(owns) => Ok(ScalarSafetySubject::Owns(owns.clone())),
        _ => Err(failure(
            DiagnosticCategory::InvalidContract,
            "safety_condition_invalid_scalar_subject",
            "value safety derivation requires a value or ownership subject",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn push_unresolvable(
    conditions: &mut Vec<RequiredSafetyCondition>,
    operation_index: u32,
    policy: SafetyClass,
    reason: UnresolvableSafetyReason,
    unlock: SafetyConditionUnlock,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
) -> Result<(), Diagnostic> {
    push_condition(
        conditions,
        operation_index,
        policy,
        SafetyCondition::Unresolvable { reason, unlock },
        source,
        target,
        profile,
    )
}

fn push_condition(
    conditions: &mut Vec<RequiredSafetyCondition>,
    operation_index: u32,
    policy: SafetyClass,
    condition: SafetyCondition,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    profile: &SafetyDerivationProfile,
) -> Result<(), Diagnostic> {
    conditions.push(RequiredSafetyCondition::derive(
        operation_index,
        policy,
        condition,
        source,
        target,
        profile,
    )?);
    Ok(())
}

fn validate_exact_transition(
    operation: &SchemaOperation,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
) -> Result<(), Diagnostic> {
    match operation.kind() {
        SchemaOperationKind::Define => {
            for fact in operation.defined_facts().expect("define exposes facts") {
                let id = fact.id();
                if find_fact(source, &id).is_some() {
                    return Err(transition_failure(
                        "safety_condition_source_define_conflict",
                        "defined fact already exists in the declared source",
                    ));
                }
                if find_fact(target, &id) != Some(fact) {
                    return Err(transition_failure(
                        "safety_condition_target_define_mismatch",
                        "defined fact does not exactly match the declared target",
                    ));
                }
            }
        }
        SchemaOperationKind::Redefine => {
            let expected = operation.expected_fact().expect("redefine exposes expected fact");
            let replacement = operation
                .replacement_fact()
                .expect("redefine exposes replacement fact");
            let id = expected.id();
            if find_fact(source, &id) != Some(expected) {
                return Err(transition_failure(
                    "safety_condition_source_redefine_mismatch",
                    "expected fact does not exactly match the declared source",
                ));
            }
            if find_fact(target, &id) != Some(replacement) {
                return Err(transition_failure(
                    "safety_condition_target_redefine_mismatch",
                    "replacement fact does not exactly match the declared target",
                ));
            }
        }
        SchemaOperationKind::Undefine => {
            let fact = operation.undefined_fact().expect("undefine exposes fact");
            let id = fact.id();
            if find_fact(source, &id) != Some(fact) {
                return Err(transition_failure(
                    "safety_condition_source_undefine_mismatch",
                    "removed fact does not exactly match the declared source",
                ));
            }
            if find_fact(target, &id).is_some() {
                return Err(transition_failure(
                    "safety_condition_target_undefine_conflict",
                    "removed fact is still present in the declared target",
                ));
            }
        }
    }
    Ok(())
}

fn find_fact<'a>(
    declared: &'a DeclaredSchema,
    id: &SchemaFactId,
) -> Option<&'a SchemaFact> {
    declared.facts().find(|fact| fact.id() == id.clone())
}

fn transition_failure(code: &'static str, message: &'static str) -> Diagnostic {
    failure(DiagnosticCategory::Integrity, code, message)
}

fn failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static safety-condition diagnostic code is canonical"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::capability::CapabilitySet;
    use type_bridge_contract::codec::FormatVersion;
    use type_bridge_contract::id::TypeKind;
    use type_bridge_contract::managed_scope::SemanticProfileBinding;
    use type_bridge_contract::schema::{
        AnnotationFactId, CanonicalValueRange, CanonicalValueSet, DocumentId,
        OwnsFact, SourceSpan, SourcedSchemaFact, TypeFact, ValueFact,
    };
    use type_bridge_contract::value::ValueTypeTag;
    use type_bridge_contract::schema_lowering::SchemaLoweringProfileBinding;

    use super::*;

    fn test_safety_profile() -> SafetyDerivationProfile {
        SafetyDerivationProfile::new(
            SemanticProfileBinding::typedb_3_12_1().expect("semantic profile"),
            SchemaLoweringProfileBinding::from_canonical_profile_bytes(
                br#"{"id":"typedb-3.12.1-schema-lowering/v1","test_fixture":"safety-condition"}"#,
            )
            .expect("test lowering profile"),
        )
        .expect("test safety profile")
    }

    fn type_id(kind: TypeKind, label: &str) -> TypeId {
        TypeId::new(kind, label).expect("fixture type")
    }

    fn declared(facts: Vec<SchemaFact>) -> DeclaredSchema {
        let facts = facts.into_iter().enumerate().map(|(index, fact)| {
            let offset = u64::try_from(index).expect("fixture offset");
            let line = u32::try_from(index + 1).expect("fixture line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("safety-condition-test").expect("document"),
                    offset,
                    offset + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("span"),
            )
        });
        DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), facts)
            .expect("declared fixture")
    }

    fn annotation_fact(
        subject: AnnotationSubjectId,
        kind: AnnotationKindId,
        value: SchemaAnnotationValue,
    ) -> SchemaFact {
        SchemaFact::Annotation(
            AnnotationFact::new(AnnotationFactId::new(subject, kind), value)
                .expect("annotation fixture"),
        )
    }

    fn owns_fixture() -> (Vec<SchemaFact>, OwnsFactId) {
        let person = type_id(TypeKind::Entity, "person");
        let age = AttributeId::new("age").expect("attribute");
        let owns = OwnsFactId::new(person.clone(), age.clone()).expect("owns");
        (
            vec![
                SchemaFact::Type(TypeFact::new(person).expect("type")),
                SchemaFact::Type(
                    TypeFact::new(type_id(TypeKind::Attribute, "age")).expect("type"),
                ),
                SchemaFact::Value(ValueFact::new(
                    ValueFactId::new(age),
                    ValueTypeTag::Long,
                )),
                SchemaFact::Owns(OwnsFact::new(owns.clone())),
            ],
            owns,
        )
    }

    #[test]
    fn condition_identity_order_and_transition_tamper_are_pinned() {
        let profile = test_safety_profile();
        let (base, owns) = owns_fixture();
        let subject = AnnotationSubjectId::Owns(owns.clone());
        let old = annotation_fact(
            subject.clone(),
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(
                Cardinality::new(0, Some(3)).expect("old card"),
            ),
        );
        let new = annotation_fact(
            subject,
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(
                Cardinality::new(1, Some(1)).expect("new card"),
            ),
        );
        let source = declared(base.iter().cloned().chain([old.clone()]).collect());
        let target = declared(base.iter().cloned().chain([new.clone()]).collect());
        let operation = SchemaOperation::redefine(old, new).expect("operation");

        let first = derive_safety_conditions(
            7,
            &operation,
            &source,
            &target,
            &profile,
        )
        .expect("conditions");
        let second = derive_safety_conditions(
            7,
            &operation,
            &source,
            &target,
            &profile,
        )
        .expect("conditions");
        assert_eq!(first, second);
        assert_eq!(first.conditions().len(), 2);
        assert!(matches!(
            first.conditions()[0].condition(),
            SafetyCondition::OwnsMinimum { minimum: 1, .. }
        ));
        assert!(matches!(
            first.conditions()[1].condition(),
            SafetyCondition::OwnsMaximum { maximum: 1, .. }
        ));
        assert_eq!(
            first.conditions()[0].id(),
            second.conditions()[0].id()
        );
        assert_eq!(
            first.conditions()[0]
                .canonical_identity_bytes()
                .expect("identity bytes"),
            second.conditions()[0]
                .canonical_identity_bytes()
                .expect("identity bytes")
        );
        let moved = derive_safety_conditions(
            8,
            &operation,
            &source,
            &target,
            &profile,
        )
        .expect("moved condition");
        assert_ne!(first.conditions()[0].id(), moved.conditions()[0].id());
        assert_eq!(
            derive_safety_conditions(7, &operation, &source, &source, &profile)
                .expect_err("target tamper")
                .code()
                .as_str(),
            "safety_condition_target_redefine_mismatch"
        );
    }

    #[test]
    fn expressible_and_gated_families_preserve_original_policy() {
        let profile = test_safety_profile();
        let person = type_id(TypeKind::Entity, "person");
        let base = vec![SchemaFact::Type(
            TypeFact::new(person.clone()).expect("type"),
        )];
        let abstract_fact = annotation_fact(
            AnnotationSubjectId::Type(person.clone()),
            AnnotationKindId::Abstract,
            SchemaAnnotationValue::Presence,
        );
        let source = declared(base.clone());
        let target = declared(base.iter().cloned().chain([abstract_fact.clone()]).collect());
        let abstract_conditions = derive_safety_conditions(
            0,
            &SchemaOperation::define(vec![abstract_fact]).expect("abstract operation"),
            &source,
            &target,
            &profile,
        )
        .expect("abstract condition");
        assert_eq!(abstract_conditions.policy(), SafetyClass::Conditional);
        assert!(abstract_conditions.resolves_conditional_requirements());
        assert!(matches!(
            abstract_conditions.conditions()[0].condition(),
            SafetyCondition::NoInstances {
                include_subtypes: false,
                ..
            }
        ));

        let (owns_base, owns) = owns_fixture();
        let key = annotation_fact(
            AnnotationSubjectId::Owns(owns),
            AnnotationKindId::Key,
            SchemaAnnotationValue::Presence,
        );
        let key_source = declared(owns_base.clone());
        let key_target = declared(owns_base.into_iter().chain([key.clone()]).collect());
        let key_conditions = derive_safety_conditions(
            1,
            &SchemaOperation::define(vec![key]).expect("key operation"),
            &key_source,
            &key_target,
            &profile,
        )
        .expect("key condition");
        assert_eq!(key_conditions.policy(), SafetyClass::BackfillRequired);
        assert!(matches!(
            key_conditions.conditions()[0].condition(),
            SafetyCondition::Unresolvable {
                reason: UnresolvableSafetyReason::KeyRequiresDistinctOwners,
                unlock: SafetyConditionUnlock::BindingDistinct,
            }
        ));

        let age = AttributeId::new("age").expect("attribute");
        let scalar_base = vec![
            SchemaFact::Type(
                TypeFact::new(type_id(TypeKind::Attribute, "age")).expect("type"),
            ),
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(age.clone()),
                ValueTypeTag::Long,
            )),
        ];
        let range = annotation_fact(
            AnnotationSubjectId::Value(ValueFactId::new(age.clone())),
            AnnotationKindId::Range,
            SchemaAnnotationValue::Range(
                CanonicalValueRange::new(
                    Some(CanonicalValue::Long(1)),
                    Some(CanonicalValue::Long(9)),
                )
                .expect("range"),
            ),
        );
        let range_source = declared(scalar_base.clone());
        let range_target = declared(scalar_base.iter().cloned().chain([range.clone()]).collect());
        let range_conditions = derive_safety_conditions(
            2,
            &SchemaOperation::define(vec![range]).expect("range operation"),
            &range_source,
            &range_target,
            &profile,
        )
        .expect("range conditions");
        assert_eq!(range_conditions.conditions().len(), 2);
        assert!(range_conditions.resolves_conditional_requirements());

        let values = annotation_fact(
            AnnotationSubjectId::Value(ValueFactId::new(age.clone())),
            AnnotationKindId::Values,
            SchemaAnnotationValue::Values(
                CanonicalValueSet::new([
                    CanonicalValue::Long(2),
                    CanonicalValue::Long(4),
                ])
                .expect("values"),
            ),
        );
        let values_target = declared(
            scalar_base
                .iter()
                .cloned()
                .chain([values.clone()])
                .collect(),
        );
        let values_conditions = derive_safety_conditions(
            3,
            &SchemaOperation::define(vec![values]).expect("values operation"),
            &range_source,
            &values_target,
            &profile,
        )
        .expect("values condition");
        assert!(matches!(
            values_conditions.conditions()[0].condition(),
            SafetyCondition::ValuesNarrowed { allowed, .. } if allowed.len() == 2
        ));

        let code = AttributeId::new("code").expect("attribute");
        let regex_base = vec![
            SchemaFact::Type(
                TypeFact::new(type_id(TypeKind::Attribute, "code")).expect("type"),
            ),
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(code.clone()),
                ValueTypeTag::String,
            )),
        ];
        let regex = annotation_fact(
            AnnotationSubjectId::Value(ValueFactId::new(code)),
            AnnotationKindId::Regex,
            SchemaAnnotationValue::Regex(
                type_bridge_contract::schema::RegexPattern::new("[0-9]+").expect("regex"),
            ),
        );
        let regex_source = declared(regex_base.clone());
        let regex_target = declared(regex_base.into_iter().chain([regex.clone()]).collect());
        let regex_conditions = derive_safety_conditions(
            4,
            &SchemaOperation::define(vec![regex]).expect("regex operation"),
            &regex_source,
            &regex_target,
            &profile,
        )
        .expect("regex gate");
        assert!(matches!(
            regex_conditions.conditions()[0].condition(),
            SafetyCondition::Unresolvable {
                reason: UnresolvableSafetyReason::RegexNarrowingRequiresValueRegex,
                unlock: SafetyConditionUnlock::ValueRegex,
            }
        ));
    }

    #[test]
    fn destructive_guards_do_not_reclassify_policy() {
        let profile = test_safety_profile();
        let person = type_id(TypeKind::Entity, "person");
        let type_fact = SchemaFact::Type(TypeFact::new(person).expect("type"));
        let source = declared(vec![type_fact.clone()]);
        let target = declared(Vec::new());
        let conditions = derive_safety_conditions(
            0,
            &SchemaOperation::undefine(type_fact),
            &source,
            &target,
            &profile,
        )
        .expect("undefine guard");
        assert_eq!(conditions.policy(), SafetyClass::Destructive);
        assert!(!conditions.resolves_conditional_requirements());
        assert_eq!(conditions.conditions()[0].policy(), SafetyClass::Destructive);
        assert!(matches!(
            conditions.conditions()[0].condition(),
            SafetyCondition::NoInstances {
                include_subtypes: true,
                ..
            }
        ));
    }

    #[test]
    fn range_and_values_emit_only_for_actual_narrowing() {
        let profile = test_safety_profile();
        let age = AttributeId::new("age").expect("attribute");
        let subject = AnnotationSubjectId::Value(ValueFactId::new(age.clone()));
        let base = vec![
            SchemaFact::Type(
                TypeFact::new(type_id(TypeKind::Attribute, "age")).expect("type"),
            ),
            SchemaFact::Value(ValueFact::new(
                ValueFactId::new(age),
                ValueTypeTag::Long,
            )),
        ];
        let range = |lower: Option<i64>, upper: Option<i64>| {
            annotation_fact(
                subject.clone(),
                AnnotationKindId::Range,
                SchemaAnnotationValue::Range(
                    CanonicalValueRange::new(
                        lower.map(CanonicalValue::Long),
                        upper.map(CanonicalValue::Long),
                    )
                    .expect("range"),
                ),
            )
        };
        let derive_range = |old: SchemaFact, new: SchemaFact| {
            let source = declared(base.iter().cloned().chain([old.clone()]).collect());
            let target = declared(base.iter().cloned().chain([new.clone()]).collect());
            derive_safety_conditions(
                9,
                &SchemaOperation::redefine(old, new).expect("range operation"),
                &source,
                &target,
                &profile,
            )
            .expect("range derivation")
        };

        let widening = derive_range(range(Some(2), Some(8)), range(Some(1), Some(9)));
        assert_eq!(widening.policy(), SafetyClass::Conditional);
        assert!(widening.conditions().is_empty());

        let narrowing = derive_range(range(Some(1), Some(9)), range(Some(2), Some(8)));
        assert_eq!(narrowing.conditions().len(), 2);
        assert!(matches!(
            narrowing.conditions()[0].condition(),
            SafetyCondition::RangeLower {
                lower: CanonicalValue::Long(2),
                ..
            }
        ));
        assert!(matches!(
            narrowing.conditions()[1].condition(),
            SafetyCondition::RangeUpper {
                upper: CanonicalValue::Long(8),
                ..
            }
        ));
        let repeated = derive_range(range(Some(1), Some(9)), range(Some(2), Some(8)));
        assert_eq!(narrowing.conditions()[0].id(), repeated.conditions()[0].id());

        let mixed = derive_range(range(Some(2), None), range(None, Some(8)));
        assert_eq!(mixed.conditions().len(), 1);
        assert!(matches!(
            mixed.conditions()[0].condition(),
            SafetyCondition::RangeUpper {
                upper: CanonicalValue::Long(8),
                ..
            }
        ));

        let values = |members: &[i64]| {
            annotation_fact(
                subject.clone(),
                AnnotationKindId::Values,
                SchemaAnnotationValue::Values(
                    CanonicalValueSet::new(
                        members.iter().copied().map(CanonicalValue::Long),
                    )
                    .expect("values"),
                ),
            )
        };
        let derive_values = |old: SchemaFact, new: SchemaFact| {
            let source = declared(base.iter().cloned().chain([old.clone()]).collect());
            let target = declared(base.iter().cloned().chain([new.clone()]).collect());
            derive_safety_conditions(
                10,
                &SchemaOperation::redefine(old, new).expect("values operation"),
                &source,
                &target,
                &profile,
            )
            .expect("values derivation")
        };

        let superset = derive_values(values(&[2, 4]), values(&[2, 4, 6]));
        assert!(superset.conditions().is_empty());
        let subset = derive_values(values(&[2, 4, 6]), values(&[2, 4]));
        assert_eq!(subset.conditions().len(), 1);
        assert!(matches!(
            subset.conditions()[0].condition(),
            SafetyCondition::ValuesNarrowed { allowed, .. } if allowed == &vec![
                CanonicalValue::Long(2),
                CanonicalValue::Long(4),
            ]
        ));
        let partial = derive_values(values(&[2, 4]), values(&[4, 6]));
        assert_eq!(partial.conditions().len(), 1);
    }
}
