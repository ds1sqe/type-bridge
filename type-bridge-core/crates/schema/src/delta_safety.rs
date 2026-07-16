//! Provider-neutral migration safety classification.

use std::collections::BTreeSet;

use serde::Serialize;
use type_bridge_contract::id::FunctionId;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationKindId, AnnotationSubjectId, SchemaAnnotationValue,
    SchemaDelta, SchemaFact, SchemaFactId, SchemaOperation, SchemaOperationKind,
};

/// The exact eight-class provider-neutral migration safety vocabulary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyClass {
    FormalOnly,
    SchemaMetadata,
    Additive,
    Conditional,
    BackfillRequired,
    Destructive,
    Opaque,
    Unsupported,
}

impl SafetyClass {
    pub const ALL: [Self; 8] = [
        Self::FormalOnly,
        Self::SchemaMetadata,
        Self::Additive,
        Self::Conditional,
        Self::BackfillRequired,
        Self::Destructive,
        Self::Opaque,
        Self::Unsupported,
    ];
}

/// Compatibility name retained for the schema-delta API.
pub type DeltaSafety = SafetyClass;

/// A malformed formal transition that cannot be assigned a safety class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafetyClassificationError {
    /// A relates replacement retained the absence of a specialization.
    UnchangedRelatesSpecialization,
    /// A redefinition changed the fact category under one identity.
    RedefinitionCategoryChanged,
}

impl SafetyClassificationError {
    /// Return the stable diagnostic message used by lowering boundaries.
    pub const fn message(self) -> &'static str {
        match self {
            Self::UnchangedRelatesSpecialization => {
                "relates redefinition does not change specialization"
            }
            Self::RedefinitionCategoryChanged => {
                "schema redefinition changed fact category"
            }
        }
    }
}

/// Classify one validated formal operation without granting execution authority.
pub fn classify_operation_safety(
    operation: &SchemaOperation,
) -> Result<SafetyClass, SafetyClassificationError> {
    let mut safety = SafetyClass::FormalOnly;
    match operation.kind() {
        SchemaOperationKind::Define => {
            let facts = operation.defined_facts().expect("define exposes facts");
            let functions = facts
                .iter()
                .filter_map(|fact| match fact {
                    SchemaFact::Function(function) => Some(function.id().clone()),
                    _ => None,
                })
                .collect::<BTreeSet<FunctionId>>();
            for fact in facts {
                safety = safety.max(classify_defined_fact(fact, &functions));
            }
        }
        SchemaOperationKind::Redefine => {
            safety = classify_redefinition(
                operation.expected_fact().expect("redefine exposes expected fact"),
                operation
                    .replacement_fact()
                    .expect("redefine exposes replacement fact"),
            )?;
        }
        SchemaOperationKind::Undefine => {
            safety = classify_undefined_fact(
                operation.undefined_fact().expect("undefine exposes fact"),
            );
        }
    }
    Ok(safety)
}

fn classify_defined_fact(
    fact: &SchemaFact,
    functions: &BTreeSet<FunctionId>,
) -> SafetyClass {
    match fact {
        SchemaFact::Relates(relates) if relates.specializes().is_some() => {
            SafetyClass::Conditional
        }
        SchemaFact::Annotation(annotation) => {
            if let AnnotationSubjectId::Function(function) = annotation.id().subject()
                && functions.contains(function)
                && matches!(
                    annotation.id().kind(),
                    AnnotationKindId::Doc | AnnotationKindId::Meta(_)
                )
            {
                SafetyClass::SchemaMetadata
            } else {
                classify_annotation(annotation, AnnotationTransition::Add, None)
            }
        }
        _ => classify_fact(fact, FactTransition::Define),
    }
}

fn classify_undefined_fact(fact: &SchemaFact) -> SafetyClass {
    match fact {
        SchemaFact::Annotation(annotation) => {
            classify_annotation(annotation, AnnotationTransition::Remove, None)
        }
        SchemaFact::Relates(relates) if relates.specializes().is_some() => {
            SafetyClass::Conditional
        }
        _ => classify_fact(fact, FactTransition::Undefine),
    }
}

fn classify_redefinition(
    expected: &SchemaFact,
    replacement: &SchemaFact,
) -> Result<SafetyClass, SafetyClassificationError> {
    match (expected, replacement) {
        (SchemaFact::Relates(old), SchemaFact::Relates(new)) => match (
            old.specializes(),
            new.specializes(),
        ) {
            (None, None) => Err(SafetyClassificationError::UnchangedRelatesSpecialization),
            _ => Ok(SafetyClass::Conditional),
        },
        (SchemaFact::Annotation(old), SchemaFact::Annotation(new)) => Ok(
            classify_annotation(new, AnnotationTransition::Change, Some(old)),
        ),
        (left, right) if std::mem::discriminant(left) == std::mem::discriminant(right) => {
            Ok(classify_fact(right, FactTransition::Redefine))
        }
        _ => Err(SafetyClassificationError::RedefinitionCategoryChanged),
    }
}

#[derive(Clone, Copy)]
enum FactTransition {
    Define,
    Undefine,
    Redefine,
}

fn classify_fact(fact: &SchemaFact, transition: FactTransition) -> SafetyClass {
    use FactTransition::{Define, Redefine, Undefine};
    use SafetyClass::{Additive, Conditional, Destructive, Opaque, Unsupported};

    match (fact, transition) {
        (SchemaFact::Type(_), Define) => Additive,
        (SchemaFact::Type(_), Undefine) => Destructive,
        (SchemaFact::Type(_), Redefine) => Unsupported,
        (SchemaFact::Sub(_), Define | Redefine) => Conditional,
        (SchemaFact::Sub(_), Undefine) => Destructive,
        (SchemaFact::Value(_), Define) => Additive,
        (SchemaFact::Value(_), Undefine | Redefine) => Destructive,
        (SchemaFact::Owns(_) | SchemaFact::Relates(_) | SchemaFact::Plays(_), Define) => {
            Additive
        }
        (SchemaFact::Owns(_) | SchemaFact::Relates(_) | SchemaFact::Plays(_), Undefine) => {
            Destructive
        }
        (SchemaFact::Owns(_) | SchemaFact::Relates(_) | SchemaFact::Plays(_), Redefine) => {
            Unsupported
        }
        (SchemaFact::Function(_), Define) => Additive,
        (SchemaFact::Function(_), Undefine) => Destructive,
        (SchemaFact::Function(_), Redefine) => Opaque,
        (SchemaFact::Struct(_), _) => Unsupported,
        (SchemaFact::Annotation(_), _) => {
            unreachable!("annotations use the annotation classifier")
        }
    }
}

#[derive(Clone, Copy)]
enum AnnotationTransition {
    Add,
    Change,
    Remove,
}

fn classify_annotation(
    annotation: &AnnotationFact,
    transition: AnnotationTransition,
    expected: Option<&AnnotationFact>,
) -> SafetyClass {
    let subject = annotation.id().subject();
    let kind = annotation.id().kind();
    if !annotation_supported(subject, kind) {
        return SafetyClass::Unsupported;
    }
    if matches!(subject, AnnotationSubjectId::Sub(_))
        || matches!(kind, AnnotationKindId::Doc | AnnotationKindId::Meta(_))
    {
        return SafetyClass::SchemaMetadata;
    }

    let mut safety = match kind {
        AnnotationKindId::Abstract => match transition {
            AnnotationTransition::Add => SafetyClass::Conditional,
            AnnotationTransition::Remove => SafetyClass::Additive,
            AnnotationTransition::Change => SafetyClass::Unsupported,
        },
        AnnotationKindId::Independent => match transition {
            AnnotationTransition::Add => SafetyClass::Additive,
            AnnotationTransition::Remove => SafetyClass::Destructive,
            AnnotationTransition::Change => SafetyClass::Unsupported,
        },
        AnnotationKindId::Key | AnnotationKindId::Unique => match transition {
            AnnotationTransition::Add => SafetyClass::BackfillRequired,
            AnnotationTransition::Remove => SafetyClass::Additive,
            AnnotationTransition::Change => SafetyClass::Unsupported,
        },
        AnnotationKindId::Card => SafetyClass::Conditional,
        AnnotationKindId::Regex | AnnotationKindId::Range | AnnotationKindId::Values => {
            match transition {
                AnnotationTransition::Add | AnnotationTransition::Change => {
                    SafetyClass::Conditional
                }
                AnnotationTransition::Remove => SafetyClass::Additive,
            }
        }
        AnnotationKindId::Doc | AnnotationKindId::Meta(_) => {
            unreachable!("metadata annotations returned above")
        }
    };

    if matches!(kind, AnnotationKindId::Card)
        && let Some(target) = annotation_cardinality(annotation)
        && let Some(default) = default_cardinality(subject)
    {
        let (from, to) = match transition {
            AnnotationTransition::Add => (default, target),
            AnnotationTransition::Change => {
                let Some(source) = expected.and_then(annotation_cardinality) else {
                    return safety;
                };
                (source, target)
            }
            AnnotationTransition::Remove => (target, default),
        };
        safety = cardinality_transition_safety(from, to);
    }
    safety
}

fn annotation_supported(subject: &AnnotationSubjectId, kind: &AnnotationKindId) -> bool {
    match subject {
        AnnotationSubjectId::Type(_) => matches!(
            kind,
            AnnotationKindId::Abstract
                | AnnotationKindId::Independent
                | AnnotationKindId::Doc
                | AnnotationKindId::Meta(_)
        ),
        AnnotationSubjectId::Sub(_) => {
            matches!(kind, AnnotationKindId::Doc | AnnotationKindId::Meta(_))
        }
        AnnotationSubjectId::Value(_) => matches!(
            kind,
            AnnotationKindId::Regex
                | AnnotationKindId::Range
                | AnnotationKindId::Values
                | AnnotationKindId::Doc
                | AnnotationKindId::Meta(_)
        ),
        AnnotationSubjectId::Owns(_) => matches!(
            kind,
            AnnotationKindId::Key
                | AnnotationKindId::Unique
                | AnnotationKindId::Card
                | AnnotationKindId::Regex
                | AnnotationKindId::Range
                | AnnotationKindId::Values
                | AnnotationKindId::Doc
                | AnnotationKindId::Meta(_)
        ),
        AnnotationSubjectId::Relates(_) => matches!(
            kind,
            AnnotationKindId::Abstract
                | AnnotationKindId::Card
                | AnnotationKindId::Doc
                | AnnotationKindId::Meta(_)
        ),
        AnnotationSubjectId::Plays(_) => matches!(
            kind,
            AnnotationKindId::Card | AnnotationKindId::Doc | AnnotationKindId::Meta(_)
        ),
        AnnotationSubjectId::Function(_) => false,
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
    match subject {
        AnnotationSubjectId::Owns(_) | AnnotationSubjectId::Relates(_) => Some((0, Some(1))),
        AnnotationSubjectId::Plays(_) => Some((0, None)),
        _ => None,
    }
}

fn cardinality_transition_safety(
    from: (u64, Option<u64>),
    to: (u64, Option<u64>),
) -> SafetyClass {
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

/// One deterministic fact-level reason for the aggregate classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaSafetyReason {
    operation_index: usize,
    fact_id: SchemaFactId,
    classification: DeltaSafety,
}

impl DeltaSafetyReason {
    /// Return the operation position in the canonical vector.
    #[must_use]
    pub const fn operation_index(&self) -> usize {
        self.operation_index
    }

    /// Return the affected fact identity.
    #[must_use]
    pub const fn fact_id(&self) -> &SchemaFactId {
        &self.fact_id
    }

    /// Return this fact's conservative safety classification.
    #[must_use]
    pub const fn classification(&self) -> DeltaSafety {
        self.classification
    }
}

/// Advisory classification only; it never grants authorization to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaSafetyReport {
    classification: DeltaSafety,
    reasons: Vec<DeltaSafetyReason>,
}

impl DeltaSafetyReport {
    /// Return the strongest condition in the delta.
    #[must_use]
    pub const fn classification(&self) -> DeltaSafety {
        self.classification
    }

    /// Return deterministic fact-level reasons in operation order.
    #[must_use]
    pub fn reasons(&self) -> &[DeltaSafetyReason] {
        &self.reasons
    }
}

/// Classify one operation and fail malformed transitions closed as unsupported.
#[must_use]
pub fn classify_schema_operation_safety(operation: &SchemaOperation) -> DeltaSafety {
    classify_operation_safety(operation).unwrap_or(DeltaSafety::Unsupported)
}

/// Classify a delta without consulting or producing an authorization decision.
#[must_use]
pub fn classify_delta_safety(delta: &SchemaDelta) -> DeltaSafetyReport {
    let mut reasons = Vec::new();
    for (operation_index, operation) in delta.operations().iter().enumerate() {
        let classification = classify_schema_operation_safety(operation);
        for fact_id in operation.affected_ids() {
            reasons.push(DeltaSafetyReason {
                operation_index,
                fact_id,
                classification,
            });
        }
    }
    let classification = reasons
        .iter()
        .map(DeltaSafetyReason::classification)
        .max()
        .unwrap_or(DeltaSafety::FormalOnly);
    DeltaSafetyReport {
        classification,
        reasons,
    }
}
