//! Adopted-genesis artifact parsing for legacy (v1) scope cutover.
//!
//! Adopting a released v1 database into canonical V2 history records the
//! independently verified head snapshot verbatim as `adopted-genesis.typeql`
//! beside the canonical migration manifests. The live pre-adoption export is
//! comparison evidence only. Every later genesis resolution re-parses the
//! snapshot bytes through the same function here, so the schema every
//! parentless manifest verifies against is fixed by one immutable, reviewable
//! artifact rather than rebuilt from a live connection.
//!
//! A pre-adoption v1 export contains the user schema plus the frozen v1
//! migration-ledger schema and nothing else: the V2 control namespace is
//! installed only by canonical apply, which has not run yet. Parsing
//! therefore rejects any reserved-namespace fact outright, requires the
//! ledger partition to be absent or exactly the frozen contract, and
//! returns the remaining facts as the adopted genesis.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use serde_json::Value;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use type_bridge_contract::id::{AttributeId, FunctionId, RoleId, StructId, TypeId, TypeKind};
use type_bridge_contract::reserved::is_typebridge_internal_label;
use type_bridge_contract::schema::{
    AnnotationSubjectId, DeclaredSchema, DocumentId, OwnsFactId, RelatesFactId, SchemaFact,
    SchemaFactId, SourcedSchemaFact,
};

use crate::function_references::reject_reserved_function_references;
use crate::{released_typeql_to_declared_projection_with_references, typeql_to_declared};

/// Lossless authority view of one released pre-adoption schema.
///
/// `declared` is the portable V2 fact projection used by canonical migration
/// verification. `legacy_identity` additionally binds every construct the
/// released parser understands, including ordered list capabilities and
/// `@distinct`, `@cascade`, and `@subkey`; comparison must use both. The raw
/// TypeQL remains the durable artifact and can always be re-parsed into this
/// exact pair.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptedGenesisAuthority {
    declared: DeclaredSchema,
    legacy_identity: Fingerprint,
    released_extension_identity: Fingerprint,
    released_extensions: Vec<ReleasedExtensionFact>,
}

impl AdoptedGenesisAuthority {
    /// Borrow the portable declared-fact projection.
    pub const fn declared(&self) -> &DeclaredSchema {
        &self.declared
    }

    /// Borrow the lossless released-schema identity fingerprint.
    pub const fn legacy_identity(&self) -> &Fingerprint {
        &self.legacy_identity
    }

    /// Borrow the identity of direct constructs absent from the V2 fact graph.
    pub const fn released_extension_identity(&self) -> &Fingerprint {
        &self.released_extension_identity
    }

    /// Require another raw schema observation to carry exactly the same
    /// released-only compatibility constructs.
    ///
    /// Callers must derive `live` from the same provider export used for the
    /// accompanying portable-fact comparison. Comparing a separate export
    /// would reintroduce a schema-observation race at the authority boundary.
    pub fn ensure_released_extension_identity_matches(
        &self,
        live: &Self,
    ) -> Result<(), Diagnostic> {
        if self.released_extension_identity == live.released_extension_identity {
            Ok(())
        } else {
            Err(failure(
                DiagnosticCategory::Integrity,
                "migration_adopted_extension_drift",
                "live released-only schema extensions differ from adopted-genesis authority",
            ))
        }
    }

    /// Require a canonical target to retain every fact carrying a V1-only extension.
    ///
    /// Ordered/list capabilities and `@distinct`, `@cascade`, and `@subkey`
    /// are deliberately outside the portable V2 fact vocabulary. A canonical
    /// migration may safely evolve other facts around them, but removing the
    /// underlying ownership or related-role fact would silently erase schema
    /// authority that the migration cannot describe or restore.
    pub fn ensure_released_extension_subjects_survive(
        &self,
        target: &DeclaredSchema,
    ) -> Result<(), Diagnostic> {
        for extension in &self.released_extensions {
            match extension {
                ReleasedExtensionFact::OmittedFunction { name, .. } => {
                    let id = SchemaFactId::Function(FunctionId::new(name.as_str())?);
                    if target.fact(&id).is_some() {
                        return Err(failure(
                            DiagnosticCategory::Integrity,
                            "adopted_genesis_extension_subject_modified",
                            "canonical migration target defines a function whose released definition is retained outside the portable fact graph",
                        )
                        .with_detail("subject", format!("{id:?}")));
                    }
                    continue;
                }
                ReleasedExtensionFact::OmittedStruct { name, .. } => {
                    let id = SchemaFactId::Struct(StructId::new(name.as_str())?);
                    if target.fact(&id).is_some() {
                        return Err(failure(
                            DiagnosticCategory::Integrity,
                            "adopted_genesis_extension_subject_modified",
                            "canonical migration target defines a struct whose released definition is retained outside the portable fact graph",
                        )
                        .with_detail("subject", format!("{id:?}")));
                    }
                    continue;
                }
                ReleasedExtensionFact::Owns { .. } | ReleasedExtensionFact::Relates { .. } => {}
            }
            let (subject, annotation_subject, extension_owner) = match extension {
                ReleasedExtensionFact::Owns {
                    owner_kind,
                    owner,
                    attribute,
                    ..
                } => {
                    let kind = match *owner_kind {
                        "entity" => TypeKind::Entity,
                        "relation" => TypeKind::Relation,
                        _ => {
                            return Err(failure(
                                DiagnosticCategory::Integrity,
                                "adopted_genesis_extension_subject_invalid",
                                "released ownership extension has an invalid owner kind",
                            ));
                        }
                    };
                    let id = OwnsFactId::new(
                        TypeId::new(kind, owner.as_str())?,
                        AttributeId::new(attribute.as_str())?,
                    )?;
                    let extension_owner = id.owner().clone();
                    (
                        SchemaFactId::Owns(id.clone()),
                        AnnotationSubjectId::Owns(id),
                        extension_owner,
                    )
                }
                ReleasedExtensionFact::Relates { relation, role, .. } => {
                    let id = RelatesFactId::new(
                        TypeId::new(TypeKind::Relation, relation.as_str())?,
                        RoleId::new(relation.as_str(), role.as_str())?,
                    )?;
                    let extension_owner = id.relation().clone();
                    (
                        SchemaFactId::Relates(id.clone()),
                        AnnotationSubjectId::Relates(id),
                        extension_owner,
                    )
                }
                ReleasedExtensionFact::OmittedFunction { .. }
                | ReleasedExtensionFact::OmittedStruct { .. } => {
                    unreachable!("omitted definitions returned above")
                }
            };
            if target.fact(&subject).is_none() {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "adopted_genesis_extension_subject_removed",
                    "canonical migration target removes a fact carrying a released-only adopted-genesis extension",
                )
                .with_detail("subject", format!("{subject:?}")));
            }
            let source_annotations = annotations_for(&self.declared, &annotation_subject);
            let target_annotations = annotations_for(target, &annotation_subject);
            if source_annotations != target_annotations {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "adopted_genesis_extension_subject_modified",
                    "canonical migration target changes portable annotations on a fact carrying a released-only adopted-genesis extension",
                )
                .with_detail("subject", format!("{subject:?}")));
            }
            let source_receivers = inherited_type_closure(&self.declared, &extension_owner);
            let target_receivers = inherited_type_closure(target, &extension_owner);
            if !source_receivers.is_subset(&target_receivers) {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "adopted_genesis_extension_inheritance_removed",
                    "canonical migration target removes an inherited released-only adopted-genesis extension",
                )
                .with_detail("subject", format!("{subject:?}")));
            }
        }
        Ok(())
    }

    /// Consume the authority and return its portable migration projection.
    pub fn into_declared(self) -> DeclaredSchema {
        self.declared
    }
}

fn annotations_for<'a>(
    schema: &'a DeclaredSchema,
    subject: &AnnotationSubjectId,
) -> Vec<&'a SchemaFact> {
    schema
        .facts()
        .filter(|fact| {
            matches!(
                fact,
                SchemaFact::Annotation(annotation) if annotation.id().subject() == subject
            )
        })
        .collect()
}

fn inherited_type_closure(schema: &DeclaredSchema, ancestor: &TypeId) -> BTreeSet<TypeId> {
    let mut closure = BTreeSet::from([ancestor.clone()]);
    loop {
        let mut changed = false;
        for fact in schema.facts() {
            if let SchemaFact::Sub(sub) = fact
                && closure.contains(sub.id().supertype())
            {
                changed |= closure.insert(sub.id().subtype().clone());
            }
        }
        if !changed {
            return closure;
        }
    }
}

/// File name of the adopted-genesis artifact inside the canonical migration
/// directory.
pub const ADOPTED_GENESIS_FILE_NAME: &str = "adopted-genesis.typeql";

/// Frozen TypeQL rendering of the released v1 migration-ledger schema.
///
/// This is the exact `migration_state_schema().to_typeql()` output of the
/// released 1.5.x line, pinned here so offline surfaces can recognize the
/// ledger without depending on the provider-bearing v1 compat crate. The
/// v1 crate carries the equality test that keeps the two in lock-step; the
/// ledger is frozen, so a divergence there is a contract break, not a
/// refactor.
pub const LEGACY_LEDGER_SCHEMA_TYPEQL: &str = "\
define

attribute migration_app_label, value string;
attribute migration_applied_at, value datetime;
attribute migration_checksum, value string;
attribute migration_direction, value string;
attribute migration_error, value string;
attribute migration_executor_ip, value string;
attribute migration_executor_mac, value string;
attribute migration_finished_at, value datetime;
attribute migration_id, value string;
attribute migration_name, value string;
attribute migration_run_id, value string;
attribute migration_started_at, value datetime;
attribute migration_status, value string;

entity type_bridge_migration,
    owns migration_id @key,
    owns migration_app_label,
    owns migration_name,
    owns migration_applied_at,
    owns migration_checksum;
entity type_bridge_migration_run,
    owns migration_run_id @key,
    owns migration_app_label,
    owns migration_name,
    owns migration_checksum,
    owns migration_direction,
    owns migration_status,
    owns migration_started_at,
    owns migration_finished_at,
    owns migration_error,
    owns migration_executor_ip,
    owns migration_executor_mac;
";

/// The exact-match label set of the frozen legacy ledger.
///
/// Legacy reservation was never prefix-based, so classification is by exact
/// label. The set is derived from [`LEGACY_LEDGER_SCHEMA_TYPEQL`] labels.
static LEGACY_LEDGER_LABELS: LazyLock<BTreeSet<&'static str>> = LazyLock::new(|| {
    BTreeSet::from([
        "type_bridge_migration",
        "type_bridge_migration_run",
        "migration_id",
        "migration_app_label",
        "migration_name",
        "migration_applied_at",
        "migration_checksum",
        "migration_run_id",
        "migration_direction",
        "migration_status",
        "migration_started_at",
        "migration_finished_at",
        "migration_error",
        "migration_executor_ip",
        "migration_executor_mac",
    ])
});

/// Return whether a schema label belongs to the frozen v1 ledger vocabulary.
#[must_use]
pub fn is_legacy_ledger_label(label: &str) -> bool {
    LEGACY_LEDGER_LABELS.contains(label)
}

/// Parse adopted-genesis bytes into the reconstructed legacy head.
///
/// The same function serves both ends of the adoption contract: the adopt
/// flow runs it over the independently reconstructed snapshot and the live
/// pre-adoption export before storing the snapshot bytes; genesis resolution
/// later runs it over the stored artifact. Reserved V2 control facts
/// fail closed (a pre-adoption database cannot carry them), and a partial
/// ledger partition is indistinguishable from corruption and is rejected.
pub fn parse_adopted_genesis(
    document: DocumentId,
    source: &str,
) -> Result<DeclaredSchema, Diagnostic> {
    parse_adopted_genesis_authority(document, source).map(AdoptedGenesisAuthority::into_declared)
}

/// Parse the raw durable artifact into portable facts plus lossless V1 identity.
pub fn parse_adopted_genesis_authority(
    document: DocumentId,
    source: &str,
) -> Result<AdoptedGenesisAuthority, Diagnostic> {
    parse_adopted_genesis_authority_with_internal(document, source, None)
}

/// Parse a live adoption export while permitting one exact V2 control partition.
///
/// This is used only for resumable adoption after canonical control state has
/// begun installation. A different, partial, or cross-referencing internal
/// partition fails closed just like a reserved namespace in pre-adoption data.
pub fn parse_adopted_genesis_authority_with_internal(
    document: DocumentId,
    source: &str,
    allowed_internal: Option<&DeclaredSchema>,
) -> Result<AdoptedGenesisAuthority, Diagnostic> {
    if source.len() > crate::MAX_TYPEQL_SCHEMA_BYTES {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "adopted_genesis_size_limit",
            "adopted-genesis TypeQL exceeds the schema byte ceiling",
        ));
    }
    let full = released_typeql_to_declared_projection_with_references(document, source).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "adopted_genesis_invalid",
            "adopted-genesis TypeQL cannot be normalized through the released compatibility grammar",
        )
    })?;
    reject_reserved_function_references(&full)?;
    let full = full.into_declared();

    let mut user = Vec::new();
    let mut ledger = Vec::new();
    let mut internal = Vec::new();
    for fact in full.facts() {
        let id = fact.id();
        let source = full.source(&id).cloned().ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_missing_provenance",
                "normalized adopted-genesis fact has no source provenance",
            )
        })?;
        let id_value = serde_json::to_value(&id).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "adopted_genesis_identity_encode_failed",
                "adopted-genesis fact identity cannot be inspected for reserved labels",
            )
        })?;
        let sourced = SourcedSchemaFact::new(fact.clone(), source);
        if value_mentions_label(&id_value, is_typebridge_internal_label) {
            internal.push(sourced);
        } else if value_mentions_label(&id_value, is_legacy_ledger_label) {
            ledger.push(sourced);
        } else {
            user.push(sourced);
        }
    }

    if !internal.is_empty() {
        let Some(expected) = allowed_internal else {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_reserved_namespace",
                "adopted genesis mentions the reserved V2 control namespace; a \
                 pre-adoption v1 database cannot carry canonical control state",
            ));
        };
        let internal = DeclaredSchema::from_facts(full.format(), CapabilitySet::new(), internal)
            .map_err(|_| {
                failure(
                    DiagnosticCategory::Integrity,
                    "adopted_genesis_internal_cross_reference",
                    "canonical control facts cross-reference adopted user facts",
                )
            })?;
        if internal.declared_identity_fingerprint() != expected.declared_identity_fingerprint() {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_internal_mismatch",
                "live canonical control facts differ from the exact resumable adoption contract",
            ));
        }
    }

    if !ledger.is_empty() {
        let ledger = DeclaredSchema::from_facts(full.format(), CapabilitySet::new(), ledger)
            .map_err(|_| ledger_mismatch())?;
        let expected_document = DocumentId::new("typebridge-legacy-ledger-schema.typeql")?;
        let expected =
            typeql_to_declared(expected_document, LEGACY_LEDGER_SCHEMA_TYPEQL).map_err(|_| {
                failure(
                    DiagnosticCategory::InvalidContract,
                    "adopted_genesis_frozen_ledger_invalid",
                    "frozen legacy migration-ledger schema cannot be normalized",
                )
            })?;
        if ledger.declared_identity_fingerprint() != expected.declared_identity_fingerprint() {
            return Err(ledger_mismatch());
        }
    }

    let declared =
        DeclaredSchema::from_facts(full.format(), CapabilitySet::new(), user).map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_cross_reference",
                "adopted-genesis user facts reference the frozen legacy ledger",
            )
        })?;

    // The frozen V1 parser owns identity for constructs absent from the V2
    // fact vocabulary. Serialize its resolved, map-ordered representation
    // after removing only the already-verified legacy ledger partition.
    let unresolved = type_bridge_core_lib::_parser::parse_typeql(source).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "adopted_genesis_released_parse_failed",
            "adopted-genesis TypeQL is outside the released schema grammar",
        )
    })?;
    let released_extensions = released_extensions(&unresolved, &full, source)?;
    let extension_bytes = serde_json::to_vec(&released_extensions).map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "adopted_genesis_extension_encode_failed",
            "released adopted-genesis extensions cannot be encoded",
        )
    })?;
    let released_extension_identity = Fingerprint::compute(
        FingerprintDomain::new("typebridge.schema.adopted-genesis-released-extensions")?,
        CanonicalizationVersion::new("typebridge.released-schema-extensions-json/v1")?,
        None,
        &extension_bytes,
    );

    let mut legacy =
        type_bridge_core_lib::_schema::TypeSchema::from_typeql(source).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "adopted_genesis_released_parse_failed",
                "adopted-genesis TypeQL is outside the released schema grammar",
            )
        })?;
    legacy
        .attributes
        .retain(|label, _| !is_legacy_ledger_label(label));
    legacy
        .entities
        .retain(|label, _| !is_legacy_ledger_label(label));
    legacy
        .relations
        .retain(|label, _| !is_legacy_ledger_label(label));
    legacy
        .attributes
        .retain(|label, _| !is_typebridge_internal_label(label));
    legacy
        .entities
        .retain(|label, _| !is_typebridge_internal_label(label));
    legacy
        .relations
        .retain(|label, _| !is_typebridge_internal_label(label));
    for attribute in legacy.attributes.values_mut() {
        normalize_released_value_type(&mut attribute.value_type);
        if let Some(values) = &mut attribute.allowed_values {
            values.sort();
            values.dedup();
        }
    }
    for entity in legacy.entities.values_mut() {
        sort_serialized(&mut entity.owns);
        entity.owns_order.sort();
        entity.owns_order.dedup();
        sort_serialized(&mut entity.plays);
    }
    for relation in legacy.relations.values_mut() {
        sort_serialized(&mut relation.owns);
        relation.owns_order.sort();
        relation.owns_order.dedup();
        sort_serialized(&mut relation.plays);
        sort_serialized(&mut relation.roles);
    }
    for function in legacy.functions.values_mut() {
        for parameter in &mut function.parameters {
            normalize_released_value_type(&mut parameter.type_);
        }
        for returned in &mut function.return_type.types {
            normalize_released_value_type(&mut returned.name);
        }
    }
    for structure in legacy.structs.values_mut() {
        for field in &mut structure.fields {
            normalize_released_value_type(&mut field.value_type);
        }
    }
    #[derive(serde::Serialize)]
    struct ReleasedAuthorityIdentity<'a> {
        schema: &'a type_bridge_core_lib::_schema::TypeSchema,
        compatibility_extensions: &'a [ReleasedExtensionFact],
    }
    let canonical = serde_json::to_vec(&ReleasedAuthorityIdentity {
        schema: &legacy,
        compatibility_extensions: &released_extensions,
    })
    .map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "adopted_genesis_identity_encode_failed",
            "released adopted-genesis identity cannot be encoded",
        )
    })?;
    let legacy_identity = Fingerprint::compute(
        FingerprintDomain::new("typebridge.schema.adopted-genesis-released")?,
        CanonicalizationVersion::new("typebridge.released-schema-json/v2")?,
        None,
        &canonical,
    );
    Ok(AdoptedGenesisAuthority {
        declared,
        legacy_identity,
        released_extension_identity,
        released_extensions,
    })
}

#[derive(Clone, Debug, serde::Serialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ReleasedExtensionFact {
    Owns {
        owner_kind: &'static str,
        owner: String,
        attribute: String,
        ordered: bool,
        distinct: bool,
        cascade: bool,
        subkey: Option<String>,
    },
    Relates {
        relation: String,
        role: String,
        ordered: bool,
        distinct: bool,
    },
    OmittedFunction {
        name: String,
        parameters: Vec<ReleasedFunctionParameter>,
        returns_stream: bool,
        returns: Vec<ReleasedFunctionReturn>,
        body_tokens: String,
    },
    OmittedStruct {
        name: String,
        fields: Vec<ReleasedStructField>,
    },
}

#[derive(Clone, Debug, serde::Serialize, Eq, Ord, PartialEq, PartialOrd)]
struct ReleasedFunctionParameter {
    name: String,
    type_name: String,
}

#[derive(Clone, Debug, serde::Serialize, Eq, Ord, PartialEq, PartialOrd)]
struct ReleasedFunctionReturn {
    type_name: String,
    optional: bool,
}

#[derive(Clone, Debug, serde::Serialize, Eq, Ord, PartialEq, PartialOrd)]
struct ReleasedStructField {
    name: String,
    value_type: String,
    optional: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScannedDefinitionToken<'a> {
    Word(&'a str),
    Literal(&'a str),
    Operator(&'a str),
    Symbol(char),
}

fn released_extensions(
    schema: &type_bridge_core_lib::_schema::TypeSchema,
    portable: &DeclaredSchema,
    source: &str,
) -> Result<Vec<ReleasedExtensionFact>, Diagnostic> {
    let mut facts = Vec::new();
    for (owner, entity) in &schema.entities {
        collect_owns_extensions("entity", owner, &entity.owns, &mut facts);
    }
    for (owner, relation) in &schema.relations {
        collect_owns_extensions("relation", owner, &relation.owns, &mut facts);
        for role in &relation.roles {
            if role.ordered || role.distinct {
                facts.push(ReleasedExtensionFact::Relates {
                    relation: owner.clone(),
                    role: role.name.clone(),
                    ordered: role.ordered,
                    distinct: role.distinct,
                });
            }
        }
    }
    let mut final_definitions = BTreeMap::new();
    for definition in type_bridge_core_lib::_parser::released_definition_extents(source) {
        let kind = match definition.kind {
            type_bridge_core_lib::_parser::ReleasedDefinitionKind::Function => 0_u8,
            type_bridge_core_lib::_parser::ReleasedDefinitionKind::Struct => 1_u8,
        };
        final_definitions.insert((kind, definition.label.clone()), definition);
    }
    for definition in final_definitions.into_values() {
        let omitted = match definition.kind {
            type_bridge_core_lib::_parser::ReleasedDefinitionKind::Function => portable
                .fact(&SchemaFactId::Function(FunctionId::new(
                    definition.label.as_str(),
                )?))
                .is_none(),
            type_bridge_core_lib::_parser::ReleasedDefinitionKind::Struct => portable
                .fact(&SchemaFactId::Struct(StructId::new(
                    definition.label.as_str(),
                )?))
                .is_none(),
        };
        if !omitted {
            continue;
        }
        reject_reserved_omitted_definition_label(definition.label.as_str())?;
        let spelling = source.get(definition.extent.clone()).ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_definition_extent_invalid",
                "released definition extent falls outside the adopted-genesis source",
            )
        })?;
        let tokens = scan_released_definition_tokens(spelling)?;
        match definition.kind {
            type_bridge_core_lib::_parser::ReleasedDefinitionKind::Function => {
                let function =
                    validate_omitted_function(schema, definition.label.as_str(), &tokens)?;
                facts.push(ReleasedExtensionFact::OmittedFunction {
                    name: definition.label,
                    parameters: function.parameters,
                    returns_stream: function.returns_stream,
                    returns: function.returns,
                    body_tokens: function.body_tokens,
                });
            }
            type_bridge_core_lib::_parser::ReleasedDefinitionKind::Struct => {
                let fields = validate_omitted_struct(schema, definition.label.as_str(), &tokens)?;
                facts.push(ReleasedExtensionFact::OmittedStruct {
                    name: definition.label,
                    fields,
                });
            }
        }
    }
    facts.sort();
    Ok(facts)
}

const MAX_RELEASED_DEFINITION_TOKENS: usize = 1_000_000;

fn scan_released_definition_tokens(
    source: &str,
) -> Result<Vec<ScannedDefinitionToken<'_>>, Diagnostic> {
    use type_bridge_core_lib::_parser::{SourceRegionKind, scan_source_regions};

    let mut tokens = Vec::new();
    for (range, kind) in scan_source_regions(source) {
        match kind {
            SourceRegionKind::LineComment => {}
            SourceRegionKind::StringLiteral => {
                push_definition_token(
                    &mut tokens,
                    ScannedDefinitionToken::Literal(&source[range]),
                )?;
            }
            SourceRegionKind::Code => {
                let mut cursor = range.start;
                while cursor < range.end {
                    let character = source[cursor..]
                        .chars()
                        .next()
                        .expect("cursor remains on a source character");
                    if character.is_whitespace() {
                        cursor += character.len_utf8();
                        continue;
                    }
                    if character.is_ascii_alphabetic() || character == '_' {
                        let start = cursor;
                        cursor += character.len_utf8();
                        while cursor < range.end {
                            let next = source[cursor..]
                                .chars()
                                .next()
                                .expect("cursor remains on a source character");
                            if next.is_ascii_alphanumeric() || matches!(next, '_' | '-') {
                                cursor += next.len_utf8();
                            } else {
                                break;
                            }
                        }
                        push_definition_token(
                            &mut tokens,
                            ScannedDefinitionToken::Word(&source[start..cursor]),
                        )?;
                    } else if character.is_ascii_digit() {
                        let start = cursor;
                        cursor += character.len_utf8();
                        while cursor < range.end {
                            let next = source[cursor..]
                                .chars()
                                .next()
                                .expect("cursor remains on a source character");
                            if next.is_ascii_alphanumeric() || next == '_' {
                                cursor += next.len_utf8();
                            } else {
                                break;
                            }
                        }
                        push_definition_token(
                            &mut tokens,
                            ScannedDefinitionToken::Word(&source[start..cursor]),
                        )?;
                    } else if is_released_operator_character(character) {
                        let start = cursor;
                        cursor += character.len_utf8();
                        while cursor < range.end {
                            let next = source[cursor..]
                                .chars()
                                .next()
                                .expect("cursor remains on a source character");
                            if is_released_operator_character(next) {
                                cursor += next.len_utf8();
                            } else {
                                break;
                            }
                        }
                        push_definition_token(
                            &mut tokens,
                            ScannedDefinitionToken::Operator(&source[start..cursor]),
                        )?;
                    } else {
                        push_definition_token(
                            &mut tokens,
                            ScannedDefinitionToken::Symbol(character),
                        )?;
                        cursor += character.len_utf8();
                    }
                }
            }
        }
    }
    Ok(tokens)
}

fn push_definition_token<'a>(
    tokens: &mut Vec<ScannedDefinitionToken<'a>>,
    token: ScannedDefinitionToken<'a>,
) -> Result<(), Diagnostic> {
    if tokens.len() >= MAX_RELEASED_DEFINITION_TOKENS {
        return Err(failure(
            DiagnosticCategory::ResourceLimit,
            "adopted_genesis_definition_token_limit",
            "released definition exceeds the compatibility token ceiling",
        ));
    }
    tokens.push(token);
    Ok(())
}

fn is_released_operator_character(character: char) -> bool {
    matches!(
        character,
        '!' | '%' | '&' | '*' | '+' | '-' | '.' | '/' | '<' | '=' | '>' | '?' | '^' | '|' | '~'
    )
}

fn canonical_released_definition_tokens(tokens: &[ScannedDefinitionToken<'_>]) -> String {
    let mut canonical = String::new();
    for token in tokens {
        let (tag, spelling) = match token {
            ScannedDefinitionToken::Word(value) => ('w', *value),
            ScannedDefinitionToken::Literal(value) => ('l', *value),
            ScannedDefinitionToken::Operator(value) => ('o', *value),
            ScannedDefinitionToken::Symbol(value) => {
                canonical.push('s');
                canonical.push_str(&u32::from(*value).to_string());
                canonical.push(';');
                continue;
            }
        };
        canonical.push(tag);
        canonical.push_str(&spelling.len().to_string());
        canonical.push(':');
        canonical.push_str(spelling);
        canonical.push(';');
    }
    canonical
}

struct ValidatedOmittedFunction {
    parameters: Vec<ReleasedFunctionParameter>,
    returns_stream: bool,
    returns: Vec<ReleasedFunctionReturn>,
    body_tokens: String,
}

fn validate_omitted_function(
    schema: &type_bridge_core_lib::_schema::TypeSchema,
    label: &str,
    tokens: &[ScannedDefinitionToken<'_>],
) -> Result<ValidatedOmittedFunction, Diagnostic> {
    let function = schema.functions.get(label).ok_or_else(|| {
        failure(
            DiagnosticCategory::Integrity,
            "adopted_genesis_omitted_function_missing",
            "released function extent has no frozen-parser definition",
        )
    })?;
    for type_name in function
        .parameters
        .iter()
        .map(|parameter| parameter.type_.as_str())
        .chain(
            function
                .return_type
                .types
                .iter()
                .map(|returned| returned.name.as_str()),
        )
    {
        reject_reserved_omitted_definition_label(type_name)?;
    }
    for token in tokens {
        if let ScannedDefinitionToken::Word(label) = token
            && is_reserved_schema_label(label)
        {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_omitted_definition_reserved_reference",
                "omitted released function references a reserved control-schema label",
            )
            .with_detail("function", function.name.clone())
            .with_detail("label", (*label).to_owned()));
        }
    }

    let body = omitted_function_body_tokens(tokens)?;
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| ReleasedFunctionParameter {
            name: parameter.name.clone(),
            type_name: normalized_released_value_type(parameter.type_.as_str()),
        })
        .collect();
    let returns = function
        .return_type
        .types
        .iter()
        .map(|returned| ReleasedFunctionReturn {
            type_name: normalized_released_value_type(returned.name.as_str()),
            optional: returned.optional,
        })
        .collect();
    Ok(ValidatedOmittedFunction {
        parameters,
        returns_stream: function.return_type.is_stream,
        returns,
        body_tokens: canonical_released_definition_tokens(body),
    })
}

fn validate_omitted_struct(
    schema: &type_bridge_core_lib::_schema::TypeSchema,
    label: &str,
    tokens: &[ScannedDefinitionToken<'_>],
) -> Result<Vec<ReleasedStructField>, Diagnostic> {
    let structure = schema.structs.get(label).ok_or_else(|| {
        failure(
            DiagnosticCategory::Integrity,
            "adopted_genesis_omitted_struct_missing",
            "released struct extent has no frozen-parser definition",
        )
    })?;
    for field in &structure.fields {
        reject_reserved_omitted_definition_label(field.value_type.as_str())?;
    }
    for token in tokens {
        if let ScannedDefinitionToken::Word(label) = token
            && is_reserved_schema_label(label)
        {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "adopted_genesis_omitted_definition_reserved_reference",
                "omitted released struct mentions a reserved control-schema label",
            )
            .with_detail("struct", structure.name.clone())
            .with_detail("label", (*label).to_owned()));
        }
    }
    Ok(structure
        .fields
        .iter()
        .map(|field| ReleasedStructField {
            name: field.name.clone(),
            value_type: normalized_released_value_type(field.value_type.as_str()),
            optional: field.optional,
        })
        .collect())
}

fn reject_reserved_omitted_definition_label(label: &str) -> Result<(), Diagnostic> {
    if is_reserved_schema_label(label) {
        Err(failure(
            DiagnosticCategory::Integrity,
            "adopted_genesis_omitted_definition_reserved_reference",
            "omitted released definition references the reserved migration-control namespace",
        )
        .with_detail("label", label.to_owned()))
    } else {
        Ok(())
    }
}

fn is_reserved_schema_label(label: &str) -> bool {
    is_typebridge_internal_label(label) || is_legacy_ledger_label(label)
}

fn omitted_function_body_tokens<'a>(
    tokens: &'a [ScannedDefinitionToken<'a>],
) -> Result<&'a [ScannedDefinitionToken<'a>], Diagnostic> {
    let arrow = tokens
        .iter()
        .position(|token| token_is_operator(token, "->"));
    let Some(arrow) = arrow else {
        return Err(omitted_function_scan_failure());
    };
    let Some(colon) = tokens[arrow + 1..]
        .iter()
        .position(|token| token_is_symbol(token, ':'))
        .map(|offset| arrow + 1 + offset)
    else {
        return Err(omitted_function_scan_failure());
    };
    Ok(&tokens[colon + 1..])
}

fn omitted_function_scan_failure() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "adopted_genesis_omitted_function_scan_failed",
        "released function signature cannot be separated from its opaque body",
    )
}

fn token_is_symbol(token: &ScannedDefinitionToken<'_>, expected: char) -> bool {
    matches!(token, ScannedDefinitionToken::Symbol(actual) if *actual == expected)
}

fn token_is_operator(token: &ScannedDefinitionToken<'_>, expected: &str) -> bool {
    matches!(token, ScannedDefinitionToken::Operator(actual) if *actual == expected)
}

fn collect_owns_extensions(
    owner_kind: &'static str,
    owner: &str,
    ownerships: &[type_bridge_core_lib::_schema::OwnedAttribute],
    facts: &mut Vec<ReleasedExtensionFact>,
) {
    for ownership in ownerships {
        if ownership.ordered
            || ownership.distinct
            || ownership.is_cascade
            || ownership.subkey_group.is_some()
        {
            facts.push(ReleasedExtensionFact::Owns {
                owner_kind,
                owner: owner.to_owned(),
                attribute: ownership.name.clone(),
                ordered: ownership.ordered,
                distinct: ownership.distinct,
                cascade: ownership.is_cascade,
                subkey: ownership.subkey_group.clone(),
            });
        }
    }
}

fn normalize_released_value_type(value: &mut String) {
    let canonical = match value.as_str() {
        "int" | "long" => "integer",
        "bool" => "boolean",
        _ => return,
    };
    value.clear();
    value.push_str(canonical);
}

fn normalized_released_value_type(value: &str) -> String {
    match value {
        "int" | "long" => "integer".to_owned(),
        "bool" => "boolean".to_owned(),
        _ => value.to_owned(),
    }
}

fn sort_serialized<T: serde::Serialize>(values: &mut [T]) {
    values.sort_by_cached_key(|value| serde_json::to_vec(value).unwrap_or_default());
}

fn ledger_mismatch() -> Diagnostic {
    failure(
        DiagnosticCategory::Integrity,
        "adopted_genesis_legacy_ledger_mismatch",
        "legacy migration-ledger facts differ from the frozen v1 contract",
    )
}

fn value_mentions_label(value: &Value, predicate: fn(&str) -> bool) -> bool {
    match value {
        Value::String(value) => predicate(value),
        Value::Array(values) => values
            .iter()
            .any(|value| value_mentions_label(value, predicate)),
        Value::Object(values) => values
            .values()
            .any(|value| value_mentions_label(value, predicate)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static adopted-genesis diagnostic code"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> DocumentId {
        DocumentId::new("adopted-genesis.typeql").expect("document id")
    }

    #[test]
    fn user_only_export_parses_to_the_user_schema() {
        let genesis =
            parse_adopted_genesis(document(), "define\nentity person;\nentity company;\n")
                .expect("user-only export");
        let direct = typeql_to_declared(document(), "define\nentity person;\nentity company;\n")
            .expect("direct parse");
        assert_eq!(
            genesis.declared_identity_fingerprint(),
            direct.declared_identity_fingerprint(),
        );
    }

    #[test]
    fn the_exact_frozen_ledger_partition_is_dropped() {
        let source = format!(
            "{}\nentity person;\n",
            LEGACY_LEDGER_SCHEMA_TYPEQL.trim_end(),
        );
        let genesis = parse_adopted_genesis(document(), &source).expect("v1 export");
        let expected =
            typeql_to_declared(document(), "define\nentity person;\n").expect("user parse");
        assert_eq!(
            genesis.declared_identity_fingerprint(),
            expected.declared_identity_fingerprint(),
        );
    }

    #[test]
    fn a_partial_ledger_partition_fails_closed() {
        let error = parse_adopted_genesis(
            document(),
            "define\nattribute migration_id, value string;\nentity person;\n",
        )
        .expect_err("partial ledger is corruption");
        assert_eq!(
            error.code().as_str(),
            "adopted_genesis_legacy_ledger_mismatch",
        );
    }

    #[test]
    fn reserved_control_facts_fail_closed() {
        let error = parse_adopted_genesis(
            document(),
            "define\nentity typebridge-internal-v2-migration-control;\n",
        )
        .expect_err("reserved namespace cannot be adopted");
        assert_eq!(error.code().as_str(), "adopted_genesis_reserved_namespace");
    }

    #[test]
    fn strict_function_body_references_to_control_schema_fail_before_projection() {
        for (reference, expected_code) in [
            ("migration_id", "reserved_schema_cross_reference"),
            (
                "typebridge-internal-v2-shadow",
                "reserved_schema_cross_reference",
            ),
            ("$kind", "migration_typedb_dynamic_function_reference"),
        ] {
            let source = format!(
                "define\nentity person;\n\
                 fun inspect($candidate: person) -> {{ person }}:\n\
                   match $candidate isa {reference};\n\
                   return {{ $candidate }};\n",
            );
            let error = parse_adopted_genesis_authority(document(), &source)
                .expect_err("strict function-body control reference must fail closed");
            assert_eq!(
                error.code().as_str(),
                expected_code,
                "reference: {reference}"
            );
        }
    }

    #[test]
    fn strict_function_literals_and_comments_do_not_create_control_references() {
        parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun inspect($candidate: person) -> { person }:\n\
               match\n\
                 # migration_id and typebridge-internal-v2-shadow are inert\n\
                 /* migration_checksum is inert too */\n\
                 $marker = \"migration_id typebridge-internal-v2-shadow\";\n\
                 $candidate isa person;\n\
               return { $candidate };\n",
        )
        .expect("literal and comment text is not schema-reference authority");
    }

    #[test]
    fn released_list_and_ownership_annotations_remain_lossless_authority() {
        let cascade = parse_adopted_genesis_authority(
            document(),
            "define\nattribute tag, value string;\nattribute name, value string;\n\
             entity person, owns tag[] @card(0..5) @distinct, owns name @cascade @subkey(primary);\n\
             entity employee sub person;\n",
        )
        .expect("released constructs are adoptable");
        let without_cascade = parse_adopted_genesis_authority(
            document(),
            "define\nattribute tag, value string;\nattribute name, value string;\n\
             entity person, owns tag[] @card(0..5) @distinct, owns name @subkey(primary);\n\
             entity employee sub person;\n",
        )
        .expect("comparison schema parses");

        assert_eq!(
            cascade.declared().declared_identity_fingerprint(),
            without_cascade.declared().declared_identity_fingerprint(),
            "portable projection deliberately lacks @cascade",
        );
        assert_ne!(cascade.legacy_identity(), without_cascade.legacy_identity());
        let extension_error = cascade
            .ensure_released_extension_identity_matches(&without_cascade)
            .expect_err("raw compatibility-extension drift must fail closed");
        assert_eq!(
            extension_error.code().as_str(),
            "migration_adopted_extension_drift"
        );
        cascade
            .ensure_released_extension_identity_matches(&cascade)
            .expect("the same raw compatibility authority matches");
        cascade
            .ensure_released_extension_subjects_survive(cascade.declared())
            .expect("the projected ownership subject survives");

        let removed = typeql_to_declared(
            document(),
            "define\nattribute tag, value string;\nattribute name, value string;\nentity person;\n",
        )
        .expect("target without ownerships parses");
        let error = cascade
            .ensure_released_extension_subjects_survive(&removed)
            .expect_err("a canonical target cannot silently erase extensions");
        assert_eq!(
            error.code().as_str(),
            "adopted_genesis_extension_subject_removed",
        );

        let modified = typeql_to_declared(
            document(),
            "define\nattribute tag, value string;\nattribute name, value string;\n\
             entity person, owns tag @card(0..4), owns name;\n\
             entity employee sub person;\n",
        )
        .expect("target with changed portable annotation parses");
        let error = cascade
            .ensure_released_extension_subjects_survive(&modified)
            .expect_err("a canonical target cannot rewrite an extended capability");
        assert_eq!(
            error.code().as_str(),
            "adopted_genesis_extension_subject_modified",
        );

        let detached = typeql_to_declared(
            document(),
            "define\nattribute tag, value string;\nattribute name, value string;\n\
             entity person, owns tag @card(0..5), owns name;\nentity employee;\n",
        )
        .expect("target with detached subtype parses");
        let error = cascade
            .ensure_released_extension_subjects_survive(&detached)
            .expect_err("a canonical target cannot silently detach extension inheritance");
        assert_eq!(
            error.code().as_str(),
            "adopted_genesis_extension_inheritance_removed",
        );
    }

    #[test]
    fn released_ordered_role_subject_must_survive_canonical_replay() {
        let authority = parse_adopted_genesis_authority(
            document(),
            "define\nrelation team, relates member[] @distinct;\n",
        )
        .expect("released ordered role is adoptable");
        authority
            .ensure_released_extension_subjects_survive(authority.declared())
            .expect("the projected relates subject survives");

        let removed = typeql_to_declared(document(), "define\nrelation team;\n")
            .expect("target without role parses");
        let error = authority
            .ensure_released_extension_subjects_survive(&removed)
            .expect_err("a canonical target cannot silently erase ordered roles");
        assert_eq!(
            error.code().as_str(),
            "adopted_genesis_extension_subject_removed",
        );
    }

    #[test]
    fn omitted_function_body_is_token_bound_but_ignores_comments_and_whitespace() {
        let stable = parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun inspect($input: missing-input) -> missing-output:\n\
               opaque-token\n\
               return { $input };\n",
        )
        .expect("frozen opaque function remains adoptable");
        let reformatted = parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun inspect ( $input : missing-input ) -> missing-output:\n\
               opaque-token /* semantically irrelevant */\n\
               return {    $input    };\n",
        )
        .expect("format-only opaque function variation remains adoptable");
        let changed = parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun inspect($input: missing-input) -> missing-output:\n\
               opaque-token\n\
               return { 7 };\n",
        )
        .expect("changed opaque body still parses through the released fallback");

        assert_eq!(
            stable.declared().declared_identity_fingerprint(),
            changed.declared().declared_identity_fingerprint(),
            "fallback-only functions are absent from the portable graph",
        );
        stable
            .ensure_released_extension_identity_matches(&reformatted)
            .expect("comments and whitespace outside literals are not semantic authority");
        assert_eq!(stable.legacy_identity(), reformatted.legacy_identity());
        assert_ne!(stable.legacy_identity(), changed.legacy_identity());
        let error = stable
            .ensure_released_extension_identity_matches(&changed)
            .expect_err("opaque body token drift must fail closed");
        assert_eq!(error.code().as_str(), "migration_adopted_extension_drift");
    }

    #[test]
    fn released_comment_return_boundary_retains_the_following_function() {
        let authority = parse_adopted_genesis_authority(
            document(),
            "define\n\
             entity person;\n\
             fun alpha() -> integer:\n\
               # return;\n\
             fun beta() -> integer:\n\
               return 2;\n",
        )
        .expect("frozen released function boundaries");

        let omitted = authority
            .released_extensions
            .iter()
            .filter_map(|extension| match extension {
                ReleasedExtensionFact::OmittedFunction { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(omitted, vec!["alpha", "beta"]);
    }

    #[test]
    fn omitted_definition_markers_in_literals_and_comments_are_inert_but_code_is_reserved() {
        parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun inspect() -> integer:\n\
               opaque-token \"return 9; migration_id typebridge-internal-v2-shadow\"\n\
               /* return 8; migration_checksum */\n\
               return { 1 };\n",
        )
        .expect("literal and comment markers neither truncate nor reserve the opaque definition");

        let error = parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun inspect() -> integer:\n\
               opaque-token migration_id\n\
               return { 1 };\n",
        )
        .expect_err("reserved ledger labels in opaque code must fail closed");
        assert_eq!(
            error.code().as_str(),
            "adopted_genesis_omitted_definition_reserved_reference"
        );
    }

    #[test]
    fn mixed_fallback_binds_every_omitted_function_and_struct_definition() {
        let base = parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun opaque() -> integer:\n  opaque-token\n  return { 1 };\n\
             fun people($candidate: person) -> { person }:\n\
               match $candidate isa person;\n\
               return { $candidate };\n\
             struct payload, value note string;\n",
        )
        .expect("mixed released fallback is adoptable");
        let function_changed = parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun opaque() -> integer:\n  opaque-token\n  return { 1 };\n\
             fun people($candidate: person) -> { person }:\n\
               match $candidate isa! person;\n\
               return { $candidate };\n\
             struct payload, value note string;\n",
        )
        .expect("mixed function change parses");
        let structure_changed = parse_adopted_genesis_authority(
            document(),
            "define\nentity person;\n\
             fun opaque() -> integer:\n  opaque-token\n  return { 1 };\n\
             fun people($candidate: person) -> { person }:\n\
               match $candidate isa person;\n\
               return { $candidate };\n\
             struct payload, value count integer;\n",
        )
        .expect("mixed struct change parses");

        for changed in [&function_changed, &structure_changed] {
            assert_eq!(
                base.declared().declared_identity_fingerprint(),
                changed.declared().declared_identity_fingerprint(),
                "one opaque sibling forces every function and struct through fallback",
            );
            base.ensure_released_extension_identity_matches(changed)
                .expect_err("every fallback-stripped definition must remain exact authority");
        }

        let conflicting_target = typeql_to_declared(
            document(),
            "define\nentity person;\n\
             fun people($candidate: person) -> { person }:\n\
               match $candidate isa person;\n\
               return { $candidate };\n",
        )
        .expect("strict target with the retained function parses");
        let error = base
            .ensure_released_extension_subjects_survive(&conflicting_target)
            .expect_err("canonical target cannot promote a retained opaque definition implicitly");
        assert_eq!(
            error.code().as_str(),
            "adopted_genesis_extension_subject_modified"
        );
    }

    #[test]
    fn the_frozen_ledger_constant_normalizes() {
        typeql_to_declared(document(), LEGACY_LEDGER_SCHEMA_TYPEQL).expect("frozen ledger parses");
    }
}
