//! Official schema construction behind opaque sibling-facing nodes.
use super::builders::type_ref;
use super::{Query, TqlError};
use typeql::{
    annotation as a,
    common::{identifier::Identifier, token},
    query::{
        QueryStructure,
        schema::{Define, Redefine, SchemaQuery, Undefine},
    },
    schema::definable::{
        self,
        type_::{self as d, capability as c},
    },
    type_::{self, NamedType, NamedTypeAny, TypeRefAny},
    value::{self, StringLiteral},
};

#[derive(Clone, Debug)]
pub struct SchemaAnnotation {
    inner: a::Annotation,
}
#[derive(Clone, Debug)]
pub struct SchemaCapability {
    inner: d::Capability,
}
impl SchemaCapability {
    pub fn annotated(mut self, values: Vec<SchemaAnnotation>) -> Self {
        self.inner.annotations.extend(annotations(values));
        self
    }
}
#[derive(Debug)]
pub struct SchemaDefinition {
    inner: definable::Definable,
    caller_body: Option<String>,
}

fn label(name: &str) -> Result<type_::Label, TqlError> {
    typeql::parse_label(name).map_err(|_| TqlError::InvalidIdentifier)
}
fn quoted(value: &str) -> Result<StringLiteral, TqlError> {
    Ok(StringLiteral {
        value: serde_json::to_string(value).map_err(|_| TqlError::InvalidLiteral)?,
    })
}
fn annotations(values: Vec<SchemaAnnotation>) -> Vec<a::Annotation> {
    values.into_iter().map(|value| value.inner).collect()
}
fn capability(base: d::CapabilityBase, values: Vec<SchemaAnnotation>) -> SchemaCapability {
    SchemaCapability {
        inner: d::Capability::new(None, base, annotations(values)),
    }
}
fn reference(name: &str, list: bool) -> Result<TypeRefAny, TqlError> {
    let name = type_ref(name)?;
    Ok(if list {
        TypeRefAny::List(type_::TypeRefList::new(None, name))
    } else {
        TypeRefAny::Type(name)
    })
}
fn named_type(name: &str) -> Result<NamedType, TqlError> {
    Ok(if token::ValueType::NAMES.contains(&name) {
        NamedType::BuiltinValueType(type_::BuiltinValueType::new(
            None,
            token::ValueType::from(name),
        ))
    } else {
        NamedType::Label(label(name)?)
    })
}

pub fn schema_flag(name: &str) -> Result<SchemaAnnotation, TqlError> {
    Ok(SchemaAnnotation {
        inner: match name {
            "abstract" => a::Annotation::Abstract(a::Abstract::new(None)),
            "independent" => a::Annotation::Independent(a::Independent::new(None)),
            "key" => a::Annotation::Key(a::Key::new(None)),
            "unique" => a::Annotation::Unique(a::Unique::new(None)),
            "distinct" => a::Annotation::Distinct(a::Distinct::new(None)),
            "cascade" => a::Annotation::Cascade(a::Cascade::new(None)),
            _ => return Err(TqlError::InvalidSchemaShape),
        },
    })
}
pub fn schema_doc(value: &str) -> Result<SchemaAnnotation, TqlError> {
    Ok(SchemaAnnotation {
        inner: a::Annotation::Doc(a::Doc::new(None, quoted(value)?)),
    })
}
pub fn schema_meta(key: &str, value: &str) -> Result<SchemaAnnotation, TqlError> {
    Ok(SchemaAnnotation {
        inner: a::Annotation::Meta(a::Meta::new(None, quoted(key)?, quoted(value)?)),
    })
}
pub fn schema_subkey(name: &str) -> Result<SchemaAnnotation, TqlError> {
    label(name)?;
    Ok(SchemaAnnotation {
        inner: a::Annotation::Subkey(a::Subkey::new(None, Identifier::from(name))),
    })
}
pub fn schema_card(min: u64, max: Option<u64>) -> Result<SchemaAnnotation, TqlError> {
    if max.is_some_and(|max| max < min) {
        return Err(TqlError::InvalidSchemaShape);
    }
    Ok(SchemaAnnotation {
        inner: a::Annotation::Cardinality(a::Cardinality::new(
            None,
            a::CardinalityRange::Range(
                value::IntegerLiteral {
                    value: min.to_string(),
                },
                max.map(|value| value::IntegerLiteral {
                    value: value.to_string(),
                }),
            ),
        )),
    })
}
pub fn schema_regex_source(pattern: &str) -> Result<SchemaAnnotation, TqlError> {
    let source = format!("\"{}\"", pattern.replace('"', "\\\""));
    let value::ValueLiteral::String(literal) =
        typeql::parse_value(&source).map_err(|_| TqlError::InvalidLiteral)?
    else {
        return Err(TqlError::InvalidLiteral);
    };
    Ok(SchemaAnnotation {
        inner: a::Annotation::Regex(a::Regex::new(None, literal)),
    })
}
pub fn schema_regex(pattern: &str) -> Result<SchemaAnnotation, TqlError> {
    Ok(SchemaAnnotation {
        inner: a::Annotation::Regex(a::Regex::new(None, quoted(pattern)?)),
    })
}
pub fn schema_values(values: &[&str]) -> Result<SchemaAnnotation, TqlError> {
    if values.is_empty() {
        return Err(TqlError::InvalidSchemaShape);
    }
    let values = values
        .iter()
        .map(|value| {
            Ok(value::Literal {
                span: None,
                inner: value::ValueLiteral::String(quoted(value)?),
            })
        })
        .collect::<Result<_, TqlError>>()?;
    Ok(SchemaAnnotation {
        inner: a::Annotation::Values(a::Values::new(None, values)),
    })
}
pub fn schema_range(min: Option<&str>, max: Option<&str>) -> Result<SchemaAnnotation, TqlError> {
    if min.is_none() && max.is_none() {
        return Err(TqlError::InvalidSchemaShape);
    }
    let literal = |source: &str| {
        typeql::parse_value(source)
            .map(|inner| value::Literal { span: None, inner })
            .map_err(|_| TqlError::InvalidLiteral)
    };
    Ok(SchemaAnnotation {
        inner: a::Annotation::Range(a::Range::new(
            None,
            min.map(literal).transpose()?,
            max.map(literal).transpose()?,
        )),
    })
}
pub fn render_schema_annotations(values: &[SchemaAnnotation]) -> String {
    values
        .iter()
        .map(|value| value.inner.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn schema_sub(parent: &str) -> Result<SchemaCapability, TqlError> {
    Ok(capability(
        d::CapabilityBase::Sub(c::Sub::new(None, label(parent)?)),
        vec![],
    ))
}
pub fn schema_value(
    name: &str,
    values: Vec<SchemaAnnotation>,
) -> Result<SchemaCapability, TqlError> {
    Ok(capability(
        d::CapabilityBase::ValueType(c::ValueType::new(None, named_type(name)?)),
        values,
    ))
}
pub fn schema_owns(
    name: &str,
    list: bool,
    values: Vec<SchemaAnnotation>,
) -> Result<SchemaCapability, TqlError> {
    Ok(capability(
        d::CapabilityBase::Owns(c::Owns::new(None, reference(name, list)?)),
        values,
    ))
}
pub fn schema_plays(
    scope: &str,
    name: &str,
    values: Vec<SchemaAnnotation>,
) -> Result<SchemaCapability, TqlError> {
    Ok(capability(
        d::CapabilityBase::Plays(c::Plays::new(
            None,
            type_::ScopedLabel::new(None, label(scope)?, label(name)?),
        )),
        values,
    ))
}
pub fn schema_relates(
    name: &str,
    list: bool,
    specialised: Option<&str>,
    values: Vec<SchemaAnnotation>,
) -> Result<SchemaCapability, TqlError> {
    Ok(capability(
        d::CapabilityBase::Relates(c::Relates::new(
            None,
            reference(name, list)?,
            specialised.map(|name| reference(name, list)).transpose()?,
        )),
        values,
    ))
}
pub fn schema_type(
    kind: Option<&str>,
    name: &str,
    values: Vec<SchemaAnnotation>,
    capabilities: Vec<SchemaCapability>,
) -> Result<SchemaDefinition, TqlError> {
    let kind = kind
        .map(|kind| {
            if token::Kind::NAMES.contains(&kind) {
                Ok(token::Kind::from(kind))
            } else {
                Err(TqlError::InvalidSchemaShape)
            }
        })
        .transpose()?;
    Ok(SchemaDefinition {
        caller_body: None,
        inner: definable::Definable::TypeDeclaration(d::Type {
            span: None,
            kind,
            label: label(name)?,
            annotations: annotations(values),
            capabilities: capabilities.into_iter().map(|value| value.inner).collect(),
        }),
    })
}
pub fn schema_struct(
    name: &str,
    fields: Vec<(&str, &str, bool)>,
) -> Result<SchemaDefinition, TqlError> {
    if fields.is_empty() {
        return Err(TqlError::InvalidSchemaShape);
    }
    label(name)?;
    let fields = fields
        .into_iter()
        .map(|(name, domain, optional)| {
            label(name)?;
            let domain = named_type(domain)?;
            let domain = if optional {
                NamedTypeAny::Optional(type_::NamedTypeOptional::new(None, domain))
            } else {
                NamedTypeAny::Simple(domain)
            };
            Ok(definable::struct_::Field::new(
                None,
                Identifier::from(name),
                domain,
                vec![],
            ))
        })
        .collect::<Result<_, TqlError>>()?;
    Ok(SchemaDefinition {
        caller_body: None,
        inner: definable::Definable::Struct(definable::struct_::Struct::new(
            None,
            Identifier::from(name),
            fields,
            vec![],
        )),
    })
}
pub fn schema_define(definitions: Vec<SchemaDefinition>) -> Query {
    schema_query(definitions, false)
}
pub fn schema_redefine(definitions: Vec<SchemaDefinition>) -> Query {
    schema_query(definitions, true)
}
fn schema_query(definitions: Vec<SchemaDefinition>, redefine: bool) -> Query {
    let bodies = definitions
        .iter()
        .enumerate()
        .filter_map(|(index, definition)| {
            definition
                .caller_body
                .as_ref()
                .map(|body| (index, body.clone()))
        })
        .collect();
    let definitions = definitions.into_iter().map(|value| value.inner).collect();
    let schema = if redefine {
        SchemaQuery::Redefine(Redefine::build(definitions))
    } else {
        SchemaQuery::Define(Define::build(definitions))
    };
    let mut query = Query::from_ast(typeql::Query {
        span: None,
        structure: QueryStructure::Schema(schema),
        has_explicit_end: false,
    });
    query.function_bodies = bodies;
    query
}

pub fn schema_values_source(values: &[&str]) -> Result<SchemaAnnotation, TqlError> {
    if values.is_empty() {
        return Err(TqlError::InvalidSchemaShape);
    }
    let values = values
        .iter()
        .map(|value| {
            let source = format!("\"{}\"", value.replace('"', "\\\""));
            typeql::parse_value(&source)
                .map(|inner| value::Literal { span: None, inner })
                .map_err(|_| TqlError::InvalidLiteral)
        })
        .collect::<Result<_, _>>()?;
    Ok(SchemaAnnotation {
        inner: a::Annotation::Values(a::Values::new(None, values)),
    })
}
pub fn render_schema_definition(value: SchemaDefinition) -> Result<String, TqlError> {
    let source = super::render(&schema_define(vec![value]))?;
    source
        .strip_prefix("define ")
        .map(str::to_owned)
        .ok_or(TqlError::InvalidSchemaShape)
}
pub fn schema_annotations_source(source: &str) -> Result<Vec<SchemaAnnotation>, TqlError> {
    let source = format!("define entity annotation_probe {source};");
    let query = typeql::parse_query(&source).map_err(|_| TqlError::InvalidSchemaShape)?;
    let QueryStructure::Schema(SchemaQuery::Define(mut define)) = query.structure else {
        return Err(TqlError::InvalidSchemaShape);
    };
    if define.definables.len() != 1 {
        return Err(TqlError::InvalidSchemaShape);
    }
    let Some(definable::Definable::TypeDeclaration(declaration)) = define.definables.pop() else {
        return Err(TqlError::InvalidSchemaShape);
    };
    if declaration.kind != Some(token::Kind::Entity)
        || declaration.label.to_string() != "annotation_probe"
        || !declaration.capabilities.is_empty()
    {
        return Err(TqlError::InvalidSchemaShape);
    }
    Ok(declaration
        .annotations
        .into_iter()
        .map(|inner| SchemaAnnotation { inner })
        .collect())
}
pub fn schema_annotation(name: &str, args: Option<&str>) -> Result<SchemaAnnotation, TqlError> {
    let source = match args {
        Some(args) => format!("@{name}({args})"),
        None => format!("@{name}"),
    };
    let mut annotations = schema_annotations_source(&source)?;
    if annotations.len() != 1 {
        return Err(TqlError::InvalidSchemaShape);
    }
    annotations.pop().ok_or(TqlError::InvalidSchemaShape)
}
/// Validate a caller-owned function and retain its body bytes through rendering.
pub fn schema_function(signature: &str, body: &str) -> Result<SchemaDefinition, TqlError> {
    let source = format!("define {signature}:\n{body}");
    let query = typeql::parse_query(&source).map_err(|_| TqlError::InvalidSchemaShape)?;
    let QueryStructure::Schema(SchemaQuery::Define(mut define)) = query.structure else {
        return Err(TqlError::InvalidSchemaShape);
    };
    if define.definables.len() != 1 {
        return Err(TqlError::InvalidSchemaShape);
    }
    let Some(inner @ definable::Definable::Function(_)) = define.definables.pop() else {
        return Err(TqlError::InvalidSchemaShape);
    };
    Ok(SchemaDefinition {
        inner,
        caller_body: Some(body.to_owned()),
    })
}

// TypeDB retains function body text in schema exports. Preserve caller-owned
// bytes so formatting cannot change declared fingerprints or create drift.
pub(super) fn restore_function_bodies(
    mut source: String,
    bodies: &[(usize, String)],
) -> Result<String, TqlError> {
    if bodies.is_empty() {
        return Ok(source);
    }
    let parsed = typeql::parse_query(&source).map_err(|_| TqlError::InvalidSchemaShape)?;
    let definitions = match &parsed.structure {
        QueryStructure::Schema(SchemaQuery::Define(value)) => &value.definables,
        QueryStructure::Schema(SchemaQuery::Redefine(value)) => &value.definables,
        _ => return Err(TqlError::InvalidSchemaShape),
    };
    let canonical = source.clone();
    for (index, body) in bodies.iter().rev() {
        let Some(definable::Definable::Function(function)) = definitions.get(*index) else {
            return Err(TqlError::InvalidSchemaShape);
        };
        let span = function.block.span.ok_or(TqlError::InvalidSchemaShape)?;
        // A trailing line comment must not consume the next official node.
        source.replace_range(span.begin_offset..span.end_offset, &format!("{body}\n"));
    }
    if !super::equivalent(&canonical, &source)? {
        return Err(TqlError::InvalidSchemaShape);
    }
    Ok(source)
}

pub fn render_schema_document(
    definitions: Vec<(SchemaDefinition, Vec<String>)>,
) -> Result<String, TqlError> {
    if definitions.is_empty() {
        return Ok("define".to_owned());
    }
    let (definitions, comments): (Vec<_>, Vec<_>) = definitions.into_iter().unzip();
    let mut source = super::render(&schema_define(definitions))?;
    if comments.iter().all(Vec::is_empty) {
        return Ok(source);
    }
    // Comments carry bindgen metadata, not grammar nodes. Locate definitions
    // with official byte spans so formatting cannot move their association.
    use typeql::common::Spanned;
    let parsed = typeql::parse_query(&source).map_err(|_| TqlError::InvalidSchemaShape)?;
    let QueryStructure::Schema(SchemaQuery::Define(define)) = parsed.structure else {
        return Err(TqlError::InvalidSchemaShape);
    };
    for (definition, comments) in define.definables.iter().zip(&comments).rev() {
        if comments.is_empty() {
            continue;
        }
        let span = match definition {
            definable::Definable::TypeDeclaration(value) => value.span(),
            definable::Definable::Function(value) => value.span(),
            definable::Definable::Struct(value) => value.span(),
            _ => return Err(TqlError::InvalidSchemaShape),
        };
        let offset = span.ok_or(TqlError::InvalidSchemaShape)?.begin_offset;
        let mut text = String::from("\n");
        for comment in comments {
            // Keep every metadata line a comment even for multiline input.
            for line in comment.lines() {
                text.push_str("# @");
                text.push_str(line);
                text.push('\n');
            }
        }
        source.insert_str(offset, &text);
    }
    Ok(source)
}

pub fn schema_type_pattern(
    variable: &str,
    kind: &str,
    target: &str,
) -> Result<super::Pattern, TqlError> {
    use typeql::{
        pattern::Pattern as OfficialPattern,
        statement::{
            Statement,
            type_::{
                Constraint, ConstraintBase, LabelConstraint, Owns, Relates, Sub, SubKind, Type,
            },
        },
    };
    let base = match kind {
        "sub!" => ConstraintBase::Sub(Sub::new(None, SubKind::Direct, type_ref(target)?)),
        "label" => ConstraintBase::Label(LabelConstraint::Name(label(target)?)),
        "owns" => ConstraintBase::Owns(Owns::new(None, TypeRefAny::Type(type_ref(target)?))),
        "relates" => ConstraintBase::Relates(Relates::new(
            None,
            TypeRefAny::Type(type_ref(target)?),
            None,
        )),
        _ => return Err(TqlError::InvalidSchemaShape),
    };
    Ok(super::Pattern {
        inner: OfficialPattern::Statement(Statement::Type(Type::new(
            None,
            None,
            type_ref(variable)?,
            vec![Constraint::new(None, base, vec![])],
        ))),
    })
}

#[derive(Debug)]
pub struct SchemaUndefinition {
    inner: typeql::schema::undefinable::Undefinable,
}

pub fn schema_undefine(values: Vec<SchemaUndefinition>) -> Query {
    Query::from_ast(typeql::Query {
        span: None,
        structure: QueryStructure::Schema(SchemaQuery::Undefine(Undefine::build(
            values.into_iter().map(|value| value.inner).collect(),
        ))),
        has_explicit_end: false,
    })
}
pub fn schema_undefine_type(name: &str) -> Result<SchemaUndefinition, TqlError> {
    Ok(SchemaUndefinition {
        inner: typeql::schema::undefinable::Undefinable::Type(label(name)?),
    })
}
pub fn schema_undefine_function(name: &str) -> Result<SchemaUndefinition, TqlError> {
    use typeql::schema::undefinable as u;
    label(name)?;
    Ok(SchemaUndefinition {
        inner: u::Undefinable::Function(u::Function::new(None, Identifier::from(name))),
    })
}
pub fn schema_undefine_struct(name: &str) -> Result<SchemaUndefinition, TqlError> {
    use typeql::schema::undefinable as u;
    label(name)?;
    Ok(SchemaUndefinition {
        inner: u::Undefinable::Struct(u::Struct::new(None, Identifier::from(name))),
    })
}
pub fn schema_undefine_capability(
    name: &str,
    capability: SchemaCapability,
) -> Result<SchemaUndefinition, TqlError> {
    use typeql::schema::undefinable as u;
    if !capability.inner.annotations.is_empty() {
        return Err(TqlError::InvalidSchemaShape);
    }
    Ok(SchemaUndefinition {
        inner: u::Undefinable::CapabilityType(u::CapabilityType::new(
            None,
            capability.inner.base,
            label(name)?,
        )),
    })
}
pub fn schema_undefine_specialisation(
    relation: &str,
    role: &str,
    parent: &str,
) -> Result<SchemaUndefinition, TqlError> {
    schema_undefine_specialisation_with_ordering(relation, role, parent, false)
}
pub fn schema_undefine_specialisation_with_ordering(
    relation: &str,
    role: &str,
    parent: &str,
    list: bool,
) -> Result<SchemaUndefinition, TqlError> {
    use typeql::schema::undefinable as u;
    Ok(SchemaUndefinition {
        inner: u::Undefinable::Specialise(u::Specialise::new(
            None,
            label(parent)?,
            label(relation)?,
            c::Relates::new(None, reference(role, list)?, None),
        )),
    })
}
pub fn schema_undefine_annotation(
    name: &str,
    key: Option<&str>,
    subject: &str,
    capability: Option<SchemaCapability>,
) -> Result<SchemaUndefinition, TqlError> {
    use typeql::schema::undefinable as u;
    if name != "meta" && key.is_some() {
        return Err(TqlError::InvalidSchemaShape);
    }
    let category = match name {
        "abstract" => u::AnnotationCategory::Abstract,
        "card" => u::AnnotationCategory::Cardinality,
        "cascade" => u::AnnotationCategory::Cascade,
        "distinct" => u::AnnotationCategory::Distinct,
        "doc" => u::AnnotationCategory::Doc,
        "independent" => u::AnnotationCategory::Independent,
        "key" => u::AnnotationCategory::Key,
        "meta" => {
            u::AnnotationCategory::Meta(quoted(key.ok_or(TqlError::InvalidSchemaShape)?)?, None)
        }
        "range" => u::AnnotationCategory::Range,
        "regex" => u::AnnotationCategory::Regex,
        "subkey" => u::AnnotationCategory::Subkey,
        "unique" => u::AnnotationCategory::Unique,
        "values" => u::AnnotationCategory::Values,
        _ => return Err(TqlError::InvalidSchemaShape),
    };
    let subject = label(subject)?;
    let inner = if let Some(capability) = capability {
        if !capability.inner.annotations.is_empty() {
            return Err(TqlError::InvalidSchemaShape);
        }
        u::Undefinable::AnnotationCapability(u::AnnotationCapability::new(
            None,
            category,
            subject,
            capability.inner.base,
        ))
    } else {
        u::Undefinable::AnnotationType(u::AnnotationType::new(None, category, subject))
    };
    Ok(SchemaUndefinition { inner })
}

pub fn schema_value_domain(
    domain: type_bridge_contract::value::ValueTypeTag,
    values: Vec<SchemaAnnotation>,
) -> SchemaCapability {
    capability(
        d::CapabilityBase::ValueType(c::ValueType::new(
            None,
            super::builders::builtin_named_type(domain),
        )),
        values,
    )
}
fn canonical_literal(
    value: &type_bridge_contract::value::CanonicalValue,
) -> Result<value::Literal, TqlError> {
    let expression = match value {
        type_bridge_contract::value::CanonicalValue::Double(value) => {
            super::literal_source(&format!("{:?}", value.get()))?
        }
        _ => super::literal(value)?,
    };
    let typeql::Expression::Value(value) = expression.inner else {
        return Err(TqlError::InvalidLiteral);
    };
    Ok(value)
}
pub fn schema_canonical_values(
    values: &[type_bridge_contract::value::CanonicalValue],
) -> Result<SchemaAnnotation, TqlError> {
    if values.is_empty() {
        return Err(TqlError::InvalidSchemaShape);
    }
    Ok(SchemaAnnotation {
        inner: a::Annotation::Values(a::Values::new(
            None,
            values
                .iter()
                .map(canonical_literal)
                .collect::<Result<_, _>>()?,
        )),
    })
}
pub fn schema_canonical_range(
    min: Option<&type_bridge_contract::value::CanonicalValue>,
    max: Option<&type_bridge_contract::value::CanonicalValue>,
) -> Result<SchemaAnnotation, TqlError> {
    if min.is_none() && max.is_none() {
        return Err(TqlError::InvalidSchemaShape);
    }
    Ok(SchemaAnnotation {
        inner: a::Annotation::Range(a::Range::new(
            None,
            min.map(canonical_literal).transpose()?,
            max.map(canonical_literal).transpose()?,
        )),
    })
}

/// Lower a typed signature while retaining the caller-owned function body.
pub fn schema_function_fact(
    function: &type_bridge_contract::schema::FunctionFact,
    values: Vec<SchemaAnnotation>,
) -> Result<SchemaDefinition, TqlError> {
    use type_bridge_contract::schema::{FunctionReturnMode, TypeReference};
    use typeql::schema::definable::function as f;
    let named = |reference: &TypeReference| -> Result<NamedType, TqlError> {
        match reference {
            TypeReference::Value(domain) => Ok(super::builders::builtin_named_type(*domain)),
            TypeReference::Schema(name) => Ok(NamedType::Label(label(name.as_str())?)),
        }
    };
    label(function.id().label().as_str())?;
    let args = function
        .signature()
        .parameters()
        .iter()
        .map(|parameter| {
            Ok(f::Argument::new(
                None,
                super::builders::variable(parameter.name().as_str())?,
                NamedTypeAny::Simple(named(parameter.type_ref())?),
            ))
        })
        .collect::<Result<_, TqlError>>()?;
    let types = function
        .signature()
        .returns()
        .elements()
        .iter()
        .map(|element| {
            let named = named(element.type_ref())?;
            Ok(if element.optional() {
                NamedTypeAny::Optional(type_::NamedTypeOptional::new(None, named))
            } else {
                NamedTypeAny::Simple(named)
            })
        })
        .collect::<Result<_, TqlError>>()?;
    let output = match function.signature().returns() {
        FunctionReturnMode::Stream(_) => f::Output::Stream(f::Stream::new(None, types)),
        FunctionReturnMode::Scalar(_) | FunctionReturnMode::Tuple(_) => {
            f::Output::Single(f::Single::new(None, types))
        }
    };
    let signature = f::Signature {
        span: None,
        ident: Identifier::from(function.id().label().as_str()),
        args,
        output,
    };
    let mut definition = schema_function(&format!("fun {signature}"), function.body().text())?;
    let definable::Definable::Function(node) = &mut definition.inner else {
        return Err(TqlError::InvalidSchemaShape);
    };
    node.annotations = annotations(values);
    Ok(definition)
}

// Released operation specs encode ordered references as source text. Parse
// that bounded slot with the official grammar before constructing the node.
fn reference_source(source: &str, relates: bool) -> Result<TypeRefAny, TqlError> {
    let kind = if relates { "relates" } else { "owns" };
    let source = format!("define reference_probe {kind} {source};");
    let query = typeql::parse_query(&source).map_err(|_| TqlError::InvalidSchemaShape)?;
    let QueryStructure::Schema(SchemaQuery::Define(mut define)) = query.structure else {
        return Err(TqlError::InvalidSchemaShape);
    };
    if define.definables.len() != 1 {
        return Err(TqlError::InvalidSchemaShape);
    }
    let Some(definable::Definable::TypeDeclaration(mut node)) = define.definables.pop() else {
        return Err(TqlError::InvalidSchemaShape);
    };
    if node.label.to_string() != "reference_probe"
        || node.kind.is_some()
        || !node.annotations.is_empty()
        || node.capabilities.len() != 1
    {
        return Err(TqlError::InvalidSchemaShape);
    }
    let capability = node
        .capabilities
        .pop()
        .ok_or(TqlError::InvalidSchemaShape)?;
    if !capability.annotations.is_empty() {
        return Err(TqlError::InvalidSchemaShape);
    }
    match capability.base {
        d::CapabilityBase::Owns(owns) if !relates => Ok(owns.owned),
        d::CapabilityBase::Relates(role) if relates && role.specialised.is_none() => {
            Ok(role.related)
        }
        _ => Err(TqlError::InvalidSchemaShape),
    }
}
pub fn schema_owns_source(
    name: &str,
    values: Vec<SchemaAnnotation>,
) -> Result<SchemaCapability, TqlError> {
    Ok(capability(
        d::CapabilityBase::Owns(c::Owns::new(None, reference_source(name, false)?)),
        values,
    ))
}
pub fn schema_relates_source(
    name: &str,
    values: Vec<SchemaAnnotation>,
) -> Result<SchemaCapability, TqlError> {
    Ok(capability(
        d::CapabilityBase::Relates(c::Relates::new(None, reference_source(name, true)?, None)),
        values,
    ))
}
pub fn schema_relates_with_parent_source(
    name: &str,
    list: bool,
    parent: Option<&str>,
    values: Vec<SchemaAnnotation>,
) -> Result<SchemaCapability, TqlError> {
    let parent = parent
        .map(|parent| reference_source(parent, true))
        .transpose()?
        .map(|parent| {
            match (list, parent) {
                // A bare legacy parent name inherits the child's ordering syntax.
                (true, TypeRefAny::Type(parent)) => {
                    Ok(TypeRefAny::List(type_::TypeRefList::new(None, parent)))
                }
                (false, TypeRefAny::List(_)) => Err(TqlError::InvalidSchemaShape),
                (_, parent) => Ok(parent),
            }
        })
        .transpose()?;
    Ok(capability(
        d::CapabilityBase::Relates(c::Relates::new(None, reference(name, list)?, parent)),
        values,
    ))
}

/// Match schema types by their builtin kind.
pub fn schema_kind_pattern(variable: &str, kind: &str) -> Result<super::Pattern, TqlError> {
    use typeql::{
        pattern::Pattern as OfficialPattern,
        statement::{Statement, type_::Type},
    };
    if !token::Kind::NAMES.contains(&kind) {
        return Err(TqlError::InvalidSchemaShape);
    }
    Ok(super::Pattern {
        inner: OfficialPattern::Statement(Statement::Type(Type::new(
            None,
            Some(token::Kind::from(kind)),
            type_ref(variable)?,
            vec![],
        ))),
    })
}
