use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::{
    ManagedScopeBinding, ManagedScopeId, SemanticProfileFingerprint,
};
use type_bridge_contract::schema::{
    AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema, InterfaceKind,
    ManagedDeclaredIdentityFingerprint, ManagedSemanticSchemaFingerprint, SchemaAnnotationValue,
    SchemaDiagnostic, SchemaDiagnostics, SchemaFact, SchemaFactId, SemanticProfile,
    SemanticSchemaFingerprint,
};
use type_bridge_contract::value::Cardinality;

use crate::TYPEDB_3_12_1_TIMEZONE_POLICY_ID;

/// Unbound evidence selecting direct fact identities observed as managed.
///
/// Selection-only evidence cannot enter persisted managed fingerprints until
/// an owning schema derives an exclusive bound scope from all declared facts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ManagedSchemaScope {
    facts: BTreeSet<SchemaFactId>,
}

impl ManagedSchemaScope {
    /// Create a deterministic managed scope from direct fact identities.
    #[must_use]
    pub fn new(facts: impl IntoIterator<Item = SchemaFactId>) -> Self {
        Self {
            facts: facts.into_iter().collect(),
        }
    }

    /// Derive a complete exclusive managed scope from every direct declared fact.
    pub fn bind_exclusive(
        scope_id: ManagedScopeId,
        declared: &DeclaredSchema,
    ) -> Result<BoundManagedSchemaScope, SchemaDiagnostics> {
        let binding = ManagedScopeBinding::exclusive(scope_id).map_err(|diagnostic| {
            SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None))
        })?;
        Ok(Self::new(declared.facts().map(SchemaFact::id)).bind(binding))
    }

    fn bind(self, binding: ManagedScopeBinding) -> BoundManagedSchemaScope {
        BoundManagedSchemaScope {
            selection: self,
            binding,
        }
    }

    /// Report whether a direct fact identity is managed.
    #[must_use]
    pub fn contains(&self, fact: &SchemaFactId) -> bool {
        self.facts.contains(fact)
    }

    /// Iterate managed identities in stable order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &SchemaFactId> {
        self.facts.iter()
    }

    /// Return the number of managed identities.
    #[must_use]
    pub fn len(&self) -> usize {
        self.facts.len()
    }

    /// Report whether the scope manages no facts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }
}

/// A managed fact selection with an explicit durable deployment/profile binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundManagedSchemaScope {
    selection: ManagedSchemaScope,
    binding: ManagedScopeBinding,
}

impl BoundManagedSchemaScope {
    /// Return the deterministic managed fact selection.
    pub const fn selection(&self) -> &ManagedSchemaScope {
        &self.selection
    }

    /// Return the durable scope/profile binding.
    pub const fn binding(&self) -> &ManagedScopeBinding {
        &self.binding
    }
}

#[derive(Serialize)]
struct SemanticSchemaView<'a> {
    format_version: FormatVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    managed_scope: Option<&'a ManagedScopeBinding>,
    semantic_profile: &'a SemanticProfileId,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_profile_fingerprint: Option<&'a SemanticProfileFingerprint>,
    timezone_policy: &'static str,
    required_capabilities: &'a CapabilitySet,
    facts: Vec<SemanticFactView<'a>>,
}

#[derive(Serialize)]
struct SemanticFactView<'a> {
    id: SchemaFactId,
    value: SemanticFactValue<'a>,
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum SemanticFactValue<'a> {
    Direct(&'a SchemaFact),
    TimeZoneRange {
        subject: AnnotationSubjectId,
        lower_utc_nanoseconds: Option<String>,
        upper_utc_nanoseconds: Option<String>,
    },
    TimeZoneValues {
        subject: AnnotationSubjectId,
        utc_nanoseconds: Vec<String>,
    },
    MaterializedCardinality {
        subject: AnnotationSubjectId,
        cardinality: Cardinality,
    },
}

#[derive(Serialize)]
struct ManagedDeclaredView<'a> {
    format_version: FormatVersion,
    managed_scope: &'a ManagedScopeBinding,
    required_capabilities: &'a CapabilitySet,
    facts: Vec<&'a SchemaFact>,
}

/// Return canonical direct-semantic bytes with equal explicit defaults normalized.
pub fn canonical_semantic_schema_bytes(
    declared: &DeclaredSchema,
    profile: &SemanticProfileId,
) -> Result<Vec<u8>, SchemaDiagnostics> {
    canonical_semantic_schema_bytes_for_scope(declared, profile, None)
}

/// Fingerprint canonical direct semantics without hashing inherited resolver closure.
pub fn semantic_schema_fingerprint(
    declared: &DeclaredSchema,
    profile: &SemanticProfileId,
) -> Result<SemanticSchemaFingerprint, SchemaDiagnostics> {
    let canonical = canonical_semantic_schema_bytes(declared, profile)?;
    SemanticSchemaFingerprint::compute(profile.clone(), &canonical)
        .map_err(|diagnostic| SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None)))
}

/// Fingerprint declared identity after explicit managed-scope filtering.
pub fn managed_declared_identity_fingerprint(
    declared: &DeclaredSchema,
    scope: &BoundManagedSchemaScope,
) -> Result<ManagedDeclaredIdentityFingerprint, SchemaDiagnostics> {
    let canonical = canonical_managed_declared_identity_bytes(declared, scope)?;
    ManagedDeclaredIdentityFingerprint::compute(&canonical)
        .map_err(|diagnostic| SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None)))
}

/// Return canonical declared-identity bytes for an explicitly bound managed scope.
pub fn canonical_managed_declared_identity_bytes(
    declared: &DeclaredSchema,
    scope: &BoundManagedSchemaScope,
) -> Result<Vec<u8>, SchemaDiagnostics> {
    let view = ManagedDeclaredView {
        format_version: declared.format(),
        managed_scope: scope.binding(),
        required_capabilities: declared.required_capabilities(),
        facts: declared
            .facts()
            .filter(|fact| scope.selection().contains(&fact.id()))
            .collect(),
    };
    to_canonical_json(&view)
        .map_err(|diagnostic| SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None)))
}

/// Fingerprint direct semantics after explicit managed-scope filtering.
pub fn managed_semantic_schema_fingerprint(
    declared: &DeclaredSchema,
    profile: &SemanticProfileId,
    scope: &BoundManagedSchemaScope,
) -> Result<ManagedSemanticSchemaFingerprint, SchemaDiagnostics> {
    let canonical = canonical_managed_semantic_schema_bytes(declared, profile, scope)?;
    ManagedSemanticSchemaFingerprint::compute(profile.clone(), &canonical)
        .map_err(|diagnostic| SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None)))
}

/// Return canonical direct-semantic bytes for an explicitly bound managed scope.
pub fn canonical_managed_semantic_schema_bytes(
    declared: &DeclaredSchema,
    profile: &SemanticProfileId,
    scope: &BoundManagedSchemaScope,
) -> Result<Vec<u8>, SchemaDiagnostics> {
    canonical_semantic_schema_bytes_for_scope(declared, profile, Some(scope))
}

fn canonical_semantic_schema_bytes_for_scope(
    declared: &DeclaredSchema,
    profile: &SemanticProfileId,
    scope: Option<&BoundManagedSchemaScope>,
) -> Result<Vec<u8>, SchemaDiagnostics> {
    let semantic_profile = SemanticProfile::resolve(profile)
        .map_err(|diagnostic| SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None)))?;
    let semantic_profile_fingerprint = scope
        .map(|_| semantic_profile.content_fingerprint())
        .transpose()
        .map_err(|diagnostic| SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None)))?;
    let facts = semantic_facts(
        declared,
        scope.map(BoundManagedSchemaScope::selection),
        &semantic_profile,
    );
    let view = SemanticSchemaView {
        format_version: declared.format(),
        managed_scope: scope.map(BoundManagedSchemaScope::binding),
        semantic_profile: profile,
        semantic_profile_fingerprint: semantic_profile_fingerprint.as_ref(),
        timezone_policy: TYPEDB_3_12_1_TIMEZONE_POLICY_ID,
        required_capabilities: declared.required_capabilities(),
        facts,
    };
    to_canonical_json(&view)
        .map_err(|diagnostic| SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None)))
}

fn semantic_facts<'a>(
    declared: &'a DeclaredSchema,
    scope: Option<&ManagedSchemaScope>,
    profile: &SemanticProfile,
) -> Vec<SemanticFactView<'a>> {
    let mut facts = BTreeMap::<SchemaFactId, SemanticFactValue<'a>>::new();
    let mut interfaces = Vec::<(AnnotationSubjectId, InterfaceKind)>::new();
    let key_owns = declared
        .facts()
        .filter(|fact| scope.is_none_or(|scope| scope.contains(&fact.id())))
        .filter_map(|fact| match fact {
            SchemaFact::Annotation(annotation)
                if annotation.id().kind() == &AnnotationKindId::Key
                    && matches!(annotation.id().subject(), AnnotationSubjectId::Owns(_)) =>
            {
                Some(annotation.id().subject().clone())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();

    for fact in declared
        .facts()
        .filter(|fact| scope.is_none_or(|scope| scope.contains(&fact.id())))
    {
        let id = fact.id();
        match fact {
            SchemaFact::Owns(owns) => interfaces.push((
                AnnotationSubjectId::Owns(owns.id().clone()),
                InterfaceKind::Owns,
            )),
            SchemaFact::Relates(relates) => interfaces.push((
                AnnotationSubjectId::Relates(relates.id().clone()),
                InterfaceKind::Relates,
            )),
            SchemaFact::Plays(plays) => interfaces.push((
                AnnotationSubjectId::Plays(plays.id().clone()),
                InterfaceKind::Plays,
            )),
            _ => {}
        }

        let value = match fact {
            SchemaFact::Annotation(annotation)
                if annotation.id().kind() == &AnnotationKindId::Card =>
            {
                let Some(kind) = interface_kind(annotation.id().subject()) else {
                    facts.insert(id, SemanticFactValue::Direct(fact));
                    continue;
                };
                match annotation.value() {
                    SchemaAnnotationValue::Cardinality(cardinality)
                        if *cardinality
                            == profile.effective_cardinality(
                                kind,
                                None,
                                key_owns.contains(annotation.id().subject()),
                            ) =>
                    {
                        SemanticFactValue::MaterializedCardinality {
                            subject: annotation.id().subject().clone(),
                            cardinality: profile.effective_cardinality(
                                kind,
                                None,
                                key_owns.contains(annotation.id().subject()),
                            ),
                        }
                    }
                    _ => SemanticFactValue::Direct(fact),
                }
            }
            SchemaFact::Annotation(annotation) => semantic_timezone_value(annotation)
                .unwrap_or(SemanticFactValue::Direct(fact)),
            _ => SemanticFactValue::Direct(fact),
        };
        facts.insert(id, value);
    }

    for (subject, kind) in interfaces {
        let id = SchemaFactId::Annotation(AnnotationFactId::new(
            subject.clone(),
            AnnotationKindId::Card,
        ));
        let cardinality =
            profile.effective_cardinality(kind, None, key_owns.contains(&subject));
        facts
            .entry(id)
            .or_insert_with(|| SemanticFactValue::MaterializedCardinality {
                subject,
                cardinality,
            });
    }

    facts
        .into_iter()
        .map(|(id, value)| SemanticFactView { id, value })
        .collect()
}

fn semantic_timezone_value<'a>(
    annotation: &'a type_bridge_contract::schema::AnnotationFact,
) -> Option<SemanticFactValue<'a>> {
    match annotation.value() {
        SchemaAnnotationValue::Range(range) => {
            let lower = range.lower().and_then(timezone_key);
            let upper = range.upper().and_then(timezone_key);
            (lower.is_some() || upper.is_some()).then(|| SemanticFactValue::TimeZoneRange {
                subject: annotation.id().subject().clone(),
                lower_utc_nanoseconds: lower,
                upper_utc_nanoseconds: upper,
            })
        }
        SchemaAnnotationValue::Values(values)
            if values
                .iter()
                .next()
                .is_some_and(|value| matches!(value, type_bridge_contract::value::CanonicalValue::DateTimeTz(_))) =>
        {
            let mut keys = values.iter().filter_map(timezone_key).collect::<Vec<_>>();
            keys.sort_by(|left, right| {
                left.parse::<i128>()
                    .expect("semantic timezone key is an i128")
                    .cmp(&right.parse::<i128>().expect("semantic timezone key is an i128"))
            });
            Some(SemanticFactValue::TimeZoneValues {
                subject: annotation.id().subject().clone(),
                utc_nanoseconds: keys,
            })
        }
        _ => None,
    }
}

fn timezone_key(value: &type_bridge_contract::value::CanonicalValue) -> Option<String> {
    let type_bridge_contract::value::CanonicalValue::DateTimeTz(value) = value else {
        return None;
    };
    Some(value.semantic_utc_nanoseconds().to_string())
}

fn interface_kind(subject: &AnnotationSubjectId) -> Option<InterfaceKind> {
    match subject {
        AnnotationSubjectId::Owns(_) => Some(InterfaceKind::Owns),
        AnnotationSubjectId::Relates(_) => Some(InterfaceKind::Relates),
        AnnotationSubjectId::Plays(_) => Some(InterfaceKind::Plays),
        _ => None,
    }
}
