use crate::_schema::*;
use std::collections::BTreeMap;
use typeql::{
    annotation::Annotation as A,
    query::{QueryStructure, schema::SchemaQuery},
    schema::definable::{
        Definable,
        type_::{CapabilityBase as C, Type},
    },
    type_::{NamedTypeAny, TypeRef, TypeRefAny},
    value::{StringLiteral, ValueLiteral},
};

fn unsupported(construct: &str) -> SchemaError {
    SchemaError::UnsupportedProjection {
        construct: construct.to_owned(),
    }
}
fn string(value: &StringLiteral) -> Result<String, SchemaError> {
    value.unescape().map_err(|_| unsupported("string literal"))
}
fn reference(value: &TypeRefAny) -> Result<(String, bool), SchemaError> {
    let (value, ordered) = match value {
        TypeRefAny::Type(value) => (value, false),
        TypeRefAny::List(value) => (&value.inner, true),
    };
    match value {
        TypeRef::Label(value) => Ok((value.to_string(), ordered)),
        _ => Err(unsupported("non-label schema reference")),
    }
}
fn named(value: &NamedTypeAny) -> Result<(String, bool), SchemaError> {
    match value {
        NamedTypeAny::Simple(value) => Ok((value.to_string(), false)),
        NamedTypeAny::Optional(value) => Ok((value.inner.to_string(), true)),
        _ => Err(unsupported("list signature type")),
    }
}
#[derive(Default)]
struct Annotations {
    abstract_: bool,
    independent: bool,
    key: bool,
    unique: bool,
    cascade: bool,
    distinct: bool,
    subkey: Option<String>,
    card: Option<Cardinality>,
    doc: Option<String>,
    meta: BTreeMap<String, String>,
    regex: Option<String>,
    values: Option<Vec<String>>,
    min: Option<String>,
    max: Option<String>,
}
fn annotations(values: &[A], allowed: &[&str]) -> Result<Annotations, SchemaError> {
    let mut output = Annotations::default();
    for value in values {
        let name = match value {
            A::Abstract(_) => "abstract",
            A::Independent(_) => "independent",
            A::Key(_) => "key",
            A::Unique(_) => "unique",
            A::Cascade(_) => "cascade",
            A::Distinct(_) => "distinct",
            A::Subkey(_) => "subkey",
            A::Cardinality(_) => "card",
            A::Doc(_) => "doc",
            A::Meta(_) => "meta",
            A::Regex(_) => "regex",
            A::Values(_) => "values",
            A::Range(_) => "range",
        };
        if !allowed.contains(&name) {
            return Err(unsupported("annotation on unsupported schema subject"));
        }
        match value {
            A::Abstract(_) => output.abstract_ = true,
            A::Independent(_) => output.independent = true,
            A::Key(_) => output.key = true,
            A::Unique(_) => output.unique = true,
            A::Cascade(_) => output.cascade = true,
            A::Distinct(_) => output.distinct = true,
            A::Subkey(value) => output.subkey = Some(value.ident.to_string()),
            A::Doc(value) => output.doc = Some(string(&value.doc)?),
            A::Meta(value) => {
                output
                    .meta
                    .insert(string(&value.key)?, string(&value.value)?);
            }
            A::Cardinality(value) => {
                use typeql::annotation::CardinalityRange;
                let (min, max) = match &value.range {
                    CardinalityRange::Exact(value) => (value, Some(value)),
                    CardinalityRange::Range(min, max) => (min, max.as_ref()),
                };
                let integer = |value: &typeql::value::IntegerLiteral| {
                    value
                        .value
                        .parse::<u32>()
                        .map_err(|_| unsupported("cardinality outside u32"))
                };
                output.card = Some(Cardinality {
                    min: integer(min)?,
                    max: max.map(integer).transpose()?,
                });
            }
            A::Regex(value) => {
                output.regex = Some(
                    value
                        .regex
                        .unescape_regex()
                        .map_err(|_| unsupported("regex literal"))?,
                )
            }
            A::Values(value) => {
                output.values = Some(
                    value
                        .values
                        .iter()
                        .map(|value| match &value.inner {
                            ValueLiteral::String(value) => string(value),
                            _ => Err(unsupported("non-string values constraint")),
                        })
                        .collect::<Result<_, _>>()?,
                )
            }
            A::Range(value) => {
                output.min = value.min.as_ref().map(ToString::to_string);
                output.max = value.max.as_ref().map(ToString::to_string);
            }
        }
    }
    Ok(output)
}
fn ownership(value: &TypeRefAny, ann: &[A]) -> Result<OwnedAttribute, SchemaError> {
    let (name, ordered) = reference(value)?;
    let a = annotations(
        ann,
        &[
            "key", "unique", "cascade", "subkey", "card", "distinct", "doc", "meta",
        ],
    )?;
    Ok(OwnedAttribute {
        name,
        is_key: a.key,
        is_unique: a.unique,
        is_cascade: a.cascade,
        subkey_group: a.subkey,
        cardinality: a.card,
        ordered,
        distinct: a.distinct,
        doc: a.doc,
        meta: a.meta,
    })
}
fn plays(value: &typeql::type_::ScopedLabel, ann: &[A]) -> Result<PlayedRole, SchemaError> {
    let a = annotations(ann, &["card", "doc", "meta"])?;
    Ok(PlayedRole {
        role_ref: value.to_string(),
        cardinality: a.card,
        doc: a.doc,
        meta: a.meta,
    })
}
fn role(
    value: &typeql::schema::definable::type_::capability::Relates,
    ann: &[A],
) -> Result<RoleSpec, SchemaError> {
    let (name, ordered) = reference(&value.related)?;
    let overrides = value.specialised.as_ref().map(reference).transpose()?;
    if overrides
        .as_ref()
        .is_some_and(|(_, parent_ordered)| *parent_ordered != ordered)
    {
        return Err(unsupported("role specialization order mismatch"));
    }
    let a = annotations(ann, &["card", "distinct", "abstract", "doc", "meta"])?;
    Ok(RoleSpec {
        name,
        overrides: overrides.map(|(name, _)| name),
        cardinality: a.card,
        distinct: a.distinct,
        ordered,
        is_abstract: a.abstract_,
        doc: a.doc,
        meta: a.meta,
    })
}
fn kind(
    name: &str,
    declarations: &[&Type],
    seen: &mut Vec<String>,
) -> Result<&'static str, SchemaError> {
    if seen.iter().any(|item| item == name) {
        return Err(SchemaError::InheritanceCycle {
            type_name: name.to_owned(),
        });
    }
    seen.push(name.to_owned());
    for declaration in declarations
        .iter()
        .filter(|value| value.label.to_string() == name)
    {
        if let Some(kind) = declaration.kind {
            return match kind {
                typeql::token::Kind::Entity => Ok("entity"),
                typeql::token::Kind::Relation => Ok("relation"),
                typeql::token::Kind::Attribute => Ok("attribute"),
                _ => Err(unsupported("schema type kind")),
            };
        }
    }
    for declaration in declarations
        .iter()
        .filter(|value| value.label.to_string() == name)
    {
        for capability in &declaration.capabilities {
            if let C::Sub(parent) = &capability.base {
                return kind(&parent.supertype_label.to_string(), declarations, seen);
            }
        }
    }
    Err(unsupported("unbound schema type extension"))
}
pub(super) fn project(source: &str) -> Result<TypeSchema, SchemaError> {
    let query = super::parse(source).map_err(|error| match error {
        super::TqlError::InvalidSyntax { line, column } => SchemaError::ParseError {
            message: "invalid TypeQL schema syntax".into(),
            line,
            column,
        },
        _ => unsupported("schema syntax"),
    })?;
    let QueryStructure::Schema(SchemaQuery::Define(define)) = &query.inner.structure else {
        return Err(unsupported("non-define schema query"));
    };
    let declarations = define
        .definables
        .iter()
        .filter_map(|value| {
            if let Definable::TypeDeclaration(value) = value {
                Some(value)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut schema = TypeSchema::new();
    for declaration in &declarations {
        let name = declaration.label.to_string();
        let kind = kind(&name, &declarations, &mut vec![])?;
        match kind {
            "entity" => {
                schema.entities.entry(name.clone()).or_insert(EntityType {
                    name,
                    parent: None,
                    is_abstract: false,
                    owns: vec![],
                    owns_order: vec![],
                    plays: vec![],
                    doc: None,
                    meta: BTreeMap::new(),
                });
            }
            "relation" => {
                schema
                    .relations
                    .entry(name.clone())
                    .or_insert(RelationType {
                        name,
                        parent: None,
                        is_abstract: false,
                        roles: vec![],
                        owns: vec![],
                        owns_order: vec![],
                        plays: vec![],
                        doc: None,
                        meta: BTreeMap::new(),
                    });
            }
            "attribute" => {
                schema
                    .attributes
                    .entry(name.clone())
                    .or_insert(AttributeType {
                        name,
                        value_type: String::new(),
                        parent: None,
                        is_abstract: false,
                        is_independent: false,
                        regex: None,
                        allowed_values: None,
                        range_min: None,
                        range_max: None,
                        doc: None,
                        meta: BTreeMap::new(),
                    });
            }
            _ => return Err(unsupported("schema kind")),
        }
    }
    for declaration in declarations {
        let name = declaration.label.to_string();
        if let Some(attribute) = schema.attributes.get_mut(&name) {
            let a = annotations(
                &declaration.annotations,
                &["abstract", "independent", "doc", "meta"],
            )?;
            attribute.is_abstract |= a.abstract_;
            attribute.is_independent |= a.independent;
            if a.doc.is_some() {
                attribute.doc = a.doc;
            }
            attribute.meta.extend(a.meta);
            for capability in &declaration.capabilities {
                match &capability.base {
                    C::Sub(value) => {
                        if !capability.annotations.is_empty() {
                            return Err(unsupported("annotated sub capability"));
                        }
                        attribute.parent = Some(value.supertype_label.to_string());
                    }
                    C::ValueType(value) => {
                        attribute.value_type = value.value_type.to_string();
                        let a =
                            annotations(&capability.annotations, &["regex", "values", "range"])?;
                        attribute.regex = a.regex;
                        attribute.allowed_values = a.values;
                        attribute.range_min = a.min;
                        attribute.range_max = a.max;
                    }
                    _ => return Err(unsupported("attribute capability")),
                }
            }
        } else {
            let a = annotations(&declaration.annotations, &["abstract", "doc", "meta"])?;
            let (parent, abstract_, owns, order, played, doc, meta, mut roles) =
                if let Some(entity) = schema.entities.get_mut(&name) {
                    (
                        &mut entity.parent,
                        &mut entity.is_abstract,
                        &mut entity.owns,
                        &mut entity.owns_order,
                        &mut entity.plays,
                        &mut entity.doc,
                        &mut entity.meta,
                        None,
                    )
                } else if let Some(relation) = schema.relations.get_mut(&name) {
                    (
                        &mut relation.parent,
                        &mut relation.is_abstract,
                        &mut relation.owns,
                        &mut relation.owns_order,
                        &mut relation.plays,
                        &mut relation.doc,
                        &mut relation.meta,
                        Some(&mut relation.roles),
                    )
                } else {
                    return Err(unsupported("unknown schema type"));
                };
            *abstract_ |= a.abstract_;
            if a.doc.is_some() {
                *doc = a.doc;
            }
            meta.extend(a.meta);
            for capability in &declaration.capabilities {
                match &capability.base {
                    C::Sub(value) => {
                        if !capability.annotations.is_empty() {
                            return Err(unsupported("annotated sub capability"));
                        }
                        *parent = Some(value.supertype_label.to_string());
                    }
                    C::Owns(value) => {
                        let value = ownership(&value.owned, &capability.annotations)?;
                        order.push(value.name.clone());
                        owns.push(value);
                    }
                    C::Plays(value) => played.push(plays(&value.role, &capability.annotations)?),
                    C::Relates(value) => roles
                        .as_mut()
                        .ok_or_else(|| unsupported("relates on entity"))?
                        .push(role(value, &capability.annotations)?),
                    _ => return Err(unsupported("object capability")),
                }
            }
        }
    }
    for definable in &define.definables {
        match definable {
            Definable::TypeDeclaration(_) => {}
            Definable::TypeRename(_) | Definable::RoleTypeRename(_) => {
                return Err(unsupported("schema rename"));
            }
            Definable::Function(function) => {
                if !function.annotations.is_empty() {
                    return Err(unsupported("annotated function signature"));
                }
                let signature = &function.signature;
                let name = signature.ident.to_string();
                let parameters = signature
                    .args
                    .iter()
                    .map(|arg| {
                        let (type_, optional) = named(&arg.type_)?;
                        if optional {
                            return Err(unsupported("optional function parameter"));
                        }
                        Ok(Parameter {
                            name: arg.var.to_string().trim_start_matches('$').to_owned(),
                            type_,
                        })
                    })
                    .collect::<Result<_, SchemaError>>()?;
                let (is_stream, types) = match &signature.output {
                    typeql::schema::definable::function::Output::Stream(value) => {
                        (true, &value.types)
                    }
                    typeql::schema::definable::function::Output::Single(value) => {
                        (false, &value.types)
                    }
                };
                let types = types
                    .iter()
                    .map(|value| {
                        named(value).map(|(name, optional)| ReturnTypeItem { name, optional })
                    })
                    .collect::<Result<_, _>>()?;
                schema.functions.insert(
                    name.clone(),
                    FunctionType {
                        name,
                        parameters,
                        return_type: ReturnType { is_stream, types },
                    },
                );
            }
            Definable::Struct(value) => {
                if !value.annotations.is_empty() {
                    return Err(unsupported("annotated struct signature"));
                }
                let name = value.ident.to_string();
                let fields = value
                    .fields
                    .iter()
                    .map(|field| {
                        if !field.annotations.is_empty() {
                            return Err(unsupported("annotated struct field"));
                        }
                        let (value_type, optional) = named(&field.type_)?;
                        Ok(StructField {
                            name: field.key.to_string(),
                            value_type,
                            optional,
                        })
                    })
                    .collect::<Result<_, SchemaError>>()?;
                schema
                    .structs
                    .insert(name.clone(), StructType { name, fields });
            }
        }
    }
    schema.validate_and_resolve()
}
