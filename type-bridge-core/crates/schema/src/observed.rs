//! Pure canonicalization of already-captured provider introspection.

use std::collections::BTreeMap;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::schema::{
    AnnotationKindId, DeclaredIdentityFingerprint, DeclaredSchema, DocumentId, SchemaDiagnostic,
    SchemaAnnotationValue, SchemaDiagnostics, SchemaFact, SchemaFactId, SourceSpan,
    SourcedSchemaFact,
};
use type_bridge_contract::semantic_profile::{InterfaceKind, SemanticProfile};

use crate::ManagedSchemaScope;

/// Version of the provider-introspection-to-direct-facts canonicalization policy.
pub const OBSERVED_SCHEMA_CANONICALIZATION_VERSION: &str = "typebridge.observed-schema/v1";

/// Provider evidence describing why an introspected fact is visible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservedFactProvenance {
    /// The fact was declared directly on its observed subject.
    Direct,
    /// The fact is an effective projection of another direct declaration.
    Inherited {
        /// Stable identity of the declaration from which the fact was inherited.
        declared_fact: SchemaFactId,
    },
    /// The provider synthesized an omitted default rather than observing a declaration.
    ServerDefault,
    /// The provider could not distinguish direct, inherited, and synthesized state.
    Ambiguous,
}

/// Deployment ownership assigned while capturing one introspected fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservedFactScope {
    /// The direct fact belongs to the deployment's managed schema scope.
    Managed,
    /// The direct fact is TypeBridge-owned runtime infrastructure, not user schema.
    TypeBridgeInternal,
}

/// One validated contract fact paired with provider provenance and deployment scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedSchemaFact {
    fact: SchemaFact,
    provenance: ObservedFactProvenance,
    scope: ObservedFactScope,
}

impl ObservedSchemaFact {
    /// Capture one provider fact without interpreting its provenance yet.
    pub const fn new(
        fact: SchemaFact,
        provenance: ObservedFactProvenance,
        scope: ObservedFactScope,
    ) -> Self {
        Self {
            fact,
            provenance,
            scope,
        }
    }

    /// Return the captured contract fact.
    pub const fn fact(&self) -> &SchemaFact {
        &self.fact
    }

    /// Return the provider provenance classification.
    pub const fn provenance(&self) -> &ObservedFactProvenance {
        &self.provenance
    }

    /// Return the deployment ownership classification.
    pub const fn scope(&self) -> ObservedFactScope {
        self.scope
    }
}

/// An immutable provider-introspection capture with no server or network behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedSchema {
    format: FormatVersion,
    required_capabilities: CapabilitySet,
    facts: Vec<ObservedSchemaFact>,
}

impl ObservedSchema {
    /// Construct a raw capture for later fail-closed canonicalization.
    #[must_use]
    pub fn new(
        format: FormatVersion,
        required_capabilities: CapabilitySet,
        facts: impl IntoIterator<Item = ObservedSchemaFact>,
    ) -> Self {
        Self {
            format,
            required_capabilities,
            facts: facts.into_iter().collect(),
        }
    }

    /// Return the owning schema format version.
    pub const fn format(&self) -> FormatVersion {
        self.format
    }

    /// Return capabilities required by the captured schema.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Iterate captured facts in provider capture order.
    pub fn facts(&self) -> impl ExactSizeIterator<Item = &ObservedSchemaFact> {
        self.facts.iter()
    }
}

/// Comparable direct schema and managed scope reconstructed from introspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalObservedSchema {
    direct_schema: DeclaredSchema,
    managed_scope: ManagedSchemaScope,
    semantic_profile: SemanticProfileId,
}

impl CanonicalObservedSchema {
    /// Return the exact observed canonicalization policy version.
    pub const fn canonicalization_version(&self) -> &'static str {
        OBSERVED_SCHEMA_CANONICALIZATION_VERSION
    }

    /// Return the reconstructed direct schema.
    ///
    /// Its source spans identify synthetic observed input because provider
    /// introspection has no source-document coordinates.
    pub const fn direct_schema(&self) -> &DeclaredSchema {
        &self.direct_schema
    }

    /// Return the reconstructed managed direct-fact scope.
    pub const fn managed_scope(&self) -> &ManagedSchemaScope {
        &self.managed_scope
    }

    /// Return the profile used to classify synthesized provider defaults.
    pub const fn semantic_profile(&self) -> &SemanticProfileId {
        &self.semantic_profile
    }

    /// Return canonical direct-identity bytes comparable with authored schemas.
    pub fn canonical_identity_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        self.direct_schema.canonical_identity_bytes()
    }

    /// Return the authored-schema-compatible direct-identity fingerprint.
    pub const fn declared_identity_fingerprint(&self) -> &DeclaredIdentityFingerprint {
        self.direct_schema.declared_identity_fingerprint()
    }
}

/// Canonicalize one captured provider schema without performing provider I/O.
///
/// Direct facts are retained, effective inherited facts and proven synthesized
/// cardinality defaults are removed, and any ambiguous provenance fails closed.
pub fn canonicalize_observed_schema(
    observed: &ObservedSchema,
    semantic_profile: &SemanticProfile,
) -> Result<CanonicalObservedSchema, SchemaDiagnostics> {
    let mut captures = BTreeMap::<SchemaFactId, Vec<&ObservedSchemaFact>>::new();
    for captured in observed.facts() {
        captures
            .entry(captured.fact().id())
            .or_default()
            .push(captured);
    }

    let mut diagnostics = Vec::new();
    let mut direct = BTreeMap::<SchemaFactId, (&SchemaFact, ObservedFactScope)>::new();
    let mut inherited_origins = Vec::<SchemaFactId>::new();
    for (id, entries) in captures {
        if entries.len() != 1 {
            diagnostics.push(observed_diagnostic(
                "duplicate_observed_fact",
                "provider introspection returned one fact identity more than once",
            ));
            continue;
        }

        let captured = entries[0];
        match captured.provenance() {
            ObservedFactProvenance::Direct => {
                direct.insert(id, (captured.fact(), captured.scope()));
            }
            ObservedFactProvenance::Inherited { declared_fact } => {
                inherited_origins.push(declared_fact.clone());
            }
            ObservedFactProvenance::ServerDefault => {
                if !is_server_default_cardinality(captured.fact(), semantic_profile) {
                    diagnostics.push(observed_diagnostic(
                        "invalid_observed_server_default",
                        "a server default must exactly match the selected profile's omitted interface cardinality",
                    ));
                }
            }
            ObservedFactProvenance::Ambiguous => diagnostics.push(observed_diagnostic(
                "ambiguous_observed_provenance",
                "direct, inherited, and synthesized provenance could not be recovered unambiguously",
            )),
        }
    }

    for declared_fact in inherited_origins {
        if !direct.contains_key(&declared_fact) {
            diagnostics.push(observed_diagnostic(
                "invalid_observed_inheritance_origin",
                "an inherited fact must identify an existing direct declaration",
            ));
        }
    }

    if !diagnostics.is_empty() {
        return Err(SchemaDiagnostics::from_vec(diagnostics));
    }

    let synthetic_source = synthetic_observed_source()?;
    let managed_scope = ManagedSchemaScope::new(
        direct
            .iter()
            .filter(|&(_, (_, scope))| *scope == ObservedFactScope::Managed)
            .map(|(id, _)| id.clone()),
    );
    let direct_schema = DeclaredSchema::from_facts(
        observed.format(),
        observed.required_capabilities().clone(),
        direct.into_values().map(|(fact, _)| {
            SourcedSchemaFact::new(fact.clone(), synthetic_source.clone())
        }),
    )?;

    Ok(CanonicalObservedSchema {
        direct_schema,
        managed_scope,
        semantic_profile: semantic_profile.id().clone(),
    })
}

fn is_server_default_cardinality(fact: &SchemaFact, profile: &SemanticProfile) -> bool {
    let SchemaFact::Annotation(annotation) = fact else {
        return false;
    };
    if annotation.id().kind() != &AnnotationKindId::Card {
        return false;
    }
    let kind = match annotation.id().subject() {
        type_bridge_contract::schema::AnnotationSubjectId::Owns(_) => InterfaceKind::Owns,
        type_bridge_contract::schema::AnnotationSubjectId::Relates(_) => InterfaceKind::Relates,
        type_bridge_contract::schema::AnnotationSubjectId::Plays(_) => InterfaceKind::Plays,
        _ => return false,
    };
    matches!(
        annotation.value(),
        SchemaAnnotationValue::Cardinality(cardinality)
            if *cardinality == profile.default_cardinality(kind)
    )
}

fn synthetic_observed_source() -> Result<SourceSpan, SchemaDiagnostics> {
    let document = DocumentId::new("observed.schema").map_err(schema_diagnostics)?;
    SourceSpan::new(document, 0, 1, 1, 1, 1, 2).map_err(schema_diagnostics)
}

fn observed_diagnostic(code: &'static str, message: &'static str) -> SchemaDiagnostic {
    SchemaDiagnostic::new(
        Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new(code).expect("static observed-schema diagnostic code is valid"),
            message,
        ),
        None,
    )
}

fn schema_diagnostics(diagnostic: Diagnostic) -> SchemaDiagnostics {
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None))
}
