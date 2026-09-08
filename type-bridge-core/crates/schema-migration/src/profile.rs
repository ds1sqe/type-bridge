//! Closed TypeDB 3.12.1 schema-transition registry.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde::Serialize;
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, FingerprintDomain, SemanticProfileId,
};
use type_bridge_contract::id::FunctionId;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationKindId, AnnotationSubjectId, SchemaAnnotationValue, SchemaFact,
    SchemaOperation, SchemaOperationKind,
};
use type_bridge_contract::schema_delta::{
    SCHEMA_TRANSITION_CAPABILITY_IDS, schema_transition_capability_vocabulary,
};
use type_bridge_contract::schema_lowering::{
    SCHEMA_LOWERING_PROFILE_CANONICALIZATION, SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN,
    SchemaLoweringProfileBinding, SchemaLoweringProfileFingerprint, SchemaLoweringProfileId,
};
use type_bridge_schema::{SafetyClass, SafetyClassificationError};

const PROVIDER: &str = "typedb";
const PROVIDER_VERSION: &str = "3.12.1";
const SEMANTIC_PROFILE: &str = "typedb-3.12.1/v1";

const CAP_TRANSACTION_ATOMIC: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[0];
const CAP_DEFINE: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[1];
const CAP_UNDEFINE: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[2];
const CAP_REDEFINE_SUB: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[3];
const CAP_REDEFINE_VALUE: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[4];
const CAP_REDEFINE_RELATES_SPECIALIZATION: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[5];
const CAP_REDEFINE_ANNOTATION: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[6];
const CAP_REDEFINE_FUNCTION: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[7];
const CAP_REPLACE_SUB_ANNOTATION: &str = SCHEMA_TRANSITION_CAPABILITY_IDS[8];

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Schema fact category addressed by a lowering rule.
pub enum FactKind {
    /// Type declaration fact.
    Type,
    /// Direct subtype fact.
    Sub,
    /// Attribute value-type fact.
    Value,
    /// Attribute ownership fact.
    Owns,
    /// Relation role declaration fact.
    Relates,
    /// Relation role specialization fact.
    RelatesSpecialization,
    /// Role-playing fact.
    Plays,
    /// Function declaration fact.
    Function,
    /// Struct declaration fact.
    Struct,
}

impl FactKind {
    /// Closed set of fact kinds in stable registry order.
    pub const ALL: [Self; 9] = [
        Self::Type,
        Self::Sub,
        Self::Value,
        Self::Owns,
        Self::Relates,
        Self::RelatesSpecialization,
        Self::Plays,
        Self::Function,
        Self::Struct,
    ];
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Operation applied to one schema fact.
pub enum FactTransition {
    /// Introduce a fact.
    Define,
    /// Remove a fact.
    Undefine,
    /// Replace a fact in place.
    Redefine,
}

impl FactTransition {
    /// Closed set of fact transitions in stable registry order.
    pub const ALL: [Self; 3] = [Self::Define, Self::Undefine, Self::Redefine];
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Schema subject category to which an annotation is attached.
pub enum AnnotationSubjectKind {
    /// Type declaration subject.
    Type,
    /// Direct subtype subject.
    Sub,
    /// Attribute value-type subject.
    Value,
    /// Attribute ownership subject.
    Owns,
    /// Relation role subject.
    Relates,
    /// Role-playing subject.
    Plays,
    /// Function declaration subject.
    Function,
    /// Struct declaration subject.
    Struct,
}

impl AnnotationSubjectKind {
    /// Closed set of annotation subjects in stable registry order.
    pub const ALL: [Self; 8] = [
        Self::Type,
        Self::Sub,
        Self::Value,
        Self::Owns,
        Self::Relates,
        Self::Plays,
        Self::Function,
        Self::Struct,
    ];
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Annotation category covered by the lowering profile.
pub enum AnnotationKind {
    /// Abstract-type annotation.
    Abstract,
    /// Independent-attribute annotation.
    Independent,
    /// Key annotation.
    Key,
    /// Unique annotation.
    Unique,
    /// Cardinality annotation.
    Card,
    /// Regular-expression constraint.
    Regex,
    /// Ordered range constraint.
    Range,
    /// Enumerated-values constraint.
    Values,
    /// Documentation annotation.
    Doc,
    /// Keyed metadata annotation.
    Meta,
}

impl AnnotationKind {
    /// Closed set of annotation kinds in stable registry order.
    pub const ALL: [Self; 10] = [
        Self::Abstract,
        Self::Independent,
        Self::Key,
        Self::Unique,
        Self::Card,
        Self::Regex,
        Self::Range,
        Self::Values,
        Self::Doc,
        Self::Meta,
    ];
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Change applied to an annotation.
pub enum AnnotationTransition {
    /// Attach an annotation that was absent.
    Add,
    /// Replace an annotation value.
    Change,
    /// Remove an annotation.
    Remove,
}

impl AnnotationTransition {
    /// Closed set of annotation transitions in stable registry order.
    pub const ALL: [Self; 3] = [Self::Add, Self::Change, Self::Remove];
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Provider operation used to lower a semantic transition.
pub enum LoweringMechanism {
    /// Lower to a TypeQL `define` statement.
    Define,
    /// Lower to a TypeQL `undefine` statement.
    Undefine,
    /// Lower to a TypeQL `redefine` statement.
    Redefine,
    /// Lower to an atomic `undefine` followed by `define`.
    AtomicUndefineDefine,
    /// The provider cannot faithfully lower the transition.
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Complete lowering and safety decision for one transition.
pub struct TransitionRule {
    /// Provider operation used for lowering.
    pub mechanism: LoweringMechanism,
    /// Required operator-safety classification.
    pub safety: SafetyClass,
    /// Capabilities required from the execution provider.
    pub required_capabilities: CapabilitySet,
    /// Whether metadata removal must address one exact metadata key.
    pub keyed_meta: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Registry row for one fact transition.
pub struct FactTransitionRule {
    /// Fact category matched by the row.
    pub fact: FactKind,
    /// Transition matched by the row.
    pub transition: FactTransition,
    /// Lowering decision for the matched pair.
    pub rule: TransitionRule,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Registry row for one annotation transition.
pub struct AnnotationTransitionRule {
    /// Subject category matched by the row.
    pub subject: AnnotationSubjectKind,
    /// Annotation category matched by the row.
    pub annotation: AnnotationKind,
    /// Transition matched by the row.
    pub transition: AnnotationTransition,
    /// Lowering decision for the matched tuple.
    pub rule: TransitionRule,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Semantic safety scenario classified independently of provider syntax.
pub enum SafetyScenario {
    /// An explicit annotation equals the provider's implicit default.
    ExplicitDefaultEquivalent,
    /// Documentation or metadata changes without stored-data impact.
    DocMetaTransition,
    /// Add an optional ownership, role, or playing interface.
    AddOptionalInterface,
    /// Add an interface with a required minimum cardinality.
    AddRequiredCardinality,
    /// Add a key or uniqueness constraint.
    AddKeyOrUnique,
    /// Expand an allowed cardinality interval.
    WidenCardinality,
    /// Contract an allowed cardinality interval.
    NarrowCardinality,
    /// Remove a cardinality annotation equal to the provider default.
    RemoveCardinalityToEqualDefault,
    /// Remove a cardinality annotation whose provider default is narrower.
    RemoveCardinalityToNarrowerDefault,
    /// Remove a cardinality annotation whose provider default is wider.
    RemoveCardinalityToWiderDefault,
    /// Add or tighten a stored-value constraint.
    AddOrTightenValueConstraint,
    /// Remove a stored-value constraint.
    RemoveValueConstraint,
    /// Mark a type abstract.
    AddAbstract,
    /// Make an abstract type concrete.
    RemoveAbstract,
    /// Mark an attribute type independent.
    AddIndependent,
    /// Remove independent attribute status.
    RemoveIndependent,
    /// Change a direct supertype.
    ChangeSub,
    /// Change a relation role specialization.
    ChangeRelatesSpecialization,
    /// Change an attribute value type.
    ChangeValueType,
    /// Remove a schema fact.
    RemoveFact,
    /// Replace a function definition.
    RedefineFunction,
    /// Request a transition unsupported by the provider profile.
    UnsupportedProviderTransition,
}

impl SafetyScenario {
    /// Closed set of safety scenarios in stable registry order.
    pub const ALL: [Self; 22] = [
        Self::ExplicitDefaultEquivalent,
        Self::DocMetaTransition,
        Self::AddOptionalInterface,
        Self::AddRequiredCardinality,
        Self::AddKeyOrUnique,
        Self::WidenCardinality,
        Self::NarrowCardinality,
        Self::RemoveCardinalityToEqualDefault,
        Self::RemoveCardinalityToNarrowerDefault,
        Self::RemoveCardinalityToWiderDefault,
        Self::AddOrTightenValueConstraint,
        Self::RemoveValueConstraint,
        Self::AddAbstract,
        Self::RemoveAbstract,
        Self::AddIndependent,
        Self::RemoveIndependent,
        Self::ChangeSub,
        Self::ChangeRelatesSpecialization,
        Self::ChangeValueType,
        Self::RemoveFact,
        Self::RedefineFunction,
        Self::UnsupportedProviderTransition,
    ];
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Additional evidence required before a migration can proceed safely.
pub enum EvidenceRequirement {
    /// No evidence beyond the verified schema delta is required.
    None,
    /// Existing stored data must be proven valid under the target schema.
    ExistingDataSatisfiesTarget,
    /// Existing instances require a data backfill.
    Backfill,
    /// Values require an explicit conversion supplied by the operator.
    ExplicitConversion,
    /// The operator must approve the classified risk.
    OperatorApproval,
    /// The execution provider must first gain support for the transition.
    ProviderSupport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
/// Safety classification and evidence policy for one semantic scenario.
pub struct SafetyScenarioRule {
    /// Scenario classified by the row.
    pub scenario: SafetyScenario,
    /// Safety class assigned to the scenario.
    pub safety: SafetyClass,
    /// Evidence required before execution.
    pub evidence: EvidenceRequirement,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Interface category with a provider-defined default cardinality.
pub enum InterfaceKind {
    /// Attribute ownership interface.
    Owns,
    /// Relation role interface.
    Relates,
    /// Role-playing interface.
    Plays,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
/// Provider default cardinality for one interface category.
pub struct InterfaceDefault {
    /// Interface category governed by this default.
    pub interface: InterfaceKind,
    /// Inclusive minimum cardinality.
    pub min: u64,
    /// Inclusive maximum cardinality, or unbounded when absent.
    pub max: Option<u64>,
}

const TYPEDB_3_12_1_INTERFACE_DEFAULTS: [InterfaceDefault; 3] = [
    InterfaceDefault {
        interface: InterfaceKind::Owns,
        min: 0,
        max: Some(1),
    },
    InterfaceDefault {
        interface: InterfaceKind::Relates,
        min: 0,
        max: Some(1),
    },
    InterfaceDefault {
        interface: InterfaceKind::Plays,
        min: 0,
        max: None,
    },
];

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
/// Measured provider behavior that supports the frozen lowering profile.
pub enum EvidenceFlag {
    /// `redefine` statements address exactly one schema subject.
    RedefineQueriesAreSingleton,
    /// A rejected `redefine` leaves the schema unchanged.
    RejectedRedefinePreservesSchema,
    /// Schema transactions commit atomically.
    SchemaTransactionsAreAtomic,
    /// Role specialization preserves data that remains valid.
    RelatesSpecializationPreservesValidData,
    /// Direct redefinition of a subtype annotation is rejected.
    SubAnnotationDirectRedefineRejected,
    /// Atomic replacement of a subtype annotation is supported.
    SubAnnotationAtomicReplacementSupported,
    /// Metadata removal addresses one exact metadata key.
    MetaRemovalIsKeyed,
    /// Function redefinition alone can leave stored metadata stale.
    FunctionRedefineLeavesStoredMetadataStale,
    /// Atomic function replacement updates its bound metadata.
    FunctionAtomicReplacementUpdatesMetadata,
    /// Struct transitions are not supported by this profile.
    StructTransitionsUnsupported,
    /// Removing independence deletes attributes without an owner.
    IndependentRemovalDeletesOwnerlessAttributes,
    /// Guarded schema changes reject incompatible stored data.
    GuardedChangesRejectInvalidData,
    /// Removing an owns cardinality restores the `0..1` default.
    OwnsCardRemovalRestoresZeroToOneDefault,
    /// Removing a relates cardinality restores the `0..1` default.
    RelatesCardRemovalRestoresZeroToOneDefault,
    /// Removing a plays cardinality restores the `0..*` default.
    PlaysCardRemovalRestoresZeroToUnboundedDefault,
}

impl EvidenceFlag {
    /// Closed set of provider evidence flags in stable registry order.
    pub const ALL: [Self; 15] = [
        Self::RedefineQueriesAreSingleton,
        Self::RejectedRedefinePreservesSchema,
        Self::SchemaTransactionsAreAtomic,
        Self::RelatesSpecializationPreservesValidData,
        Self::SubAnnotationDirectRedefineRejected,
        Self::SubAnnotationAtomicReplacementSupported,
        Self::MetaRemovalIsKeyed,
        Self::FunctionRedefineLeavesStoredMetadataStale,
        Self::FunctionAtomicReplacementUpdatesMetadata,
        Self::StructTransitionsUnsupported,
        Self::IndependentRemovalDeletesOwnerlessAttributes,
        Self::GuardedChangesRejectInvalidData,
        Self::OwnsCardRemovalRestoresZeroToOneDefault,
        Self::RelatesCardRemovalRestoresZeroToOneDefault,
        Self::PlaysCardRemovalRestoresZeroToUnboundedDefault,
    ];
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Frozen, fingerprinted registry for lowering schema deltas to one provider.
pub struct SchemaLoweringProfile {
    /// Stable profile identifier.
    pub id: SchemaLoweringProfileId,
    /// Domain used to fingerprint canonical profile bytes.
    pub fingerprint_domain: FingerprintDomain,
    /// Canonicalization identity used for profile bytes.
    pub canonicalization: CanonicalizationVersion,
    /// Schema semantic profile accepted by this lowering profile.
    pub semantic_profile: SemanticProfileId,
    /// Provider family name.
    pub provider: String,
    /// Exact measured provider version.
    pub provider_version: String,
    /// Whether schema statements execute transactionally.
    pub transactional_schema_queries: bool,
    /// Capabilities required to execute every supported rule.
    pub required_capabilities: CapabilitySet,
    /// Provider defaults for schema interfaces.
    pub interface_defaults: Vec<InterfaceDefault>,
    /// Closed fact-transition rule table.
    pub fact_rules: Vec<FactTransitionRule>,
    /// Closed annotation-transition rule table.
    pub annotation_rules: Vec<AnnotationTransitionRule>,
    /// Closed semantic safety rule table.
    pub safety_rules: Vec<SafetyScenarioRule>,
    /// Measured provider evidence bound by the profile.
    pub evidence: Vec<EvidenceFlag>,
}

impl SchemaLoweringProfile {
    /// Return the rule for an exact fact transition, when registered.
    pub fn fact_rule(&self, fact: FactKind, transition: FactTransition) -> Option<&TransitionRule> {
        self.fact_rules
            .iter()
            .find(|row| row.fact == fact && row.transition == transition)
            .map(|row| &row.rule)
    }

    /// Return the rule for an exact annotation transition, when registered.
    pub fn annotation_rule(
        &self,
        subject: AnnotationSubjectKind,
        annotation: AnnotationKind,
        transition: AnnotationTransition,
    ) -> Option<&TransitionRule> {
        self.annotation_rules
            .iter()
            .find(|row| {
                row.subject == subject
                    && row.annotation == annotation
                    && row.transition == transition
            })
            .map(|row| &row.rule)
    }

    /// Return the safety rule for an exact semantic scenario.
    pub fn safety_rule(&self, scenario: SafetyScenario) -> Option<&SafetyScenarioRule> {
        self.safety_rules
            .iter()
            .find(|row| row.scenario == scenario)
    }
}

#[derive(Debug)]
pub(crate) struct OperationTransitionClassification {
    pub(crate) safety: SafetyClass,
    pub(crate) atomic: bool,
    pub(crate) required_capabilities: CapabilitySet,
}

pub(crate) fn classify_operation_transition(
    operation: &SchemaOperation,
) -> Result<OperationTransitionClassification, SafetyClassificationError> {
    let mut rules = Vec::new();
    match operation.kind() {
        SchemaOperationKind::Define => {
            let facts = operation.defined_facts().expect("define exposes facts");
            let functions = facts
                .iter()
                .filter_map(|fact| match fact {
                    SchemaFact::Function(function) => Some(function.id().clone()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            for fact in facts {
                rules.push(classify_defined_fact(fact, &functions));
            }
        }
        SchemaOperationKind::Redefine => rules.push(classify_redefinition(
            operation
                .expected_fact()
                .expect("redefine exposes expected fact"),
            operation
                .replacement_fact()
                .expect("redefine exposes replacement fact"),
        )?),
        SchemaOperationKind::Undefine => rules.push(classify_undefined_fact(
            operation.undefined_fact().expect("undefine exposes fact"),
        )),
    }

    let mut safety = SafetyClass::FormalOnly;
    let mut atomic = false;
    let mut required_capabilities = CapabilitySet::new();
    for rule in rules {
        if safety_rank(rule.safety) > safety_rank(safety) {
            safety = rule.safety;
        }
        atomic |= rule.mechanism == LoweringMechanism::AtomicUndefineDefine;
        for capability in rule.required_capabilities.iter().cloned() {
            required_capabilities.insert(capability);
        }
    }
    Ok(OperationTransitionClassification {
        safety,
        atomic,
        required_capabilities,
    })
}

fn classify_defined_fact(fact: &SchemaFact, functions: &BTreeSet<FunctionId>) -> TransitionRule {
    match fact {
        SchemaFact::Relates(relates) if relates.specializes().is_some() => {
            fact_transition_rule(FactKind::RelatesSpecialization, FactTransition::Define)
        }
        SchemaFact::Annotation(annotation) => {
            if let AnnotationSubjectId::Function(function) = annotation.id().subject()
                && functions.contains(function)
                && matches!(
                    annotation.id().kind(),
                    AnnotationKindId::Doc | AnnotationKindId::Meta(_)
                )
            {
                return define(SafetyClass::SchemaMetadata, false);
            }
            classify_annotation(annotation, AnnotationTransition::Add, None)
        }
        _ => fact_transition_rule(fact_kind(fact), FactTransition::Define),
    }
}

fn classify_undefined_fact(fact: &SchemaFact) -> TransitionRule {
    match fact {
        SchemaFact::Annotation(annotation) => {
            classify_annotation(annotation, AnnotationTransition::Remove, None)
        }
        _ => fact_transition_rule(fact_kind(fact), FactTransition::Undefine),
    }
}

fn classify_redefinition(
    expected: &SchemaFact,
    replacement: &SchemaFact,
) -> Result<TransitionRule, SafetyClassificationError> {
    match (expected, replacement) {
        (SchemaFact::Relates(old), SchemaFact::Relates(new)) => {
            let transition = match (old.specializes(), new.specializes()) {
                (None, Some(_)) => FactTransition::Define,
                (Some(_), None) => FactTransition::Undefine,
                (Some(_), Some(_)) => FactTransition::Redefine,
                (None, None) => {
                    return Err(SafetyClassificationError::UnchangedRelatesSpecialization);
                }
            };
            Ok(fact_transition_rule(
                FactKind::RelatesSpecialization,
                transition,
            ))
        }
        (SchemaFact::Annotation(old), SchemaFact::Annotation(new)) => Ok(classify_annotation(
            new,
            AnnotationTransition::Change,
            Some(old),
        )),
        (left, right) if std::mem::discriminant(left) == std::mem::discriminant(right) => Ok(
            fact_transition_rule(fact_kind(right), FactTransition::Redefine),
        ),
        _ => Err(SafetyClassificationError::RedefinitionCategoryChanged),
    }
}

fn fact_kind(fact: &SchemaFact) -> FactKind {
    match fact {
        SchemaFact::Type(_) => FactKind::Type,
        SchemaFact::Sub(_) => FactKind::Sub,
        SchemaFact::Value(_) => FactKind::Value,
        SchemaFact::Owns(_) => FactKind::Owns,
        SchemaFact::Relates(_) => FactKind::Relates,
        SchemaFact::Plays(_) => FactKind::Plays,
        SchemaFact::Annotation(_) => unreachable!("annotations use the annotation registry"),
        SchemaFact::Function(_) => FactKind::Function,
        SchemaFact::Struct(_) => FactKind::Struct,
    }
}

fn classify_annotation(
    annotation: &AnnotationFact,
    transition: AnnotationTransition,
    expected: Option<&AnnotationFact>,
) -> TransitionRule {
    let subject = annotation_subject_kind(annotation.id().subject());
    let kind = annotation_kind(annotation.id().kind());
    let mut rule = annotation_transition_rule(subject, kind, transition);
    if kind == AnnotationKind::Card
        && let Some(target) = annotation_cardinality(annotation)
        && let Some(default) = default_cardinality(annotation.id().subject())
    {
        let (from, to) = match transition {
            AnnotationTransition::Add => (default, target),
            AnnotationTransition::Change => {
                let Some(source) = expected.and_then(annotation_cardinality) else {
                    return rule;
                };
                (source, target)
            }
            AnnotationTransition::Remove => (target, default),
        };
        rule.safety = cardinality_transition_safety(from, to);
    }
    rule
}

fn annotation_subject_kind(subject: &AnnotationSubjectId) -> AnnotationSubjectKind {
    match subject {
        AnnotationSubjectId::Type(_) => AnnotationSubjectKind::Type,
        AnnotationSubjectId::Sub(_) => AnnotationSubjectKind::Sub,
        AnnotationSubjectId::Value(_) => AnnotationSubjectKind::Value,
        AnnotationSubjectId::Owns(_) => AnnotationSubjectKind::Owns,
        AnnotationSubjectId::Relates(_) => AnnotationSubjectKind::Relates,
        AnnotationSubjectId::Plays(_) => AnnotationSubjectKind::Plays,
        AnnotationSubjectId::Function(_) => AnnotationSubjectKind::Function,
    }
}

fn annotation_kind(kind: &AnnotationKindId) -> AnnotationKind {
    match kind {
        AnnotationKindId::Abstract => AnnotationKind::Abstract,
        AnnotationKindId::Independent => AnnotationKind::Independent,
        AnnotationKindId::Key => AnnotationKind::Key,
        AnnotationKindId::Unique => AnnotationKind::Unique,
        AnnotationKindId::Card => AnnotationKind::Card,
        AnnotationKindId::Regex => AnnotationKind::Regex,
        AnnotationKindId::Range => AnnotationKind::Range,
        AnnotationKindId::Values => AnnotationKind::Values,
        AnnotationKindId::Doc => AnnotationKind::Doc,
        AnnotationKindId::Meta(_) => AnnotationKind::Meta,
    }
}

fn annotation_cardinality(annotation: &AnnotationFact) -> Option<(u64, Option<u64>)> {
    match annotation.value() {
        SchemaAnnotationValue::Cardinality(cardinality) => {
            Some(((*cardinality).min(), (*cardinality).max()))
        }
        _ => None,
    }
}

fn default_cardinality(subject: &AnnotationSubjectId) -> Option<(u64, Option<u64>)> {
    let interface = match subject {
        AnnotationSubjectId::Owns(_) => InterfaceKind::Owns,
        AnnotationSubjectId::Relates(_) => InterfaceKind::Relates,
        AnnotationSubjectId::Plays(_) => InterfaceKind::Plays,
        _ => return None,
    };
    TYPEDB_3_12_1_INTERFACE_DEFAULTS
        .iter()
        .find(|default| default.interface == interface)
        .map(|default| (default.min, default.max))
}

fn cardinality_transition_safety(from: (u64, Option<u64>), to: (u64, Option<u64>)) -> SafetyClass {
    if from == to {
        SafetyClass::FormalOnly
    } else if interval_contains(to, from) {
        SafetyClass::Additive
    } else if interval_contains(from, to) {
        SafetyClass::BackfillRequired
    } else {
        SafetyClass::Conditional
    }
}

fn interval_contains(outer: (u64, Option<u64>), inner: (u64, Option<u64>)) -> bool {
    outer.0 <= inner.0
        && match (outer.1, inner.1) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(outer), Some(inner)) => outer >= inner,
        }
}

fn safety_rank(safety: SafetyClass) -> u8 {
    match safety {
        SafetyClass::FormalOnly => 0,
        SafetyClass::SchemaMetadata => 1,
        SafetyClass::Additive => 2,
        SafetyClass::Conditional => 3,
        SafetyClass::BackfillRequired => 4,
        SafetyClass::Destructive => 5,
        SafetyClass::Opaque => 6,
        SafetyClass::Unsupported => 7,
    }
}

fn capabilities(ids: &[&str]) -> CapabilitySet {
    let mut capabilities = CapabilitySet::new();
    for id in ids {
        capabilities
            .insert(CapabilityId::new(*id).expect("fixed schema-lowering capability id is valid"));
    }
    capabilities
}

fn transition_rule(
    mechanism: LoweringMechanism,
    safety: SafetyClass,
    required: &[&str],
    keyed_meta: bool,
) -> TransitionRule {
    TransitionRule {
        mechanism,
        safety,
        required_capabilities: capabilities(required),
        keyed_meta,
    }
}

fn unsupported(keyed_meta: bool) -> TransitionRule {
    transition_rule(
        LoweringMechanism::Unsupported,
        SafetyClass::Unsupported,
        &[],
        keyed_meta,
    )
}

fn define(safety: SafetyClass, keyed_meta: bool) -> TransitionRule {
    transition_rule(LoweringMechanism::Define, safety, &[CAP_DEFINE], keyed_meta)
}

fn undefine(safety: SafetyClass, keyed_meta: bool) -> TransitionRule {
    transition_rule(
        LoweringMechanism::Undefine,
        safety,
        &[CAP_UNDEFINE],
        keyed_meta,
    )
}

fn redefine(safety: SafetyClass, capability: &str, keyed_meta: bool) -> TransitionRule {
    transition_rule(
        LoweringMechanism::Redefine,
        safety,
        &[capability],
        keyed_meta,
    )
}

/// Resolve the compiled lowering rule for a schema fact transition.
pub fn fact_transition_rule(fact: FactKind, transition: FactTransition) -> TransitionRule {
    use FactKind as F;
    use FactTransition as T;
    use SafetyClass as S;

    match (fact, transition) {
        (F::Type, T::Define) => define(S::Additive, false),
        (F::Type, T::Undefine) => undefine(S::Destructive, false),
        (F::Type, T::Redefine) => unsupported(false),
        (F::Sub, T::Define) => define(S::Conditional, false),
        (F::Sub, T::Undefine) => undefine(S::Destructive, false),
        (F::Sub, T::Redefine) => redefine(S::Conditional, CAP_REDEFINE_SUB, false),
        (F::Value, T::Define) => define(S::Additive, false),
        (F::Value, T::Undefine) => undefine(S::Destructive, false),
        (F::Value, T::Redefine) => redefine(S::Destructive, CAP_REDEFINE_VALUE, false),
        (F::Owns | F::Relates | F::Plays, T::Define) => define(S::Additive, false),
        (F::Owns | F::Relates | F::Plays, T::Undefine) => undefine(S::Destructive, false),
        (F::Owns | F::Relates | F::Plays, T::Redefine) => unsupported(false),
        (F::RelatesSpecialization, T::Define) => define(S::Conditional, false),
        (F::RelatesSpecialization, T::Undefine) => undefine(S::Conditional, false),
        (F::RelatesSpecialization, T::Redefine) => {
            redefine(S::Conditional, CAP_REDEFINE_RELATES_SPECIALIZATION, false)
        }
        (F::Function, T::Define) => define(S::Additive, false),
        (F::Function, T::Undefine) => undefine(S::Destructive, false),
        (F::Function, T::Redefine) => redefine(S::Opaque, CAP_REDEFINE_FUNCTION, false),
        (F::Struct, _) => unsupported(false),
    }
}

fn annotation_is_supported(subject: AnnotationSubjectKind, annotation: AnnotationKind) -> bool {
    use AnnotationKind as A;
    use AnnotationSubjectKind as S;

    match subject {
        S::Type => matches!(annotation, A::Abstract | A::Independent | A::Doc | A::Meta),
        S::Sub => matches!(annotation, A::Doc | A::Meta),
        S::Value => matches!(
            annotation,
            A::Regex | A::Range | A::Values | A::Doc | A::Meta
        ),
        S::Owns => matches!(
            annotation,
            A::Key | A::Unique | A::Card | A::Regex | A::Range | A::Values | A::Doc | A::Meta
        ),
        S::Relates => matches!(annotation, A::Abstract | A::Card | A::Doc | A::Meta),
        S::Plays => matches!(annotation, A::Card | A::Doc | A::Meta),
        S::Function | S::Struct => false,
    }
}

/// Resolve the compiled lowering rule for an annotation transition.
pub fn annotation_transition_rule(
    subject: AnnotationSubjectKind,
    annotation: AnnotationKind,
    transition: AnnotationTransition,
) -> TransitionRule {
    use AnnotationKind as A;
    use AnnotationSubjectKind as S;
    use AnnotationTransition as T;
    use SafetyClass as C;

    let keyed_meta = annotation == A::Meta;
    if !annotation_is_supported(subject, annotation) {
        return unsupported(keyed_meta);
    }
    if subject == S::Sub {
        return match transition {
            T::Add => define(C::SchemaMetadata, keyed_meta),
            T::Change => transition_rule(
                LoweringMechanism::AtomicUndefineDefine,
                C::SchemaMetadata,
                &[
                    CAP_TRANSACTION_ATOMIC,
                    CAP_UNDEFINE,
                    CAP_DEFINE,
                    CAP_REPLACE_SUB_ANNOTATION,
                ],
                keyed_meta,
            ),
            T::Remove => undefine(C::SchemaMetadata, keyed_meta),
        };
    }

    match annotation {
        A::Doc | A::Meta => match transition {
            T::Add => define(C::SchemaMetadata, keyed_meta),
            T::Change => redefine(C::SchemaMetadata, CAP_REDEFINE_ANNOTATION, keyed_meta),
            T::Remove => undefine(C::SchemaMetadata, keyed_meta),
        },
        A::Abstract | A::Independent | A::Key | A::Unique => match transition {
            T::Change => unsupported(keyed_meta),
            T::Add => {
                let safety = match annotation {
                    A::Abstract => C::Conditional,
                    A::Independent => C::Additive,
                    A::Key | A::Unique => C::BackfillRequired,
                    _ => unreachable!(),
                };
                define(safety, keyed_meta)
            }
            T::Remove => {
                let safety = match annotation {
                    A::Independent => C::Destructive,
                    A::Abstract | A::Key | A::Unique => C::Additive,
                    _ => unreachable!(),
                };
                undefine(safety, keyed_meta)
            }
        },
        A::Card => match transition {
            T::Add => define(C::Conditional, keyed_meta),
            T::Change => redefine(C::Conditional, CAP_REDEFINE_ANNOTATION, keyed_meta),
            T::Remove => undefine(C::Conditional, keyed_meta),
        },
        A::Regex | A::Range | A::Values => match transition {
            T::Add => define(C::Conditional, keyed_meta),
            T::Change => redefine(C::Conditional, CAP_REDEFINE_ANNOTATION, keyed_meta),
            T::Remove => undefine(C::Additive, keyed_meta),
        },
    }
}

fn safety_rule(scenario: SafetyScenario) -> SafetyScenarioRule {
    use EvidenceRequirement as E;
    use SafetyClass as C;
    use SafetyScenario as S;

    let (safety, evidence) = match scenario {
        S::ExplicitDefaultEquivalent | S::RemoveCardinalityToEqualDefault => {
            (C::FormalOnly, E::None)
        }
        S::DocMetaTransition => (C::SchemaMetadata, E::None),
        S::AddOptionalInterface
        | S::WidenCardinality
        | S::RemoveCardinalityToWiderDefault
        | S::RemoveValueConstraint
        | S::RemoveAbstract
        | S::AddIndependent => (C::Additive, E::None),
        S::AddRequiredCardinality
        | S::AddKeyOrUnique
        | S::NarrowCardinality
        | S::RemoveCardinalityToNarrowerDefault => (C::BackfillRequired, E::Backfill),
        S::AddOrTightenValueConstraint
        | S::AddAbstract
        | S::ChangeSub
        | S::ChangeRelatesSpecialization => (C::Conditional, E::ExistingDataSatisfiesTarget),
        S::RemoveIndependent | S::RemoveFact => (C::Destructive, E::OperatorApproval),
        S::ChangeValueType => (C::Destructive, E::ExplicitConversion),
        S::RedefineFunction => (C::Opaque, E::OperatorApproval),
        S::UnsupportedProviderTransition => (C::Unsupported, E::ProviderSupport),
    };
    SafetyScenarioRule {
        scenario,
        safety,
        evidence,
    }
}

fn build_profile() -> SchemaLoweringProfile {
    let mut fact_rules = Vec::with_capacity(FactKind::ALL.len() * FactTransition::ALL.len());
    for fact in FactKind::ALL {
        for transition in FactTransition::ALL {
            fact_rules.push(FactTransitionRule {
                fact,
                transition,
                rule: fact_transition_rule(fact, transition),
            });
        }
    }
    let mut annotation_rules = Vec::with_capacity(
        AnnotationSubjectKind::ALL.len()
            * AnnotationKind::ALL.len()
            * AnnotationTransition::ALL.len(),
    );
    for subject in AnnotationSubjectKind::ALL {
        for annotation in AnnotationKind::ALL {
            for transition in AnnotationTransition::ALL {
                annotation_rules.push(AnnotationTransitionRule {
                    subject,
                    annotation,
                    transition,
                    rule: annotation_transition_rule(subject, annotation, transition),
                });
            }
        }
    }

    SchemaLoweringProfile {
        id: SchemaLoweringProfileId::typedb_3_12_1(),
        fingerprint_domain: FingerprintDomain::new(SCHEMA_LOWERING_PROFILE_FINGERPRINT_DOMAIN)
            .expect("fixed fingerprint domain is valid"),
        canonicalization: CanonicalizationVersion::new(SCHEMA_LOWERING_PROFILE_CANONICALIZATION)
            .expect("fixed canonicalization is valid"),
        semantic_profile: SemanticProfileId::new(SEMANTIC_PROFILE)
            .expect("fixed semantic profile is valid"),
        provider: PROVIDER.to_owned(),
        provider_version: PROVIDER_VERSION.to_owned(),
        transactional_schema_queries: true,
        required_capabilities: schema_transition_capability_vocabulary(),
        interface_defaults: TYPEDB_3_12_1_INTERFACE_DEFAULTS.to_vec(),
        fact_rules,
        annotation_rules,
        safety_rules: SafetyScenario::ALL.into_iter().map(safety_rule).collect(),
        evidence: EvidenceFlag::ALL.to_vec(),
    }
}

/// Return the immutable TypeDB 3.12.1 lowering profile.
pub fn typedb_3_12_1_profile() -> &'static SchemaLoweringProfile {
    static PROFILE: OnceLock<SchemaLoweringProfile> = OnceLock::new();
    PROFILE.get_or_init(build_profile)
}

/// Encode the compiled lowering profile as canonical JSON bytes.
pub fn canonical_profile_bytes() -> Vec<u8> {
    to_canonical_json(typedb_3_12_1_profile())
        .expect("the trusted schema-lowering profile has canonical bytes")
}

/// Compute the canonical fingerprint of the compiled lowering profile.
pub fn profile_fingerprint() -> SchemaLoweringProfileFingerprint {
    SchemaLoweringProfileFingerprint::compute(&canonical_profile_bytes())
}

/// Resolve the executable registry's exact canonical profile binding.
pub fn schema_lowering_profile_binding() -> Result<SchemaLoweringProfileBinding, Diagnostic> {
    SchemaLoweringProfileBinding::from_canonical_profile_bytes(&canonical_profile_bytes())
}
