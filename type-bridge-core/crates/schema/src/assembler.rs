use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::{Label, RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    DeclaredSchema, PlaysFact, PlaysFactId, RelatesFact, RelatesFactId, SchemaDiagnostic,
    SchemaDiagnostics, SchemaFact, SchemaFactId, SourceSpan, SourcedSchemaFact, StructFact,
};

use crate::diagnostic::{diagnostic, diagnostic_with_related};

struct PendingRelates {
    id: RelatesFactId,
    specializes: Option<(Label, SourceSpan)>,
    source: SourceSpan,
}

struct PendingPlays {
    player: Label,
    relation: Label,
    role: Label,
    source: SourceSpan,
}

/// Source-language-neutral construction of direct schema facts.
///
/// Parsers provide validated contract identities and source spans. The assembler
/// owns duplicate detection and the forward-reference resolution needed to mint
/// stable role and playing identities consistently across source languages.
pub struct FactAssembler {
    format: FormatVersion,
    facts: Vec<SourcedSchemaFact>,
    fact_sources: BTreeMap<SchemaFactId, SourceSpan>,
    capabilities: CapabilitySet,
    capability_sources: BTreeMap<CapabilityId, SourceSpan>,
    declaration_labels: BTreeMap<Label, (SchemaFactId, SourceSpan)>,
    type_labels: BTreeMap<Label, (TypeId, SourceSpan)>,
    direct_parents: BTreeMap<TypeId, (TypeId, SourceSpan)>,
    pending_relates: Vec<PendingRelates>,
    pending_relates_sources: BTreeMap<RelatesFactId, SourceSpan>,
    pending_plays: Vec<PendingPlays>,
}

impl FactAssembler {
    /// Start an assembler for one canonical fact-format version.
    #[must_use]
    pub fn new(format: FormatVersion) -> Self {
        Self {
            format,
            facts: Vec::new(),
            fact_sources: BTreeMap::new(),
            capabilities: CapabilitySet::new(),
            capability_sources: BTreeMap::new(),
            declaration_labels: BTreeMap::new(),
            type_labels: BTreeMap::new(),
            direct_parents: BTreeMap::new(),
            pending_relates: Vec::new(),
            pending_relates_sources: BTreeMap::new(),
            pending_plays: Vec::new(),
        }
    }

    /// Add one required capability, retaining duplicate provenance.
    pub fn require_capability(
        &mut self,
        capability: CapabilityId,
        source: SourceSpan,
    ) -> Result<(), SchemaDiagnostics> {
        if let Some(previous) = self.capability_sources.get(&capability) {
            return Err(diagnostic_with_related(
                DiagnosticCategory::InvalidContract,
                "duplicate_required_capability",
                format!("required capability `{capability}` is declared more than once"),
                source,
                previous.clone(),
                "first capability requirement is here",
            ));
        }
        self.capability_sources.insert(capability.clone(), source);
        self.capabilities.insert(capability);
        Ok(())
    }

    /// Insert one fully identified direct fact.
    pub fn insert_fact(
        &mut self,
        fact: SchemaFact,
        source: SourceSpan,
    ) -> Result<(), SchemaDiagnostics> {
        let id = fact.id();
        if let Some(previous) = self.fact_sources.get(&id) {
            return Err(diagnostic_with_related(
                DiagnosticCategory::InvalidContract,
                "duplicate_schema_fact",
                "a direct schema fact is declared more than once",
                source,
                previous.clone(),
                "first declaration is here",
            ));
        }

        let declaration_label = match &fact {
            SchemaFact::Type(type_fact) => Some(type_fact.id().label()),
            SchemaFact::Struct(struct_fact) => Some(struct_fact.id().label()),
            _ => None,
        };
        if let Some(label) = declaration_label {
            if let Some((previous_id, previous_source)) = self.declaration_labels.get(label) {
                let (code, message, related_message) = if matches!(
                    (&id, previous_id),
                    (SchemaFactId::Type(_), SchemaFactId::Type(_))
                ) {
                    (
                        "duplicate_schema_type_label",
                        format!("schema label `{label}` is declared with more than one type kind"),
                        "first type declaration is here",
                    )
                } else {
                    (
                        "duplicate_schema_label",
                        format!("schema label `{label}` is declared as both a type and a struct"),
                        "first type or struct declaration is here",
                    )
                };
                return Err(diagnostic_with_related(
                    DiagnosticCategory::InvalidContract,
                    code,
                    message,
                    source,
                    previous_source.clone(),
                    related_message,
                ));
            }
            self.declaration_labels
                .insert(label.clone(), (id.clone(), source.clone()));
        }

        if let SchemaFact::Type(type_fact) = &fact {
            let type_id = type_fact.id();
            self.type_labels
                .insert(type_id.label().clone(), (type_id.clone(), source.clone()));
        }

        if let SchemaFact::Sub(sub_fact) = &fact {
            let subtype = sub_fact.id().subtype();
            let supertype = sub_fact.id().supertype();
            if let Some((previous_parent, previous_source)) = self.direct_parents.get(subtype) {
                if previous_parent != supertype {
                    return Err(diagnostic_with_related(
                        DiagnosticCategory::InvalidContract,
                        "multiple_direct_schema_parents",
                        format!(
                            "schema type `{}` has more than one direct parent",
                            subtype.label()
                        ),
                        source,
                        previous_source.clone(),
                        "first direct parent is declared here",
                    ));
                }
            } else {
                self.direct_parents
                    .insert(subtype.clone(), (supertype.clone(), source.clone()));
            }
        }

        self.fact_sources.insert(id, source.clone());
        self.facts.push(SourcedSchemaFact::new(fact, source));
        Ok(())
    }

    /// Insert one struct declaration through the shared declaration namespace.
    pub fn insert_struct(
        &mut self,
        fact: StructFact,
        source: SourceSpan,
    ) -> Result<(), SchemaDiagnostics> {
        self.insert_fact(SchemaFact::Struct(fact), source)
    }

    /// Queue a related-role declaration whose specialization may target an ancestor.
    pub fn insert_relates(
        &mut self,
        id: RelatesFactId,
        specializes: Option<(Label, SourceSpan)>,
        source: SourceSpan,
    ) -> Result<(), SchemaDiagnostics> {
        if let Some(previous) = self.pending_relates_sources.get(&id) {
            return Err(diagnostic_with_related(
                DiagnosticCategory::InvalidContract,
                "duplicate_schema_fact",
                "a direct related-role fact is declared more than once",
                source,
                previous.clone(),
                "first declaration is here",
            ));
        }
        if let Some(previous) = self.fact_sources.get(&SchemaFactId::Relates(id.clone())) {
            return Err(diagnostic_with_related(
                DiagnosticCategory::InvalidContract,
                "duplicate_schema_fact",
                "a direct related-role fact is declared more than once",
                source,
                previous.clone(),
                "first declaration is here",
            ));
        }
        self.pending_relates_sources
            .insert(id.clone(), source.clone());
        self.pending_relates.push(PendingRelates {
            id,
            specializes,
            source,
        });
        Ok(())
    }

    /// Queue a player-keyed playing declaration for forward resolution.
    pub fn insert_plays(
        &mut self,
        player: Label,
        relation: Label,
        role: Label,
        source: SourceSpan,
    ) {
        self.pending_plays.push(PendingPlays {
            player,
            relation,
            role,
            source,
        });
    }

    /// Materialize deferred identities and construct the validated declared graph.
    pub fn finish(mut self) -> Result<DeclaredSchema, SchemaDiagnostics> {
        self.materialize_relates()?;
        self.materialize_plays()?;
        DeclaredSchema::from_facts(self.format, self.capabilities, self.facts)
    }

    fn materialize_relates(&mut self) -> Result<(), SchemaDiagnostics> {
        let declarations = std::mem::take(&mut self.pending_relates);
        for declaration in &declarations {
            let specializes = declaration
                .specializes
                .as_ref()
                .map(|(label, source)| {
                    self.resolve_inherited_role(
                        declaration.id.relation(),
                        label,
                        source,
                        &declarations,
                    )
                })
                .transpose()?;
            let fact = RelatesFact::new(declaration.id.clone(), specializes)
                .map_err(|error| contract(error, declaration.source.clone()))?;
            self.insert_fact(SchemaFact::Relates(fact), declaration.source.clone())?;
        }
        Ok(())
    }

    fn materialize_plays(&mut self) -> Result<(), SchemaDiagnostics> {
        let declarations = std::mem::take(&mut self.pending_plays);
        for declaration in declarations {
            let player = self
                .type_labels
                .get(&declaration.player)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| {
                    diagnostic(
                        DiagnosticCategory::InvalidContract,
                        "unknown_schema_player",
                        format!("unknown playing type `{}`", declaration.player),
                        Some(declaration.source.clone()),
                    )
                })?;
            if !matches!(player.kind(), TypeKind::Entity | TypeKind::Relation) {
                return Err(diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "invalid_schema_player_kind",
                    "only entity and relation types can play roles",
                    Some(declaration.source),
                ));
            }
            let relation = self
                .type_labels
                .get(&declaration.relation)
                .map(|(id, _)| id.clone())
                .ok_or_else(|| {
                    diagnostic(
                        DiagnosticCategory::InvalidContract,
                        "unknown_schema_relation",
                        format!("unknown relation type `{}`", declaration.relation),
                        Some(declaration.source.clone()),
                    )
                })?;
            if relation.kind() != TypeKind::Relation {
                return Err(diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "invalid_schema_relation_kind",
                    "a playing relation reference must name a relation type",
                    Some(declaration.source),
                ));
            }
            let role = RoleId::new(relation.label().as_str(), declaration.role.as_str())
                .map_err(|error| contract(error, declaration.source.clone()))?;
            let id = PlaysFactId::new(player, role)
                .map_err(|error| contract(error, declaration.source.clone()))?;
            self.insert_fact(SchemaFact::Plays(PlaysFact::new(id)), declaration.source)?;
        }
        Ok(())
    }

    fn resolve_inherited_role(
        &self,
        relation: &TypeId,
        role_label: &Label,
        source: &SourceSpan,
        declarations: &[PendingRelates],
    ) -> Result<RoleId, SchemaDiagnostics> {
        let mut current = relation.clone();
        let mut visited = BTreeSet::new();
        loop {
            if !visited.insert(current.clone()) {
                return Err(diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "schema_inheritance_cycle",
                    "relation inheritance contains a cycle",
                    Some(source.clone()),
                ));
            }
            let Some((parent, _)) = self.direct_parents.get(&current) else {
                return Err(diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "invalid_role_specialization",
                    "specialized role is not declared by any ancestor relation",
                    Some(source.clone()),
                ));
            };
            if let Some(role) = declarations.iter().find_map(|candidate| {
                (candidate.id.relation() == parent && candidate.id.role().label() == role_label)
                    .then(|| candidate.id.role().clone())
            }) {
                return Ok(role);
            }
            if let Some(role) = self.facts.iter().find_map(|sourced| {
                let SchemaFact::Relates(fact) = sourced.fact() else {
                    return None;
                };
                (fact.id().relation() == parent && fact.id().role().label() == role_label)
                    .then(|| fact.id().role().clone())
            }) {
                return Ok(role);
            }
            current = parent.clone();
        }
    }
}

fn contract(error: Diagnostic, source: SourceSpan) -> SchemaDiagnostics {
    SchemaDiagnostics::one(SchemaDiagnostic::new(error, Some(source)))
}
