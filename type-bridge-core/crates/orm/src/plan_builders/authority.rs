//! The schema authority helper plans are validated against.

use std::collections::BTreeMap;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::projection::RuntimeProjection;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationKindId, AnnotationSubjectId, DeclaredSchema, DocumentId, OwnsFact,
    PlaysFact, RelatesFact, RelatesFactId, SchemaFact, SchemaFactId, SourceSpan, SourcedSchemaFact,
    SubFact, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_schema::VerifiedSchemaAuthority;

/// Declared schema and managed semantics a helper plan carries.
#[derive(Debug)]
pub(crate) struct PlanAuthority {
    declared: DeclaredSchema,
    managed_semantics: ManagedSemanticSchemaFingerprint,
    /// Whether the declarations come from a verified package authority.
    /// Plans built from projection-rebuilt declarations run locally only.
    verified: bool,
}

impl PlanAuthority {
    /// A package's verified authority, used as is.
    pub(crate) fn from_verified(authority: &VerifiedSchemaAuthority) -> Self {
        Self {
            declared: authority.declared_schema().clone(),
            managed_semantics: authority.managed_state().managed_semantic_schema().clone(),
            verified: true,
        }
    }

    /// Rebuild the declarations the plan validator reads from a projection
    /// installed without a verified authority: types, subtypes, value types,
    /// ownerships, roles, plays, and the `@abstract` and `@key` annotations.
    /// Plans are identified by the projection's semantic fingerprint.
    pub(crate) fn from_projection(projection: &RuntimeProjection) -> Result<Self, Diagnostic> {
        let mut facts = BTreeMap::<SchemaFactId, SchemaFact>::new();
        let mut add = |fact: SchemaFact| {
            facts.entry(fact.id()).or_insert(fact);
        };
        let wanted = |kind: &AnnotationKindId| {
            matches!(kind, AnnotationKindId::Abstract | AnnotationKindId::Key)
        };
        for (type_id, model) in projection.models() {
            let declaration = model.declaration();
            add(SchemaFact::Type(TypeFact::new(type_id.clone())?));
            if let Some(sub) = declaration.direct_sub() {
                add(SchemaFact::Sub(SubFact::new(sub.id().clone())));
            }
            if let Some(value_type) = declaration.value_type() {
                let attribute =
                    type_bridge_contract::id::AttributeId::new(type_id.label().as_str())?;
                add(SchemaFact::Value(ValueFact::new(
                    ValueFactId::new(attribute),
                    value_type,
                )));
            }
            for field in declaration.direct_fields() {
                add(SchemaFact::Owns(OwnsFact::new(field.clone())));
            }
            for (role, declared) in declaration.direct_roles() {
                add(SchemaFact::Relates(RelatesFact::new(
                    RelatesFactId::new(type_id.clone(), role.clone())?,
                    declared.specializes().cloned(),
                )?));
            }
            for plays in declaration.direct_plays() {
                add(SchemaFact::Plays(PlaysFact::new(plays.clone())));
            }
            let field_annotations = model
                .query_tokens()
                .fields()
                .values()
                .flat_map(|token| token.annotations().values());
            for annotation in declaration.annotations().values().chain(field_annotations) {
                let declared_here = match annotation.id().subject() {
                    AnnotationSubjectId::Type(subject) => subject == type_id,
                    // Field tokens carry effective ownerships; only the
                    // declaring one is a fact.
                    AnnotationSubjectId::Owns(subject) => {
                        declaration.direct_fields().contains(subject)
                    }
                    _ => false,
                };
                if declared_here && wanted(annotation.id().kind()) {
                    add(SchemaFact::Annotation(AnnotationFact::new(
                        annotation.id().clone(),
                        annotation.value().clone(),
                    )?));
                }
            }
        }
        let document = DocumentId::new("typebridge-projection-plan-authority")?;
        let sourced = facts.into_values().enumerate().map(|(index, fact)| {
            let byte = u64::try_from(index).unwrap_or(u64::MAX - 1);
            let line = u32::try_from(index + 1).unwrap_or(u32::MAX);
            SourceSpan::new(document.clone(), byte, byte + 1, line, 1, line, 2)
                .map(|span| SourcedSchemaFact::new(fact, span))
        });
        let sourced = sourced.collect::<Result<Vec<_>, _>>()?;
        let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
            .map_err(|_| unavailable())?;
        let semantic = projection.semantic_fingerprint().as_fingerprint();
        let profile = semantic
            .semantic_profile()
            .cloned()
            .ok_or_else(unavailable)?;
        let managed_semantics =
            ManagedSemanticSchemaFingerprint::compute(profile, &semantic.digest().bytes())?;
        Ok(Self {
            declared,
            managed_semantics,
            verified: false,
        })
    }

    #[cfg(test)]
    pub(crate) const fn new(
        declared: DeclaredSchema,
        managed_semantics: ManagedSemanticSchemaFingerprint,
    ) -> Self {
        Self {
            declared,
            managed_semantics,
            verified: true,
        }
    }

    #[cfg(test)]
    pub(crate) const fn unverified(mut self) -> Self {
        self.verified = false;
        self
    }

    pub(crate) const fn verified(&self) -> bool {
        self.verified
    }

    pub(crate) const fn declared(&self) -> &DeclaredSchema {
        &self.declared
    }

    pub(crate) const fn managed_semantics(&self) -> &ManagedSemanticSchemaFingerprint {
        &self.managed_semantics
    }
}

fn unavailable() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new("write_plan_authority_unavailable").expect("static diagnostic code"),
        "the installed projection cannot form write-plan schema authority",
    )
}
