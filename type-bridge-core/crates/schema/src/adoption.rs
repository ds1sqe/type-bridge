//! Pure adoption of canonical observed schema as an immutable managed baseline.

use std::collections::BTreeSet;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::schema::{
    DeclaredSchema, ManagedDeclaredIdentityFingerprint, ManagedFactSelection, ManagedSchemaState,
    ManagedScopeId, ManagedSemanticSchemaFingerprint, SchemaDiagnostic, SchemaDiagnostics,
    SchemaFactId, SchemaOperation, SourcedSchemaFact,
};
use type_bridge_contract::semantic_profile::SemanticProfile;

use crate::{
    BoundManagedSchemaScope, CanonicalObservedSchema, ManagedSchemaScope, ResolvedSchema,
    managed_declared_identity_fingerprint, managed_semantic_schema_fingerprint,
    resolve_schema_with_capabilities,
};

/// An immutable, operation-free starting point adopted from canonical observation.
#[derive(Clone, Debug)]
pub struct AdoptionBaseline {
    declared_schema: DeclaredSchema,
    resolved_schema: ResolvedSchema,
    bound_scope: BoundManagedSchemaScope,
    semantic_profile: SemanticProfileId,
    managed_state: ManagedSchemaState,
    operations: Vec<SchemaOperation>,
}

impl AdoptionBaseline {
    /// Return the rebuilt managed-only direct declaration.
    pub const fn declared_schema(&self) -> &DeclaredSchema {
        &self.declared_schema
    }

    /// Return the validated pure resolution of the adopted declaration.
    pub const fn resolved_schema(&self) -> &ResolvedSchema {
        &self.resolved_schema
    }

    /// Return the fixed exclusive scope binding and complete selection.
    pub const fn bound_scope(&self) -> &BoundManagedSchemaScope {
        &self.bound_scope
    }

    /// Return the exact semantic profile used by canonicalization and resolution.
    pub const fn semantic_profile(&self) -> &SemanticProfileId {
        &self.semantic_profile
    }

    /// Return the exact managed declaration and semantic fingerprints with context.
    pub const fn managed_state(&self) -> &ManagedSchemaState {
        &self.managed_state
    }

    /// Return the exact managed declared-identity fingerprint.
    pub const fn managed_declared_identity(&self) -> &ManagedDeclaredIdentityFingerprint {
        self.managed_state.managed_declared_identity()
    }

    /// Return the exact managed semantic-schema fingerprint.
    pub const fn managed_semantic_schema(&self) -> &ManagedSemanticSchemaFingerprint {
        self.managed_state.managed_semantic_schema()
    }

    /// Adoption records a baseline and never authorizes or schedules executable operations.
    pub fn operations(&self) -> &[SchemaOperation] {
        &self.operations
    }
}

/// Adopt canonical observed direct facts as one exclusive managed baseline.
pub fn adopt_observed_schema(
    observed: &CanonicalObservedSchema,
    scope_id: ManagedScopeId,
    semantic_profile: &SemanticProfileId,
    available_capabilities: &CapabilitySet,
) -> Result<AdoptionBaseline, SchemaDiagnostics> {
    SemanticProfile::resolve(semantic_profile).map_err(no_source)?;
    if observed.semantic_profile() != semantic_profile {
        return Err(adoption_failure(
            "adoption_semantic_profile_mismatch",
            "the adoption profile differs from the profile used to canonicalize observation",
        ));
    }
    observed
        .direct_schema()
        .required_capabilities()
        .ensure_supported_by(available_capabilities)
        .map_err(no_source)?;

    let selected: BTreeSet<SchemaFactId> = observed.managed_scope().iter().cloned().collect();
    let mut sourced = Vec::with_capacity(selected.len());
    for id in &selected {
        let fact = observed.direct_schema().fact(id).ok_or_else(|| {
            adoption_failure(
                "adoption_scope_fact_mismatch",
                "the canonical managed selection references an absent direct fact",
            )
        })?;
        let source = observed
            .direct_schema()
            .source(id)
            .cloned()
            .ok_or_else(|| {
                adoption_failure(
                    "adoption_scope_source_mismatch",
                    "the canonical managed selection references a fact without provenance",
                )
            })?;
        sourced.push(SourcedSchemaFact::new(fact.clone(), source));
    }

    let declared_schema = DeclaredSchema::from_facts(
        observed.direct_schema().format(),
        observed.direct_schema().required_capabilities().clone(),
        sourced,
    )?;
    let rebuilt_ids: BTreeSet<_> = declared_schema.facts().map(|fact| fact.id()).collect();
    if rebuilt_ids != selected {
        return Err(adoption_failure(
            "adoption_scope_selection_mismatch",
            "the rebuilt declaration is not exactly the canonical managed direct selection",
        ));
    }

    let bound_scope = ManagedSchemaScope::bind_exclusive(scope_id.clone(), &declared_schema)?;
    if bound_scope.binding().id() != &scope_id || bound_scope.selection().iter().ne(selected.iter())
    {
        return Err(adoption_failure(
            "adoption_exclusive_scope_mismatch",
            "the fixed exclusive scope does not exactly bind the requested direct selection",
        ));
    }

    let resolved_schema = resolve_schema_with_capabilities(
        &declared_schema,
        semantic_profile,
        available_capabilities,
    )?;
    let selection =
        ManagedFactSelection::new(bound_scope.selection().iter().cloned()).map_err(no_source)?;
    let declared_fingerprint =
        managed_declared_identity_fingerprint(&declared_schema, &bound_scope)?;
    let semantic_fingerprint =
        managed_semantic_schema_fingerprint(&declared_schema, semantic_profile, &bound_scope)?;
    let managed_state = ManagedSchemaState::new(
        declared_schema.format(),
        declared_schema.required_capabilities().clone(),
        bound_scope.binding().clone(),
        selection,
        declared_schema.declared_identity_fingerprint().clone(),
        declared_fingerprint,
        semantic_fingerprint,
    )
    .map_err(no_source)?;

    Ok(AdoptionBaseline {
        declared_schema,
        resolved_schema,
        bound_scope,
        semantic_profile: semantic_profile.clone(),
        managed_state,
        operations: Vec::new(),
    })
}

fn adoption_failure(code: &'static str, message: &'static str) -> SchemaDiagnostics {
    no_source(Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static adoption diagnostic code is canonical"),
        message,
    ))
}

fn no_source(diagnostic: Diagnostic) -> SchemaDiagnostics {
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None))
}
