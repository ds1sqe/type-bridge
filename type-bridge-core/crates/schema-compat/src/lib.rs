//! One-way compatibility front-ends for the V2 schema fact graph.
//!
//! This crate is deliberately unpublished. Source-language parsers converge on
//! `type_bridge_schema::FactAssembler`; contract and schema crates never depend
//! on compatibility parsers or their transitive grammar dependencies.

mod descriptor;
mod function_references;
mod literal;

pub use function_references::{
    FunctionBodyReferences, SchemaReference, TypeqlDeclaredSchema,
};

pub use descriptor::{
    GENERATED_DECLARED_DESCRIPTOR_PATH, GENERATED_DECLARED_DESCRIPTOR_V1,
    GeneratedDeclaredDescriptorSetV1, generate_package_with_declared_descriptors,
    generated_declared_descriptors_json, generated_descriptors_to_declared,
    typeql_to_generated_descriptors,
};

/// Comparison-only reporting between the frozen V1 parser and the V2 fact graph.
pub mod shadow;

pub use shadow::{
    ShadowCompared, ShadowComparison, ShadowCoverage, ShadowCoverageState, ShadowDimension,
    ShadowFinding,
    ShadowLaneNotRun, ShadowLaneOutcome, ShadowLaneRejection, ShadowLaneSummary,
    ShadowUnavailableLane, ShadowVerdict, V1ShadowInternalError, V1ShadowReport,
    v1_shadow_report,
};

use std::collections::BTreeMap;

use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{
    AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind,
};
use type_bridge_contract::limits::MAX_CANONICAL_COLLECTION_LEN;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId,
    CanonicalValueRange, CanonicalValueSet, DeclaredSchema, DocText, DocumentId, FunctionBody,
    FunctionFact, FunctionParameter, FunctionReturnElement, FunctionReturnMode,
    FunctionSignature, OwnsFact, OwnsFactId, PlaysFactId, RegexPattern, RelatesFactId,
    SchemaAnnotationValue, SchemaDiagnostic, SchemaDiagnostics, SchemaFact, SourceSpan, StructFact,
    StructField, SubFact, SubFactId, TypeFact, TypeReference, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, Cardinality, ValueTypeTag};
use type_bridge_schema::FactAssembler;
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
use typeql::Annotation;

use crate::literal::{canonical_literal, validate_quoted_string};

/// Defensive source bound applied before entering the third-party parser.
pub const MAX_TYPEQL_SCHEMA_BYTES: usize = 16 * 1024 * 1024;

/// Parse one TypeQL `define` query into the canonical declared schema graph
/// and derive neutral references from every function body.
pub fn typeql_to_declared_with_references(
    document: DocumentId,
    source: &str,
) -> Result<TypeqlDeclaredSchema, SchemaDiagnostics> {
    if source.len() > MAX_TYPEQL_SCHEMA_BYTES {
        return Err(error(
            DiagnosticCategory::InvalidContract,
            "typeql_schema_size_limit",
            "TypeQL schema source exceeds the compatibility parser limit",
            None,
        ));
    }

    let query = typeql::parse_query(source).map_err(|parse_error| {
        error(
            DiagnosticCategory::InvalidContract,
            "invalid_typeql_schema",
            format!("TypeQL schema parsing failed: {parse_error}"),
            None,
        )
    })?;
    let definables = match query.structure {
        QueryStructure::Schema(SchemaQuery::Define(define)) => define.definables,
        _ => {
            return Err(error(
                DiagnosticCategory::InvalidContract,
                "expected_typeql_define",
                "schema compatibility input must contain exactly one define query",
                query_span(&document, source, query.span)?,
            ));
        }
    };

    let declarations: Vec<&TypeDeclaration> = definables
        .iter()
        .filter_map(|definable| match definable {
            Definable::TypeDeclaration(declaration) => Some(declaration),
            _ => None,
        })
        .collect();
    let kinds = infer_type_kinds(&document, source, &declarations)?;
    let mut ids = BTreeMap::new();
    let mut assembler = FactAssembler::new(FormatVersion::V1);
    let mut function_body_references = BTreeMap::new();

    for (declaration, kind) in declarations.iter().zip(kinds) {
        let label = typeql_label(&declaration.label);
        let declaration_span = source_span(&document, source, declaration.span)?;
        let id = TypeId::new(kind, label.clone())
            .map_err(|diagnostic| contract(diagnostic, declaration_span.clone()))?;
        assembler.insert_fact(
            SchemaFact::Type(
                TypeFact::new(id.clone())
                    .map_err(|diagnostic| contract(diagnostic, declaration_span.clone()))?,
            ),
            declaration_span,
        )?;
        ids.entry(label).or_insert(id);
    }

    for declaration in &declarations {
        let label = typeql_label(&declaration.label);
        let id = ids.get(&label).cloned().ok_or_else(|| {
            error(
                DiagnosticCategory::InvalidContract,
                "unknown_typeql_type",
                format!("TypeQL declaration `{label}` has no inferred type identity"),
                query_span(&document, source, declaration.span).ok().flatten(),
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
                function_body_references.insert(
                    function_id,
                    function_references::collect_function_body_references(&function.block),
                );
            }
        }
    }

    assembler
        .finish()
        .map(|declared| TypeqlDeclaredSchema::new(declared, function_body_references))
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
    let rendered_typeql = type_bridge_toml_transpiler::toml_to_typeql(toml_source).map_err(
        |transpile_error| {
            error(
                DiagnosticCategory::InvalidContract,
                "invalid_toml_schema",
                format!("TOML schema transpilation failed: {transpile_error}"),
                None,
            )
        },
    )?;
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

fn infer_type_kinds(
    document: &DocumentId,
    source: &str,
    declarations: &[&TypeDeclaration],
) -> Result<Vec<TypeKind>, SchemaDiagnostics> {
    let mut inferred = Vec::with_capacity(declarations.len());
    for declaration in declarations {
        let mut kind = declaration
            .kind
            .as_ref()
            .map(|kind| kind_from_token(&kind.to_string()))
            .transpose()
            .map_err(|message| at(document, source, declaration.span, "unsupported_typeql_kind", message))?;
        for capability in &declaration.capabilities {
            let hint = match &capability.base {
                CapabilityBase::ValueType(_) => Some(TypeKind::Attribute),
                CapabilityBase::Relates(_) => Some(TypeKind::Relation),
                CapabilityBase::Sub(sub) => root_kind(&typeql_label(&sub.supertype_label)),
                _ => None,
            };
            if let Some(hint) = hint {
                merge_kind(&mut kind, hint).map_err(|message| {
                    at(document, source, capability.span, "conflicting_typeql_kind", message)
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

fn insert_capability(
    assembler: &mut FactAssembler,
    ids: &BTreeMap<String, TypeId>,
    owner: &TypeId,
    capability: &Capability,
    document: &DocumentId,
    source: &str,
) -> Result<(), SchemaDiagnostics> {
    let capability_span = source_span(document, source, capability.span)?;
    let subject = match &capability.base {
        CapabilityBase::Sub(sub) => {
            let parent_label = typeql_label(&sub.supertype_label);
            if root_kind(&parent_label).is_some() {
                reject_annotations_if_present(capability, document, source)?;
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
                at(document, source, value.span, "unsupported_typeql_value_type", message)
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
                at(document, source, owns.span, "unsupported_typeql_owns", message)
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
                at(document, source, relates.span, "unsupported_typeql_relates", message)
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
            let relation_label = typeql_label(&plays.role.scope);
            let role_label = typeql_label(&plays.role.name);
            let relation = Label::new(&relation_label)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            let role = Label::new(&role_label)
                .map_err(|diagnostic| contract(diagnostic, capability_span.clone()))?;
            assembler.insert_plays(owner.label().clone(), relation, role, capability_span.clone());
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

    insert_annotations(
        assembler,
        subject,
        &capability.annotations,
        document,
        source,
    )
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
            at(document, source, field.span, "unsupported_typeql_struct_field", message)
        })?;
        let value_type = named_value_type(named).map_err(|message| {
            at(document, source, field.span, "unsupported_typeql_struct_field", message)
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
            at(document, source, argument.span, "unsupported_typeql_function_parameter", message)
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
        let name = Label::new(name)
            .map_err(|diagnostic| contract(diagnostic, argument_span))?;
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
    let body_text = source.get(block_span.begin_offset..block_span.end_offset).ok_or_else(|| {
        error(
            DiagnosticCategory::InvalidContract,
            "invalid_typeql_function_body_span",
            "TypeQL function body span is outside the original source",
            Some(function_span.clone()),
        )
    })?;
    let body_span = source_span(document, source, Some(block_span))?;
    let body = FunctionBody::new(body_text)
        .map_err(|diagnostic| contract(diagnostic, body_span))?;
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
            let (named, optional) = simple_or_optional_named(type_)
                .map_err(|message| at(document, source, span, "unsupported_typeql_function_return", message))?;
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
        let annotation_span = source_span(document, source, annotation.span())?;
        let (kind, value) = match annotation {
            Annotation::Abstract(_) => (AnnotationKindId::Abstract, SchemaAnnotationValue::Presence),
            Annotation::Independent(_) => {
                (AnnotationKindId::Independent, SchemaAnnotationValue::Presence)
            }
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
                (AnnotationKindId::Card, SchemaAnnotationValue::Cardinality(cardinality))
            }
            Annotation::Regex(regex) => {
                validate_annotation_string(
                    &regex.regex,
                    "regex",
                    annotation_span.clone(),
                )?;
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
                (AnnotationKindId::Regex, SchemaAnnotationValue::Regex(pattern))
            }
            Annotation::Doc(doc) => {
                validate_annotation_string(
                    &doc.doc,
                    "doc",
                    annotation_span.clone(),
                )?;
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
                validate_annotation_string(
                    &meta.key,
                    "meta_key",
                    annotation_span.clone(),
                )?;
                let key = meta.key.unescape().map_err(|unescape_error| {
                    error(
                        DiagnosticCategory::InvalidContract,
                        "invalid_typeql_meta_key",
                        format!("TypeQL metadata key decoding failed: {unescape_error}"),
                        Some(annotation_span.clone()),
                    )
                })?;
                validate_annotation_string(
                    &meta.value,
                    "meta_value",
                    annotation_span.clone(),
                )?;
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
                (kind, SchemaAnnotationValue::Meta(CanonicalValue::String(value)))
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
                (AnnotationKindId::Values, SchemaAnnotationValue::Values(values))
            }
        };
        let id = AnnotationFactId::new(subject.clone(), kind);
        let fact = AnnotationFact::new(id, value)
            .map_err(|diagnostic| contract(diagnostic, annotation_span.clone()))?;
        assembler.insert_fact(SchemaFact::Annotation(fact), annotation_span)?;
    }
    Ok(())
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
    capability: &Capability,
    document: &DocumentId,
    source: &str,
) -> Result<(), SchemaDiagnostics> {
    if let Some(annotation) = capability.annotations.first() {
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
    let NamedType::BuiltinValueType(value_type) = named else {
        return Err("schema-defined value types are not live-pinned here".to_owned());
    };
    value_type_from_token(&value_type.token.to_string())
}

fn value_type_from_token(token: &str) -> Result<ValueTypeTag, String> {
    match token {
        "string" => Ok(ValueTypeTag::String),
        "integer" => Ok(ValueTypeTag::Long),
        "double" => Ok(ValueTypeTag::Double),
        "boolean" => Ok(ValueTypeTag::Boolean),
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
        Ok(span) => error(DiagnosticCategory::InvalidContract, code, message, Some(span)),
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
