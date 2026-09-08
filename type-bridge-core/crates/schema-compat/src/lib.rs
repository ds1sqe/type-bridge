//! One-way compatibility front-ends for the V2 schema fact graph.
//!
//! This public supporting crate is deliberately narrow. Source-language parsers
//! converge on `type_bridge_schema::FactAssembler`; contract and schema crates
//! never depend on compatibility parsers or their transitive grammar
//! dependencies.

#![deny(missing_docs)]

mod adopted_genesis;
mod descriptor;
mod function_references;
mod literal;
mod live_authority;
mod released_syntax;

pub use adopted_genesis::{
    ADOPTED_GENESIS_FILE_NAME, AdoptedGenesisAuthority, LEGACY_LEDGER_SCHEMA_TYPEQL,
    is_legacy_ledger_label, parse_adopted_genesis, parse_adopted_genesis_authority,
    parse_adopted_genesis_authority_with_internal,
};
pub use function_references::{FunctionBodyReferences, SchemaReference, TypeqlDeclaredSchema};
pub use live_authority::{
    LiveLegacyLedgerPresence, LiveQueryAuthorityState, LiveQueryControlPresence,
    MANAGED_FENCE_SCHEMA_TYPEQL, legacy_ledger_schema_presence, managed_fence_schema_presence,
    rebuild_live_query_authority, rebuild_live_query_authority_state,
};

pub use descriptor::{
    GENERATED_DECLARED_DESCRIPTOR_PATH, GENERATED_DECLARED_DESCRIPTOR_V1,
    GeneratedDeclaredDescriptorSetV1, attach_declared_descriptors,
    empty_generated_declared_descriptors_json, generate_package_with_declared_descriptors,
    generated_declared_descriptors_json, generated_descriptors_to_declared,
    released_typeql_to_declared_lossless_projection,
    released_typeql_to_declared_lossless_projection_with_references,
    released_typeql_to_declared_projection, released_typeql_to_declared_projection_with_references,
    typeql_to_generated_descriptors,
};

/// Comparison-only reporting between the frozen V1 parser and the V2 fact graph.
pub mod shadow;

pub use shadow::{
    ShadowCompared, ShadowComparison, ShadowCoverage, ShadowCoverageState, ShadowDimension,
    ShadowFinding, ShadowLaneNotRun, ShadowLaneOutcome, ShadowLaneRejection, ShadowLaneSummary,
    ShadowUnavailableLane, ShadowVerdict, V1ShadowInternalError, V1ShadowReport, v1_shadow_report,
};

use std::collections::{BTreeMap, BTreeSet};

use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{
    AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind,
};
use type_bridge_contract::limits::MAX_CANONICAL_COLLECTION_LEN;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CanonicalValueRange,
    CanonicalValueSet, DeclaredSchema, DocText, DocumentId, FunctionBody, FunctionFact,
    FunctionParameter, FunctionReturnElement, FunctionReturnMode, FunctionSignature, OwnsFact,
    OwnsFactId, PlaysFactId, RegexPattern, RelatesFactId, SchemaAnnotationValue, SchemaDiagnostic,
    SchemaDiagnostics, SchemaFact, SourceSpan, StructFact, StructField, SubFact, SubFactId,
    TypeFact, TypeReference, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, Cardinality, ValueTypeTag};
use type_bridge_schema::FactAssembler;
use typeql::Annotation;
use typeql::annotation::CardinalityRange;
use typeql::common::{Span, Spanned};
use typeql::query::{QueryStructure, schema::SchemaQuery};
use typeql::schema::definable::{
    Definable,
    function::{Function, Output},
    struct_::Struct,
    type_::{Capability, CapabilityBase, Type as TypeDeclaration},
};
use typeql::type_::{NamedType, NamedTypeAny, TypeRef, TypeRefAny};

use crate::literal::{canonical_literal, validate_quoted_string};
use crate::released_syntax::{ReleasedAnnotationTarget, ReleasedSyntax};

/// Defensive source bound applied before entering the third-party parser.
pub const MAX_TYPEQL_SCHEMA_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TypeqlSourceSizePolicy {
    Defensive,
    TrustedGenerator,
}

impl TypeqlSourceSizePolicy {
    const fn allows(self, source_len: usize) -> bool {
        matches!(self, Self::TrustedGenerator) || source_len <= MAX_TYPEQL_SCHEMA_BYTES
    }
}

fn ensure_typeql_source_size(
    source: &str,
    size_policy: TypeqlSourceSizePolicy,
) -> Result<(), SchemaDiagnostics> {
    if size_policy.allows(source.len()) {
        return Ok(());
    }
    Err(error(
        DiagnosticCategory::InvalidContract,
        "typeql_schema_size_limit",
        "TypeQL schema source exceeds the compatibility parser limit",
        None,
    ))
}

/// Parse one TypeQL `define` query into the canonical declared schema graph
/// and derive neutral references from every function body.
pub fn typeql_to_declared_with_references(
    document: DocumentId,
    source: &str,
) -> Result<TypeqlDeclaredSchema, SchemaDiagnostics> {
    typeql_to_declared_with_references_with_size_policy(
        document,
        source,
        TypeqlSourceSizePolicy::Defensive,
    )
}

pub(crate) fn typeql_to_declared_with_references_with_size_policy(
    document: DocumentId,
    source: &str,
    size_policy: TypeqlSourceSizePolicy,
) -> Result<TypeqlDeclaredSchema, SchemaDiagnostics> {
    ensure_typeql_source_size(source, size_policy)?;
    typeql_to_declared_with_references_impl(document, source, source, None, None, None, None)
}

pub(crate) fn released_typeql_to_declared_with_references(
    document: DocumentId,
    released: &ReleasedSyntax,
    size_policy: TypeqlSourceSizePolicy,
) -> Result<TypeqlDeclaredSchema, SchemaDiagnostics> {
    ensure_typeql_source_size(released.original_source(), size_policy)?;
    let reference_projection = released_unresolved_capability_ranges(&document, released)?;
    typeql_to_declared_with_references_impl(
        document,
        released.original_source(),
        released.source(),
        Some(released),
        None,
        None,
        Some(&reference_projection.played_role_declarations),
    )
}

pub(crate) fn released_typeql_to_declared_with_references_omitting_capabilities(
    document: DocumentId,
    released: &ReleasedSyntax,
    omitted_declarations: &BTreeSet<usize>,
    omitted_capabilities: &BTreeSet<usize>,
    played_role_declarations: &BTreeMap<usize, String>,
    size_policy: TypeqlSourceSizePolicy,
) -> Result<TypeqlDeclaredSchema, SchemaDiagnostics> {
    ensure_typeql_source_size(released.original_source(), size_policy)?;
    typeql_to_declared_with_references_impl(
        document,
        released.original_source(),
        released.source(),
        Some(released),
        Some(omitted_declarations),
        Some(omitted_capabilities),
        Some(played_role_declarations),
    )
}

/// Index every non-portable declaration and causally unresolved capability in
/// one released-schema pass.
///
/// Descriptor generation is intentionally open-world, but retrying the whole
/// parser once per missing reference makes a small partial export quadratic.
/// This index mirrors the released merge algebra, validates the surviving
/// direct role graph in memory, and returns original byte ranges for every
/// declaration/fact the generator-only projection must omit plus the declaring
/// role scope for valid inherited plays edges. Descriptor omissions are kept
/// separate from the older render-only role repair so newly unsupported
/// canonical identities cannot change released model bytes.
#[derive(Default)]
pub(crate) struct ReleasedReferenceProjection {
    pub(crate) omitted_declarations: BTreeMap<usize, usize>,
    pub(crate) omitted: BTreeMap<usize, usize>,
    pub(crate) omitted_from_render: BTreeMap<usize, usize>,
    pub(crate) played_role_declarations: BTreeMap<usize, String>,
}

pub(crate) fn released_unresolved_capability_ranges(
    document: &DocumentId,
    released: &ReleasedSyntax,
) -> Result<ReleasedReferenceProjection, SchemaDiagnostics> {
    let queries = typeql::parse_queries(released.source()).map_err(|parse_error| {
        error(
            DiagnosticCategory::InvalidContract,
            "invalid_typeql_schema",
            format!("TypeQL schema parsing failed: {parse_error}"),
            None,
        )
    })?;
    let mut definables = Vec::new();
    for query in queries {
        match query.structure {
            QueryStructure::Schema(SchemaQuery::Define(define)) => {
                definables.extend(define.definables);
            }
            _ => {
                return Err(error(
                    DiagnosticCategory::InvalidContract,
                    "expected_typeql_define",
                    "schema compatibility input must contain only define queries",
                    query_span(document, released.original_source(), query.span)?,
                ));
            }
        }
    }
    restore_released_labels(released, &mut definables);
    let declarations = definables
        .iter()
        .filter_map(|definable| match definable {
            Definable::TypeDeclaration(declaration) => Some(declaration),
            _ => None,
        })
        .collect::<Vec<_>>();
    let kinds = infer_type_kinds(document, released.original_source(), &declarations, true)?;
    let mut omitted_declarations = BTreeMap::new();
    let mut invalid_type_labels = BTreeSet::new();
    for (declaration, kind) in declarations.iter().zip(&kinds) {
        let label = typeql_label(&declaration.label);
        let portable = TypeId::new(*kind, label.clone())
            .and_then(TypeFact::new)
            .is_ok();
        if !portable {
            invalid_type_labels.insert(label);
            if let Some(span) = declaration.span {
                omitted_declarations.insert(span.begin_offset, span.end_offset);
            }
        }
    }
    // Every reopening of a non-portable identity belongs to the same omitted
    // declaration closure, including a kindless standalone `plays` line.
    for declaration in &declarations {
        if invalid_type_labels.contains(&typeql_label(&declaration.label))
            && let Some(span) = declaration.span
        {
            omitted_declarations.insert(span.begin_offset, span.end_offset);
        }
    }
    let ids = declarations
        .iter()
        .zip(&kinds)
        .filter(|(declaration, _)| {
            !declaration
                .span
                .is_some_and(|span| omitted_declarations.contains_key(&span.begin_offset))
        })
        .map(|(declaration, kind)| (typeql_label(&declaration.label), *kind))
        .collect::<BTreeMap<_, _>>();

    let mut last_attribute_declaration = BTreeMap::new();
    let mut last_sub_capability = BTreeMap::new();
    let mut last_value_capability = BTreeMap::new();
    for (declaration_index, (declaration, kind)) in declarations.iter().zip(&kinds).enumerate() {
        let label = typeql_label(&declaration.label);
        if *kind == TypeKind::Attribute {
            last_attribute_declaration.insert(label.clone(), declaration_index);
        }
        for (capability_index, capability) in declaration.capabilities.iter().enumerate() {
            if matches!(capability.base, CapabilityBase::Sub(_)) {
                last_sub_capability.insert(label.clone(), (declaration_index, capability_index));
            }
            if matches!(capability.base, CapabilityBase::ValueType(_)) {
                last_value_capability.insert(label.clone(), (declaration_index, capability_index));
            }
        }
    }

    struct IndexedCapability<'a> {
        owner: String,
        owner_kind: TypeKind,
        capability: &'a Capability,
        start: usize,
        end: usize,
    }

    let mut effective = Vec::new();
    let mut first_object_capability = BTreeSet::new();
    for (declaration_index, (declaration, kind)) in declarations.iter().zip(&kinds).enumerate() {
        if declaration
            .span
            .is_some_and(|span| omitted_declarations.contains_key(&span.begin_offset))
        {
            continue;
        }
        let owner = typeql_label(&declaration.label);
        if *kind == TypeKind::Attribute
            && last_attribute_declaration.get(&owner) != Some(&declaration_index)
        {
            continue;
        }
        for (capability_index, capability) in declaration.capabilities.iter().enumerate() {
            if matches!(capability.base, CapabilityBase::Sub(_))
                && last_sub_capability.get(&owner) != Some(&(declaration_index, capability_index))
            {
                continue;
            }
            if matches!(capability.base, CapabilityBase::ValueType(_))
                && last_value_capability.get(&owner) != Some(&(declaration_index, capability_index))
            {
                continue;
            }
            if *kind != TypeKind::Attribute
                && let Some(identity) = released_object_capability_identity(capability)
                && !first_object_capability.insert((owner.clone(), identity))
            {
                continue;
            }
            let Some(span) = capability.span else {
                continue;
            };
            effective.push(IndexedCapability {
                owner: owner.clone(),
                owner_kind: *kind,
                capability,
                start: span.begin_offset,
                end: span.end_offset,
            });
        }
    }

    let mut omitted = BTreeMap::new();
    let mut parents = BTreeMap::<String, String>::new();
    for indexed in &effective {
        match &indexed.capability.base {
            CapabilityBase::Sub(sub) => {
                let parent = typeql_label(&sub.supertype_label);
                if root_kind(&parent).is_some() {
                    continue;
                }
                match ids.get(&parent) {
                    None => {
                        omitted.insert(indexed.start, indexed.end);
                    }
                    Some(parent_kind) if *parent_kind == indexed.owner_kind => {
                        parents.insert(indexed.owner.clone(), parent);
                    }
                    Some(_) => {}
                }
            }
            CapabilityBase::Owns(owns) => {
                if let Ok(attribute) = plain_type_ref(&owns.owned)
                    && !ids.contains_key(&attribute)
                {
                    omitted.insert(indexed.start, indexed.end);
                }
            }
            _ => {}
        }
    }

    let mut roles = BTreeMap::<(String, String), ReleasedIndexedRole>::new();
    let mut role_names_by_relation = BTreeMap::<String, Vec<String>>::new();
    for (capability_index, indexed) in effective.iter().enumerate() {
        let CapabilityBase::Relates(relates) = &indexed.capability.base else {
            continue;
        };
        let Ok(role) = plain_type_ref(&relates.related) else {
            continue;
        };
        let specializes = relates
            .specialised
            .as_ref()
            .and_then(|specialized| plain_type_ref(specialized).ok());
        let role_is_portable = Label::new(&role).is_ok()
            && specializes
                .as_ref()
                .is_none_or(|label| Label::new(label).is_ok());
        if !role_is_portable {
            omitted.insert(indexed.start, indexed.end);
            continue;
        }
        role_names_by_relation
            .entry(indexed.owner.clone())
            .or_default()
            .push(role.clone());
        roles.insert(
            (indexed.owner.clone(), role),
            ReleasedIndexedRole {
                capability_index,
                specializes,
            },
        );
    }

    let relation_names = ids
        .iter()
        .filter_map(|(name, kind)| (*kind == TypeKind::Relation).then_some(name.clone()))
        .collect::<BTreeSet<_>>();
    let mut played_role_names_by_relation = BTreeMap::<String, BTreeSet<String>>::new();
    for indexed in &effective {
        if let CapabilityBase::Plays(plays) = &indexed.capability.base {
            let relation = typeql_label(&plays.role.scope);
            let role = typeql_label(&plays.role.name);
            if Label::new(&relation).is_err() || Label::new(&role).is_err() {
                omitted.insert(indexed.start, indexed.end);
                continue;
            }
            played_role_names_by_relation
                .entry(relation)
                .or_default()
                .insert(role);
        }
    }
    let role_resolution = released_role_validities(
        &relation_names,
        &role_names_by_relation,
        &played_role_names_by_relation,
        &roles,
        &parents,
    );
    for (key, validity) in &role_resolution.direct {
        if *validity == ReleasedRoleValidity::Invalid {
            let indexed = &effective[roles[key].capability_index];
            omitted.insert(indexed.start, indexed.end);
        }
    }

    let mut omitted_from_render = BTreeMap::new();
    for (key, validity) in &role_resolution.direct {
        if *validity == ReleasedRoleValidity::Invalid {
            let indexed = &effective[roles[key].capability_index];
            omitted_from_render.insert(indexed.start, indexed.end);
        }
    }

    let mut played_role_declarations = BTreeMap::new();
    let mut first_portable_plays = BTreeSet::new();
    for indexed in &effective {
        let CapabilityBase::Plays(plays) = &indexed.capability.base else {
            continue;
        };
        if omitted.contains_key(&indexed.start) {
            continue;
        }
        let relation = typeql_label(&plays.role.scope);
        let role = typeql_label(&plays.role.name);
        match ids.get(&relation) {
            None => {
                omitted.insert(indexed.start, indexed.end);
            }
            Some(TypeKind::Relation) => {
                match role_resolution.plays.get(&(relation.clone(), role.clone())) {
                    Some(ReleasedRoleValidity::Valid) => {
                        if let Some(declaration) =
                            role_resolution.play_declarations.get(&(relation, role))
                        {
                            let identity = (
                                indexed.owner.clone(),
                                declaration.clone(),
                                typeql_label(&plays.role.name),
                            );
                            if first_portable_plays.insert(identity) {
                                played_role_declarations.insert(indexed.start, declaration.clone());
                            } else {
                                // Distinct released role refs may collapse to
                                // the same inherited direct role identity.
                                // Preserve the first frozen capability and
                                // record the later alias as open-world evidence
                                // instead of feeding a duplicate fact to the
                                // canonical assembler.
                                omitted.insert(indexed.start, indexed.end);
                            }
                        }
                    }
                    Some(ReleasedRoleValidity::Invalid) | None => {
                        omitted.insert(indexed.start, indexed.end);
                    }
                    Some(ReleasedRoleValidity::Indeterminate) => {}
                }
            }
            Some(_) => {}
        }
    }

    Ok(ReleasedReferenceProjection {
        omitted_declarations,
        omitted,
        omitted_from_render,
        played_role_declarations,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReleasedRoleValidity {
    Valid,
    Invalid,
    Indeterminate,
}

#[derive(Clone, Debug)]
struct ReleasedIndexedRole {
    capability_index: usize,
    specializes: Option<String>,
}

struct ReleasedRoleResolution {
    direct: BTreeMap<(String, String), ReleasedRoleValidity>,
    plays: BTreeMap<(String, String), ReleasedRoleValidity>,
    play_declarations: BTreeMap<(String, String), String>,
}

/// Resolve direct role specializations in relation-tree order without
/// recursion or repeated ancestry walks.
///
/// `direct_labels` indexes valid direct declarations on the current ancestor
/// path. Specialization does not remove its target from this index: the
/// canonical assembler binds `as <label>` to a direct ancestor declaration,
/// including one already replaced in the effective role view.
///
/// `effective_labels` separately mirrors the frozen generator's inherited
/// local role names. A valid specialization removes its immediate inherited
/// target name and adds its local name; plays checks query that set after the
/// relation's own declarations have been applied. Explicit exit frames
/// restore both views before visiting a sibling.
///
/// This is O((relations + roles) log roles), uses a heap work stack for deep
/// inheritance, and exactly models the released parser after invalid direct
/// specializations have been omitted: an omitted role contributes no label
/// for a descendant specialization to bind.
fn released_role_validities(
    relation_names: &BTreeSet<String>,
    role_names_by_relation: &BTreeMap<String, Vec<String>>,
    played_role_names_by_relation: &BTreeMap<String, BTreeSet<String>>,
    roles: &BTreeMap<(String, String), ReleasedIndexedRole>,
    parents: &BTreeMap<String, String>,
) -> ReleasedRoleResolution {
    let mut children = BTreeMap::<String, Vec<String>>::new();
    let mut roots = Vec::new();
    for relation in relation_names {
        match parents
            .get(relation)
            .filter(|parent| relation_names.contains(*parent))
        {
            Some(parent) => children
                .entry(parent.clone())
                .or_default()
                .push(relation.clone()),
            None => roots.push(relation.clone()),
        }
    }

    // Any relation not reachable from a root participates in, or descends
    // from, an inheritance cycle. Preserve its capabilities as indeterminate
    // so the canonical assembler reports the cycle instead of this
    // compatibility index inventing a different omission.
    let mut direct = roles
        .keys()
        .cloned()
        .map(|key| (key, ReleasedRoleValidity::Indeterminate))
        .collect::<BTreeMap<_, _>>();
    let mut plays = played_role_names_by_relation
        .iter()
        .flat_map(|(relation, role_names)| {
            role_names.iter().map(|role_name| {
                (
                    (relation.clone(), role_name.clone()),
                    ReleasedRoleValidity::Indeterminate,
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let mut direct_labels = BTreeSet::<String>::new();
    let mut effective_roles = BTreeMap::<String, String>::new();
    enum Traversal {
        Enter(String),
        Exit {
            direct: Vec<(String, bool)>,
            effective: Vec<(String, Option<String>)>,
        },
    }
    let mut work = roots
        .into_iter()
        .rev()
        .map(Traversal::Enter)
        .collect::<Vec<_>>();
    let mut play_declarations = BTreeMap::new();

    while let Some(frame) = work.pop() {
        match frame {
            Traversal::Enter(relation) => {
                let mut additions = BTreeSet::new();
                let mut removals = BTreeSet::new();
                if let Some(role_names) = role_names_by_relation.get(&relation) {
                    for role_name in role_names {
                        let key = (relation.clone(), role_name.clone());
                        let role = &roles[&key];
                        let resolved = match role.specializes.as_ref() {
                            Some(specialized) if direct_labels.contains(specialized) => {
                                removals.insert(specialized.clone());
                                ReleasedRoleValidity::Valid
                            }
                            Some(_) => ReleasedRoleValidity::Invalid,
                            None => ReleasedRoleValidity::Valid,
                        };
                        direct.insert(key, resolved);
                        if resolved == ReleasedRoleValidity::Valid {
                            additions.insert(role_name.clone());
                        }
                    }
                }

                let affected = removals.union(&additions).cloned().collect::<Vec<_>>();
                let mut previous_effective = Vec::with_capacity(affected.len());
                for role_name in affected {
                    let previous = effective_roles.get(&role_name).cloned();
                    previous_effective.push((role_name.clone(), previous));
                    if additions.contains(&role_name) {
                        effective_roles.insert(role_name, relation.clone());
                    } else {
                        effective_roles.remove(&role_name);
                    }
                }
                let mut previous_direct = Vec::with_capacity(additions.len());
                for role_name in &additions {
                    let was_visible = direct_labels.contains(role_name);
                    previous_direct.push((role_name.clone(), was_visible));
                    direct_labels.insert(role_name.clone());
                }

                if let Some(played_role_names) = played_role_names_by_relation.get(&relation) {
                    for role_name in played_role_names {
                        let declaration = effective_roles.get(role_name);
                        plays.insert(
                            (relation.clone(), role_name.clone()),
                            if declaration.is_some() {
                                ReleasedRoleValidity::Valid
                            } else {
                                ReleasedRoleValidity::Invalid
                            },
                        );
                        if let Some(declaration) = declaration {
                            play_declarations
                                .insert((relation.clone(), role_name.clone()), declaration.clone());
                        }
                    }
                }

                work.push(Traversal::Exit {
                    direct: previous_direct,
                    effective: previous_effective,
                });
                if let Some(relation_children) = children.get(&relation) {
                    work.extend(
                        relation_children
                            .iter()
                            .rev()
                            .cloned()
                            .map(Traversal::Enter),
                    );
                }
            }
            Traversal::Exit { direct, effective } => {
                for (role_name, was_visible) in direct {
                    if was_visible {
                        direct_labels.insert(role_name);
                    } else {
                        direct_labels.remove(&role_name);
                    }
                }
                for (role_name, previous) in effective {
                    if let Some(declaration) = previous {
                        effective_roles.insert(role_name, declaration);
                    } else {
                        effective_roles.remove(&role_name);
                    }
                }
            }
        }
    }

    ReleasedRoleResolution {
        direct,
        plays,
        play_declarations,
    }
}

fn typeql_to_declared_with_references_impl(
    document: DocumentId,
    source: &str,
    parser_source: &str,
    released: Option<&ReleasedSyntax>,
    omitted_released_declarations: Option<&BTreeSet<usize>>,
    omitted_released_capabilities: Option<&BTreeSet<usize>>,
    played_role_declarations: Option<&BTreeMap<usize, String>>,
) -> Result<TypeqlDeclaredSchema, SchemaDiagnostics> {
    // Released schema sources may carry several define blocks; every query
    // must still be a define, and their definables merge in source order.
    let queries = typeql::parse_queries(parser_source).map_err(|parse_error| {
        error(
            DiagnosticCategory::InvalidContract,
            "invalid_typeql_schema",
            format!("TypeQL schema parsing failed: {parse_error}"),
            None,
        )
    })?;
    if queries.is_empty() {
        return Err(error(
            DiagnosticCategory::InvalidContract,
            "expected_typeql_define",
            "schema compatibility input must contain at least one define query",
            None,
        ));
    }
    let mut definables = Vec::new();
    for query in queries {
        match query.structure {
            QueryStructure::Schema(SchemaQuery::Define(define)) => {
                definables.extend(define.definables);
            }
            _ => {
                return Err(error(
                    DiagnosticCategory::InvalidContract,
                    "expected_typeql_define",
                    "schema compatibility input must contain only define queries",
                    query_span(&document, source, query.span)?,
                ));
            }
        }
    }
    if let Some(released) = released {
        restore_released_labels(released, &mut definables);
    }

    let declarations: Vec<&TypeDeclaration> = definables
        .iter()
        .filter_map(|definable| match definable {
            Definable::TypeDeclaration(declaration) => Some(declaration),
            _ => None,
        })
        .collect();
    let kinds = infer_type_kinds(&document, source, &declarations, released.is_some())?;
    let mut ids = BTreeMap::new();
    let mut assembler = FactAssembler::new(FormatVersion::V1);
    let mut function_body_references = BTreeMap::new();

    for (declaration, kind) in declarations.iter().zip(&kinds) {
        if released_declaration_is_omitted(declaration, omitted_released_declarations) {
            continue;
        }
        let label = typeql_label(&declaration.label);
        let declaration_span = source_span(&document, source, declaration.span)?;
        let id = TypeId::new(*kind, label.clone())
            .map_err(|diagnostic| contract(diagnostic, declaration_span.clone()))?;
        // Released renders re-open declared labels freely — kindless
        // standalone `plays` lines and explicit split declarations alike —
        // in any order. Every compatible re-opening merges into the one
        // identity; genuinely conflicting kinds were already rejected by
        // `infer_type_kinds` with both spans.
        if ids.contains_key(&label) {
            continue;
        }
        assembler.insert_fact(
            SchemaFact::Type(
                TypeFact::new(id.clone())
                    .map_err(|diagnostic| contract(diagnostic, declaration_span.clone()))?,
            ),
            declaration_span,
        )?;
        ids.entry(label).or_insert(id);
    }

    if released.is_none() {
        // Keep the strict adapter's original fact-insertion order and
        // duplicate diagnostics. Released merge behavior belongs only to the
        // compatibility projection below.
        for declaration in &declarations {
            let label = typeql_label(&declaration.label);
            let id = ids.get(&label).cloned().ok_or_else(|| {
                error(
                    DiagnosticCategory::InvalidContract,
                    "unknown_typeql_type",
                    format!("TypeQL declaration `{label}` has no inferred type identity"),
                    query_span(&document, source, declaration.span)
                        .ok()
                        .flatten(),
                )
            })?;
            insert_annotations(
                &mut assembler,
                AnnotationSubjectId::Type(id.clone()),
                &declaration.annotations,
                &document,
                source,
            )?;
            for capability in &declaration.capabilities {
                insert_capability(
                    &mut assembler,
                    &ids,
                    &id,
                    capability,
                    CapabilityAnnotations::Strict(&capability.annotations),
                    &document,
                    source,
                )?;
            }
        }
    } else {
        // Reproduce the released parser's merge algebra before entering the
        // strict fact assembler. Attribute declarations are map replacements
        // (the final declaration wins); entity and relation declarations merge,
        // with type annotations accumulated under their own last-write/OR
        // identities and `sub` taking the final spelling.
        let mut last_attribute_declaration = BTreeMap::new();
        let mut last_sub_capability = BTreeMap::new();
        let mut last_value_capability = BTreeMap::new();
        for (declaration_index, (declaration, kind)) in declarations.iter().zip(&kinds).enumerate()
        {
            if released_declaration_is_omitted(declaration, omitted_released_declarations) {
                continue;
            }
            let label = typeql_label(&declaration.label);
            if *kind == TypeKind::Attribute {
                last_attribute_declaration.insert(label.clone(), declaration_index);
            }
            for (capability_index, capability) in declaration.capabilities.iter().enumerate() {
                if matches!(&capability.base, CapabilityBase::Sub(_)) {
                    last_sub_capability
                        .insert(label.clone(), (declaration_index, capability_index));
                }
                if matches!(&capability.base, CapabilityBase::ValueType(_)) {
                    last_value_capability
                        .insert(label.clone(), (declaration_index, capability_index));
                }
            }
        }

        let mut merged_type_annotations: BTreeMap<String, Vec<Annotation>> = BTreeMap::new();
        let mut merged_value_annotations: BTreeMap<String, Vec<Annotation>> = BTreeMap::new();
        for (declaration_index, (declaration, kind)) in declarations.iter().zip(&kinds).enumerate()
        {
            if released_declaration_is_omitted(declaration, omitted_released_declarations) {
                continue;
            }
            let label = typeql_label(&declaration.label);
            if *kind == TypeKind::Attribute
                && last_attribute_declaration.get(&label) != Some(&declaration_index)
            {
                continue;
            }
            if let Some(released) = released {
                for annotation in declaration.annotations.iter().chain(
                    declaration
                        .capabilities
                        .iter()
                        .flat_map(|capability| &capability.annotations),
                ) {
                    match released_annotation_target(released, annotation) {
                        Some(ReleasedAnnotationTarget::Value) => merged_value_annotations
                            .entry(label.clone())
                            .or_default()
                            .push(annotation.clone()),
                        Some(ReleasedAnnotationTarget::Type) => merged_type_annotations
                            .entry(label.clone())
                            .or_default()
                            .push(annotation.clone()),
                        Some(ReleasedAnnotationTarget::Capability) => {}
                        None if *kind == TypeKind::Attribute => {
                            let target = released_attribute_annotation_target(annotation);
                            let annotations = match target {
                                ReleasedAnnotationTarget::Value => &mut merged_value_annotations,
                                ReleasedAnnotationTarget::Type => &mut merged_type_annotations,
                                ReleasedAnnotationTarget::Capability => continue,
                            };
                            annotations
                                .entry(label.clone())
                                .or_default()
                                .push(annotation.clone());
                        }
                        None => {}
                    }
                }
            } else {
                merged_type_annotations
                    .entry(label)
                    .or_default()
                    .extend(declaration.annotations.iter().cloned());
            }
        }
        for (label, annotations) in merged_type_annotations {
            let id = ids.get(&label).cloned().ok_or_else(|| {
                error(
                    DiagnosticCategory::InvalidContract,
                    "unknown_typeql_type",
                    format!("TypeQL declaration `{label}` has no inferred type identity"),
                    None,
                )
            })?;
            insert_released_annotations(
                &mut assembler,
                AnnotationSubjectId::Type(id.clone()),
                &annotations,
                &document,
                source,
            )?;
        }

        // The released generator observes the first owns/plays/relates capability
        // by identity, including that capability's annotations. Its parser keeps
        // duplicates in the initial declaration internally, but every released
        // emitter resolves the ordered name back to the first matching capability;
        // compatible projection therefore deduplicates both within one declaration
        // and across later reopenings. Attribute declarations are replacements,
        // and within the final declaration their last `sub` and `value` clauses win.
        let mut first_object_capability = BTreeMap::new();
        for (declaration_index, (declaration, kind)) in declarations.iter().zip(&kinds).enumerate()
        {
            if released_declaration_is_omitted(declaration, omitted_released_declarations) {
                continue;
            }
            let label = typeql_label(&declaration.label);
            if *kind == TypeKind::Attribute
                && last_attribute_declaration.get(&label) != Some(&declaration_index)
            {
                continue;
            }
            let id = ids.get(&label).cloned().ok_or_else(|| {
                error(
                    DiagnosticCategory::InvalidContract,
                    "unknown_typeql_type",
                    format!("TypeQL declaration `{label}` has no inferred type identity"),
                    query_span(&document, source, declaration.span)
                        .ok()
                        .flatten(),
                )
            })?;
            for (capability_index, capability) in declaration.capabilities.iter().enumerate() {
                if matches!(&capability.base, CapabilityBase::Sub(_))
                    && last_sub_capability.get(&label)
                        != Some(&(declaration_index, capability_index))
                {
                    continue;
                }
                if matches!(&capability.base, CapabilityBase::ValueType(_))
                    && last_value_capability.get(&label)
                        != Some(&(declaration_index, capability_index))
                {
                    continue;
                }
                if *kind != TypeKind::Attribute
                    && let Some(capability_id) = released_object_capability_identity(capability)
                {
                    let key = (label.clone(), capability_id);
                    if first_object_capability.contains_key(&key) {
                        continue;
                    }
                    first_object_capability.insert(key, (declaration_index, capability_index));
                }
                if omitted_released_capabilities.is_some_and(|omitted| {
                    capability
                        .span
                        .is_some_and(|span| omitted.contains(&span.begin_offset))
                }) {
                    continue;
                }
                let released_annotations;
                let annotations = if let Some(released) = released {
                    released_annotations = capability
                        .annotations
                        .iter()
                        .filter(|annotation| {
                            released_annotation_target(released, annotation)
                                == Some(ReleasedAnnotationTarget::Capability)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    released_annotations.as_slice()
                } else {
                    capability.annotations.as_slice()
                };
                insert_capability(
                    &mut assembler,
                    &ids,
                    &id,
                    capability,
                    CapabilityAnnotations::Released {
                        annotations,
                        played_role_declaration: capability.span.and_then(|span| {
                            played_role_declarations
                                .and_then(|declarations| declarations.get(&span.begin_offset))
                                .map(String::as_str)
                        }),
                    },
                    &document,
                    source,
                )?;
            }
        }

        for (label, annotations) in merged_value_annotations {
            let Some(first) = annotations.first() else {
                continue;
            };
            let annotation_span = source_span(&document, source, first.span())?;
            let attribute = AttributeId::new(&label)
                .map_err(|diagnostic| contract(diagnostic, annotation_span))?;
            insert_released_annotations(
                &mut assembler,
                AnnotationSubjectId::Value(ValueFactId::new(attribute)),
                &annotations,
                &document,
                source,
            )?;
        }
    }

    for definable in &definables {
        match definable {
            Definable::TypeDeclaration(_) => {}
            Definable::Struct(structure) => {
                insert_struct(&mut assembler, structure, &document, source)?;
            }
            Definable::Function(function) => {
                insert_function(&mut assembler, function, &document, source)?;
                let function_id = FunctionId::new(function.signature.ident.as_str_unchecked())
                    .expect("TypeQL emitted a function identifier rejected by the contract");
                let body_span = source_span(&document, source, function.block.span)?;
                let references =
                    function_references::collect_function_body_references(&function.block)
                        .map_err(|diagnostic| contract(diagnostic, body_span))?;
                function_body_references.insert(function_id, references);
            }
        }
    }

    assembler
        .finish()
        .map(|declared| TypeqlDeclaredSchema::new(declared, function_body_references))
}

fn restore_released_labels(released: &ReleasedSyntax, definables: &mut [Definable]) {
    for definable in definables {
        if let Definable::TypeDeclaration(declaration) = definable {
            released.restore_label(&mut declaration.label);
        }
    }
}

/// Parse one TypeQL `define` query into the canonical declared schema graph.
///
/// This compatibility wrapper discards only the derived function-body index;
/// declared facts and their fingerprints are unchanged.
pub fn typeql_to_declared(
    document: DocumentId,
    source: &str,
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    typeql_to_declared_with_references(document, source).map(TypeqlDeclaredSchema::into_declared)
}

/// Parse TypeQL and return canonical direct facts in identity order.
pub fn typeql_to_facts(
    document: DocumentId,
    source: &str,
) -> Result<Vec<SchemaFact>, SchemaDiagnostics> {
    let declared = typeql_to_declared(document, source)?;
    Ok(declared.facts().cloned().collect())
}

/// Transpile one legacy TOML schema and adapt its rendered TypeQL into the
/// canonical declared schema graph.
///
/// `rendered_typeql_document` identifies the generated TypeQL, not the input
/// TOML. Diagnostics from TypeQL parsing and adaptation therefore contain
/// offsets into the rendered source and never claim to locate original TOML
/// text. TOML decoding and validation diagnostics have no source span because
/// the legacy transpiler does not expose structured TOML locations.
pub fn toml_to_declared(
    rendered_typeql_document: DocumentId,
    toml_source: &str,
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    let rendered_typeql =
        type_bridge_toml_transpiler::toml_to_typeql(toml_source).map_err(|transpile_error| {
            error(
                DiagnosticCategory::InvalidContract,
                "invalid_toml_schema",
                format!("TOML schema transpilation failed: {transpile_error}"),
                None,
            )
        })?;
    typeql_to_declared(rendered_typeql_document, &rendered_typeql)
}

/// Transpile legacy TOML and return canonical direct facts in identity order.
///
/// See [`toml_to_declared`] for the provenance contract of
/// `rendered_typeql_document`.
pub fn toml_to_facts(
    rendered_typeql_document: DocumentId,
    toml_source: &str,
) -> Result<Vec<SchemaFact>, SchemaDiagnostics> {
    let declared = toml_to_declared(rendered_typeql_document, toml_source)?;
    Ok(declared.facts().cloned().collect())
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum ReleasedObjectCapabilityIdentity {
    Owns(String),
    Relates(String),
    Plays(String, String),
}

fn released_declaration_is_omitted(
    declaration: &TypeDeclaration,
    omitted: Option<&BTreeSet<usize>>,
) -> bool {
    omitted.is_some_and(|omitted| {
        declaration
            .span
            .is_some_and(|span| omitted.contains(&span.begin_offset))
    })
}

fn released_object_capability_identity(
    capability: &Capability,
) -> Option<ReleasedObjectCapabilityIdentity> {
    match &capability.base {
        CapabilityBase::Owns(owns) => plain_type_ref(&owns.owned)
            .ok()
            .map(ReleasedObjectCapabilityIdentity::Owns),
        CapabilityBase::Relates(relates) => plain_type_ref(&relates.related)
            .ok()
            .map(ReleasedObjectCapabilityIdentity::Relates),
        CapabilityBase::Plays(plays) => Some(ReleasedObjectCapabilityIdentity::Plays(
            typeql_label(&plays.role.scope),
            typeql_label(&plays.role.name),
        )),
        _ => None,
    }
}

fn released_annotation_target(
    released: &ReleasedSyntax,
    annotation: &Annotation,
) -> Option<ReleasedAnnotationTarget> {
    annotation
        .span()
        .map(|span| span.begin_offset)
        .and_then(|start| released.annotation_target(start))
}

fn released_attribute_annotation_target(annotation: &Annotation) -> ReleasedAnnotationTarget {
    match annotation {
        Annotation::Regex(_) | Annotation::Range(_) | Annotation::Values(_) => {
            ReleasedAnnotationTarget::Value
        }
        _ => ReleasedAnnotationTarget::Type,
    }
}

fn infer_type_kinds(
    document: &DocumentId,
    source: &str,
    declarations: &[&TypeDeclaration],
    released: bool,
) -> Result<Vec<TypeKind>, SchemaDiagnostics> {
    let mut inferred = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        let mut kind = declaration
            .kind
            .as_ref()
            .map(|kind| kind_from_token(&kind.to_string()))
            .transpose()
            .map_err(|message| {
                at(
                    document,
                    source,
                    declaration.span,
                    "unsupported_typeql_kind",
                    message,
                )
            })?;
        for capability in &declaration.capabilities {
            let hint = match &capability.base {
                CapabilityBase::ValueType(_) => Some(TypeKind::Attribute),
                CapabilityBase::Relates(_) => Some(TypeKind::Relation),
                CapabilityBase::Sub(sub) => root_kind(&typeql_label(&sub.supertype_label)),
                _ => None,
            };
            if let Some(hint) = hint {
                merge_kind(&mut kind, hint).map_err(|message| {
                    at(
                        document,
                        source,
                        capability.span,
                        "conflicting_typeql_kind",
                        message,
                    )
                })?;
            }
        }
        inferred.push(kind);
    }

    loop {
        let mut known = BTreeMap::new();
        for (declaration, kind) in declarations.iter().zip(&inferred) {
            if let Some(kind) = kind {
                let label = typeql_label(&declaration.label);
                if let Some(previous) = known.insert(label.clone(), *kind)
                    && previous != *kind
                {
                    return Err(at(
                        document,
                        source,
                        declaration.span,
                        "conflicting_typeql_kind",
                        format!("TypeQL label `{label}` is declared with incompatible kinds"),
                    ));
                }
            }
        }
        let mut changed = false;
        for (index, declaration) in declarations.iter().enumerate() {
            if inferred[index].is_some() {
                continue;
            }
            // A kindless statement re-opening an already-classified label
            // (released renders emit standalone `plays` lines this way)
            // inherits that label's kind.
            if let Some(own_kind) = known.get(&typeql_label(&declaration.label)) {
                inferred[index] = Some(*own_kind);
                changed = true;
                continue;
            }
            for capability in &declaration.capabilities {
                let CapabilityBase::Sub(sub) = &capability.base else {
                    continue;
                };
                if let Some(parent_kind) = known.get(&typeql_label(&sub.supertype_label)) {
                    inferred[index] = Some(*parent_kind);
                    changed = true;
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }

    if released {
        for (declaration, kind) in declarations.iter().zip(&mut inferred) {
            let plays_only = declaration.kind.is_none()
                && !declaration.capabilities.is_empty()
                && declaration
                    .capabilities
                    .iter()
                    .all(|capability| matches!(capability.base, CapabilityBase::Plays(_)));
            if !plays_only {
                continue;
            }
            match kind {
                Some(TypeKind::Attribute) => {
                    return Err(at(
                        document,
                        source,
                        declaration.span,
                        "conflicting_typeql_kind",
                        format!(
                            "released standalone plays label `{}` conflicts with an attribute declaration",
                            typeql_label(&declaration.label)
                        ),
                    ));
                }
                Some(TypeKind::Entity | TypeKind::Relation) => {}
                Some(TypeKind::Struct) => {
                    return Err(at(
                        document,
                        source,
                        declaration.span,
                        "conflicting_typeql_kind",
                        format!(
                            "released standalone plays label `{}` cannot resolve to a struct",
                            typeql_label(&declaration.label)
                        ),
                    ));
                }
                None => *kind = Some(TypeKind::Entity),
            }
        }
    }

    declarations
        .iter()
        .zip(inferred)
        .map(|(declaration, kind)| {
            kind.ok_or_else(|| {
                at(
                    document,
                    source,
                    declaration.span,
                    "ambiguous_typeql_kind",
                    format!(
                        "TypeQL declaration `{}` does not identify an entity, relation, or attribute kind",
                        typeql_label(&declaration.label)
                    ),
                )
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum CapabilityAnnotations<'a> {
    Strict(&'a [Annotation]),
    Released {
        annotations: &'a [Annotation],
        played_role_declaration: Option<&'a str>,
    },
}

impl CapabilityAnnotations<'_> {
    const fn annotations(&self) -> &[Annotation] {
        match self {
            Self::Strict(annotations) | Self::Released { annotations, .. } => annotations,
        }
    }

    const fn played_role_declaration(&self) -> Option<&str> {
        match self {
            Self::Strict(_) => None,
            Self::Released {
                played_role_declaration,
                ..
            } => *played_role_declaration,
        }
    }
}

fn insert_capability(
    assembler: &mut FactAssembler,
    ids: &BTreeMap<String, TypeId>,
    owner: &TypeId,
    capability: &Capability,
    annotations: CapabilityAnnotations<'_>,
    document: &DocumentId,
    source: &str,
) -> Result<(), SchemaDiagnostics> {
    let annotation_slice = annotations.annotations();
    let capability_span = source_span(document, source, capability.span)?;
    let subject = match &capability.base {
        CapabilityBase::Sub(sub) => {
            let parent_label = typeql_label(&sub.supertype_label);
            if root_kind(&parent_label).is_some() {
                reject_annotations_if_present(annotation_slice, document, source)?;
                return Ok(());
            }
            let parent = ids.get(&parent_label).cloned().ok_or_else(|| {
                at(
                    document,
                    source,
                    sub.span,
                    "unknown_typeql_parent",
                    format!("unknown TypeQL parent `{parent_label}`"),
                )
            })?;
            let id = SubFactId::new(owner.clone(), parent)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            assembler.insert_fact(SchemaFact::Sub(SubFact::new(id.clone())), capability_span)?;
            AnnotationSubjectId::Sub(id)
        }
        CapabilityBase::ValueType(value) => {
            let value_type = named_value_type(&value.value_type).map_err(|message| {
                at(
                    document,
                    source,
                    value.span,
                    "unsupported_typeql_value_type",
                    message,
                )
            })?;
            let attribute = AttributeId::new(owner.label().as_str())
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            let id = ValueFactId::new(attribute);
            assembler.insert_fact(
                SchemaFact::Value(ValueFact::new(id.clone(), value_type)),
                capability_span,
            )?;
            AnnotationSubjectId::Value(id)
        }
        CapabilityBase::Owns(owns) => {
            let attribute_label = plain_type_ref(&owns.owned).map_err(|message| {
                at(
                    document,
                    source,
                    owns.span,
                    "unsupported_typeql_owns",
                    message,
                )
            })?;
            let attribute_type = ids.get(&attribute_label).ok_or_else(|| {
                at(
                    document,
                    source,
                    owns.span,
                    "unknown_typeql_attribute",
                    format!("unknown owned attribute `{attribute_label}`"),
                )
            })?;
            if attribute_type.kind() != TypeKind::Attribute {
                return Err(at(
                    document,
                    source,
                    owns.span,
                    "invalid_typeql_owned_kind",
                    "TypeQL owns targets must be attribute types",
                ));
            }
            let attribute = AttributeId::new(attribute_label)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            let id = OwnsFactId::new(owner.clone(), attribute)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            assembler.insert_fact(SchemaFact::Owns(OwnsFact::new(id.clone())), capability_span)?;
            AnnotationSubjectId::Owns(id)
        }
        CapabilityBase::Relates(relates) => {
            let role_label = plain_type_ref(&relates.related).map_err(|message| {
                at(
                    document,
                    source,
                    relates.span,
                    "unsupported_typeql_relates",
                    message,
                )
            })?;
            let role = RoleId::new(owner.label().as_str(), &role_label)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            let id = RelatesFactId::new(owner.clone(), role)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            let specializes = relates
                .specialised
                .as_ref()
                .map(|specialized| {
                    plain_type_ref(specialized)
                        .map_err(|message| {
                            at(
                                document,
                                source,
                                specialized.span(),
                                "unsupported_typeql_role_specialization",
                                message,
                            )
                        })
                        .and_then(|label| {
                            let span = source_span(document, source, specialized.span())?;
                            let label = Label::new(label)
                                .map_err(|diagnostic| contract(diagnostic, span.clone()))?;
                            Ok((label, span))
                        })
                })
                .transpose()?;
            assembler.insert_relates(id.clone(), specializes, capability_span)?;
            AnnotationSubjectId::Relates(id)
        }
        CapabilityBase::Plays(plays) => {
            let relation_label = annotations
                .played_role_declaration()
                .map(str::to_owned)
                .unwrap_or_else(|| typeql_label(&plays.role.scope));
            let role_label = typeql_label(&plays.role.name);
            let relation = Label::new(&relation_label)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            let role = Label::new(&role_label)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            assembler.insert_plays(
                owner.label().clone(),
                relation,
                role,
                capability_span.clone(),
            );
            let role_id = RoleId::new(relation_label, role_label)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            let id = PlaysFactId::new(owner.clone(), role_id)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            AnnotationSubjectId::Plays(id)
        }
        CapabilityBase::Alias(alias) => {
            return Err(at(
                document,
                source,
                alias.span,
                "unsupported_typeql_alias",
                "TypeQL aliases require an explicit capability contract",
            ));
        }
    };

    match annotations {
        CapabilityAnnotations::Strict(_) => {
            insert_annotations(assembler, subject, annotation_slice, document, source)
        }
        CapabilityAnnotations::Released { .. } => {
            insert_released_annotations(assembler, subject, annotation_slice, document, source)
        }
    }
}

fn insert_struct(
    assembler: &mut FactAssembler,
    structure: &Struct,
    document: &DocumentId,
    source: &str,
) -> Result<(), SchemaDiagnostics> {
    if let Some(annotation) = structure.annotations.first() {
        return Err(at(
            document,
            source,
            annotation.span(),
            "unsupported_typeql_struct_annotation",
            "struct annotations are not part of the portable V2 fact contract",
        ));
    }
    let structure_span = source_span(document, source, structure.span)?;
    let id = StructId::new(structure.ident.as_str_unchecked())
        .map_err(|diagnostic| contract(diagnostic, structure_span.clone()))?;
    let mut fields = Vec::with_capacity(structure.fields.len());
    for field in &structure.fields {
        if let Some(annotation) = field.annotations.first() {
            return Err(at(
                document,
                source,
                annotation.span(),
                "unsupported_typeql_struct_field_annotation",
                "struct field annotations are not live-pinned for the portable contract",
            ));
        }
        let (named, optional) = simple_or_optional_named(&field.type_).map_err(|message| {
            at(
                document,
                source,
                field.span,
                "unsupported_typeql_struct_field",
                message,
            )
        })?;
        let value_type = named_value_type(named).map_err(|message| {
            at(
                document,
                source,
                field.span,
                "unsupported_typeql_struct_field",
                message,
            )
        })?;
        let field_span = source_span(document, source, field.span)?;
        let name = Label::new(field.key.as_str_unchecked())
            .map_err(|diagnostic| contract(diagnostic, field_span))?;
        fields.push(StructField::new(name, value_type, optional));
    }
    let fact = StructFact::new(id, fields)
        .map_err(|diagnostic| contract(diagnostic, structure_span.clone()))?;
    assembler.insert_fact(SchemaFact::Struct(fact), structure_span)
}

fn insert_function(
    assembler: &mut FactAssembler,
    function: &Function,
    document: &DocumentId,
    source: &str,
) -> Result<(), SchemaDiagnostics> {
    let function_span = source_span(document, source, function.span)?;
    let id = FunctionId::new(function.signature.ident.as_str_unchecked())
        .map_err(|diagnostic| contract(diagnostic, function_span.clone()))?;
    let mut parameters = Vec::with_capacity(function.signature.args.len());
    for argument in &function.signature.args {
        let (named, optional) = simple_or_optional_named(&argument.type_).map_err(|message| {
            at(
                document,
                source,
                argument.span,
                "unsupported_typeql_function_parameter",
                message,
            )
        })?;
        if optional {
            return Err(at(
                document,
                source,
                argument.span,
                "unsupported_typeql_function_parameter",
                "optional function parameters are not part of the V2 signature contract",
            ));
        }
        let name = argument.var.name().ok_or_else(|| {
            at(
                document,
                source,
                argument.span,
                "anonymous_typeql_function_parameter",
                "function parameters must use named variables",
            )
        })?;
        let argument_span = source_span(document, source, argument.span)?;
        let name = Label::new(name).map_err(|diagnostic| contract(diagnostic, argument_span))?;
        parameters.push(FunctionParameter::new(name, type_reference(named)?));
    }
    let returns = match &function.signature.output {
        Output::Single(single) => {
            let elements = return_elements(&single.types, document, source, single.span)?;
            if elements.len() == 1 {
                FunctionReturnMode::scalar(elements.into_iter().next().expect("one return element"))
            } else {
                FunctionReturnMode::tuple(elements)
                    .map_err(|diagnostic| contract(diagnostic, function_span.clone()))?
            }
        }
        Output::Stream(stream) => FunctionReturnMode::stream(return_elements(
            &stream.types,
            document,
            source,
            stream.span,
        )?)
        .map_err(|diagnostic| contract(diagnostic, function_span.clone()))?,
    };
    let signature = FunctionSignature::new(parameters, returns)
        .map_err(|diagnostic| contract(diagnostic, function_span.clone()))?;
    let block_span = function.block.span.ok_or_else(|| {
        error(
            DiagnosticCategory::InvalidContract,
            "missing_typeql_function_body_span",
            "TypeQL parser did not retain the function body span",
            Some(function_span.clone()),
        )
    })?;
    let body_text = source
        .get(block_span.begin_offset..block_span.end_offset)
        .ok_or_else(|| {
            error(
                DiagnosticCategory::InvalidContract,
                "invalid_typeql_function_body_span",
                "TypeQL function body span is outside the original source",
                Some(function_span.clone()),
            )
        })?;
    let body_span = source_span(document, source, Some(block_span))?;
    let body =
        FunctionBody::new(body_text).map_err(|diagnostic| contract(diagnostic, body_span))?;
    assembler.insert_fact(
        SchemaFact::Function(FunctionFact::new(id.clone(), signature, body)),
        function_span,
    )?;
    insert_annotations(
        assembler,
        AnnotationSubjectId::Function(id),
        &function.annotations,
        document,
        source,
    )
}

fn return_elements(
    types: &[NamedTypeAny],
    document: &DocumentId,
    source: &str,
    span: Option<Span>,
) -> Result<Vec<FunctionReturnElement>, SchemaDiagnostics> {
    types
        .iter()
        .map(|type_| {
            let (named, optional) = simple_or_optional_named(type_).map_err(|message| {
                at(
                    document,
                    source,
                    span,
                    "unsupported_typeql_function_return",
                    message,
                )
            })?;
            Ok(FunctionReturnElement::new(type_reference(named)?, optional))
        })
        .collect()
}

fn insert_annotations(
    assembler: &mut FactAssembler,
    subject: AnnotationSubjectId,
    annotations: &[Annotation],
    document: &DocumentId,
    source: &str,
) -> Result<(), SchemaDiagnostics> {
    for annotation in annotations {
        insert_released_annotations(
            assembler,
            subject.clone(),
            core::slice::from_ref(annotation),
            document,
            source,
        )?;
    }
    Ok(())
}

fn insert_released_annotations(
    assembler: &mut FactAssembler,
    subject: AnnotationSubjectId,
    annotations: &[Annotation],
    document: &DocumentId,
    source: &str,
) -> Result<(), SchemaDiagnostics> {
    // The frozen parser stores one value per annotation identity: presence
    // flags accumulate, while doc/regex/values/card and each meta key use the
    // final spelling. Repeated attribute ranges update only the bounds present
    // in each spelling (`@range(1..) @range(..5)` becomes `1..5`). Stage the
    // merged facts before handing them to the strict assembler so compatible
    // repetitions do not look like direct fact duplication.
    let mut merged = BTreeMap::new();
    let mut released_range: Option<(
        Option<&typeql::value::Literal>,
        Option<&typeql::value::Literal>,
        SourceSpan,
    )> = None;
    for annotation in annotations {
        let annotation_span = source_span(document, source, annotation.span())?;
        if let Annotation::Range(range) = annotation {
            let (lower, upper, latest_span) =
                released_range.get_or_insert((None, None, annotation_span.clone()));
            if let Some(minimum) = range.min.as_ref() {
                *lower = Some(minimum);
            }
            if let Some(maximum) = range.max.as_ref() {
                *upper = Some(maximum);
            }
            *latest_span = annotation_span;
            continue;
        }
        let kind = annotation_identity_kind(annotation, annotation_span.clone())?;
        merged.insert(
            AnnotationFactId::new(subject.clone(), kind),
            (annotation, annotation_span),
        );
    }
    for (id, (annotation, annotation_span)) in merged {
        let (kind, value) = match annotation {
            Annotation::Abstract(_) => {
                (AnnotationKindId::Abstract, SchemaAnnotationValue::Presence)
            }
            Annotation::Independent(_) => (
                AnnotationKindId::Independent,
                SchemaAnnotationValue::Presence,
            ),
            Annotation::Key(_) => (AnnotationKindId::Key, SchemaAnnotationValue::Presence),
            Annotation::Unique(_) => (AnnotationKindId::Unique, SchemaAnnotationValue::Presence),
            Annotation::Cardinality(cardinality) => {
                let cardinality = match &cardinality.range {
                    CardinalityRange::Exact(exact) => {
                        let exact = parse_u64(&exact.value, &annotation_span)?;
                        Cardinality::new(exact, Some(exact))
                    }
                    CardinalityRange::Range(minimum, maximum) => Cardinality::new(
                        parse_u64(&minimum.value, &annotation_span)?,
                        maximum
                            .as_ref()
                            .map(|maximum| parse_u64(&maximum.value, &annotation_span))
                            .transpose()?,
                    ),
                }
                .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
                (
                    AnnotationKindId::Card,
                    SchemaAnnotationValue::Cardinality(cardinality),
                )
            }
            Annotation::Regex(regex) => {
                validate_annotation_string(&regex.regex, "regex", annotation_span.clone())?;
                let text = regex.regex.unescape_regex().map_err(|unescape_error| {
                    error(
                        DiagnosticCategory::InvalidContract,
                        "invalid_typeql_regex",
                        format!("TypeQL regex decoding failed: {unescape_error}"),
                        Some(annotation_span.clone()),
                    )
                })?;
                let pattern = RegexPattern::new(text)
                    .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
                (
                    AnnotationKindId::Regex,
                    SchemaAnnotationValue::Regex(pattern),
                )
            }
            Annotation::Doc(doc) => {
                validate_annotation_string(&doc.doc, "doc", annotation_span.clone())?;
                let text = doc.doc.unescape().map_err(|unescape_error| {
                    error(
                        DiagnosticCategory::InvalidContract,
                        "invalid_typeql_doc",
                        format!("TypeQL documentation decoding failed: {unescape_error}"),
                        Some(annotation_span.clone()),
                    )
                })?;
                let text = DocText::new(text)
                    .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
                (AnnotationKindId::Doc, SchemaAnnotationValue::Doc(text))
            }
            Annotation::Meta(meta) => {
                validate_annotation_string(&meta.key, "meta_key", annotation_span.clone())?;
                let key = meta.key.unescape().map_err(|unescape_error| {
                    error(
                        DiagnosticCategory::InvalidContract,
                        "invalid_typeql_meta_key",
                        format!("TypeQL metadata key decoding failed: {unescape_error}"),
                        Some(annotation_span.clone()),
                    )
                })?;
                validate_annotation_string(&meta.value, "meta_value", annotation_span.clone())?;
                let value = meta.value.unescape().map_err(|unescape_error| {
                    error(
                        DiagnosticCategory::InvalidContract,
                        "invalid_typeql_meta_value",
                        format!("TypeQL metadata value decoding failed: {unescape_error}"),
                        Some(annotation_span.clone()),
                    )
                })?;
                let kind = AnnotationKindId::meta(key)
                    .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
                let value = CanonicalString::new(value)
                    .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
                (
                    kind,
                    SchemaAnnotationValue::Meta(CanonicalValue::String(value)),
                )
            }
            Annotation::Cascade(_) | Annotation::Distinct(_) | Annotation::Subkey(_) => {
                return Err(error(
                    DiagnosticCategory::UnsupportedCapability,
                    "unsupported_typeql_annotation",
                    "TypeQL annotation has no portable V2 annotation identity",
                    Some(annotation_span),
                ));
            }
            Annotation::Range(range) => {
                let lower = range
                    .min
                    .as_ref()
                    .map(|literal| annotation_literal(literal, document, source))
                    .transpose()?;
                let upper = range
                    .max
                    .as_ref()
                    .map(|literal| annotation_literal(literal, document, source))
                    .transpose()?;
                let range = CanonicalValueRange::new(lower, upper)
                    .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
                (AnnotationKindId::Range, SchemaAnnotationValue::Range(range))
            }
            Annotation::Values(values) => {
                if values.values.len() > MAX_CANONICAL_COLLECTION_LEN {
                    return Err(error(
                        DiagnosticCategory::InvalidContract,
                        "values_annotation_member_limit_exceeded",
                        format!(
                            "@values contains {} members; the maximum is {MAX_CANONICAL_COLLECTION_LEN}",
                            values.values.len()
                        ),
                        Some(annotation_span),
                    ));
                }
                let values = values
                    .values
                    .iter()
                    .map(|literal| annotation_literal(literal, document, source))
                    .collect::<Result<Vec<_>, _>>()?;
                let values = CanonicalValueSet::new(values)
                    .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
                (
                    AnnotationKindId::Values,
                    SchemaAnnotationValue::Values(values),
                )
            }
        };
        debug_assert_eq!(id.kind(), &kind);
        let fact = AnnotationFact::new(id, value)
            .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
        assembler.insert_fact(SchemaFact::Annotation(fact), annotation_span)?;
    }
    if let Some((lower, upper, annotation_span)) = released_range
        && (lower.is_some() || upper.is_some())
    {
        let lower = lower
            .map(|literal| annotation_literal(literal, document, source))
            .transpose()?;
        let upper = upper
            .map(|literal| annotation_literal(literal, document, source))
            .transpose()?;
        let range = CanonicalValueRange::new(lower, upper)
            .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
        let id = AnnotationFactId::new(subject, AnnotationKindId::Range);
        let fact = AnnotationFact::new(id, SchemaAnnotationValue::Range(range))
            .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
        assembler.insert_fact(SchemaFact::Annotation(fact), annotation_span)?;
    }
    Ok(())
}

fn annotation_identity_kind(
    annotation: &Annotation,
    annotation_span: SourceSpan,
) -> Result<AnnotationKindId, SchemaDiagnostics> {
    Ok(match annotation {
        Annotation::Abstract(_) => AnnotationKindId::Abstract,
        Annotation::Independent(_) => AnnotationKindId::Independent,
        Annotation::Key(_) => AnnotationKindId::Key,
        Annotation::Unique(_) => AnnotationKindId::Unique,
        Annotation::Cardinality(_) => AnnotationKindId::Card,
        Annotation::Regex(_) => AnnotationKindId::Regex,
        Annotation::Doc(_) => AnnotationKindId::Doc,
        Annotation::Range(_) => AnnotationKindId::Range,
        Annotation::Values(_) => AnnotationKindId::Values,
        Annotation::Meta(meta) => {
            validate_annotation_string(&meta.key, "meta_key", annotation_span.clone())?;
            let key = meta.key.unescape().map_err(|unescape_error| {
                error(
                    DiagnosticCategory::InvalidContract,
                    "invalid_typeql_meta_key",
                    format!("TypeQL metadata key decoding failed: {unescape_error}"),
                    Some(annotation_span.clone()),
                )
            })?;
            AnnotationKindId::meta(key)
                .map_err(|diagnostic| contract(diagnostic, annotation_span))?
        }
        Annotation::Cascade(_) | Annotation::Distinct(_) | Annotation::Subkey(_) => {
            return Err(error(
                DiagnosticCategory::UnsupportedCapability,
                "unsupported_typeql_annotation",
                "TypeQL annotation has no portable V2 annotation identity",
                Some(annotation_span),
            ));
        }
    })
}

fn validate_annotation_string(
    value: &typeql::value::StringLiteral,
    domain: &'static str,
    span: SourceSpan,
) -> Result<(), SchemaDiagnostics> {
    validate_quoted_string(value, domain).map_err(|conversion| {
        error(
            DiagnosticCategory::InvalidContract,
            conversion.code(),
            conversion.message(),
            Some(span),
        )
    })
}

fn annotation_literal(
    literal: &typeql::value::Literal,
    document: &DocumentId,
    source: &str,
) -> Result<CanonicalValue, SchemaDiagnostics> {
    canonical_literal(literal).map_err(|conversion| {
        at(
            document,
            source,
            literal.span,
            conversion.code(),
            conversion.message(),
        )
    })
}

fn reject_annotations_if_present(
    annotations: &[Annotation],
    document: &DocumentId,
    source: &str,
) -> Result<(), SchemaDiagnostics> {
    if let Some(annotation) = annotations.first() {
        return Err(at(
            document,
            source,
            annotation.span(),
            "unsupported_root_sub_annotation",
            "annotations on built-in root sub declarations have no portable subject identity",
        ));
    }
    Ok(())
}

fn plain_type_ref(reference: &TypeRefAny) -> Result<String, String> {
    match reference {
        TypeRefAny::Type(TypeRef::Label(label)) => Ok(typeql_label(label)),
        TypeRefAny::Type(TypeRef::Scoped(_)) => {
            Err("scoped type references are not valid in this capability".to_owned())
        }
        TypeRefAny::Type(TypeRef::Variable(_)) => {
            Err("type variables are not valid in schema declarations".to_owned())
        }
        TypeRefAny::List(_) => Err("list capability references are not live-pinned".to_owned()),
    }
}

fn simple_or_optional_named(reference: &NamedTypeAny) -> Result<(&NamedType, bool), String> {
    match reference {
        NamedTypeAny::Simple(named) => Ok((named, false)),
        NamedTypeAny::Optional(optional) => Ok((&optional.inner, true)),
        NamedTypeAny::List(_) => Err("list type references are not live-pinned".to_owned()),
    }
}

fn type_reference(named: &NamedType) -> Result<TypeReference, SchemaDiagnostics> {
    TypeReference::from_token(named.to_string()).map_err(contract_without_span)
}

fn named_value_type(named: &NamedType) -> Result<ValueTypeTag, String> {
    match named {
        NamedType::BuiltinValueType(value_type) => {
            value_type_from_token(&value_type.token.to_string())
        }
        // Released schema text spells some builtins through the frozen
        // alias table (`int`, `long`, `bool`); the strict grammar lexes
        // those as labels, and this compatibility front-end honors them.
        NamedType::Label(label) => value_type_from_token(&typeql_label(label)),
    }
}

fn value_type_from_token(token: &str) -> Result<ValueTypeTag, String> {
    match token {
        "string" => Ok(ValueTypeTag::String),
        "integer" | "int" | "long" => Ok(ValueTypeTag::Long),
        "double" => Ok(ValueTypeTag::Double),
        "boolean" | "bool" => Ok(ValueTypeTag::Boolean),
        "date" => Ok(ValueTypeTag::Date),
        "datetime" => Ok(ValueTypeTag::DateTime),
        "datetime-tz" => Ok(ValueTypeTag::DateTimeTz),
        "decimal" => Ok(ValueTypeTag::Decimal),
        "duration" => Ok(ValueTypeTag::Duration),
        _ => Err(format!("unsupported TypeQL value type `{token}`")),
    }
}

fn kind_from_token(token: &str) -> Result<TypeKind, String> {
    match token {
        "entity" => Ok(TypeKind::Entity),
        "relation" => Ok(TypeKind::Relation),
        "attribute" => Ok(TypeKind::Attribute),
        "role" => Err("roles must be declared through relation relates facts".to_owned()),
        _ => Err(format!("unsupported TypeQL type kind `{token}`")),
    }
}

fn root_kind(label: &str) -> Option<TypeKind> {
    match label {
        "entity" => Some(TypeKind::Entity),
        "relation" => Some(TypeKind::Relation),
        "attribute" => Some(TypeKind::Attribute),
        _ => None,
    }
}

fn merge_kind(kind: &mut Option<TypeKind>, hint: TypeKind) -> Result<(), String> {
    match kind {
        Some(current) if *current != hint => {
            Err("TypeQL declaration contains incompatible kind evidence".to_owned())
        }
        Some(_) => Ok(()),
        None => {
            *kind = Some(hint);
            Ok(())
        }
    }
}

fn typeql_label(label: &typeql::type_::Label) -> String {
    label.ident.as_str_unchecked().to_owned()
}

fn parse_u64(value: &str, span: &SourceSpan) -> Result<u64, SchemaDiagnostics> {
    value.parse().map_err(|_| {
        error(
            DiagnosticCategory::InvalidContract,
            "invalid_typeql_cardinality",
            "TypeQL cardinality is outside the portable unsigned 64-bit domain",
            Some(span.clone()),
        )
    })
}

fn source_span(
    document: &DocumentId,
    source: &str,
    span: Option<Span>,
) -> Result<SourceSpan, SchemaDiagnostics> {
    let span = span.unwrap_or(Span {
        begin_offset: 0,
        end_offset: source.len(),
    });
    if span.begin_offset > span.end_offset
        || span.end_offset > source.len()
        || !source.is_char_boundary(span.begin_offset)
        || !source.is_char_boundary(span.end_offset)
    {
        return Err(error(
            DiagnosticCategory::InvalidContract,
            "invalid_typeql_source_span",
            "TypeQL parser returned a source span outside the original input",
            None,
        ));
    }
    let (line, column) = line_column(source, span.begin_offset)?;
    let (end_line, end_column) = line_column(source, span.end_offset)?;
    SourceSpan::new(
        document.clone(),
        span.begin_offset as u64,
        span.end_offset as u64,
        line,
        column,
        end_line,
        end_column,
    )
    .map_err(contract_without_span)
}

fn query_span(
    document: &DocumentId,
    source: &str,
    span: Option<Span>,
) -> Result<Option<SourceSpan>, SchemaDiagnostics> {
    source_span(document, source, span).map(Some)
}

fn line_column(source: &str, offset: usize) -> Result<(u32, u32), SchemaDiagnostics> {
    let prefix = &source[..offset];
    let line_count = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column_count = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, tail)| tail)
        .chars()
        .count()
        + 1;
    let line = u32::try_from(line_count).map_err(|_| {
        error(
            DiagnosticCategory::InvalidContract,
            "typeql_source_position_overflow",
            "TypeQL source line exceeds the portable position domain",
            None,
        )
    })?;
    let column = u32::try_from(column_count).map_err(|_| {
        error(
            DiagnosticCategory::InvalidContract,
            "typeql_source_position_overflow",
            "TypeQL source column exceeds the portable position domain",
            None,
        )
    })?;
    Ok((line, column))
}

fn at(
    document: &DocumentId,
    source: &str,
    span: Option<Span>,
    code: &'static str,
    message: impl Into<String>,
) -> SchemaDiagnostics {
    match source_span(document, source, span) {
        Ok(span) => error(
            DiagnosticCategory::InvalidContract,
            code,
            message,
            Some(span),
        ),
        Err(error) => error,
    }
}

fn error(
    category: DiagnosticCategory,
    code: &'static str,
    message: impl Into<String>,
    primary: Option<SourceSpan>,
) -> SchemaDiagnostics {
    let diagnostic = Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static schema compatibility diagnostic code is valid"),
        message,
    );
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, primary))
}

fn contract(diagnostic: Diagnostic, span: SourceSpan) -> SchemaDiagnostics {
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, Some(span)))
}

fn contract_without_span(diagnostic: Diagnostic) -> SchemaDiagnostics {
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_generator_size_policy_does_not_widen_defensive_inputs() {
        assert!(TypeqlSourceSizePolicy::Defensive.allows(MAX_TYPEQL_SCHEMA_BYTES));
        assert!(!TypeqlSourceSizePolicy::Defensive.allows(MAX_TYPEQL_SCHEMA_BYTES + 1));
        assert!(TypeqlSourceSizePolicy::TrustedGenerator.allows(MAX_TYPEQL_SCHEMA_BYTES + 1));
    }

    #[test]
    fn deep_role_specialization_index_uses_heap_stack_and_one_tree_walk() {
        const DEPTH: usize = 25_000;
        let mut relation_names = BTreeSet::new();
        let mut role_names_by_relation = BTreeMap::<String, Vec<String>>::new();
        let mut roles = BTreeMap::new();
        let mut parents = BTreeMap::new();

        for index in 0..DEPTH {
            let relation = format!("relation-{index}");
            let role = if index == 0 {
                "root-role".to_owned()
            } else {
                format!("role-{index}")
            };
            relation_names.insert(relation.clone());
            role_names_by_relation.insert(relation.clone(), vec![role.clone()]);
            roles.insert(
                (relation.clone(), role),
                ReleasedIndexedRole {
                    capability_index: index,
                    specializes: (index != 0).then(|| "root-role".to_owned()),
                },
            );
            if index != 0 {
                parents.insert(relation, format!("relation-{}", index - 1));
            }
        }

        let leaf = format!("relation-{}", DEPTH - 1);
        let leaf_role = format!("role-{}", DEPTH - 1);
        let played = BTreeMap::from([(
            leaf.clone(),
            BTreeSet::from(["root-role".to_owned(), leaf_role.clone()]),
        )]);
        let resolution = released_role_validities(
            &relation_names,
            &role_names_by_relation,
            &played,
            &roles,
            &parents,
        );

        assert_eq!(resolution.direct.len(), DEPTH);
        assert!(
            resolution
                .direct
                .values()
                .all(|validity| *validity == ReleasedRoleValidity::Valid)
        );
        assert_eq!(
            resolution.plays.get(&(leaf.clone(), leaf_role)),
            Some(&ReleasedRoleValidity::Valid)
        );
        assert_eq!(
            resolution.plays.get(&(leaf, "root-role".to_owned())),
            Some(&ReleasedRoleValidity::Invalid)
        );
    }
}
