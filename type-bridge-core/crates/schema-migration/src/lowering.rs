//! Deterministic offline lowering of validated schema deltas to TypeDB 3.12.1 TypeQL.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::Serialize;
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::id::{FunctionId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationKindId, AnnotationSubjectId, FunctionFact, FunctionReturnElement,
    FunctionReturnMode, RelatesFact, SchemaAnnotationValue, SchemaDelta, SchemaFact,
    SchemaFactId, SchemaOperation, SchemaOperationKind, TypeReference,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

use type_bridge_schema::SafetyClass;

use crate::profile::{classify_operation_transition, typedb_3_12_1_profile};
use crate::{
    SchemaLoweringProfileFingerprint, SchemaLoweringProfileId, profile_fingerprint,
};

const CODE_PROFILE_MISMATCH: &str = "schema_lowering_profile_mismatch";
const CODE_CAPABILITY_MISMATCH: &str = "schema_lowering_capability_mismatch";
const CODE_CONTEXT_MISMATCH: &str = "schema_lowering_fact_context_mismatch";
const CODE_REQUIRES_ASSERTION: &str = "schema_lowering_requires_assertion";
const CODE_REQUIRES_BACKFILL: &str = "schema_lowering_requires_backfill";
const CODE_DESTRUCTIVE: &str = "schema_lowering_destructive";
const CODE_OPAQUE: &str = "schema_lowering_opaque";
const CODE_UNSUPPORTED: &str = "schema_lowering_unsupported";
const CODE_INVALID_TRANSITION: &str = "schema_lowering_invalid_transition";
const CODE_RENDER_CONTEXT: &str = "schema_lowering_render_context_missing";

/// Stable, provider-neutral failure returned before any provider I/O exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SchemaLoweringDiagnostic {
    code: &'static str,
    message: &'static str,
    operation_index: Option<usize>,
    safety: Option<SafetyClass>,
    missing_capabilities: Vec<CapabilityId>,
}

impl SchemaLoweringDiagnostic {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            operation_index: None,
            safety: None,
            missing_capabilities: Vec::new(),
        }
    }

    fn at_operation(mut self, operation_index: usize) -> Self {
        self.operation_index = Some(operation_index);
        self
    }

    fn with_safety(mut self, safety: SafetyClass) -> Self {
        self.safety = Some(safety);
        self
    }

    fn with_missing_capabilities(mut self, missing: Vec<CapabilityId>) -> Self {
        self.missing_capabilities = missing;
        self
    }

    /// Return the stable machine-readable diagnostic code.
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Return the stable human-readable summary.
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// Return the outer delta-operation index, when applicable.
    pub const fn operation_index(&self) -> Option<usize> {
        self.operation_index
    }

    /// Return the safety class which stopped lowering, when applicable.
    pub const fn safety(&self) -> Option<SafetyClass> {
        self.safety
    }

    /// Return missing provider capabilities in deterministic order.
    pub fn missing_capabilities(&self) -> &[CapabilityId] {
        &self.missing_capabilities
    }
}

impl fmt::Display for SchemaLoweringDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for SchemaLoweringDiagnostic {}

/// Exact fact payloads associated with one managed schema state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaFactCatalog(BTreeMap<SchemaFactId, SchemaFact>);

impl SchemaFactCatalog {
    /// Build a deterministic catalog, rejecting duplicate identities.
    pub fn new(
        facts: impl IntoIterator<Item = SchemaFact>,
    ) -> Result<Self, SchemaLoweringDiagnostic> {
        let mut catalog = BTreeMap::new();
        for fact in facts {
            let id = fact.id();
            if catalog.insert(id, fact).is_some() {
                return Err(SchemaLoweringDiagnostic::new(
                    CODE_CONTEXT_MISMATCH,
                    "schema fact catalog contains a duplicate identity",
                ));
            }
        }
        Ok(Self(catalog))
    }

    /// Return an empty catalog.
    pub fn empty() -> Self {
        Self(BTreeMap::new())
    }

    /// Return one exact fact payload.
    pub fn get(&self, id: &SchemaFactId) -> Option<&SchemaFact> {
        self.0.get(id)
    }

    /// Iterate in canonical fact-identity order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&SchemaFactId, &SchemaFact)> {
        self.0.iter()
    }

    fn matches_selection(
        &self,
        selection: &type_bridge_contract::schema::ManagedFactSelection,
    ) -> bool {
        self.0.len() == selection.len()
            && self.0.keys().zip(selection.iter()).all(|(left, right)| left == right)
    }
}

/// Fixed lowering-profile identity plus provider capabilities available to the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaLoweringBinding {
    profile_id: SchemaLoweringProfileId,
    profile_fingerprint: SchemaLoweringProfileFingerprint,
    available_capabilities: CapabilitySet,
}

impl SchemaLoweringBinding {
    /// Bind caller capabilities to the exact compiled lowering profile.
    pub fn new(
        profile_id: SchemaLoweringProfileId,
        profile_fingerprint: SchemaLoweringProfileFingerprint,
        available_capabilities: CapabilitySet,
    ) -> Result<Self, SchemaLoweringDiagnostic> {
        let profile = typedb_3_12_1_profile();
        if profile_id != profile.id || profile_fingerprint != crate::profile_fingerprint() {
            return Err(SchemaLoweringDiagnostic::new(
                CODE_PROFILE_MISMATCH,
                "schema lowering profile identity or fingerprint does not match the compiled registry",
            ));
        }
        Ok(Self {
            profile_id,
            profile_fingerprint,
            available_capabilities,
        })
    }

    /// Bind capabilities to the current compiled profile.
    pub fn current(
        available_capabilities: CapabilitySet,
    ) -> Result<Self, SchemaLoweringDiagnostic> {
        Self::new(
            typedb_3_12_1_profile().id.clone(),
            profile_fingerprint(),
            available_capabilities,
        )
    }

    /// Return available provider capabilities.
    pub const fn available_capabilities(&self) -> &CapabilitySet {
        &self.available_capabilities
    }

    /// Return the exact compiled lowering-profile identity.
    pub const fn profile_id(&self) -> &SchemaLoweringProfileId {
        &self.profile_id
    }

    /// Return the exact compiled lowering-profile content fingerprint.
    pub const fn profile_fingerprint(&self) -> &SchemaLoweringProfileFingerprint {
        &self.profile_fingerprint
    }
}

/// TypeQL schema query verb.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeQlVerb {
    Define,
    Undefine,
    Redefine,
}

impl TypeQlVerb {
    fn as_str(self) -> &'static str {
        match self {
            Self::Define => "define",
            Self::Undefine => "undefine",
            Self::Redefine => "redefine",
        }
    }
}

/// One complete TypeQL schema query.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TypeQlStatement {
    verb: TypeQlVerb,
    query: String,
}

impl TypeQlStatement {
    fn new(verb: TypeQlVerb, body: String) -> Self {
        Self {
            verb,
            query: format!("{}\n{body}", verb.as_str()),
        }
    }

    /// Return the schema query verb.
    pub const fn verb(&self) -> TypeQlVerb {
        self.verb
    }

    /// Return exact deterministic query text.
    pub fn query(&self) -> &str {
        &self.query
    }
}

/// Outer formal operation represented by a statement unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementOperationKind {
    Define,
    Redefine,
    Undefine,
}

impl From<SchemaOperationKind> for StatementOperationKind {
    fn from(value: SchemaOperationKind) -> Self {
        match value {
            SchemaOperationKind::Define => Self::Define,
            SchemaOperationKind::Redefine => Self::Redefine,
            SchemaOperationKind::Undefine => Self::Undefine,
        }
    }
}

/// One preserved outer delta operation and its atomic TypeQL query sequence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StatementUnit {
    operation_index: usize,
    operation_kind: StatementOperationKind,
    safety: SafetyClass,
    atomic: bool,
    affected_ids: Vec<SchemaFactId>,
    required_capabilities: CapabilitySet,
    statements: Vec<TypeQlStatement>,
}

impl StatementUnit {
    pub const fn operation_index(&self) -> usize {
        self.operation_index
    }

    pub const fn operation_kind(&self) -> StatementOperationKind {
        self.operation_kind
    }

    pub const fn safety(&self) -> SafetyClass {
        self.safety
    }

    pub const fn atomic(&self) -> bool {
        self.atomic
    }

    pub fn affected_ids(&self) -> &[SchemaFactId] {
        &self.affected_ids
    }

    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    pub fn statements(&self) -> &[TypeQlStatement] {
        &self.statements
    }
}

/// Provider-bound, safety-gated statement plan retaining its exact formal delta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaLoweringPlan {
    delta: SchemaDelta,
    profile_id: SchemaLoweringProfileId,
    profile_fingerprint: SchemaLoweringProfileFingerprint,
    units: Vec<StatementUnit>,
}

impl SchemaLoweringPlan {
    pub const fn delta(&self) -> &SchemaDelta {
        &self.delta
    }

    pub const fn profile_id(&self) -> &SchemaLoweringProfileId {
        &self.profile_id
    }

    pub const fn profile_fingerprint(&self) -> &SchemaLoweringProfileFingerprint {
        &self.profile_fingerprint
    }

    pub fn units(&self) -> &[StatementUnit] {
        &self.units
    }
}

/// Lower a complete formal delta after validating its exact fact payload context.
pub fn lower_schema_delta(
    delta: &SchemaDelta,
    source_facts: &SchemaFactCatalog,
    target_facts: &SchemaFactCatalog,
    binding: &SchemaLoweringBinding,
) -> Result<SchemaLoweringPlan, SchemaLoweringDiagnostic> {
    lower_schema_delta_with_verified_assertions(
        delta,
        source_facts,
        target_facts,
        binding,
        &[],
        false,
    )
}

pub(crate) fn lower_schema_delta_with_verified_assertions(
    delta: &SchemaDelta,
    source_facts: &SchemaFactCatalog,
    target_facts: &SchemaFactCatalog,
    binding: &SchemaLoweringBinding,
    conditional_operation_indices: &[usize],
    destructive_approved: bool,
) -> Result<SchemaLoweringPlan, SchemaLoweringDiagnostic> {
    if !source_facts.matches_selection(delta.source().selection())
        || !target_facts.matches_selection(delta.target().selection())
    {
        return Err(SchemaLoweringDiagnostic::new(
            CODE_CONTEXT_MISMATCH,
            "source or target fact catalog does not match the delta managed selection",
        ));
    }
    if conditional_operation_indices
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
        || conditional_operation_indices
            .last()
            .is_some_and(|index| *index >= delta.operations().len())
    {
        return Err(SchemaLoweringDiagnostic::new(
            CODE_CONTEXT_MISMATCH,
            "verified conditional operation indices are not canonical for this delta",
        ));
    }
    let units = delta
        .operations()
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            lower_operation(
                index,
                operation,
                source_facts,
                target_facts,
                binding,
                conditional_operation_indices.binary_search(&index).is_ok(),
                destructive_approved,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SchemaLoweringPlan {
        delta: delta.clone(),
        profile_id: binding.profile_id.clone(),
        profile_fingerprint: binding.profile_fingerprint.clone(),
        units,
    })
}

fn lower_operation(
    operation_index: usize,
    operation: &SchemaOperation,
    source_facts: &SchemaFactCatalog,
    target_facts: &SchemaFactCatalog,
    binding: &SchemaLoweringBinding,
    conditional_resolved: bool,
    destructive_approved: bool,
) -> Result<StatementUnit, SchemaLoweringDiagnostic> {
    let classification = classify_operation_transition(operation).map_err(|error| {
        SchemaLoweringDiagnostic::new(CODE_INVALID_TRANSITION, error.message())
            .at_operation(operation_index)
    })?;
    let missing = classification
        .required_capabilities
        .iter()
        .filter(|capability| !binding.available_capabilities.contains(capability))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(SchemaLoweringDiagnostic::new(
            CODE_CAPABILITY_MISMATCH,
            "provider capabilities do not satisfy the schema lowering unit",
        )
        .at_operation(operation_index)
        .with_missing_capabilities(missing));
    }
    gate_safety(
        operation_index,
        classification.safety,
        conditional_resolved,
        destructive_approved,
    )?;
    let statements = render_operation(operation_index, operation, source_facts, target_facts)?;
    Ok(StatementUnit {
        operation_index,
        operation_kind: operation.kind().into(),
        safety: classification.safety,
        atomic: classification.atomic,
        affected_ids: operation.affected_ids(),
        required_capabilities: classification.required_capabilities,
        statements,
    })
}

fn gate_safety(
    operation_index: usize,
    safety: SafetyClass,
    conditional_resolved: bool,
    destructive_approved: bool,
) -> Result<(), SchemaLoweringDiagnostic> {
    if conditional_resolved && safety != SafetyClass::Conditional {
        return Err(SchemaLoweringDiagnostic::new(
            CODE_INVALID_TRANSITION,
            "assertion coverage targets a non-conditional schema operation",
        )
        .at_operation(operation_index)
        .with_safety(safety));
    }
    let (code, message) = match safety {
        SafetyClass::FormalOnly | SafetyClass::SchemaMetadata | SafetyClass::Additive => {
            return Ok(());
        }
        SafetyClass::Conditional if conditional_resolved => return Ok(()),
        SafetyClass::Conditional => (
            CODE_REQUIRES_ASSERTION,
            "schema transition requires an explicit data assertion",
        ),
        SafetyClass::BackfillRequired => (
            CODE_REQUIRES_BACKFILL,
            "schema transition requires an explicit backfill plan",
        ),
        SafetyClass::Destructive if destructive_approved => return Ok(()),
        SafetyClass::Destructive => (
            CODE_DESTRUCTIVE,
            "destructive schema transition requires an identity-bound approval",
        ),
        SafetyClass::Opaque => (
            CODE_OPAQUE,
            "opaque schema transition requires explicit operator intent",
        ),
        SafetyClass::Unsupported => (
            CODE_UNSUPPORTED,
            "schema transition is unsupported by the TypeDB 3.12.1 lowering profile",
        ),
    };
    Err(SchemaLoweringDiagnostic::new(code, message)
        .at_operation(operation_index)
        .with_safety(safety))
}

#[derive(Debug)]
struct RenderFailure {
    code: &'static str,
    message: &'static str,
}

fn render_operation(
    operation_index: usize,
    operation: &SchemaOperation,
    source_facts: &SchemaFactCatalog,
    target_facts: &SchemaFactCatalog,
) -> Result<Vec<TypeQlStatement>, SchemaLoweringDiagnostic> {
    render_operation_inner(operation, source_facts, target_facts).map_err(|failure| {
        SchemaLoweringDiagnostic::new(failure.code, failure.message)
            .at_operation(operation_index)
    })
}

fn render_operation_inner(
    operation: &SchemaOperation,
    source_facts: &SchemaFactCatalog,
    target_facts: &SchemaFactCatalog,
) -> Result<Vec<TypeQlStatement>, RenderFailure> {
    match operation.kind() {
        SchemaOperationKind::Define => {
            let facts = operation.defined_facts().expect("define exposes facts");
            let function_ids = facts
                .iter()
                .filter_map(|fact| match fact {
                    SchemaFact::Function(function) => Some(function.id().clone()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            let mut function_annotations = BTreeMap::<FunctionId, Vec<&AnnotationFact>>::new();
            for fact in facts {
                if let SchemaFact::Annotation(annotation) = fact
                    && let AnnotationSubjectId::Function(function) = annotation.id().subject()
                    && function_ids.contains(function)
                {
                    function_annotations
                        .entry(function.clone())
                        .or_default()
                        .push(annotation);
                }
            }
            let mut bodies = Vec::new();
            for fact in facts {
                match fact {
                    SchemaFact::Annotation(annotation)
                        if matches!(annotation.id().subject(), AnnotationSubjectId::Function(id) if function_ids.contains(id)) => {}
                    SchemaFact::Function(function) => bodies.push(render_function(
                        function,
                        function_annotations
                            .get(function.id())
                            .map(Vec::as_slice)
                            .unwrap_or_default(),
                    )?),
                    _ => bodies.push(render_definition(fact, target_facts, true)?),
                }
            }
            Ok(vec![TypeQlStatement::new(TypeQlVerb::Define, bodies.join("\n"))])
        }
        SchemaOperationKind::Undefine => Ok(vec![TypeQlStatement::new(
            TypeQlVerb::Undefine,
            render_undefinition(
                operation.undefined_fact().expect("undefine exposes fact"),
                source_facts,
            )?,
        )]),
        SchemaOperationKind::Redefine => {
            let expected = operation.expected_fact().expect("redefine exposes expected");
            let replacement = operation
                .replacement_fact()
                .expect("redefine exposes replacement");
            if let (SchemaFact::Annotation(old), SchemaFact::Annotation(new)) =
                (expected, replacement)
                && matches!(old.id().subject(), AnnotationSubjectId::Sub(_))
                && matches!(old.id().kind(), AnnotationKindId::Doc | AnnotationKindId::Meta(_))
            {
                return Ok(vec![
                    TypeQlStatement::new(
                        TypeQlVerb::Undefine,
                        render_annotation_undefinition(old, source_facts)?,
                    ),
                    TypeQlStatement::new(
                        TypeQlVerb::Define,
                        render_annotation_definition(new, target_facts, false)?,
                    ),
                ]);
            }
            if let (SchemaFact::Relates(old), SchemaFact::Relates(new)) = (expected, replacement) {
                return match (old.specializes(), new.specializes()) {
                    (None, Some(_)) => Ok(vec![TypeQlStatement::new(
                        TypeQlVerb::Define,
                        render_relates(new),
                    )]),
                    (Some(old_parent), None) => Ok(vec![TypeQlStatement::new(
                        TypeQlVerb::Undefine,
                        format!(
                            "as {} from {} relates {};",
                            old_parent.label().as_str(),
                            old.id().relation().label().as_str(),
                            old.id().role().label().as_str()
                        ),
                    )]),
                    (Some(_), Some(_)) => Ok(vec![TypeQlStatement::new(
                        TypeQlVerb::Redefine,
                        render_relates(new),
                    )]),
                    (None, None) => Err(RenderFailure {
                        code: CODE_INVALID_TRANSITION,
                        message: "relates redefinition does not change specialization",
                    }),
                };
            }
            Ok(vec![TypeQlStatement::new(
                TypeQlVerb::Redefine,
                render_definition(replacement, target_facts, false)?,
            )])
        }
    }
}

fn render_definition(
    fact: &SchemaFact,
    catalog: &SchemaFactCatalog,
    defining: bool,
) -> Result<String, RenderFailure> {
    match fact {
        SchemaFact::Type(fact) => Ok(format!(
            "{} {};",
            type_kind(fact.id().kind()),
            fact.id().label().as_str()
        )),
        SchemaFact::Sub(fact) => Ok(format!(
            "{} sub {};",
            fact.id().subtype().label().as_str(),
            fact.id().supertype().label().as_str()
        )),
        SchemaFact::Value(fact) => Ok(format!(
            "{} value {};",
            fact.id().attribute().label().as_str(),
            value_type(fact.value_type())
        )),
        SchemaFact::Owns(fact) => Ok(format!(
            "{} owns {};",
            fact.id().owner().label().as_str(),
            fact.id().attribute().label().as_str()
        )),
        SchemaFact::Relates(fact) => Ok(render_relates(fact)),
        SchemaFact::Plays(fact) => Ok(format!(
            "{} plays {}:{};",
            fact.id().player().label().as_str(),
            fact.id().role().declaring_relation().as_str(),
            fact.id().role().label().as_str()
        )),
        SchemaFact::Annotation(fact) => render_annotation_definition(fact, catalog, defining),
        SchemaFact::Function(fact) => render_function(fact, &[]),
        SchemaFact::Struct(_) => Err(RenderFailure {
            code: CODE_UNSUPPORTED,
            message: "TypeDB 3.12.1 does not admit the pinned struct transition grammar",
        }),
    }
}

fn render_undefinition(
    fact: &SchemaFact,
    catalog: &SchemaFactCatalog,
) -> Result<String, RenderFailure> {
    match fact {
        SchemaFact::Type(fact) => Ok(format!(
            "{} {};",
            type_kind(fact.id().kind()),
            fact.id().label().as_str()
        )),
        SchemaFact::Sub(fact) => Ok(format!(
            "sub {} from {};",
            fact.id().supertype().label().as_str(),
            fact.id().subtype().label().as_str()
        )),
        SchemaFact::Value(fact) => Ok(format!(
            "value {} from {};",
            value_type(fact.value_type()),
            fact.id().attribute().label().as_str()
        )),
        SchemaFact::Owns(fact) => Ok(format!(
            "owns {} from {};",
            fact.id().attribute().label().as_str(),
            fact.id().owner().label().as_str()
        )),
        SchemaFact::Relates(fact) => Ok(format!(
            "relates {} from {};",
            fact.id().role().label().as_str(),
            fact.id().relation().label().as_str()
        )),
        SchemaFact::Plays(fact) => Ok(format!(
            "plays {}:{} from {};",
            fact.id().role().declaring_relation().as_str(),
            fact.id().role().label().as_str(),
            fact.id().player().label().as_str()
        )),
        SchemaFact::Annotation(fact) => render_annotation_undefinition(fact, catalog),
        SchemaFact::Function(fact) => Ok(format!("fun {};", fact.id().label().as_str())),
        SchemaFact::Struct(_) => Err(RenderFailure {
            code: CODE_UNSUPPORTED,
            message: "TypeDB 3.12.1 does not admit the pinned struct transition grammar",
        }),
    }
}

fn render_relates(fact: &RelatesFact) -> String {
    let specializes = fact
        .specializes()
        .map(|role| format!(" as {}", role.label().as_str()))
        .unwrap_or_default();
    format!(
        "{} relates {}{};",
        fact.id().relation().label().as_str(),
        fact.id().role().label().as_str(),
        specializes
    )
}

fn render_annotation_definition(
    annotation: &AnnotationFact,
    catalog: &SchemaFactCatalog,
    defining: bool,
) -> Result<String, RenderFailure> {
    let subject = render_annotation_subject(annotation.id().subject(), catalog, defining)?;
    Ok(format!("{subject} {};", render_annotation(annotation)))
}

fn render_annotation_undefinition(
    annotation: &AnnotationFact,
    catalog: &SchemaFactCatalog,
) -> Result<String, RenderFailure> {
    let subject = render_annotation_subject(annotation.id().subject(), catalog, false)?;
    Ok(format!(
        "{} from {subject};",
        render_annotation_selector(annotation.id().kind())
    ))
}

fn render_annotation_subject(
    subject: &AnnotationSubjectId,
    catalog: &SchemaFactCatalog,
    defining: bool,
) -> Result<String, RenderFailure> {
    match subject {
        AnnotationSubjectId::Type(id) if defining => Ok(format!(
            "{} {}",
            type_kind(id.kind()),
            id.label().as_str()
        )),
        AnnotationSubjectId::Type(id) => Ok(id.label().as_str().to_owned()),
        AnnotationSubjectId::Sub(id) => Ok(format!(
            "{} sub {}",
            id.subtype().label().as_str(),
            id.supertype().label().as_str()
        )),
        AnnotationSubjectId::Value(id) => {
            let fact_id = SchemaFactId::Value(id.clone());
            let Some(SchemaFact::Value(value)) = catalog.get(&fact_id) else {
                return Err(RenderFailure {
                    code: CODE_RENDER_CONTEXT,
                    message: "value annotation rendering requires its exact value fact payload",
                });
            };
            Ok(format!(
                "{} value {}",
                id.attribute().label().as_str(),
                value_type(value.value_type())
            ))
        }
        AnnotationSubjectId::Owns(id) => Ok(format!(
            "{} owns {}",
            id.owner().label().as_str(),
            id.attribute().label().as_str()
        )),
        AnnotationSubjectId::Relates(id) => Ok(format!(
            "{} relates {}",
            id.relation().label().as_str(),
            id.role().label().as_str()
        )),
        AnnotationSubjectId::Plays(id) => Ok(format!(
            "{} plays {}:{}",
            id.player().label().as_str(),
            id.role().declaring_relation().as_str(),
            id.role().label().as_str()
        )),
        AnnotationSubjectId::Function(_) => Err(RenderFailure {
            code: CODE_UNSUPPORTED,
            message: "persistent function annotations are unsupported; fold metadata into function definition",
        }),
    }
}

fn render_annotation(annotation: &AnnotationFact) -> String {
    match (annotation.id().kind(), annotation.value()) {
        (AnnotationKindId::Abstract, SchemaAnnotationValue::Presence) => "@abstract".into(),
        (AnnotationKindId::Independent, SchemaAnnotationValue::Presence) => "@independent".into(),
        (AnnotationKindId::Key, SchemaAnnotationValue::Presence) => "@key".into(),
        (AnnotationKindId::Unique, SchemaAnnotationValue::Presence) => "@unique".into(),
        (AnnotationKindId::Card, SchemaAnnotationValue::Cardinality(cardinality)) => format!(
            "@card({}..{})",
            (*cardinality).min(),
            (*cardinality)
                .max()
                .map(|value| value.to_string())
                .unwrap_or_default()
        ),
        (AnnotationKindId::Regex, SchemaAnnotationValue::Regex(regex)) => {
            format!("@regex({})", quote(regex.as_str()))
        }
        (AnnotationKindId::Range, SchemaAnnotationValue::Range(range)) => format!(
            "@range({}..{})",
            range.lower().map(render_value).unwrap_or_default(),
            range.upper().map(render_value).unwrap_or_default()
        ),
        (AnnotationKindId::Values, SchemaAnnotationValue::Values(values)) => format!(
            "@values({})",
            values.iter().map(render_value).collect::<Vec<_>>().join(", ")
        ),
        (AnnotationKindId::Doc, SchemaAnnotationValue::Doc(doc)) => {
            format!("@doc({})", quote(doc.as_str()))
        }
        (AnnotationKindId::Meta(key), SchemaAnnotationValue::Meta(value)) => format!(
            "@meta({}, {})",
            quote(key.as_str()),
            render_value(value)
        ),
        _ => unreachable!("annotation constructors preserve kind-safe payloads"),
    }
}

fn render_annotation_selector(kind: &AnnotationKindId) -> String {
    match kind {
        AnnotationKindId::Abstract => "@abstract".into(),
        AnnotationKindId::Independent => "@independent".into(),
        AnnotationKindId::Key => "@key".into(),
        AnnotationKindId::Unique => "@unique".into(),
        AnnotationKindId::Card => "@card".into(),
        AnnotationKindId::Regex => "@regex".into(),
        AnnotationKindId::Range => "@range".into(),
        AnnotationKindId::Values => "@values".into(),
        AnnotationKindId::Doc => "@doc".into(),
        AnnotationKindId::Meta(key) => format!("@meta({})", quote(key.as_str())),
    }
}

fn render_function(
    function: &FunctionFact,
    annotations: &[&AnnotationFact],
) -> Result<String, RenderFailure> {
    let parameters = function
        .signature()
        .parameters()
        .iter()
        .map(|parameter| {
            format!(
                "${}: {}",
                parameter.name().as_str(),
                render_type_reference(parameter.type_ref())
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let returns = match function.signature().returns() {
        FunctionReturnMode::Scalar(element) => render_return_element(element),
        FunctionReturnMode::Tuple(elements) => format!(
            "({})",
            elements
                .iter()
                .map(render_return_element)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        FunctionReturnMode::Stream(elements) => format!(
            "{{ {} }}",
            elements
                .iter()
                .map(render_return_element)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let mut rendered_annotations = annotations
        .iter()
        .map(|annotation| render_annotation(annotation))
        .collect::<Vec<_>>();
    rendered_annotations.sort();
    let suffix = if rendered_annotations.is_empty() {
        String::new()
    } else {
        format!(" {}", rendered_annotations.join(" "))
    };
    Ok(format!(
        "fun {}({parameters}) -> {returns}{suffix}:\n{}",
        function.id().label().as_str(),
        function.body().text()
    ))
}

fn render_return_element(element: &FunctionReturnElement) -> String {
    format!(
        "{}{}",
        render_type_reference(element.type_ref()),
        if element.optional() { "?" } else { "" }
    )
}

fn render_type_reference(reference: &TypeReference) -> String {
    match reference {
        TypeReference::Value(value) => value_type(*value).into(),
        TypeReference::Schema(label) => label.as_str().into(),
    }
}

fn render_value(value: &CanonicalValue) -> String {
    match value {
        CanonicalValue::String(value) => quote(value.as_str()),
        CanonicalValue::Long(value) => value.to_string(),
        CanonicalValue::Double(value) => format!("{:?}", value.get()),
        CanonicalValue::Boolean(value) => value.to_string(),
        CanonicalValue::Date(value) => value.to_string(),
        CanonicalValue::DateTime(value) => value.to_string(),
        CanonicalValue::DateTimeTz(value) => value.to_string(),
        CanonicalValue::Decimal(value) => format!("{}dec", value.as_str()),
        CanonicalValue::Duration(value) => value.to_string(),
    }
}

fn quote(value: &str) -> String {
    serde_json::to_string(value).expect("strings always serialize")
}

fn type_kind(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Entity => "entity",
        TypeKind::Relation => "relation",
        TypeKind::Attribute => "attribute",
        TypeKind::Struct => "struct",
    }
}

fn value_type(value: ValueTypeTag) -> &'static str {
    match value.as_str() {
        "long" => "integer",
        "datetime_tz" => "datetime-tz",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::id::{AttributeId, Label, RoleId, StructId, TypeId};
    use type_bridge_contract::schema::{
        AnnotationFactId, CanonicalValueRange, CanonicalValueSet, DocText, FunctionBody,
        FunctionParameter, FunctionSignature, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId,
        RegexPattern, RelatesFactId, SchemaOperation, StructFact, StructField, SubFact,
        SubFactId, TypeFact, ValueFact, ValueFactId,
    };
    use type_bridge_contract::value::{CanonicalString, Cardinality};

    fn type_id(kind: TypeKind, label: &str) -> TypeId {
        TypeId::new(kind, label).unwrap()
    }

    fn attribute_id(label: &str) -> AttributeId {
        AttributeId::new(label).unwrap()
    }

    fn role_id(relation: &str, role: &str) -> RoleId {
        RoleId::new(relation, role).unwrap()
    }

    fn value_fact(label: &str, value_type: ValueTypeTag) -> SchemaFact {
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(attribute_id(label)),
            value_type,
        ))
    }

    fn annotation(
        subject: AnnotationSubjectId,
        kind: AnnotationKindId,
        value: SchemaAnnotationValue,
    ) -> SchemaFact {
        SchemaFact::Annotation(
            AnnotationFact::new(AnnotationFactId::new(subject, kind), value).unwrap(),
        )
    }

    fn function(body: &str) -> SchemaFact {
        SchemaFact::Function(FunctionFact::new(
            FunctionId::new("answer").unwrap(),
            FunctionSignature::new(
                vec![FunctionParameter::new(
                    Label::new("seed").unwrap(),
                    TypeReference::Value(ValueTypeTag::Long),
                )],
                FunctionReturnMode::scalar(FunctionReturnElement::new(
                    TypeReference::Value(ValueTypeTag::Long),
                    false,
                )),
            )
            .unwrap(),
            FunctionBody::new(body).unwrap(),
        ))
    }

    fn full_binding() -> SchemaLoweringBinding {
        SchemaLoweringBinding::current(
            typedb_3_12_1_profile().required_capabilities.clone(),
        )
        .unwrap()
    }

    fn dump(
        name: &str,
        operation: SchemaOperation,
        source: &SchemaFactCatalog,
        target: &SchemaFactCatalog,
        output: &mut String,
    ) {
        output.push_str("## ");
        output.push_str(name);
        output.push('\n');
        for (index, statement) in render_operation_inner(&operation, source, target)
            .unwrap()
            .iter()
            .enumerate()
        {
            if index != 0 {
                output.push_str("-- atomic-next --\n");
            }
            output.push_str(statement.query());
            output.push('\n');
        }
    }

    #[test]
    fn supported_renderer_matches_exhaustive_golden() {
        let empty = SchemaFactCatalog::empty();
        let person = type_id(TypeKind::Entity, "person");
        let employee = type_id(TypeKind::Entity, "employee");
        let name = attribute_id("name");
        let owns_id = OwnsFactId::new(person.clone(), name.clone()).unwrap();
        let relation = type_id(TypeKind::Relation, "friendship");
        let role = role_id("friendship", "friend");
        let relates_id = RelatesFactId::new(relation.clone(), role.clone()).unwrap();
        let child_relation = type_id(TypeKind::Relation, "child-relation");
        let child_role = role_id("child-relation", "child-role");
        let child_relates = RelatesFactId::new(child_relation.clone(), child_role.clone()).unwrap();
        let parent_a = role_id("parent-relation", "parent-role-a");
        let parent_b = role_id("parent-relation", "parent-role-b");
        let plays_id = PlaysFactId::new(person.clone(), role.clone()).unwrap();
        let sub_id = SubFactId::new(employee.clone(), person.clone()).unwrap();
        let string_value = value_fact("name", ValueTypeTag::String);
        let integer_value = value_fact("name", ValueTypeTag::Long);
        let string_catalog = SchemaFactCatalog::new([string_value.clone()]).unwrap();
        let integer_catalog = SchemaFactCatalog::new([integer_value.clone()]).unwrap();
        let mut output = String::new();

        let type_fact = SchemaFact::Type(TypeFact::new(person.clone()).unwrap());
        dump("type-define", SchemaOperation::define(vec![type_fact.clone()]).unwrap(), &empty, &empty, &mut output);
        dump("type-undefine", SchemaOperation::undefine(type_fact), &empty, &empty, &mut output);
        let sub_fact = SchemaFact::Sub(SubFact::new(sub_id.clone()));
        dump("sub-define", SchemaOperation::define(vec![sub_fact.clone()]).unwrap(), &empty, &empty, &mut output);
        dump("sub-undefine", SchemaOperation::undefine(sub_fact), &empty, &empty, &mut output);
        dump("value-define", SchemaOperation::define(vec![string_value.clone()]).unwrap(), &empty, &string_catalog, &mut output);
        dump("value-redefine", SchemaOperation::redefine(string_value.clone(), integer_value.clone()).unwrap(), &string_catalog, &integer_catalog, &mut output);
        dump("value-undefine", SchemaOperation::undefine(string_value.clone()), &string_catalog, &empty, &mut output);
        let owns = SchemaFact::Owns(OwnsFact::new(owns_id.clone()));
        dump("owns-define", SchemaOperation::define(vec![owns.clone()]).unwrap(), &empty, &empty, &mut output);
        dump("owns-undefine", SchemaOperation::undefine(owns), &empty, &empty, &mut output);
        let relates = SchemaFact::Relates(RelatesFact::new(relates_id, None).unwrap());
        dump("relates-define", SchemaOperation::define(vec![relates.clone()]).unwrap(), &empty, &empty, &mut output);
        dump("relates-undefine", SchemaOperation::undefine(relates), &empty, &empty, &mut output);
        let specialization_a = SchemaFact::Relates(RelatesFact::new(child_relates.clone(), Some(parent_a.clone())).unwrap());
        let specialization_b = SchemaFact::Relates(RelatesFact::new(child_relates.clone(), Some(parent_b.clone())).unwrap());
        let unspecialized = SchemaFact::Relates(RelatesFact::new(child_relates, None).unwrap());
        dump("specialization-define", SchemaOperation::redefine(unspecialized.clone(), specialization_a.clone()).unwrap(), &empty, &empty, &mut output);
        dump("specialization-redefine", SchemaOperation::redefine(specialization_a.clone(), specialization_b.clone()).unwrap(), &empty, &empty, &mut output);
        dump("specialization-undefine", SchemaOperation::redefine(specialization_b, unspecialized).unwrap(), &empty, &empty, &mut output);
        let plays = SchemaFact::Plays(PlaysFact::new(plays_id));
        dump("plays-define", SchemaOperation::define(vec![plays.clone()]).unwrap(), &empty, &empty, &mut output);
        dump("plays-undefine", SchemaOperation::undefine(plays), &empty, &empty, &mut output);

        let doc_old = annotation(
            AnnotationSubjectId::Type(person.clone()),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("line\n\"quoted\"").unwrap()),
        );
        let doc_new = annotation(
            AnnotationSubjectId::Type(person.clone()),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("changed").unwrap()),
        );
        dump("doc-define", SchemaOperation::define(vec![doc_old.clone()]).unwrap(), &empty, &empty, &mut output);
        dump("doc-redefine", SchemaOperation::redefine(doc_old.clone(), doc_new).unwrap(), &empty, &empty, &mut output);
        dump("doc-undefine", SchemaOperation::undefine(doc_old), &empty, &empty, &mut output);
        let regex_old = annotation(
            AnnotationSubjectId::Value(ValueFactId::new(name.clone())),
            AnnotationKindId::Regex,
            SchemaAnnotationValue::Regex(RegexPattern::new("^a+\\\\d$").unwrap()),
        );
        let regex_new = annotation(
            AnnotationSubjectId::Value(ValueFactId::new(name.clone())),
            AnnotationKindId::Regex,
            SchemaAnnotationValue::Regex(RegexPattern::new("^b+$").unwrap()),
        );
        dump("regex-redefine", SchemaOperation::redefine(regex_old, regex_new).unwrap(), &string_catalog, &string_catalog, &mut output);
        let card_old = annotation(
            AnnotationSubjectId::Owns(owns_id.clone()),
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(Cardinality::new(0, Some(1)).unwrap()),
        );
        let card_new = annotation(
            AnnotationSubjectId::Owns(owns_id),
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(Cardinality::new(0, Some(2)).unwrap()),
        );
        dump("card-redefine", SchemaOperation::redefine(card_old, card_new).unwrap(), &empty, &empty, &mut output);
        let range = annotation(
            AnnotationSubjectId::Value(ValueFactId::new(name.clone())),
            AnnotationKindId::Range,
            SchemaAnnotationValue::Range(CanonicalValueRange::new(Some(CanonicalValue::Long(1)), Some(CanonicalValue::Long(10))).unwrap()),
        );
        dump("range-define", SchemaOperation::define(vec![range]).unwrap(), &empty, &string_catalog, &mut output);
        let values = annotation(
            AnnotationSubjectId::Value(ValueFactId::new(name)),
            AnnotationKindId::Values,
            SchemaAnnotationValue::Values(CanonicalValueSet::new([CanonicalValue::Long(2), CanonicalValue::Long(1)]).unwrap()),
        );
        dump("values-define", SchemaOperation::define(vec![values]).unwrap(), &empty, &integer_catalog, &mut output);
        let meta_old = annotation(
            AnnotationSubjectId::Sub(sub_id.clone()),
            AnnotationKindId::meta("owner").unwrap(),
            SchemaAnnotationValue::Meta(CanonicalValue::String(CanonicalString::new("old").unwrap())),
        );
        let meta_new = annotation(
            AnnotationSubjectId::Sub(sub_id),
            AnnotationKindId::meta("owner").unwrap(),
            SchemaAnnotationValue::Meta(CanonicalValue::String(CanonicalString::new("new").unwrap())),
        );
        dump("sub-meta-fallback", SchemaOperation::redefine(meta_old.clone(), meta_new).unwrap(), &empty, &empty, &mut output);
        dump("meta-keyed-undefine", SchemaOperation::undefine(meta_old), &empty, &empty, &mut output);

        let function_fact = function("match\n  let $value = $seed;\nreturn first $value;");
        let function_doc = annotation(
            AnnotationSubjectId::Function(FunctionId::new("answer").unwrap()),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("answer docs").unwrap()),
        );
        let function_meta = annotation(
            AnnotationSubjectId::Function(FunctionId::new("answer").unwrap()),
            AnnotationKindId::meta("owner").unwrap(),
            SchemaAnnotationValue::Meta(CanonicalValue::String(CanonicalString::new("core").unwrap())),
        );
        dump("function-define-with-metadata", SchemaOperation::define(vec![function_meta, function_fact.clone(), function_doc]).unwrap(), &empty, &empty, &mut output);
        let changed_function = function("match\n  let $value = 2;\nreturn first $value;");
        dump("function-redefine", SchemaOperation::redefine(function_fact.clone(), changed_function).unwrap(), &empty, &empty, &mut output);
        dump("function-undefine", SchemaOperation::undefine(function_fact), &empty, &empty, &mut output);

        assert_eq!(output, include_str!("../tests/fixtures/lowering-supported-v1.txt"));
    }

    #[test]
    fn safety_profile_and_capability_rejections_match_golden() {
        let empty = SchemaFactCatalog::empty();
        let full = full_binding();
        let person = type_id(TypeKind::Entity, "person");
        let employee = type_id(TypeKind::Entity, "employee");
        let sub = SchemaFact::Sub(SubFact::new(SubFactId::new(employee, person.clone()).unwrap()));
        let name = attribute_id("name");
        let owns_id = OwnsFactId::new(person.clone(), name).unwrap();
        let key = annotation(
            AnnotationSubjectId::Owns(owns_id),
            AnnotationKindId::Key,
            SchemaAnnotationValue::Presence,
        );
        let type_fact = SchemaFact::Type(TypeFact::new(person).unwrap());
        let function_old = function("match\n  let $value = 1;\nreturn first $value;");
        let function_new = function("match\n  let $value = 2;\nreturn first $value;");
        let struct_fact = SchemaFact::Struct(
            StructFact::new(
                StructId::new("record").unwrap(),
                vec![StructField::new(Label::new("field").unwrap(), ValueTypeTag::Long, false)],
            )
            .unwrap(),
        );
        let persistent_function_doc = annotation(
            AnnotationSubjectId::Function(FunctionId::new("answer").unwrap()),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("docs").unwrap()),
        );
        let cases = [
            ("conditional", SchemaOperation::define(vec![sub]).unwrap(), &full),
            ("backfill", SchemaOperation::define(vec![key]).unwrap(), &full),
            ("destructive", SchemaOperation::undefine(type_fact.clone()), &full),
            ("opaque", SchemaOperation::redefine(function_old, function_new).unwrap(), &full),
            ("struct-unsupported", SchemaOperation::define(vec![struct_fact]).unwrap(), &full),
            ("persistent-function-metadata", SchemaOperation::define(vec![persistent_function_doc]).unwrap(), &full),
        ];
        let mut output = String::new();
        for (name, operation, binding) in cases {
            let error = lower_operation(0, &operation, &empty, &empty, binding, false, false)
                .unwrap_err();
            output.push_str(name);
            output.push('|');
            output.push_str(error.code());
            output.push('\n');
        }
        let no_capabilities = SchemaLoweringBinding::current(CapabilitySet::new()).unwrap();
        let capability_error = lower_operation(
            0,
            &SchemaOperation::define(vec![type_fact]).unwrap(),
            &empty,
            &empty,
            &no_capabilities,
            false,
            false,
        )
        .unwrap_err();
        output.push_str("capability|");
        output.push_str(capability_error.code());
        output.push('\n');
        let profile_error = SchemaLoweringBinding::new(
            SchemaLoweringProfileId::typedb_3_12_1(),
            SchemaLoweringProfileFingerprint::compute(b"wrong profile bytes"),
            CapabilitySet::new(),
        )
        .unwrap_err();
        output.push_str("profile|");
        output.push_str(profile_error.code());
        output.push('\n');
        assert_eq!(output, include_str!("../tests/fixtures/lowering-rejections-v1.txt"));
    }

    #[test]
    fn equal_default_cardinality_is_formal_only_and_lowers() {
        let person = type_id(TypeKind::Entity, "person");
        let owns = OwnsFactId::new(person, attribute_id("name")).unwrap();
        let card = annotation(
            AnnotationSubjectId::Owns(owns),
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(Cardinality::new(0, Some(1)).unwrap()),
        );
        let unit = lower_operation(
            0,
            &SchemaOperation::undefine(card).into(),
            &SchemaFactCatalog::empty(),
            &SchemaFactCatalog::empty(),
            &full_binding(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(unit.safety(), SafetyClass::FormalOnly);
        assert_eq!(unit.statements()[0].query(), "undefine\n@card from person owns name;");
    }

    #[test]
    fn safety_gate_matches_exhaustive_golden() {
        let mut output = String::new();
        for safety in SafetyClass::ALL {
            let name = match safety {
                SafetyClass::FormalOnly => "formal_only",
                SafetyClass::SchemaMetadata => "schema_metadata",
                SafetyClass::Additive => "additive",
                SafetyClass::Conditional => "conditional",
                SafetyClass::BackfillRequired => "backfill_required",
                SafetyClass::Destructive => "destructive",
                SafetyClass::Opaque => "opaque",
                SafetyClass::Unsupported => "unsupported",
            };
            output.push_str(name);
            match gate_safety(7, safety, false, false) {
                Ok(()) => output.push_str("|accepted\n"),
                Err(error) => {
                    assert_eq!(error.operation_index(), Some(7));
                    assert_eq!(error.safety(), Some(safety));
                    output.push('|');
                    output.push_str(error.code());
                    output.push('|');
                    output.push_str(error.message());
                    output.push('\n');
                }
            }
        }
        assert_eq!(
            output,
            include_str!("../tests/fixtures/lowering-safety-gate-v1.txt")
        );
    }

    #[test]
    fn destructive_gate_opens_only_under_approval() {
        assert!(gate_safety(0, SafetyClass::Destructive, false, true).is_ok());
        // An approval never opens classes no approval can execute.
        assert!(gate_safety(0, SafetyClass::Opaque, false, true).is_err());
        assert!(gate_safety(0, SafetyClass::BackfillRequired, false, true).is_err());
        assert!(gate_safety(0, SafetyClass::Unsupported, false, true).is_err());
        assert!(gate_safety(0, SafetyClass::Conditional, false, true).is_err());
    }
}
