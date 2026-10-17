//! Deterministic offline lowering of validated schema deltas to TypeDB 3.12.1 TypeQL.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::Serialize;
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::id::{FunctionId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationKindId, AnnotationSubjectId, CollectionMode, RelatesFact,
    RelatesFactId, SchemaAnnotationValue, SchemaDelta, SchemaFact, SchemaFactId, SchemaOperation,
    SchemaOperationKind,
};
use type_bridge_contract::value::CanonicalValue;
use type_bridge_core_lib::__internal as tql;

use type_bridge_schema::SafetyClass;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_schema::ManagedDeltaContext;

use crate::profile::{MigrationProfile, classify_operation_transition};
use crate::{SchemaLoweringProfileFingerprint, SchemaLoweringProfileId};

const CODE_PROFILE_MISMATCH: &str = "schema_lowering_profile_mismatch";
const CODE_SEMANTIC_PROFILE_UNSUPPORTED: &str = "schema_lowering_semantic_profile_unsupported";
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
            && self
                .0
                .keys()
                .zip(selection.iter())
                .all(|(left, right)| left == right)
    }
}

/// Registry-selected lowering-profile identity plus provider capabilities
/// available to the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaLoweringBinding {
    profile_id: SchemaLoweringProfileId,
    profile_fingerprint: SchemaLoweringProfileFingerprint,
    available_capabilities: CapabilitySet,
}

impl SchemaLoweringBinding {
    /// Bind caller capabilities to the exact compiled lowering profile named
    /// by `profile_id`.
    pub fn new(
        profile_id: SchemaLoweringProfileId,
        profile_fingerprint: SchemaLoweringProfileFingerprint,
        available_capabilities: CapabilitySet,
    ) -> Result<Self, SchemaLoweringDiagnostic> {
        let registry = MigrationProfile::for_lowering_profile_id(&profile_id);
        if registry.is_none_or(|registry| profile_fingerprint != registry.fingerprint()) {
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

    /// Bind capabilities to the compiled registry for a semantic profile.
    pub fn for_semantic_profile(
        semantic_profile: &SemanticProfileId,
        available_capabilities: CapabilitySet,
    ) -> Result<Self, SchemaLoweringDiagnostic> {
        let registry =
            MigrationProfile::for_semantic_profile(semantic_profile).ok_or_else(|| {
                SchemaLoweringDiagnostic::new(
                    CODE_SEMANTIC_PROFILE_UNSUPPORTED,
                    "no schema-transition registry exists for the semantic profile",
                )
            })?;
        Self::new(
            registry.lowering_profile_id(),
            registry.fingerprint(),
            available_capabilities,
        )
    }

    /// Bind capabilities to the TypeDB 3.12.1 lowering profile, the only
    /// profile 2.2 compiled.
    #[deprecated(
        since = "2.3.0",
        note = "binds the 3.12.1 profile only; use for_semantic_profile or for_context; removed in 3.0"
    )]
    pub fn current(
        available_capabilities: CapabilitySet,
    ) -> Result<Self, SchemaLoweringDiagnostic> {
        Self::new(
            crate::profile::typedb_3_12_1_profile().id.clone(),
            crate::profile::profile_fingerprint(),
            available_capabilities,
        )
    }

    /// Bind a delta context's capabilities to its semantic profile's registry.
    pub fn for_context(context: &ManagedDeltaContext) -> Result<Self, SchemaLoweringDiagnostic> {
        Self::for_semantic_profile(
            context.semantic_profile(),
            context.available_capabilities().clone(),
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
    /// Introduce schema definitions.
    Define,
    /// Remove schema definitions.
    Undefine,
    /// Replace schema definitions in place.
    Redefine,
}

/// One complete TypeQL schema query.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TypeQlStatement {
    verb: TypeQlVerb,
    query: String,
}

impl TypeQlStatement {
    fn new(verb: TypeQlVerb, query: tql::Query) -> Result<Self, RenderFailure> {
        Ok(Self {
            verb,
            query: tql::render(&query)?,
        })
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
    /// The source delta operation defines a fact.
    Define,
    /// The source delta operation replaces a fact.
    Redefine,
    /// The source delta operation removes a fact.
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
    /// Return the zero-based index of the source delta operation.
    pub const fn operation_index(&self) -> usize {
        self.operation_index
    }

    /// Return the formal operation represented by this unit.
    pub const fn operation_kind(&self) -> StatementOperationKind {
        self.operation_kind
    }

    /// Return the classified safety level for the operation.
    pub const fn safety(&self) -> SafetyClass {
        self.safety
    }

    /// Return whether every statement must execute atomically.
    pub const fn atomic(&self) -> bool {
        self.atomic
    }

    /// Return the canonical schema fact identities affected by the unit.
    pub fn affected_ids(&self) -> &[SchemaFactId] {
        &self.affected_ids
    }

    /// Return the provider capabilities required to execute the unit.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return the deterministic TypeQL statements in execution order.
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
    /// Return the exact formal delta retained by the plan.
    pub const fn delta(&self) -> &SchemaDelta {
        &self.delta
    }

    /// Return the lowering-profile identity used to build the plan.
    pub const fn profile_id(&self) -> &SchemaLoweringProfileId {
        &self.profile_id
    }

    /// Return the lowering-profile content fingerprint used to build the plan.
    pub const fn profile_fingerprint(&self) -> &SchemaLoweringProfileFingerprint {
        &self.profile_fingerprint
    }

    /// Return operation units in source-delta order.
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
    discharged_operation_indices: &[usize],
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
    if discharged_operation_indices
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
        || discharged_operation_indices
            .last()
            .is_some_and(|index| *index >= delta.operations().len())
    {
        return Err(SchemaLoweringDiagnostic::new(
            CODE_CONTEXT_MISMATCH,
            "verified discharged operation indices are not canonical for this delta",
        ));
    }
    // TypeDB deletes an attribute's value declaration together with the
    // attribute type. Sending a separate `undefine value ...` first is not a
    // valid intermediate schema because a concrete attribute must retain its
    // value type. Preserve the formal delta unit, but let the later exact type
    // deletion own that provider-side cascade.
    let deleted_attributes = delta
        .operations()
        .iter()
        .filter_map(|operation| match operation.undefined_fact() {
            Some(SchemaFact::Type(fact)) if fact.id().kind() == TypeKind::Attribute => {
                Some(fact.id().label().as_str().to_owned())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let units = delta
        .operations()
        .iter()
        .enumerate()
        .map(|(index, operation)| {
            let mut unit = lower_operation(
                index,
                operation,
                source_facts,
                target_facts,
                binding,
                discharged_operation_indices.binary_search(&index).is_ok(),
                destructive_approved,
            )?;
            if matches!(
                operation.undefined_fact(),
                Some(SchemaFact::Value(fact))
                    if deleted_attributes.contains(fact.id().attribute().label().as_str())
            ) {
                unit.statements.clear();
            }
            Ok(unit)
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
    verifier_resolved: bool,
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
        verifier_resolved,
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
    verifier_resolved: bool,
    destructive_approved: bool,
) -> Result<(), SchemaLoweringDiagnostic> {
    if verifier_resolved
        && !matches!(
            safety,
            SafetyClass::Conditional | SafetyClass::BackfillRequired
        )
    {
        return Err(SchemaLoweringDiagnostic::new(
            CODE_INVALID_TRANSITION,
            "verifier resolution targets an operation outside the resolvable safety classes",
        )
        .at_operation(operation_index)
        .with_safety(safety));
    }
    let (code, message) = match safety {
        SafetyClass::FormalOnly | SafetyClass::SchemaMetadata | SafetyClass::Additive => {
            return Ok(());
        }
        SafetyClass::Conditional if verifier_resolved => return Ok(()),
        SafetyClass::Conditional => (
            CODE_REQUIRES_ASSERTION,
            "schema transition requires an explicit data assertion",
        ),
        SafetyClass::BackfillRequired if verifier_resolved => return Ok(()),
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
        SchemaLoweringDiagnostic::new(failure.code, failure.message).at_operation(operation_index)
    })
}

impl From<tql::TqlError> for RenderFailure {
    fn from(_: tql::TqlError) -> Self {
        Self {
            code: CODE_UNSUPPORTED,
            message: "schema statement cannot be rendered through the official TypeQL AST",
        }
    }
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
            let mut function_annotations =
                BTreeMap::<FunctionId, Vec<tql::SchemaAnnotation>>::new();
            for fact in facts {
                if let SchemaFact::Annotation(annotation) = fact
                    && let AnnotationSubjectId::Function(function) = annotation.id().subject()
                    && function_ids.contains(function)
                {
                    function_annotations
                        .entry(function.clone())
                        .or_default()
                        .push(annotation_node(annotation)?);
                }
            }
            for values in function_annotations.values_mut() {
                values.sort_by_cached_key(|value| {
                    tql::render_schema_annotations(std::slice::from_ref(value))
                });
            }
            let mut definitions = Vec::new();
            for fact in facts {
                match fact {
                    SchemaFact::Annotation(annotation) if matches!(annotation.id().subject(), AnnotationSubjectId::Function(id) if function_ids.contains(id)) =>
                        {}
                    SchemaFact::Function(function) => definitions.push(tql::schema_function_fact(
                        function,
                        function_annotations
                            .remove(function.id())
                            .unwrap_or_default(),
                    )?),
                    _ => definitions.push(definition_node(fact, target_facts, true)?),
                }
            }
            Ok(vec![TypeQlStatement::new(
                TypeQlVerb::Define,
                tql::schema_define(definitions),
            )?])
        }
        SchemaOperationKind::Undefine => Ok(vec![TypeQlStatement::new(
            TypeQlVerb::Undefine,
            tql::schema_undefine(vec![undefinition_node(
                operation.undefined_fact().expect("undefine exposes fact"),
                source_facts,
            )?]),
        )?]),
        SchemaOperationKind::Redefine => {
            let expected = operation
                .expected_fact()
                .expect("redefine exposes expected");
            let replacement = operation
                .replacement_fact()
                .expect("redefine exposes replacement");
            if let (SchemaFact::Annotation(old), SchemaFact::Annotation(new)) =
                (expected, replacement)
                && matches!(old.id().subject(), AnnotationSubjectId::Sub(_))
                && matches!(
                    old.id().kind(),
                    AnnotationKindId::Doc | AnnotationKindId::Meta(_)
                )
            {
                return Ok(vec![
                    TypeQlStatement::new(
                        TypeQlVerb::Undefine,
                        tql::schema_undefine(vec![annotation_undefinition(old, source_facts)?]),
                    )?,
                    TypeQlStatement::new(
                        TypeQlVerb::Define,
                        tql::schema_define(vec![annotation_definition(new, target_facts, false)?]),
                    )?,
                ]);
            }
            if let (SchemaFact::Relates(old), SchemaFact::Relates(new)) = (expected, replacement) {
                return match (old.specializes(), new.specializes()) {
                    (None, Some(_)) => Ok(vec![TypeQlStatement::new(
                        TypeQlVerb::Define,
                        tql::schema_define(vec![relates_definition(new, target_facts)?]),
                    )?]),
                    (Some(parent), None) => Ok(vec![TypeQlStatement::new(
                        TypeQlVerb::Undefine,
                        tql::schema_undefine(vec![
                            tql::schema_undefine_specialisation_with_ordering(
                                old.id().relation().label().as_str(),
                                old.id().role().label().as_str(),
                                parent.label().as_str(),
                                old.collection_mode() == CollectionMode::OrderedList,
                            )?,
                        ]),
                    )?]),
                    (Some(_), Some(_)) => Ok(vec![TypeQlStatement::new(
                        TypeQlVerb::Redefine,
                        tql::schema_redefine(vec![relates_definition(new, target_facts)?]),
                    )?]),
                    (None, None) => Err(RenderFailure {
                        code: CODE_INVALID_TRANSITION,
                        message: "relates redefinition does not change specialization",
                    }),
                };
            }
            Ok(vec![TypeQlStatement::new(
                TypeQlVerb::Redefine,
                tql::schema_redefine(vec![definition_node(replacement, target_facts, false)?]),
            )?])
        }
    }
}

fn definition_node(
    fact: &SchemaFact,
    catalog: &SchemaFactCatalog,
    defining: bool,
) -> Result<tql::SchemaDefinition, RenderFailure> {
    let node = match fact {
        SchemaFact::Type(fact) => tql::schema_type(
            Some(type_kind(fact.id().kind())),
            fact.id().label().as_str(),
            vec![],
            vec![],
        )?,
        SchemaFact::Sub(fact) => tql::schema_type(
            None,
            fact.id().subtype().label().as_str(),
            vec![],
            vec![tql::schema_sub(fact.id().supertype().label().as_str())?],
        )?,
        SchemaFact::Value(fact) => tql::schema_type(
            None,
            fact.id().attribute().label().as_str(),
            vec![],
            vec![tql::schema_value_domain(fact.value_type(), vec![])],
        )?,
        SchemaFact::Owns(fact) => tql::schema_type(
            None,
            fact.id().owner().label().as_str(),
            vec![],
            vec![tql::schema_owns(
                fact.id().attribute().label().as_str(),
                fact.collection_mode() == CollectionMode::OrderedList,
                vec![],
            )?],
        )?,
        SchemaFact::Relates(fact) => return relates_definition(fact, catalog),
        SchemaFact::Plays(fact) => tql::schema_type(
            None,
            fact.id().player().label().as_str(),
            vec![],
            vec![tql::schema_plays(
                fact.id().role().declaring_relation().as_str(),
                fact.id().role().label().as_str(),
                vec![],
            )?],
        )?,
        SchemaFact::Annotation(fact) => return annotation_definition(fact, catalog, defining),
        SchemaFact::Function(fact) => tql::schema_function_fact(fact, vec![])?,
        SchemaFact::Struct(_) => {
            return Err(RenderFailure {
                code: CODE_UNSUPPORTED,
                message: "TypeDB 3.12.1 does not admit the pinned struct transition grammar",
            });
        }
    };
    Ok(node)
}
fn undefinition_node(
    fact: &SchemaFact,
    catalog: &SchemaFactCatalog,
) -> Result<tql::SchemaUndefinition, RenderFailure> {
    let node = match fact {
        SchemaFact::Type(fact) => tql::schema_undefine_type(fact.id().label().as_str())?,
        SchemaFact::Sub(fact) => tql::schema_undefine_capability(
            fact.id().subtype().label().as_str(),
            tql::schema_sub(fact.id().supertype().label().as_str())?,
        )?,
        SchemaFact::Value(fact) => tql::schema_undefine_capability(
            fact.id().attribute().label().as_str(),
            tql::schema_value_domain(fact.value_type(), vec![]),
        )?,
        SchemaFact::Owns(fact) => tql::schema_undefine_capability(
            fact.id().owner().label().as_str(),
            tql::schema_owns(
                fact.id().attribute().label().as_str(),
                fact.collection_mode() == CollectionMode::OrderedList,
                vec![],
            )?,
        )?,
        SchemaFact::Relates(fact) => tql::schema_undefine_capability(
            fact.id().relation().label().as_str(),
            tql::schema_relates(
                fact.id().role().label().as_str(),
                fact.collection_mode() == CollectionMode::OrderedList,
                None,
                vec![],
            )?,
        )?,
        SchemaFact::Plays(fact) => tql::schema_undefine_capability(
            fact.id().player().label().as_str(),
            tql::schema_plays(
                fact.id().role().declaring_relation().as_str(),
                fact.id().role().label().as_str(),
                vec![],
            )?,
        )?,
        SchemaFact::Annotation(fact) => return annotation_undefinition(fact, catalog),
        SchemaFact::Function(fact) => tql::schema_undefine_function(fact.id().label().as_str())?,
        SchemaFact::Struct(_) => {
            return Err(RenderFailure {
                code: CODE_UNSUPPORTED,
                message: "TypeDB 3.12.1 does not admit the pinned struct transition grammar",
            });
        }
    };
    Ok(node)
}
fn relates_definition(
    fact: &RelatesFact,
    catalog: &SchemaFactCatalog,
) -> Result<tql::SchemaDefinition, RenderFailure> {
    if let Some(parent) = fact.specializes() {
        let invalid = |_| RenderFailure {
            code: CODE_INVALID_TRANSITION,
            message: "invalid specialized role identity",
        };
        let parent_id = RelatesFactId::new(
            TypeId::new(TypeKind::Relation, parent.declaring_relation().as_str())
                .map_err(invalid)?,
            parent.clone(),
        )
        .map_err(invalid)?;
        if let Some(SchemaFact::Relates(parent)) = catalog.get(&SchemaFactId::Relates(parent_id))
            && parent.collection_mode() != fact.collection_mode()
        {
            return Err(RenderFailure {
                code: CODE_INVALID_TRANSITION,
                message: "a specialized role must preserve parent collection mode",
            });
        }
    }
    Ok(tql::schema_type(
        None,
        fact.id().relation().label().as_str(),
        vec![],
        vec![tql::schema_relates(
            fact.id().role().label().as_str(),
            fact.collection_mode() == CollectionMode::OrderedList,
            fact.specializes().map(|role| role.label().as_str()),
            vec![],
        )?],
    )?)
}
fn annotation_subject(
    subject: &AnnotationSubjectId,
    catalog: &SchemaFactCatalog,
) -> Result<(String, Option<tql::SchemaCapability>), RenderFailure> {
    let (name, capability) = match subject {
        AnnotationSubjectId::Type(id) => (id.label().as_str(), None),
        AnnotationSubjectId::Sub(id) => (
            id.subtype().label().as_str(),
            Some(tql::schema_sub(id.supertype().label().as_str())?),
        ),
        AnnotationSubjectId::Value(id) => {
            let Some(SchemaFact::Value(value)) = catalog.get(&SchemaFactId::Value(id.clone()))
            else {
                return Err(RenderFailure {
                    code: CODE_RENDER_CONTEXT,
                    message: "value annotation rendering requires its exact value fact payload",
                });
            };
            (
                id.attribute().label().as_str(),
                Some(tql::schema_value_domain(value.value_type(), vec![])),
            )
        }
        AnnotationSubjectId::Owns(id) => (
            id.owner().label().as_str(),
            Some(tql::schema_owns(
                id.attribute().label().as_str(),
                catalog.get(&SchemaFactId::Owns(id.clone())).is_some_and(|fact| {
                    matches!(fact, SchemaFact::Owns(fact) if fact.collection_mode() == CollectionMode::OrderedList)
                }),
                vec![],
            )?),
        ),
        AnnotationSubjectId::Relates(id) => (
            id.relation().label().as_str(),
            Some(tql::schema_relates(
                id.role().label().as_str(),
                catalog.get(&SchemaFactId::Relates(id.clone())).is_some_and(|fact| {
                    matches!(fact, SchemaFact::Relates(fact) if fact.collection_mode() == CollectionMode::OrderedList)
                }),
                None,
                vec![],
            )?),
        ),
        AnnotationSubjectId::Plays(id) => (
            id.player().label().as_str(),
            Some(tql::schema_plays(
                id.role().declaring_relation().as_str(),
                id.role().label().as_str(),
                vec![],
            )?),
        ),
        AnnotationSubjectId::Function(_) => {
            return Err(RenderFailure {
                code: CODE_UNSUPPORTED,
                message: "persistent function annotations are unsupported; fold metadata into function definition",
            });
        }
    };
    Ok((name.to_owned(), capability))
}
fn annotation_definition(
    annotation: &AnnotationFact,
    catalog: &SchemaFactCatalog,
    defining: bool,
) -> Result<tql::SchemaDefinition, RenderFailure> {
    let value = annotation_node(annotation)?;
    let (name, capability) = annotation_subject(annotation.id().subject(), catalog)?;
    let (head, capabilities) = if let Some(capability) = capability {
        (vec![], vec![capability.annotated(vec![value])])
    } else {
        (vec![value], vec![])
    };
    let kind = if defining {
        match annotation.id().subject() {
            AnnotationSubjectId::Type(id) => Some(type_kind(id.kind())),
            _ => None,
        }
    } else {
        None
    };
    Ok(tql::schema_type(kind, &name, head, capabilities)?)
}
fn annotation_undefinition(
    annotation: &AnnotationFact,
    catalog: &SchemaFactCatalog,
) -> Result<tql::SchemaUndefinition, RenderFailure> {
    reject_distinct_lowering(annotation.id().kind())?;
    let (name, capability) = annotation_subject(annotation.id().subject(), catalog)?;
    let (category, key) = match annotation.id().kind() {
        AnnotationKindId::Abstract => ("abstract", None),
        AnnotationKindId::Independent => ("independent", None),
        AnnotationKindId::Key => ("key", None),
        AnnotationKindId::Unique => ("unique", None),
        AnnotationKindId::Distinct => ("distinct", None),
        AnnotationKindId::Card => ("card", None),
        AnnotationKindId::Regex => ("regex", None),
        AnnotationKindId::Range => ("range", None),
        AnnotationKindId::Values => ("values", None),
        AnnotationKindId::Doc => ("doc", None),
        AnnotationKindId::Meta(key) => ("meta", Some(key.as_str())),
    };
    Ok(tql::schema_undefine_annotation(
        category, key, &name, capability,
    )?)
}
fn reject_distinct_lowering(kind: &AnnotationKindId) -> Result<(), RenderFailure> {
    if kind == &AnnotationKindId::Distinct {
        return Err(RenderFailure {
            code: CODE_UNSUPPORTED,
            message: "schema transition is unsupported by the TypeDB 3.12.1 lowering profile",
        });
    }
    Ok(())
}
fn annotation_node(annotation: &AnnotationFact) -> Result<tql::SchemaAnnotation, RenderFailure> {
    reject_distinct_lowering(annotation.id().kind())?;
    let node = match (annotation.id().kind(), annotation.value()) {
        (AnnotationKindId::Abstract, SchemaAnnotationValue::Presence) => {
            tql::schema_flag("abstract")?
        }
        (AnnotationKindId::Independent, SchemaAnnotationValue::Presence) => {
            tql::schema_flag("independent")?
        }
        (AnnotationKindId::Key, SchemaAnnotationValue::Presence) => tql::schema_flag("key")?,
        (AnnotationKindId::Unique, SchemaAnnotationValue::Presence) => tql::schema_flag("unique")?,
        (AnnotationKindId::Card, SchemaAnnotationValue::Cardinality(card)) => {
            tql::schema_card((*card).min(), (*card).max())?
        }
        (AnnotationKindId::Regex, SchemaAnnotationValue::Regex(regex)) => {
            tql::schema_regex(regex.as_str())?
        }
        (AnnotationKindId::Range, SchemaAnnotationValue::Range(range)) => {
            tql::schema_canonical_range(range.lower(), range.upper())?
        }
        (AnnotationKindId::Values, SchemaAnnotationValue::Values(values)) => {
            tql::schema_canonical_values(&values.iter().cloned().collect::<Vec<_>>())?
        }
        (AnnotationKindId::Doc, SchemaAnnotationValue::Doc(doc)) => tql::schema_doc(doc.as_str())?,
        (
            AnnotationKindId::Meta(key),
            SchemaAnnotationValue::Meta(CanonicalValue::String(value)),
        ) => tql::schema_meta(key.as_str(), value.as_str())?,
        _ => {
            return Err(RenderFailure {
                code: CODE_UNSUPPORTED,
                message: "annotation payload is unsupported by the official TypeQL AST",
            });
        }
    };
    Ok(node)
}
fn type_kind(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Entity => "entity",
        TypeKind::Relation => "relation",
        TypeKind::Attribute => "attribute",
        TypeKind::Struct => "struct",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::id::{AttributeId, Label, RoleId, StructId, TypeId};
    use type_bridge_contract::schema::{
        AnnotationFactId, CanonicalValueRange, CanonicalValueSet, DocText, FunctionBody,
        FunctionParameter, FunctionSignature, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId,
        RegexPattern, RelatesFactId, SchemaOperation, StructFact, StructField, SubFact, SubFactId,
        TypeFact, ValueFact, ValueFactId,
    };
    use type_bridge_contract::schema::{
        FunctionFact, FunctionReturnElement, FunctionReturnMode, TypeReference,
    };
    use type_bridge_contract::value::{CanonicalString, Cardinality, ValueTypeTag};

    #[test]
    #[allow(deprecated)]
    fn deprecated_current_binds_the_3_12_1_registry() {
        let capabilities = CapabilitySet::default();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        assert_eq!(
            SchemaLoweringBinding::current(capabilities.clone()).unwrap(),
            SchemaLoweringBinding::for_semantic_profile(&profile, capabilities).unwrap()
        );
    }

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
        let profile = MigrationProfile::Typedb3_12_1;
        SchemaLoweringBinding::for_semantic_profile(
            &profile.semantic_profile_id(),
            profile.profile().required_capabilities.clone(),
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
    fn tuple_function_lowering_corrects_old_invalid_header_and_keeps_body_bytes() {
        let body = "match\n  # retained tuple comment\n  let $value = $seed;\nreturn first $value, $value;\n";
        let element = FunctionReturnElement::new(TypeReference::Value(ValueTypeTag::Long), false);
        let function = FunctionFact::new(
            FunctionId::new("pair").unwrap(),
            FunctionSignature::new(
                vec![FunctionParameter::new(
                    Label::new("seed").unwrap(),
                    TypeReference::Value(ValueTypeTag::Long),
                )],
                FunctionReturnMode::tuple(vec![element.clone(), element]).unwrap(),
            )
            .unwrap(),
            FunctionBody::new(body).unwrap(),
        );
        let fact = SchemaFact::Function(function.clone());
        let catalog = SchemaFactCatalog::new(vec![fact.clone()]).unwrap();
        let statements = render_operation_inner(
            &SchemaOperation::define(vec![fact]).unwrap(),
            &SchemaFactCatalog::empty(),
            &catalog,
        )
        .unwrap();
        let new = statements[0].query();
        let old = format!("define fun pair($seed: integer) -> (integer, integer):\n{body}");
        assert!(tql::parse(&old).is_err());
        assert!(new.contains("-> integer, integer:"), "{new}");
        assert!(new.contains(body), "{new}");
        assert_eq!(function.body().text(), body);
        tql::parse(new).unwrap();
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
        dump(
            "type-define",
            SchemaOperation::define(vec![type_fact.clone()]).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "type-undefine",
            SchemaOperation::undefine(type_fact),
            &empty,
            &empty,
            &mut output,
        );
        let sub_fact = SchemaFact::Sub(SubFact::new(sub_id.clone()));
        dump(
            "sub-define",
            SchemaOperation::define(vec![sub_fact.clone()]).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "sub-undefine",
            SchemaOperation::undefine(sub_fact),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "value-define",
            SchemaOperation::define(vec![string_value.clone()]).unwrap(),
            &empty,
            &string_catalog,
            &mut output,
        );
        dump(
            "value-redefine",
            SchemaOperation::redefine(string_value.clone(), integer_value.clone()).unwrap(),
            &string_catalog,
            &integer_catalog,
            &mut output,
        );
        dump(
            "value-undefine",
            SchemaOperation::undefine(string_value.clone()),
            &string_catalog,
            &empty,
            &mut output,
        );
        let owns = SchemaFact::Owns(OwnsFact::new(owns_id.clone()));
        dump(
            "owns-define",
            SchemaOperation::define(vec![owns.clone()]).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "owns-undefine",
            SchemaOperation::undefine(owns),
            &empty,
            &empty,
            &mut output,
        );
        let relates = SchemaFact::Relates(RelatesFact::new(relates_id, None).unwrap());
        dump(
            "relates-define",
            SchemaOperation::define(vec![relates.clone()]).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "relates-undefine",
            SchemaOperation::undefine(relates),
            &empty,
            &empty,
            &mut output,
        );
        let specialization_a = SchemaFact::Relates(
            RelatesFact::new(child_relates.clone(), Some(parent_a.clone())).unwrap(),
        );
        let specialization_b = SchemaFact::Relates(
            RelatesFact::new(child_relates.clone(), Some(parent_b.clone())).unwrap(),
        );
        let unspecialized = SchemaFact::Relates(RelatesFact::new(child_relates, None).unwrap());
        dump(
            "specialization-define",
            SchemaOperation::redefine(unspecialized.clone(), specialization_a.clone()).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "specialization-redefine",
            SchemaOperation::redefine(specialization_a.clone(), specialization_b.clone()).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "specialization-undefine",
            SchemaOperation::redefine(specialization_b, unspecialized).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        let plays = SchemaFact::Plays(PlaysFact::new(plays_id));
        dump(
            "plays-define",
            SchemaOperation::define(vec![plays.clone()]).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "plays-undefine",
            SchemaOperation::undefine(plays),
            &empty,
            &empty,
            &mut output,
        );

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
        dump(
            "doc-define",
            SchemaOperation::define(vec![doc_old.clone()]).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "doc-redefine",
            SchemaOperation::redefine(doc_old.clone(), doc_new).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "doc-undefine",
            SchemaOperation::undefine(doc_old),
            &empty,
            &empty,
            &mut output,
        );
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
        dump(
            "regex-redefine",
            SchemaOperation::redefine(regex_old, regex_new).unwrap(),
            &string_catalog,
            &string_catalog,
            &mut output,
        );
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
        dump(
            "card-redefine",
            SchemaOperation::redefine(card_old, card_new).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        let range = annotation(
            AnnotationSubjectId::Value(ValueFactId::new(name.clone())),
            AnnotationKindId::Range,
            SchemaAnnotationValue::Range(
                CanonicalValueRange::new(
                    Some(CanonicalValue::Long(1)),
                    Some(CanonicalValue::Long(10)),
                )
                .unwrap(),
            ),
        );
        dump(
            "range-define",
            SchemaOperation::define(vec![range]).unwrap(),
            &empty,
            &string_catalog,
            &mut output,
        );
        let values = annotation(
            AnnotationSubjectId::Value(ValueFactId::new(name)),
            AnnotationKindId::Values,
            SchemaAnnotationValue::Values(
                CanonicalValueSet::new([CanonicalValue::Long(2), CanonicalValue::Long(1)]).unwrap(),
            ),
        );
        dump(
            "values-define",
            SchemaOperation::define(vec![values]).unwrap(),
            &empty,
            &integer_catalog,
            &mut output,
        );
        let meta_old = annotation(
            AnnotationSubjectId::Sub(sub_id.clone()),
            AnnotationKindId::meta("owner").unwrap(),
            SchemaAnnotationValue::Meta(CanonicalValue::String(
                CanonicalString::new("old").unwrap(),
            )),
        );
        let meta_new = annotation(
            AnnotationSubjectId::Sub(sub_id),
            AnnotationKindId::meta("owner").unwrap(),
            SchemaAnnotationValue::Meta(CanonicalValue::String(
                CanonicalString::new("new").unwrap(),
            )),
        );
        dump(
            "sub-meta-fallback",
            SchemaOperation::redefine(meta_old.clone(), meta_new).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "meta-keyed-undefine",
            SchemaOperation::undefine(meta_old),
            &empty,
            &empty,
            &mut output,
        );

        let function_fact = function("match\n  let $value = $seed;\nreturn first $value;");
        let function_doc = annotation(
            AnnotationSubjectId::Function(FunctionId::new("answer").unwrap()),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("answer docs").unwrap()),
        );
        let function_meta = annotation(
            AnnotationSubjectId::Function(FunctionId::new("answer").unwrap()),
            AnnotationKindId::meta("owner").unwrap(),
            SchemaAnnotationValue::Meta(CanonicalValue::String(
                CanonicalString::new("core").unwrap(),
            )),
        );
        dump(
            "function-define-with-metadata",
            SchemaOperation::define(vec![function_meta, function_fact.clone(), function_doc])
                .unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        let changed_function = function("match\n  let $value = 2;\nreturn first $value;");
        dump(
            "function-redefine",
            SchemaOperation::redefine(function_fact.clone(), changed_function).unwrap(),
            &empty,
            &empty,
            &mut output,
        );
        dump(
            "function-undefine",
            SchemaOperation::undefine(function_fact),
            &empty,
            &empty,
            &mut output,
        );

        let old = include_str!("../tests/fixtures/lowering-supported-v1.txt");
        let sections = |source: &str| {
            source
                .split("## ")
                .skip(1)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let old = sections(old);
        let new = sections(&output);
        assert_eq!(old.len(), new.len());
        for (old, new) in old.iter().zip(&new) {
            let (old_name, old_queries) = old.split_once('\n').unwrap();
            let (new_name, new_queries) = new.split_once('\n').unwrap();
            assert_eq!(old_name, new_name);
            let old = old_queries.split("-- atomic-next --").collect::<Vec<_>>();
            let new = new_queries.split("-- atomic-next --").collect::<Vec<_>>();
            assert_eq!(old.len(), new.len(), "{old_name}");
            for (old, new) in old.iter().zip(new) {
                assert!(
                    tql::equivalent(old, new).unwrap(),
                    "{old_name}: {old} != {new}"
                );
            }
        }
    }

    #[test]
    fn safety_profile_and_capability_rejections_match_golden() {
        let empty = SchemaFactCatalog::empty();
        let full = full_binding();
        let person = type_id(TypeKind::Entity, "person");
        let employee = type_id(TypeKind::Entity, "employee");
        let sub = SchemaFact::Sub(SubFact::new(
            SubFactId::new(employee, person.clone()).unwrap(),
        ));
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
                vec![StructField::new(
                    Label::new("field").unwrap(),
                    ValueTypeTag::Long,
                    false,
                )],
            )
            .unwrap(),
        );
        let persistent_function_doc = annotation(
            AnnotationSubjectId::Function(FunctionId::new("answer").unwrap()),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("docs").unwrap()),
        );
        let cases = [
            (
                "conditional",
                SchemaOperation::define(vec![sub]).unwrap(),
                &full,
            ),
            (
                "backfill",
                SchemaOperation::define(vec![key]).unwrap(),
                &full,
            ),
            (
                "destructive",
                SchemaOperation::undefine(type_fact.clone()),
                &full,
            ),
            (
                "opaque",
                SchemaOperation::redefine(function_old, function_new).unwrap(),
                &full,
            ),
            (
                "struct-unsupported",
                SchemaOperation::define(vec![struct_fact]).unwrap(),
                &full,
            ),
            (
                "persistent-function-metadata",
                SchemaOperation::define(vec![persistent_function_doc]).unwrap(),
                &full,
            ),
        ];
        let mut output = String::new();
        for (name, operation, binding) in cases {
            let error =
                lower_operation(0, &operation, &empty, &empty, binding, false, false).unwrap_err();
            output.push_str(name);
            output.push('|');
            output.push_str(error.code());
            output.push('\n');
        }
        let no_capabilities = SchemaLoweringBinding::for_semantic_profile(
            &MigrationProfile::Typedb3_12_1.semantic_profile_id(),
            CapabilitySet::new(),
        )
        .unwrap();
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
            SchemaLoweringProfileFingerprint::compute_for_profile(
                &SchemaLoweringProfileId::typedb_3_12_1(),
                b"wrong profile bytes",
            ),
            CapabilitySet::new(),
        )
        .unwrap_err();
        output.push_str("profile|");
        output.push_str(profile_error.code());
        output.push('\n');
        assert_eq!(
            output,
            include_str!("../tests/fixtures/lowering-rejections-v1.txt")
        );
    }

    #[test]
    fn distinct_migration_lowering_is_explicitly_unsupported() {
        let person = type_id(TypeKind::Entity, "ordered-person");
        let owns = OwnsFactId::new(person, attribute_id("ordered-name")).unwrap();
        let distinct = annotation(
            AnnotationSubjectId::Owns(owns),
            AnnotationKindId::Distinct,
            SchemaAnnotationValue::Presence,
        );
        let operations = [
            SchemaOperation::define(vec![distinct.clone()]).unwrap(),
            SchemaOperation::undefine(distinct),
        ];

        for (index, operation) in operations.into_iter().enumerate() {
            let error = lower_operation(
                index,
                &operation,
                &SchemaFactCatalog::empty(),
                &SchemaFactCatalog::empty(),
                &full_binding(),
                false,
                false,
            )
            .unwrap_err();
            assert_eq!(error.code(), CODE_UNSUPPORTED);
            assert_eq!(
                error.message(),
                "schema transition is unsupported by the TypeDB 3.12.1 lowering profile",
            );
            assert_eq!(error.operation_index(), Some(index));
            assert_eq!(error.safety(), Some(SafetyClass::Unsupported));
            assert!(error.missing_capabilities().is_empty());

            let render_error = render_operation(
                index,
                &operation,
                &SchemaFactCatalog::empty(),
                &SchemaFactCatalog::empty(),
            )
            .unwrap_err();
            assert_eq!(render_error.code(), CODE_UNSUPPORTED);
            assert_eq!(render_error.operation_index(), Some(index));
        }
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
            &SchemaOperation::undefine(card),
            &SchemaFactCatalog::empty(),
            &SchemaFactCatalog::empty(),
            &full_binding(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(unit.safety(), SafetyClass::FormalOnly);
        assert!(
            tql::equivalent(
                unit.statements()[0].query(),
                "undefine\n@card from person owns name;"
            )
            .unwrap()
        );
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

    #[test]
    fn backfill_gate_requires_verifier_discharge_and_retains_raw_unit_safety() {
        let person = type_id(TypeKind::Entity, "person");
        let owns = OwnsFactId::new(person, attribute_id("name")).unwrap();
        let owns_fact = SchemaFact::Owns(OwnsFact::new(owns.clone()));
        let target = SchemaFactCatalog::new([owns_fact]).unwrap();
        let key = annotation(
            AnnotationSubjectId::Owns(owns),
            AnnotationKindId::Key,
            SchemaAnnotationValue::Presence,
        );
        let operation = SchemaOperation::define(vec![key]).unwrap();

        let unverified = lower_operation(
            0,
            &operation,
            &SchemaFactCatalog::empty(),
            &target,
            &full_binding(),
            false,
            false,
        )
        .expect_err("an unverified backfill operation must stay closed");
        assert_eq!(unverified.code(), CODE_REQUIRES_BACKFILL);

        let verified = lower_operation(
            0,
            &operation,
            &SchemaFactCatalog::empty(),
            &target,
            &full_binding(),
            true,
            false,
        )
        .expect("exact verifier discharge admits the operation");
        assert_eq!(verified.safety(), SafetyClass::BackfillRequired);

        for safety in [
            SafetyClass::FormalOnly,
            SafetyClass::SchemaMetadata,
            SafetyClass::Additive,
            SafetyClass::Destructive,
            SafetyClass::Opaque,
            SafetyClass::Unsupported,
        ] {
            assert!(gate_safety(0, safety, true, false).is_err());
        }
    }
    #[test]
    fn ordered_facts_correct_old_scalar_output_through_every_builder() {
        let owns_id =
            OwnsFactId::new(type_id(TypeKind::Entity, "person"), attribute_id("name")).unwrap();
        let owns = SchemaFact::Owns(
            OwnsFact::new(owns_id.clone()).with_collection_mode(CollectionMode::OrderedList),
        );
        let parent_id = RelatesFactId::new(
            type_id(TypeKind::Relation, "base"),
            role_id("base", "participant"),
        )
        .unwrap();
        let parent = SchemaFact::Relates(
            RelatesFact::new_with_collection_mode(
                parent_id.clone(),
                None,
                CollectionMode::OrderedList,
            )
            .unwrap(),
        );
        let child_id = RelatesFactId::new(
            type_id(TypeKind::Relation, "review"),
            role_id("review", "reviewer"),
        )
        .unwrap();
        let child = SchemaFact::Relates(
            RelatesFact::new_with_collection_mode(
                child_id.clone(),
                Some(parent_id.role().clone()),
                CollectionMode::OrderedList,
            )
            .unwrap(),
        );
        let unspecialized = SchemaFact::Relates(
            RelatesFact::new_with_collection_mode(child_id, None, CollectionMode::OrderedList)
                .unwrap(),
        );
        let catalog =
            SchemaFactCatalog::new([owns.clone(), parent.clone(), child.clone()]).unwrap();
        let owns_doc = annotation(
            AnnotationSubjectId::Owns(owns_id),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("ordered").unwrap()),
        );
        let relates_doc = annotation(
            AnnotationSubjectId::Relates(parent_id),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(DocText::new("ordered").unwrap()),
        );
        let define = |fact: SchemaFact| SchemaOperation::define(vec![fact]).unwrap();
        let cases = [
            ("owns_define", define(owns.clone())),
            ("owns_undefine", SchemaOperation::undefine(owns)),
            ("relates_define", define(parent.clone())),
            ("relates_undefine", SchemaOperation::undefine(parent)),
            ("specialisation_define", define(child.clone())),
            (
                "specialisation_remove",
                SchemaOperation::redefine(child, unspecialized).unwrap(),
            ),
            ("owns_doc_define", define(owns_doc.clone())),
            ("owns_doc_undefine", SchemaOperation::undefine(owns_doc)),
            ("relates_doc_define", define(relates_doc.clone())),
            (
                "relates_doc_undefine",
                SchemaOperation::undefine(relates_doc),
            ),
        ];
        let baseline: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../tests/fixtures/native_ordered_pre_ast.json"
        ))
        .unwrap();
        for ((name, operation), baseline) in cases.into_iter().zip(&baseline) {
            assert_eq!(name, baseline["case"]);
            let statements = render_operation_inner(&operation, &catalog, &catalog).unwrap();
            assert_eq!(statements.len(), 1);
            let new = statements[0].query();
            let old = baseline["old"].as_str().unwrap();
            tql::parse(new).unwrap();
            tql::parse(old).unwrap();
            assert!(
                new.contains(baseline["corrected_contains"].as_str().unwrap()),
                "{name}: {new}"
            );
            assert!(
                !tql::equivalent(old, new).unwrap(),
                "{name}: old scalar output must differ"
            );
        }
    }

    #[test]
    fn native_specialisation_refuses_known_collection_mode_mismatches() {
        for ordered in [false, true] {
            let parent_id = RelatesFactId::new(
                type_id(TypeKind::Relation, "base"),
                role_id("base", "participant"),
            )
            .unwrap();
            let mode = if ordered {
                CollectionMode::OrderedList
            } else {
                CollectionMode::Unordered
            };
            let opposite = if ordered {
                CollectionMode::Unordered
            } else {
                CollectionMode::OrderedList
            };
            let parent = SchemaFact::Relates(
                RelatesFact::new_with_collection_mode(parent_id.clone(), None, mode).unwrap(),
            );
            let child = SchemaFact::Relates(
                RelatesFact::new_with_collection_mode(
                    RelatesFactId::new(
                        type_id(TypeKind::Relation, "review"),
                        role_id("review", "reviewer"),
                    )
                    .unwrap(),
                    Some(parent_id.role().clone()),
                    opposite,
                )
                .unwrap(),
            );
            let catalog = SchemaFactCatalog::new([parent, child.clone()]).unwrap();
            let error = render_operation_inner(
                &SchemaOperation::define(vec![child]).unwrap(),
                &SchemaFactCatalog::empty(),
                &catalog,
            )
            .unwrap_err();
            assert_eq!(error.code, CODE_INVALID_TRANSITION);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires an isolated TypeDB 3.13.6 server"]
    async fn native_ordered_formatter_exports_and_removes_schema_live() {
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_orm::{ConnectOptions, Database};
        use type_bridge_schema_compat::typeql_to_declared;
        let baseline_text = "define attribute name, value string; entity person; relation base @abstract; relation review @abstract, sub base;";
        let target_text = "define attribute name, value string; entity person, owns name[]; relation base @abstract, relates participant[] @abstract; relation review @abstract, sub base, relates reviewer[] as participant[];";
        let declared = |text: &str| {
            typeql_to_declared(DocumentId::new("ordered-live.tql").unwrap(), text).unwrap()
        };
        let baseline = declared(baseline_text);
        let target = declared(target_text);
        let additions = target
            .facts()
            .filter(|fact| match fact {
                SchemaFact::Owns(_) | SchemaFact::Relates(_) => true,
                SchemaFact::Annotation(annotation) => {
                    matches!(annotation.id().subject(), AnnotationSubjectId::Relates(_))
                }
                _ => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        let catalog = SchemaFactCatalog::new(target.facts().cloned()).unwrap();
        let statements = render_operation_inner(
            &SchemaOperation::define(additions.clone()).unwrap(),
            &catalog,
            &catalog,
        )
        .unwrap();
        let child = additions
            .iter()
            .find_map(|fact| match fact {
                SchemaFact::Relates(relates) if relates.specializes().is_some() => {
                    Some(relates.clone())
                }
                _ => None,
            })
            .unwrap();
        let unspecialized = RelatesFact::new_with_collection_mode(
            child.id().clone(),
            None,
            CollectionMode::OrderedList,
        )
        .unwrap();
        let mut removals = render_operation_inner(
            &SchemaOperation::redefine(
                SchemaFact::Relates(child),
                SchemaFact::Relates(unspecialized.clone()),
            )
            .unwrap(),
            &catalog,
            &catalog,
        )
        .unwrap();
        removals.extend(
            render_operation_inner(
                &SchemaOperation::undefine(SchemaFact::Relates(unspecialized)),
                &catalog,
                &catalog,
            )
            .unwrap(),
        );
        for fact in &additions {
            if match fact {
                SchemaFact::Owns(_) => true,
                SchemaFact::Relates(relates) => relates.specializes().is_none(),
                _ => false,
            } {
                removals.extend(
                    render_operation_inner(
                        &SchemaOperation::undefine(fact.clone()),
                        &catalog,
                        &catalog,
                    )
                    .unwrap(),
                );
            }
        }
        let address = std::env::var("TYPEDB_ADDRESS").expect("TYPEDB_ADDRESS");
        let username = std::env::var("TYPEDB_USERNAME").unwrap_or_else(|_| "admin".into());
        let password = std::env::var("TYPEDB_PASSWORD").unwrap_or_else(|_| "password".into());
        let mut options = ConnectOptions::default();
        if let Ok(port) = std::env::var("TYPEDB_HTTP_PORT") {
            options.http_port = port.parse().unwrap();
        }
        let name = format!(
            "tb_ordered_formatter_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        type_bridge_orm::ensure_database_exists(&address, &name, &username, &password, options)
            .await
            .unwrap();
        let database = std::sync::Arc::new(
            Database::connect_with_options(&address, &name, &username, &password, options)
                .await
                .unwrap(),
        );
        let live = database.clone();
        // Join before cleanup so assertion failures still delete this test's database.
        let result = tokio::spawn(async move {
            assert_eq!(
                live.server_version(),
                Some(type_bridge_core_lib::version::Version::new(3, 13, 6))
            );
            let mut transaction = live.schema_transaction().await.unwrap();
            transaction.query(baseline_text).await.unwrap();
            for statement in statements {
                transaction.query(statement.query()).await.unwrap();
            }
            transaction.commit().await.unwrap();
            let exported = live.schema_text().await.unwrap();
            for marker in [
                "owns name[]",
                "relates participant[]",
                "reviewer[] as participant[]",
            ] {
                assert!(exported.contains(marker), "{marker} absent from {exported}");
            }
            assert_eq!(
                declared(&exported).declared_identity_fingerprint(),
                target.declared_identity_fingerprint()
            );
            let mut transaction = live.schema_transaction().await.unwrap();
            for statement in removals {
                transaction.query(statement.query()).await.unwrap();
            }
            transaction.commit().await.unwrap();
            let exported = live.schema_text().await.unwrap();
            assert_eq!(
                declared(&exported).declared_identity_fingerprint(),
                baseline.declared_identity_fingerprint()
            );
        })
        .await;
        database.delete_database().await.unwrap();
        result.unwrap();
    }
}
