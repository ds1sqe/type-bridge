use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};
use type_bridge_contract::id::{
    AttributeId, FunctionId, RoleId, StructId, TypeId, TypeKind,
};
use type_bridge_contract::schema::{
    AnnotationKindId, AnnotationSubjectId, DeclaredIdentityFingerprint, DeclaredSchema,
    FunctionFact, InterfaceKind, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId, RelatesFact,
    SchemaAnnotationValue, SchemaDiagnostics, SchemaFact, SchemaFactId, SemanticProfile,
    SemanticSchemaFingerprint, StructFact, StructField, SubFactId, ValueFactId,
};
use type_bridge_contract::value::{Cardinality, ValueTypeTag};

use crate::semantic_schema_fingerprint;

/// Direct-fact identity plus the inheritance path used to derive an effective entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolutionOrigin {
    declared: SchemaFactId,
    inheritance_path: Vec<TypeId>,
}

impl ResolutionOrigin {
    fn direct(declared: SchemaFactId) -> Self {
        Self {
            declared,
            inheritance_path: Vec::new(),
        }
    }

    fn inherited(&self, via: TypeId) -> Self {
        let mut origin = self.clone();
        origin.inheritance_path.push(via);
        origin
    }

    /// Return the direct fact from which this entry was derived.
    #[must_use]
    pub const fn declared(&self) -> &SchemaFactId {
        &self.declared
    }

    /// Return the ordered inheritance path; empty means direct.
    #[must_use]
    pub fn inheritance_path(&self) -> &[TypeId] {
        &self.inheritance_path
    }

    /// Report whether the effective entry is direct on its resolved type.
    #[must_use]
    pub fn is_direct(&self) -> bool {
        self.inheritance_path.is_empty()
    }
}

/// Effective ownership, including resolved constraints and direct origin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveOwns {
    id: OwnsFactId,
    origin: ResolutionOrigin,
    annotations: BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
    cardinality: Cardinality,
    key: bool,
    unique: bool,
}

impl EffectiveOwns {
    /// Return the effective ownership identity for the resolved owner.
    #[must_use]
    pub const fn id(&self) -> &OwnsFactId {
        &self.id
    }

    /// Return its direct declaration origin.
    #[must_use]
    pub const fn origin(&self) -> &ResolutionOrigin {
        &self.origin
    }

    /// Return all direct or inherited annotations keyed by independent kind.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationKindId, SchemaAnnotationValue> {
        &self.annotations
    }

    /// Return explicit or profile-materialized cardinality.
    #[must_use]
    pub const fn cardinality(&self) -> Cardinality {
        self.cardinality
    }

    /// Report key semantics.
    #[must_use]
    pub const fn is_key(&self) -> bool {
        self.key
    }

    /// Report uniqueness semantics independent of key.
    #[must_use]
    pub const fn is_unique(&self) -> bool {
        self.unique
    }
}

/// Effective role-playing interface and its independent annotations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectivePlays {
    id: PlaysFactId,
    origin: ResolutionOrigin,
    annotations: BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
    cardinality: Cardinality,
}

impl EffectivePlays {
    /// Return the effective playing identity.
    #[must_use]
    pub const fn id(&self) -> &PlaysFactId {
        &self.id
    }

    /// Return its direct declaration origin.
    #[must_use]
    pub const fn origin(&self) -> &ResolutionOrigin {
        &self.origin
    }

    /// Return independent annotations for this exact player-role fact.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationKindId, SchemaAnnotationValue> {
        &self.annotations
    }

    /// Return explicit or profile-materialized cardinality.
    #[must_use]
    pub const fn cardinality(&self) -> Cardinality {
        self.cardinality
    }
}

/// Derived relation-role identity that can represent an inherited role interface.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct EffectiveRelatesId {
    relation: TypeId,
    role: RoleId,
}

impl EffectiveRelatesId {
    fn new(relation: TypeId, role: RoleId) -> Self {
        Self { relation, role }
    }

    /// Return the relation on which the role is effective.
    #[must_use]
    pub const fn relation(&self) -> &TypeId {
        &self.relation
    }

    /// Return the role identity, whose declaring relation may be an ancestor.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }
}

/// Effective related role, including specialization replacement history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveRelates {
    id: EffectiveRelatesId,
    origin: ResolutionOrigin,
    annotations: BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
    cardinality: Cardinality,
    replaced_roles: BTreeSet<RoleId>,
    is_abstract: bool,
}

impl EffectiveRelates {
    /// Return the effective related-role identity.
    #[must_use]
    pub const fn id(&self) -> &EffectiveRelatesId {
        &self.id
    }

    /// Return its direct declaration origin.
    #[must_use]
    pub const fn origin(&self) -> &ResolutionOrigin {
        &self.origin
    }

    /// Return independent annotations on the exact relates fact.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationKindId, SchemaAnnotationValue> {
        &self.annotations
    }

    /// Return explicit or profile-materialized cardinality.
    #[must_use]
    pub const fn cardinality(&self) -> Cardinality {
        self.cardinality
    }

    /// Return every inherited role interface replaced by specialization.
    #[must_use]
    pub const fn replaced_roles(&self) -> &BTreeSet<RoleId> {
        &self.replaced_roles
    }

    /// Report whether this role cannot be instantiated directly.
    #[must_use]
    pub const fn is_abstract(&self) -> bool {
        self.is_abstract
    }
}

/// Effective direct or inherited attribute value domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectiveValueType {
    value_type: ValueTypeTag,
    origin: ResolutionOrigin,
    annotations: BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
}

impl EffectiveValueType {
    /// Return the effective value domain.
    #[must_use]
    pub const fn value_type(&self) -> ValueTypeTag {
        self.value_type
    }

    /// Return its direct value-fact origin.
    #[must_use]
    pub const fn origin(&self) -> &ResolutionOrigin {
        &self.origin
    }

    /// Return effective value-domain constraints.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationKindId, SchemaAnnotationValue> {
        &self.annotations
    }
}

/// Fully resolved type semantics, rebuilt only from direct facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedType {
    id: TypeId,
    supertypes: Vec<TypeId>,
    subtypes: BTreeSet<TypeId>,
    annotations: BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
    value_type: Option<EffectiveValueType>,
    owns: BTreeMap<AttributeId, EffectiveOwns>,
    plays: BTreeMap<RoleId, EffectivePlays>,
    relates: BTreeMap<RoleId, EffectiveRelates>,
    key_attributes: BTreeSet<AttributeId>,
    unique_attributes: BTreeSet<AttributeId>,
    owned_attribute_order: Vec<AttributeId>,
    is_abstract: bool,
    constructible: bool,
}

impl ResolvedType {
    /// Return the stable type identity.
    #[must_use]
    pub const fn id(&self) -> &TypeId {
        &self.id
    }

    /// Return transitive supertypes from nearest to furthest.
    #[must_use]
    pub fn supertypes(&self) -> &[TypeId] {
        &self.supertypes
    }

    /// Return all transitive subtypes.
    #[must_use]
    pub const fn subtypes(&self) -> &BTreeSet<TypeId> {
        &self.subtypes
    }

    /// Return effective type annotations.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationKindId, SchemaAnnotationValue> {
        &self.annotations
    }

    /// Return the effective attribute value domain, if this is an attribute.
    #[must_use]
    pub const fn value_type(&self) -> Option<&EffectiveValueType> {
        self.value_type.as_ref()
    }

    /// Return effective ownerships keyed by attribute identity.
    #[must_use]
    pub const fn owns(&self) -> &BTreeMap<AttributeId, EffectiveOwns> {
        &self.owns
    }

    /// Return effective role-playing interfaces keyed by role identity.
    #[must_use]
    pub const fn plays(&self) -> &BTreeMap<RoleId, EffectivePlays> {
        &self.plays
    }

    /// Return effective related roles for relation types.
    #[must_use]
    pub const fn relates(&self) -> &BTreeMap<RoleId, EffectiveRelates> {
        &self.relates
    }

    /// Return effective key attributes.
    #[must_use]
    pub const fn key_attributes(&self) -> &BTreeSet<AttributeId> {
        &self.key_attributes
    }

    /// Return effective unique attributes not implied solely by key.
    #[must_use]
    pub const fn unique_attributes(&self) -> &BTreeSet<AttributeId> {
        &self.unique_attributes
    }

    /// Return deterministic ownership order metadata.
    #[must_use]
    pub fn owned_attribute_order(&self) -> &[AttributeId] {
        &self.owned_attribute_order
    }

    /// Report direct abstractness.
    #[must_use]
    pub const fn is_abstract(&self) -> bool {
        self.is_abstract
    }

    /// Report whether instances can be constructed under effective role constraints.
    #[must_use]
    pub const fn is_constructible(&self) -> bool {
        self.constructible
    }
}

/// Resolved role descriptor and accepted concrete player types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedRole {
    id: RoleId,
    accepted_players: BTreeSet<TypeId>,
    replacing_roles: BTreeSet<RoleId>,
    is_abstract: bool,
}

impl ResolvedRole {
    /// Return the declared role identity.
    #[must_use]
    pub const fn id(&self) -> &RoleId {
        &self.id
    }

    /// Return concrete entity or relation types accepted as players.
    #[must_use]
    pub const fn accepted_players(&self) -> &BTreeSet<TypeId> {
        &self.accepted_players
    }

    /// Return ancestor role interfaces this role replaces.
    #[must_use]
    pub const fn replacing_roles(&self) -> &BTreeSet<RoleId> {
        &self.replacing_roles
    }

    /// Report declared role abstractness.
    #[must_use]
    pub const fn is_abstract(&self) -> bool {
        self.is_abstract
    }
}

/// Opaque resolved function retained without speculative body typing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedFunction {
    declaration: FunctionFact,
    annotations: BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
}

impl ResolvedFunction {
    /// Return the stable function identity.
    #[must_use]
    pub const fn id(&self) -> &FunctionId {
        self.declaration.id()
    }

    /// Return the validated direct declaration without rewriting its body.
    #[must_use]
    pub const fn declaration(&self) -> &FunctionFact {
        &self.declaration
    }

    /// Return function documentation and metadata.
    #[must_use]
    pub const fn annotations(&self) -> &BTreeMap<AnnotationKindId, SchemaAnnotationValue> {
        &self.annotations
    }
}

/// A validated struct in resolved schema output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedStruct {
    id: StructId,
    fields: Vec<StructField>,
}

impl ResolvedStruct {
    /// Return the stable struct identity.
    #[must_use]
    pub const fn id(&self) -> &StructId {
        &self.id
    }

    /// Return fields in their semantic declaration order.
    #[must_use]
    pub fn fields(&self) -> &[StructField] {
        &self.fields
    }
}

/// Stable content-addressed query descriptor identity.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct DescriptorId(String);

impl DescriptorId {
    /// Return the lowercase content-addressed descriptor string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable direct-fact descriptor lookup.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DescriptorIndex {
    descriptors: BTreeMap<DescriptorId, SchemaFactId>,
}

impl DescriptorIndex {
    /// Return the fact associated with a descriptor.
    #[must_use]
    pub fn get(&self, descriptor: &DescriptorId) -> Option<&SchemaFactId> {
        self.descriptors.get(descriptor)
    }

    /// Iterate descriptors in stable content order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&DescriptorId, &SchemaFactId)> {
        self.descriptors.iter()
    }
}

/// Projection dependency edges and deterministic strongly connected components.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SchemaDependencyGraph {
    edges: BTreeMap<TypeId, BTreeSet<TypeId>>,
    strongly_connected_components: Vec<BTreeSet<TypeId>>,
}

impl SchemaDependencyGraph {
    /// Return outgoing projection dependencies for one type.
    #[must_use]
    pub fn dependencies(&self, id: &TypeId) -> Option<&BTreeSet<TypeId>> {
        self.edges.get(id)
    }

    /// Return deterministic projection-dependency SCCs.
    #[must_use]
    pub fn strongly_connected_components(&self) -> &[BTreeSet<TypeId>] {
        &self.strongly_connected_components
    }
}

/// Immutable pure resolution result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSchema {
    declared_identity: DeclaredIdentityFingerprint,
    semantic_fingerprint: SemanticSchemaFingerprint,
    types: BTreeMap<TypeId, ResolvedType>,
    roles: BTreeMap<RoleId, ResolvedRole>,
    functions: BTreeMap<FunctionId, ResolvedFunction>,
    structs: BTreeMap<StructId, ResolvedStruct>,
    descriptor_index: DescriptorIndex,
    dependency_graph: SchemaDependencyGraph,
}

impl ResolvedSchema {
    /// Return the unchanged direct-declaration identity.
    #[must_use]
    pub const fn declared_identity_fingerprint(&self) -> &DeclaredIdentityFingerprint {
        &self.declared_identity
    }

    /// Return canonical direct semantics under the selected profile.
    #[must_use]
    pub const fn semantic_fingerprint(&self) -> &SemanticSchemaFingerprint {
        &self.semantic_fingerprint
    }

    /// Return resolved types by stable identity.
    #[must_use]
    pub const fn types(&self) -> &BTreeMap<TypeId, ResolvedType> {
        &self.types
    }

    /// Return resolved roles by declaring-role identity.
    #[must_use]
    pub const fn roles(&self) -> &BTreeMap<RoleId, ResolvedRole> {
        &self.roles
    }

    /// Return opaque resolved functions.
    #[must_use]
    pub const fn functions(&self) -> &BTreeMap<FunctionId, ResolvedFunction> {
        &self.functions
    }

    /// Return resolved structs in stable identity order.
    #[must_use]
    pub const fn structs(&self) -> &BTreeMap<StructId, ResolvedStruct> {
        &self.structs
    }

    /// Return stable direct-fact descriptors.
    #[must_use]
    pub const fn descriptor_index(&self) -> &DescriptorIndex {
        &self.descriptor_index
    }

    /// Return projection dependency SCCs.
    #[must_use]
    pub const fn dependency_graph(&self) -> &SchemaDependencyGraph {
        &self.dependency_graph
    }
}

#[derive(Default)]
struct DirectIndex {
    types: BTreeSet<TypeId>,
    parents: BTreeMap<TypeId, Vec<SubFactId>>,
    values: BTreeMap<AttributeId, (ValueTypeTag, ValueFactId)>,
    owns: BTreeMap<TypeId, Vec<OwnsFact>>,
    relates: BTreeMap<TypeId, Vec<RelatesFact>>,
    plays: BTreeMap<TypeId, Vec<PlaysFact>>,
    annotations:
        BTreeMap<AnnotationSubjectId, BTreeMap<AnnotationKindId, SchemaAnnotationValue>>,
    functions: BTreeMap<FunctionId, FunctionFact>,
    structs: BTreeMap<StructId, StructFact>,
}

/// Schema-feature capabilities implemented by the built-in compatibility resolver.
pub const BUILTIN_SCHEMA_CAPABILITY_IDS: &[&str] = &[
    "schema.annotations",
    "schema.doc-meta",
    "schema.roles",
];

fn builtin_schema_capabilities() -> CapabilitySet {
    BUILTIN_SCHEMA_CAPABILITY_IDS
        .iter()
        .map(|capability| {
            CapabilityId::new(*capability).expect("built-in schema capability IDs are canonical")
        })
        .collect()
}

/// Resolve direct schema facts under the built-in semantic-profile compatibility surface.
pub fn resolve(
    declared: &DeclaredSchema,
    profile_id: &SemanticProfileId,
) -> Result<ResolvedSchema, SchemaDiagnostics> {
    resolve_schema_with_capabilities(declared, profile_id, &builtin_schema_capabilities())
}

/// Resolve direct schema facts using an explicitly available schema-feature set.
pub fn resolve_schema_with_capabilities(
    declared: &DeclaredSchema,
    profile_id: &SemanticProfileId,
    available_capabilities: &CapabilitySet,
) -> Result<ResolvedSchema, SchemaDiagnostics> {
    declared
        .required_capabilities()
        .ensure_supported_by(available_capabilities)
        .map_err(|diagnostic| {
            type_bridge_contract::schema::SchemaDiagnostics::one(
                type_bridge_contract::schema::SchemaDiagnostic::new(diagnostic, None),
            )
        })?;
    let profile = SemanticProfile::resolve(profile_id).map_err(|diagnostic| {
        type_bridge_contract::schema::SchemaDiagnostics::one(
            type_bridge_contract::schema::SchemaDiagnostic::new(diagnostic, None),
        )
    })?;
    let index = DirectIndex::build(declared)?;
    let parents = validate_parents(declared, &index)?;
    validate_inheritance_cycles(declared, &index.types, &parents)?;

    let mut order = index.types.iter().cloned().collect::<Vec<_>>();
    order.sort_by_key(|id| (ancestor_chain(id, &parents).len(), id.clone()));
    let mut types = BTreeMap::<TypeId, ResolvedType>::new();

    for id in order {
        let parent = parents.get(&id);
        let inherited = parent.and_then(|parent| types.get(parent)).cloned();
        let mut resolved = inherited
            .as_ref()
            .map(|parent| inherit_type(parent, &id, &profile))
            .transpose()?
            .unwrap_or_else(|| empty_resolved_type(id.clone()));
        resolved.id = id.clone();
        resolved.supertypes = ancestor_chain(&id, &parents);

        let direct_annotations = annotations_for(&index, AnnotationSubjectId::Type(id.clone()));
        if let Some(independent) = inherited
            .as_ref()
            .and_then(|parent| parent.annotations.get(&AnnotationKindId::Independent))
        {
            resolved
                .annotations
                .insert(AnnotationKindId::Independent, independent.clone());
        }
        resolved.annotations.extend(direct_annotations);
        resolved.is_abstract = resolved.annotations.contains_key(&AnnotationKindId::Abstract);

        if id.kind() == TypeKind::Attribute {
            let attribute = AttributeId::new(id.label().as_str()).map_err(no_source)?;
            if let Some((value_type, value_id)) = index.values.get(&attribute) {
                resolved.value_type = Some(EffectiveValueType {
                    value_type: *value_type,
                    origin: ResolutionOrigin::direct(SchemaFactId::Value(value_id.clone())),
                    annotations: annotations_for(&index, AnnotationSubjectId::Value(value_id.clone())),
                });
            }
        }

        for owns in index.owns.get(&id).into_iter().flatten() {
            let annotations = annotations_for(
                &index,
                AnnotationSubjectId::Owns(owns.id().clone()),
            );
            let effective = EffectiveOwns {
                id: owns.id().clone(),
                origin: ResolutionOrigin::direct(SchemaFactId::Owns(owns.id().clone())),
                cardinality: cardinality(
                    &annotations,
                    &profile,
                    InterfaceKind::Owns,
                ),
                key: annotations.contains_key(&AnnotationKindId::Key),
                unique: annotations.contains_key(&AnnotationKindId::Unique)
                    || annotations.contains_key(&AnnotationKindId::Key),
                annotations,
            };
            resolved
                .owns
                .insert(owns.id().attribute().clone(), effective);
        }

        for plays in index.plays.get(&id).into_iter().flatten() {
            let annotations = annotations_for(
                &index,
                AnnotationSubjectId::Plays(plays.id().clone()),
            );
            let effective = EffectivePlays {
                id: plays.id().clone(),
                origin: ResolutionOrigin::direct(SchemaFactId::Plays(plays.id().clone())),
                cardinality: cardinality(
                    &annotations,
                    &profile,
                    InterfaceKind::Plays,
                ),
                annotations,
            };
            resolved
                .plays
                .insert(plays.id().role().clone(), effective);
        }

        for relates in index.relates.get(&id).into_iter().flatten() {
            let annotations = annotations_for(
                &index,
                AnnotationSubjectId::Relates(relates.id().clone()),
            );
            let mut replaced_roles = BTreeSet::new();
            if let Some(specialized) = relates.specializes() {
                let replaced = resolved
                    .relates
                    .iter()
                    .find_map(|(role, effective)| {
                        (role == specialized || effective.replaced_roles.contains(specialized))
                            .then(|| role.clone())
                    })
                    .ok_or_else(|| {
                        source_error(
                            declared,
                            &SchemaFactId::Relates(relates.id().clone()),
                            "invalid_role_specialization",
                            "specialized role is not effective on the child relation",
                        )
                    })?;
                let replaced = resolved
                    .relates
                    .remove(&replaced)
                    .expect("located effective role exists");
                replaced_roles.extend(replaced.replaced_roles);
                replaced_roles.insert(specialized.clone());
            }
            let effective = EffectiveRelates {
                id: EffectiveRelatesId::new(
                    id.clone(),
                    relates.id().role().clone(),
                ),
                origin: ResolutionOrigin::direct(SchemaFactId::Relates(relates.id().clone())),
                cardinality: cardinality(
                    &annotations,
                    &profile,
                    InterfaceKind::Relates,
                ),
                is_abstract: annotations.contains_key(&AnnotationKindId::Abstract),
                annotations,
                replaced_roles,
            };
            resolved
                .relates
                .insert(relates.id().role().clone(), effective);
        }

        resolved.key_attributes = resolved
            .owns
            .values()
            .filter(|owns| owns.key)
            .map(|owns| owns.id.attribute().clone())
            .collect();
        resolved.unique_attributes = resolved
            .owns
            .values()
            .filter(|owns| owns.unique)
            .map(|owns| owns.id.attribute().clone())
            .collect();
        resolved.owned_attribute_order = resolved.owns.keys().cloned().collect();
        resolved.constructible = !resolved.is_abstract
            && !resolved.relates.values().any(EffectiveRelates::is_abstract);
        types.insert(id, resolved);
    }

    populate_subtypes(&mut types, &parents);
    let roles = resolve_roles(&types, &index);
    let functions = index
        .functions
        .values()
        .map(|function| {
            (
                function.id().clone(),
                ResolvedFunction {
                    declaration: function.clone(),
                    annotations: annotations_for(
                        &index,
                        AnnotationSubjectId::Function(function.id().clone()),
                    ),
                },
            )
        })
        .collect();
    let structs = index
        .structs
        .values()
        .map(|fact| {
            (
                fact.id().clone(),
                ResolvedStruct {
                    id: fact.id().clone(),
                    fields: fact.fields().to_vec(),
                },
            )
        })
        .collect();
    let descriptor_index = descriptor_index(declared)?;
    let dependency_graph = dependency_graph(&types, &roles);
    let semantic_fingerprint = semantic_schema_fingerprint(declared, profile_id)?;

    Ok(ResolvedSchema {
        declared_identity: declared.declared_identity_fingerprint().clone(),
        semantic_fingerprint,
        types,
        roles,
        functions,
        structs,
        descriptor_index,
        dependency_graph,
    })
}

impl DirectIndex {
    fn build(declared: &DeclaredSchema) -> Result<Self, SchemaDiagnostics> {
        let mut index = Self::default();
        for fact in declared.facts() {
            match fact {
                SchemaFact::Type(fact) => {
                    index.types.insert(fact.id().clone());
                }
                SchemaFact::Sub(fact) => index
                    .parents
                    .entry(fact.id().subtype().clone())
                    .or_default()
                    .push(fact.id().clone()),
                SchemaFact::Value(fact) => {
                    index.values.insert(
                        fact.id().attribute().clone(),
                        (fact.value_type(), fact.id().clone()),
                    );
                }
                SchemaFact::Owns(fact) => index
                    .owns
                    .entry(fact.id().owner().clone())
                    .or_default()
                    .push(fact.clone()),
                SchemaFact::Relates(fact) => index
                    .relates
                    .entry(fact.id().relation().clone())
                    .or_default()
                    .push(fact.clone()),
                SchemaFact::Plays(fact) => index
                    .plays
                    .entry(fact.id().player().clone())
                    .or_default()
                    .push(fact.clone()),
                SchemaFact::Annotation(fact) => {
                    index
                        .annotations
                        .entry(fact.id().subject().clone())
                        .or_default()
                        .insert(fact.id().kind().clone(), fact.value().clone());
                }
                SchemaFact::Function(fact) => {
                    index.functions.insert(fact.id().clone(), fact.clone());
                }
                SchemaFact::Struct(fact) => {
                    index.structs.insert(fact.id().clone(), fact.clone());
                }
            }
        }
        Ok(index)
    }
}

fn validate_parents(
    declared: &DeclaredSchema,
    index: &DirectIndex,
) -> Result<BTreeMap<TypeId, TypeId>, SchemaDiagnostics> {
    let mut parents = BTreeMap::new();
    for (subtype, facts) in &index.parents {
        if facts.len() > 1 {
            let first = SchemaFactId::Sub(facts[0].clone());
            let second = SchemaFactId::Sub(facts[1].clone());
            let primary = declared.source(&second).cloned();
            let related = declared.source(&first).cloned();
            if let (Some(primary), Some(related)) = (primary, related) {
                return Err(crate::yaml::diagnostic_with_related(
                    DiagnosticCategory::InvalidContract,
                    "multiple_type_parents",
                    "a schema type has more than one same-kind parent",
                    primary,
                    related,
                    "first parent declaration is here",
                ));
            }
            return Err(source_error(
                declared,
                &second,
                "multiple_type_parents",
                "a schema type has more than one same-kind parent",
            ));
        }
        let parent = facts[0].supertype().clone();
        if subtype.kind() != parent.kind() {
            return Err(source_error(
                declared,
                &SchemaFactId::Sub(facts[0].clone()),
                "invalid_type_parent_kind",
                "schema subtype and supertype must have the same kind",
            ));
        }
        parents.insert(subtype.clone(), parent);
    }
    Ok(parents)
}

fn validate_inheritance_cycles(
    declared: &DeclaredSchema,
    types: &BTreeSet<TypeId>,
    parents: &BTreeMap<TypeId, TypeId>,
) -> Result<(), SchemaDiagnostics> {
    for start in types {
        let mut current = start;
        let mut visited = BTreeSet::new();
        while let Some(parent) = parents.get(current) {
            if !visited.insert(current.clone()) {
                let fact = SubFactId::new(current.clone(), parent.clone()).map_err(no_source)?;
                return Err(source_error(
                    declared,
                    &SchemaFactId::Sub(fact),
                    "schema_inheritance_cycle",
                    "schema type inheritance contains a cycle",
                ));
            }
            current = parent;
        }
    }
    Ok(())
}

fn ancestor_chain(id: &TypeId, parents: &BTreeMap<TypeId, TypeId>) -> Vec<TypeId> {
    let mut ancestors = Vec::new();
    let mut current = id;
    while let Some(parent) = parents.get(current) {
        ancestors.push(parent.clone());
        current = parent;
    }
    ancestors
}

fn empty_resolved_type(id: TypeId) -> ResolvedType {
    ResolvedType {
        id,
        supertypes: Vec::new(),
        subtypes: BTreeSet::new(),
        annotations: BTreeMap::new(),
        value_type: None,
        owns: BTreeMap::new(),
        plays: BTreeMap::new(),
        relates: BTreeMap::new(),
        key_attributes: BTreeSet::new(),
        unique_attributes: BTreeSet::new(),
        owned_attribute_order: Vec::new(),
        is_abstract: false,
        constructible: true,
    }
}

fn inherit_type(
    parent: &ResolvedType,
    child: &TypeId,
    profile: &SemanticProfile,
) -> Result<ResolvedType, SchemaDiagnostics> {
    let mut resolved = empty_resolved_type(child.clone());
    resolved.annotations = parent
        .annotations
        .iter()
        .filter(|(kind, _)| **kind == AnnotationKindId::Independent)
        .map(|(kind, value)| (kind.clone(), value.clone()))
        .collect();
    resolved.value_type = parent.value_type.as_ref().map(|value| EffectiveValueType {
        value_type: value.value_type,
        origin: value.origin.inherited(parent.id.clone()),
        annotations: value.annotations.clone(),
    });
    for owns in parent.owns.values() {
        let id = OwnsFactId::new(child.clone(), owns.id.attribute().clone()).map_err(no_source)?;
        let mut inherited = owns.clone();
        inherited.id = id;
        inherited.origin = inherited.origin.inherited(parent.id.clone());
        inherited.cardinality = profile.effective_cardinality(
            InterfaceKind::Owns,
            inherited
                .annotations
                .get(&AnnotationKindId::Card)
                .and_then(cardinality_value),
            inherited.key,
        );
        resolved
            .owns
            .insert(inherited.id.attribute().clone(), inherited);
    }
    for plays in parent.plays.values() {
        let id = PlaysFactId::new(child.clone(), plays.id.role().clone()).map_err(no_source)?;
        let mut inherited = plays.clone();
        inherited.id = id;
        inherited.origin = inherited.origin.inherited(parent.id.clone());
        resolved
            .plays
            .insert(inherited.id.role().clone(), inherited);
    }
    for relates in parent.relates.values() {
        let mut inherited = relates.clone();
        inherited.id = EffectiveRelatesId::new(child.clone(), relates.id.role().clone());
        inherited.origin = inherited.origin.inherited(parent.id.clone());
        resolved
            .relates
            .insert(inherited.id.role().clone(), inherited);
    }
    Ok(resolved)
}

fn annotations_for(
    index: &DirectIndex,
    subject: AnnotationSubjectId,
) -> BTreeMap<AnnotationKindId, SchemaAnnotationValue> {
    index.annotations.get(&subject).cloned().unwrap_or_default()
}

fn cardinality(
    annotations: &BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
    profile: &SemanticProfile,
    kind: InterfaceKind,
) -> Cardinality {
    profile.effective_cardinality(
        kind,
        annotations
            .get(&AnnotationKindId::Card)
            .and_then(cardinality_value),
        kind == InterfaceKind::Owns && annotations.contains_key(&AnnotationKindId::Key),
    )
}

fn cardinality_value(value: &SchemaAnnotationValue) -> Option<Cardinality> {
    match value {
        SchemaAnnotationValue::Cardinality(cardinality) => Some(*cardinality),
        _ => None,
    }
}

fn populate_subtypes(
    types: &mut BTreeMap<TypeId, ResolvedType>,
    parents: &BTreeMap<TypeId, TypeId>,
) {
    let ids = types.keys().cloned().collect::<Vec<_>>();
    for id in ids {
        for parent in ancestor_chain(&id, parents) {
            if let Some(parent) = types.get_mut(&parent) {
                parent.subtypes.insert(id.clone());
            }
        }
    }
}

fn resolve_roles(
    types: &BTreeMap<TypeId, ResolvedType>,
    index: &DirectIndex,
) -> BTreeMap<RoleId, ResolvedRole> {
    let mut roles = BTreeMap::new();
    for relates in index.relates.values().flatten() {
        let annotations = annotations_for(
            index,
            AnnotationSubjectId::Relates(relates.id().clone()),
        );
        roles.insert(
            relates.id().role().clone(),
            ResolvedRole {
                id: relates.id().role().clone(),
                accepted_players: BTreeSet::new(),
                replacing_roles: BTreeSet::new(),
                is_abstract: annotations.contains_key(&AnnotationKindId::Abstract),
            },
        );
    }
    for resolved in types.values() {
        for relates in resolved.relates.values() {
            if let Some(role) = roles.get_mut(relates.id.role()) {
                role.replacing_roles.extend(relates.replaced_roles.clone());
            }
        }
    }
    for resolved in types.values().filter(|resolved| resolved.constructible) {
        for plays in resolved.plays.values() {
            if let Some(role) = roles.get_mut(plays.id.role()) {
                role.accepted_players.insert(resolved.id.clone());
            }
        }
    }
    roles
}

fn descriptor_index(declared: &DeclaredSchema) -> Result<DescriptorIndex, SchemaDiagnostics> {
    let domain = FingerprintDomain::new("typebridge.schema.descriptor").map_err(no_source)?;
    let canonicalization = CanonicalizationVersion::new("typebridge.schema-fact-id-json/v1")
        .map_err(no_source)?;
    let mut descriptors = BTreeMap::new();
    for fact in declared.facts() {
        let id = fact.id();
        let canonical = to_canonical_json(&id).map_err(no_source)?;
        let digest = Fingerprint::compute(
            domain.clone(),
            canonicalization.clone(),
            None,
            &canonical,
        )
        .digest()
        .to_hex();
        descriptors.insert(DescriptorId(format!("schema:{digest}")), id);
    }
    Ok(DescriptorIndex { descriptors })
}

fn dependency_graph(
    types: &BTreeMap<TypeId, ResolvedType>,
    roles: &BTreeMap<RoleId, ResolvedRole>,
) -> SchemaDependencyGraph {
    let mut edges = types
        .keys()
        .cloned()
        .map(|id| (id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    for resolved in types.values() {
        if let Some(parent) = resolved.supertypes.first() {
            edges
                .entry(resolved.id.clone())
                .or_default()
                .insert(parent.clone());
        }
        for attribute in resolved.owns.keys() {
            if let Ok(attribute) = TypeId::new(TypeKind::Attribute, attribute.label().as_str()) {
                edges
                    .entry(resolved.id.clone())
                    .or_default()
                    .insert(attribute);
            }
        }
        for role in resolved.plays.keys() {
            if let Ok(relation) = TypeId::new(
                TypeKind::Relation,
                role.declaring_relation().as_str(),
            ) {
                edges
                    .entry(resolved.id.clone())
                    .or_default()
                    .insert(relation);
            }
        }
        for relates in resolved.relates.values() {
            if let Some(role) = roles.get(relates.id().role()) {
                edges
                    .entry(resolved.id.clone())
                    .or_default()
                    .extend(role.accepted_players().iter().cloned());
            }
        }
    }
    let strongly_connected_components = strongly_connected_components(&edges);
    SchemaDependencyGraph {
        edges,
        strongly_connected_components,
    }
}

fn strongly_connected_components(
    edges: &BTreeMap<TypeId, BTreeSet<TypeId>>,
) -> Vec<BTreeSet<TypeId>> {
    struct Tarjan<'a> {
        edges: &'a BTreeMap<TypeId, BTreeSet<TypeId>>,
        index: usize,
        indices: BTreeMap<TypeId, usize>,
        lowlinks: BTreeMap<TypeId, usize>,
        stack: Vec<TypeId>,
        on_stack: BTreeSet<TypeId>,
        components: Vec<BTreeSet<TypeId>>,
    }
    impl Tarjan<'_> {
        fn visit(&mut self, node: TypeId) {
            let index = self.index;
            self.index += 1;
            self.indices.insert(node.clone(), index);
            self.lowlinks.insert(node.clone(), index);
            self.stack.push(node.clone());
            self.on_stack.insert(node.clone());

            for dependency in self.edges.get(&node).into_iter().flatten() {
                if !self.indices.contains_key(dependency) {
                    self.visit(dependency.clone());
                    let low = self.lowlinks[&node].min(self.lowlinks[dependency]);
                    self.lowlinks.insert(node.clone(), low);
                } else if self.on_stack.contains(dependency) {
                    let low = self.lowlinks[&node].min(self.indices[dependency]);
                    self.lowlinks.insert(node.clone(), low);
                }
            }

            if self.lowlinks[&node] == self.indices[&node] {
                let mut component = BTreeSet::new();
                loop {
                    let member = self.stack.pop().expect("SCC root has a stack member");
                    self.on_stack.remove(&member);
                    component.insert(member.clone());
                    if member == node {
                        break;
                    }
                }
                self.components.push(component);
            }
        }
    }

    let mut tarjan = Tarjan {
        edges,
        index: 0,
        indices: BTreeMap::new(),
        lowlinks: BTreeMap::new(),
        stack: Vec::new(),
        on_stack: BTreeSet::new(),
        components: Vec::new(),
    };
    for node in edges.keys() {
        if !tarjan.indices.contains_key(node) {
            tarjan.visit(node.clone());
        }
    }
    tarjan
        .components
        .sort_by(|left, right| left.first().cmp(&right.first()));
    tarjan.components
}

fn source_error(
    declared: &DeclaredSchema,
    fact: &SchemaFactId,
    code: &'static str,
    message: &'static str,
) -> SchemaDiagnostics {
    crate::yaml::diagnostic(
        DiagnosticCategory::InvalidContract,
        code,
        message,
        declared.source(fact).cloned(),
    )
}

fn no_source(diagnostic: type_bridge_contract::diagnostic::Diagnostic) -> SchemaDiagnostics {
    type_bridge_contract::schema::SchemaDiagnostics::one(
        type_bridge_contract::schema::SchemaDiagnostic::new(diagnostic, None),
    )
}
