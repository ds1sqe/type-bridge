//! Pure managed-schema diff, replay, and inversion.

use std::collections::BTreeMap;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, ManagedFactSelection, ManagedSchemaState, ManagedScopeId,
    PatchFormatVersion, SchemaDelta, SchemaDiagnostic, SchemaDiagnostics, SchemaFact, SchemaFactId,
    SchemaOperation, SchemaOperationKind, SourceSpan, SourcedSchemaFact,
};

use crate::delta_dependencies::{FactDependencyGraph, plan_schema_operations};
use crate::{
    ManagedSchemaScope, managed_declared_identity_fingerprint, managed_semantic_schema_fingerprint,
    resolve_schema_with_capabilities,
};

/// Explicit inputs which determine a managed schema state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedDeltaContext {
    scope_id: ManagedScopeId,
    semantic_profile: SemanticProfileId,
    available_capabilities: CapabilitySet,
}

impl ManagedDeltaContext {
    /// Construct an exclusive managed-scope context.
    #[must_use]
    pub const fn new(
        scope_id: ManagedScopeId,
        semantic_profile: SemanticProfileId,
        available_capabilities: CapabilitySet,
    ) -> Self {
        Self {
            scope_id,
            semantic_profile,
            available_capabilities,
        }
    }

    /// Return the durable exclusive scope identity.
    #[must_use]
    pub const fn scope_id(&self) -> &ManagedScopeId {
        &self.scope_id
    }

    /// Return the semantic profile used for defaults.
    #[must_use]
    pub const fn semantic_profile(&self) -> &SemanticProfileId {
        &self.semantic_profile
    }

    /// Return the capabilities available to pure resolution.
    #[must_use]
    pub const fn available_capabilities(&self) -> &CapabilitySet {
        &self.available_capabilities
    }
}

/// A structured failure from contract checks or schema resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaError {
    /// A protocol-level delta or replay invariant failed.
    Contract(Diagnostic),
    /// Declared-schema construction or resolution failed.
    Schema(SchemaDiagnostics),
}

impl From<Diagnostic> for DeltaError {
    fn from(value: Diagnostic) -> Self {
        Self::Contract(value)
    }
}

impl From<SchemaDiagnostics> for DeltaError {
    fn from(value: SchemaDiagnostics) -> Self {
        Self::Schema(value)
    }
}

/// Derive the exact managed state from declaration facts and explicit context.
pub fn managed_schema_state(
    declared: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<ManagedSchemaState, DeltaError> {
    declared
        .required_capabilities()
        .ensure_supported_by(context.available_capabilities())?;
    let bound = ManagedSchemaScope::bind_exclusive(context.scope_id.clone(), declared)?;
    let _resolved = resolve_schema_with_capabilities(
        declared,
        context.semantic_profile(),
        context.available_capabilities(),
    )?;
    let selection = ManagedFactSelection::new(bound.selection().iter().cloned())?;
    let declared_fingerprint = managed_declared_identity_fingerprint(declared, &bound)?;
    let semantic_fingerprint =
        managed_semantic_schema_fingerprint(declared, context.semantic_profile(), &bound)?;
    Ok(ManagedSchemaState::new(
        declared.format(),
        declared.required_capabilities().clone(),
        bound.binding().clone(),
        selection,
        declared.declared_identity_fingerprint().clone(),
        declared_fingerprint,
        semantic_fingerprint,
    )?)
}

/// Compute an exact formal managed-schema delta.
pub fn diff_managed(
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<SchemaDelta, DeltaError> {
    let source_state = managed_schema_state(source, context)?;
    let target_state = managed_schema_state(target, context)?;
    let operations = plan_schema_operations(source, target)?;
    Ok(SchemaDelta::new(
        PatchFormatVersion::V1,
        source_state,
        target_state,
        operations,
    )?)
}

/// Verify and apply a delta entirely in memory.
pub fn apply_delta(
    source: &DeclaredSchema,
    delta: &SchemaDelta,
    context: &ManagedDeltaContext,
) -> Result<DeclaredSchema, DeltaError> {
    delta
        .required_capabilities()
        .ensure_supported_by(context.available_capabilities())?;
    let actual_source = managed_schema_state(source, context)?;
    if &actual_source != delta.source() {
        return Err(failure(
            "schema_delta_source_state_mismatch",
            "declared source does not match the delta source state",
        )
        .into());
    }

    let (facts, spans) = replay(source, delta.operations())?;
    let sourced = facts
        .into_iter()
        .map(|(id, fact)| {
            let source = spans.get(&id).cloned().ok_or_else(|| {
                failure(
                    "schema_delta_missing_source_span",
                    format!("replayed fact {id:?} has no ephemeral source span"),
                )
            })?;
            Ok(SourcedSchemaFact::new(fact, source))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let target = DeclaredSchema::from_facts(
        delta.target().format(),
        delta.target().required_capabilities().clone(),
        sourced,
    )?;
    let actual_target = managed_schema_state(&target, context)?;
    if &actual_target != delta.target() {
        return Err(failure(
            "schema_delta_target_state_mismatch",
            "replayed declaration does not match the delta target state",
        )
        .into());
    }
    Ok(target)
}

/// Reverse operation order, invert every operation, and swap exact states.
pub fn inverse_delta(delta: &SchemaDelta) -> Result<SchemaDelta, DeltaError> {
    let operations = delta
        .operations()
        .iter()
        .rev()
        .flat_map(SchemaOperation::inverse)
        .collect();
    Ok(SchemaDelta::new(
        delta.format(),
        delta.target().clone(),
        delta.source().clone(),
        operations,
    )?)
}

/// Replayed fact state paired with each fact's originating source span.
type ReplayedFacts = (
    BTreeMap<SchemaFactId, SchemaFact>,
    BTreeMap<SchemaFactId, SourceSpan>,
);

fn replay(
    source: &DeclaredSchema,
    operations: &[SchemaOperation],
) -> Result<ReplayedFacts, DeltaError> {
    let mut facts = BTreeMap::new();
    let mut spans = BTreeMap::new();
    for fact in source.facts() {
        let id = fact.id();
        let span = source.source(&id).cloned().ok_or_else(|| {
            failure(
                "schema_delta_missing_source_span",
                format!("source fact {id:?} has no source span"),
            )
        })?;
        facts.insert(id.clone(), fact.clone());
        spans.insert(id, span);
    }

    for (operation_index, operation) in operations.iter().enumerate() {
        reject_opaque_function(operation)?;
        match operation.kind() {
            SchemaOperationKind::Define => {
                let definitions = operation
                    .defined_facts()
                    .expect("define operation exposes definitions");
                for fact in definitions {
                    let id = fact.id();
                    if facts.contains_key(&id) {
                        return Err(failure(
                            "schema_delta_define_collision",
                            format!("define collides with existing fact {id:?}"),
                        )
                        .into());
                    }
                }
                for (fact_index, fact) in definitions.iter().enumerate() {
                    let id = fact.id();
                    facts.insert(id.clone(), fact.clone());
                    spans.insert(id, synthetic_span(operation_index, fact_index)?);
                }
                validate_inventory(&facts)?;
            }
            SchemaOperationKind::Redefine => {
                let expected = operation
                    .expected_fact()
                    .expect("redefine operation exposes expected fact");
                let replacement = operation
                    .replacement_fact()
                    .expect("redefine operation exposes replacement fact");
                let id = expected.id();
                if facts.get(&id) != Some(expected) {
                    return Err(failure(
                        "schema_delta_expected_fact_mismatch",
                        format!("redefine expected fact does not match source at {id:?}"),
                    )
                    .into());
                }
                facts.insert(id.clone(), replacement.clone());
                spans.insert(id, synthetic_span(operation_index, 0)?);
                validate_inventory(&facts)?;
            }
            SchemaOperationKind::Undefine => {
                let expected = operation
                    .undefined_fact()
                    .expect("undefine operation exposes expected fact");
                let id = expected.id();
                if facts.get(&id) != Some(expected) {
                    return Err(failure(
                        "schema_delta_expected_fact_mismatch",
                        format!("undefine expected fact does not match source at {id:?}"),
                    )
                    .into());
                }
                let graph = FactDependencyGraph::from_facts(facts.values())?;
                if let Some(dependents) = graph.dependents(&id)
                    && let Some(survivor) = dependents
                        .iter()
                        .find(|dependent| facts.contains_key(*dependent))
                {
                    return Err(failure(
                        "schema_delta_survivor_dependency",
                        format!("cannot undefine {id:?} while dependent {survivor:?} survives"),
                    )
                    .into());
                }
                facts.remove(&id);
                spans.remove(&id);
            }
        }
    }
    validate_inventory(&facts)?;
    Ok((facts, spans))
}

fn validate_inventory(facts: &BTreeMap<SchemaFactId, SchemaFact>) -> Result<(), DeltaError> {
    let graph = FactDependencyGraph::from_facts(facts.values())?;
    graph.validate_complete()?;
    Ok(())
}

fn reject_opaque_function(operation: &SchemaOperation) -> Result<(), DeltaError> {
    let contains_function = operation
        .defined_facts()
        .into_iter()
        .flatten()
        .chain(operation.expected_fact())
        .chain(operation.replacement_fact())
        .chain(operation.undefined_fact())
        .any(|fact| matches!(fact, SchemaFact::Function(_)));
    if contains_function {
        return Err(failure(
            "unsupported_function_migration",
            "automatic migration of opaque function bodies is unsupported",
        )
        .into());
    }
    Ok(())
}

fn synthetic_span(operation_index: usize, fact_index: usize) -> Result<SourceSpan, Diagnostic> {
    let ordinal = operation_index
        .checked_mul(1_000)
        .and_then(|value| value.checked_add(fact_index))
        .ok_or_else(|| {
            failure(
                "schema_delta_span_overflow",
                "synthetic span ordinal overflow",
            )
        })?;
    let byte_start = u64::try_from(ordinal)
        .map_err(|_| failure("schema_delta_span_overflow", "synthetic span byte overflow"))?;
    let line = u32::try_from(ordinal + 1)
        .map_err(|_| failure("schema_delta_span_overflow", "synthetic span line overflow"))?;
    SourceSpan::new(
        DocumentId::new("typebridge.schema-delta.synthetic")?,
        byte_start,
        byte_start + 1,
        line,
        1,
        line,
        2,
    )
}

fn failure(code: &'static str, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new(code).expect("static diagnostic code is canonical"),
        message,
    )
}

#[allow(dead_code)]
fn no_source(diagnostic: Diagnostic) -> SchemaDiagnostics {
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None))
}
