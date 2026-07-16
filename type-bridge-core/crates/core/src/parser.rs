//! Winnow-based parser for TypeQL `define` blocks.
//!
//! Converts TypeQL schema text into [`TypeSchema`](crate::schema::TypeSchema) structs.

use std::collections::BTreeMap;

use winnow::combinator::{alt, delimited, opt, repeat, separated};
use winnow::error::ContextError;
use winnow::prelude::*;
use winnow::token::{literal, take_until, take_while};

use crate::schema::{
    AttributeType, Cardinality, EntityType, FunctionType, OwnedAttribute, Parameter, PlayedRole,
    RelationType, ReturnType, ReturnTypeItem, RoleSpec, SchemaError, StructField, StructType,
    TypeSchema,
};

/// Type alias for parser results.
type PResult<T> = winnow::error::Result<T>;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Parse a TypeQL schema string into an unresolved [`TypeSchema`].
///
/// Handles one or more `define` blocks. Functions and structs are parsed into
/// the schema's `functions` and `structs` maps.
pub fn parse_typeql(input: &str) -> Result<TypeSchema, SchemaError> {
    let mut input_ref = input;
    match parse_schema(&mut input_ref) {
        Ok(schema) => Ok(schema),
        Err(_e) => {
            let consumed = input.len() - input_ref.len();
            let parsed_portion = &input[..consumed];
            let line = parsed_portion.chars().filter(|c| *c == '\n').count() + 1;
            let last_newline = parsed_portion.rfind('\n').map(|i| i + 1).unwrap_or(0);
            let column = consumed - last_newline + 1;
            Err(SchemaError::ParseError {
                message: format!("unexpected input at line {}:{}", line, column),
                line,
                column,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level parsers
// ---------------------------------------------------------------------------

fn parse_schema(input: &mut &str) -> PResult<TypeSchema> {
    ws_comments(input);
    let blocks: Vec<Vec<Statement>> = repeat(0.., parse_define_block).parse_next(input)?;
    ws_comments(input);

    if !input.is_empty() {
        return Err(ContextError::new());
    }

    let mut standalone_plays = Vec::new();
    let mut schema = TypeSchema::new();
    for block in blocks {
        for stmt in block {
            match stmt {
                Statement::Attribute(attr) => {
                    schema.attributes.insert(attr.name.clone(), attr);
                }
                Statement::Entity(entity) => {
                    let name = entity.name.clone();
                    if let Some(existing) = schema.entities.get_mut(&name) {
                        // Merge: TypeQL allows split definitions for the same type
                        if entity.parent.is_some() {
                            existing.parent = entity.parent;
                        }
                        if entity.is_abstract {
                            existing.is_abstract = true;
                        }
                        if entity.doc.is_some() {
                            existing.doc = entity.doc;
                        }
                        existing.meta.extend(entity.meta);
                        for own in entity.owns {
                            if !existing.owns.iter().any(|o| o.name == own.name) {
                                existing.owns.push(own);
                            }
                        }
                        for order in entity.owns_order {
                            if !existing.owns_order.contains(&order) {
                                existing.owns_order.push(order);
                            }
                        }
                        for play in entity.plays {
                            if !existing.plays.iter().any(|p| p.role_ref == play.role_ref) {
                                existing.plays.push(play);
                            }
                        }
                    } else {
                        schema.entities.insert(name, entity);
                    }
                }
                Statement::Relation(relation) => {
                    let name = relation.name.clone();
                    if let Some(existing) = schema.relations.get_mut(&name) {
                        // Merge: TypeQL allows split definitions for the same type
                        if relation.parent.is_some() {
                            existing.parent = relation.parent;
                        }
                        if relation.is_abstract {
                            existing.is_abstract = true;
                        }
                        if relation.doc.is_some() {
                            existing.doc = relation.doc;
                        }
                        existing.meta.extend(relation.meta);
                        for own in relation.owns {
                            if !existing.owns.iter().any(|o| o.name == own.name) {
                                existing.owns.push(own);
                            }
                        }
                        for order in relation.owns_order {
                            if !existing.owns_order.contains(&order) {
                                existing.owns_order.push(order);
                            }
                        }
                        for play in relation.plays {
                            if !existing.plays.iter().any(|p| p.role_ref == play.role_ref) {
                                existing.plays.push(play);
                            }
                        }
                        for role in relation.roles {
                            if !existing.roles.iter().any(|r| r.name == role.name) {
                                existing.roles.push(role);
                            }
                        }
                    } else {
                        schema.relations.insert(name, relation);
                    }
                }
                Statement::Function(func) => {
                    schema.functions.insert(func.name.clone(), func);
                }
                Statement::Struct(struct_type) => {
                    schema.structs.insert(struct_type.name.clone(), struct_type);
                }
                Statement::Plays(plays) => {
                    standalone_plays.push(plays);
                }
            }
        }
    }

    for plays in standalone_plays {
        if let Some(entity) = schema.entities.get_mut(&plays.player) {
            if !entity
                .plays
                .iter()
                .any(|p| p.role_ref == plays.played.role_ref)
            {
                entity.plays.push(plays.played);
            }
        } else if let Some(relation) = schema.relations.get_mut(&plays.player) {
            if !relation
                .plays
                .iter()
                .any(|p| p.role_ref == plays.played.role_ref)
            {
                relation.plays.push(plays.played);
            }
        } else {
            // Player type is not explicitly defined yet. Create a shell entity.
            let entity = EntityType {
                name: plays.player.clone(),
                parent: None,
                is_abstract: false,
                owns: Vec::new(),
                owns_order: Vec::new(),
                plays: vec![plays.played],
                doc: None,
                meta: Default::default(),
            };
            schema.entities.insert(plays.player, entity);
        }
    }

    Ok(schema)
}

fn parse_define_block(input: &mut &str) -> PResult<Vec<Statement>> {
    ws_comments(input);
    literal("define").parse_next(input)?;
    ws_comments_required(input)?;
    let stmts: Vec<Option<Statement>> = repeat(1.., parse_statement).parse_next(input)?;
    Ok(stmts.into_iter().flatten().collect())
}

// ---------------------------------------------------------------------------
// Statement dispatch
// ---------------------------------------------------------------------------

struct StandalonePlays {
    player: String,
    played: PlayedRole,
}

enum Statement {
    Attribute(AttributeType),
    Entity(EntityType),
    Relation(RelationType),
    Function(FunctionType),
    Struct(StructType),
    Plays(StandalonePlays),
}

fn parse_statement(input: &mut &str) -> PResult<Option<Statement>> {
    ws_comments(input);
    alt((
        parse_attribute_def.map(|a| Some(Statement::Attribute(a))),
        parse_entity_def.map(|e| Some(Statement::Entity(e))),
        parse_relation_def.map(|r| Some(Statement::Relation(r))),
        parse_function_def.map(|f| Some(Statement::Function(f))),
        parse_struct_def.map(|s| Some(Statement::Struct(s))),
        parse_standalone_plays.map(|p| Some(Statement::Plays(p))),
    ))
    .parse_next(input)
}

fn parse_standalone_plays(input: &mut &str) -> PResult<StandalonePlays> {
    let player = identifier(input)?;
    ws_comments_required(input)?;
    let played = parse_plays_statement(input)?;
    ws_comments(input);
    literal(";").parse_next(input)?;
    Ok(StandalonePlays {
        player: player.to_string(),
        played,
    })
}

// ---------------------------------------------------------------------------
// Attribute definition
// ---------------------------------------------------------------------------

fn parse_attribute_def(input: &mut &str) -> PResult<AttributeType> {
    literal("attribute").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    let mut attr = AttributeType {
        name: name.to_string(),
        value_type: String::new(),
        parent: None,
        is_abstract: false,
        is_independent: false,
        regex: None,
        allowed_values: None,
        range_min: None,
        range_max: None,
        doc: None,
        meta: Default::default(),
    };

    loop {
        ws_comments(input);
        opt_comma(input);
        ws_comments(input);
        if opt(literal(";")).parse_next(input)?.is_some() {
            break;
        }
        parse_attribute_opt(input, &mut attr)?;
    }

    Ok(attr)
}

fn parse_attribute_opt<'a>(input: &mut &'a str, attr: &mut AttributeType) -> PResult<()> {
    alt((
        |i: &mut &'a str| {
            literal("sub").parse_next(i)?;
            ws_comments_required(i)?;
            let parent = identifier(i)?;
            attr.parent = Some(parent.to_string());
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@abstract").parse_next(i)?;
            attr.is_abstract = true;
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@independent").parse_next(i)?;
            attr.is_independent = true;
            Ok(())
        },
        |i: &mut &'a str| {
            let text = parse_doc_annotation(i)?;
            attr.doc = Some(text);
            Ok(())
        },
        |i: &mut &'a str| {
            let (key, value) = parse_meta_annotation(i)?;
            attr.meta.insert(key, value);
            Ok(())
        },
        |i: &mut &'a str| {
            literal("value").parse_next(i)?;
            ws_comments_required(i)?;
            let vt = value_type_name(i)?;
            attr.value_type = vt.to_string();
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@regex").parse_next(i)?;
            ws_comments(i);
            let s = delimited("(", string_literal, ")").parse_next(i)?;
            attr.regex = Some(s);
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@values").parse_next(i)?;
            ws_comments(i);
            let vals: Vec<String> =
                delimited("(", separated(1.., padded(string_literal), ","), ")").parse_next(i)?;
            attr.allowed_values = Some(vals);
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@range").parse_next(i)?;
            ws_comments(i);
            let content: &str = delimited("(", take_until(0.., ")"), ")").parse_next(i)?;
            let content = content.trim();
            if let Some((min_s, max_s)) = content.split_once("..") {
                let min_s = min_s.trim();
                let max_s = max_s.trim();
                if !min_s.is_empty() {
                    attr.range_min = Some(min_s.to_string());
                }
                if !max_s.is_empty() {
                    attr.range_max = Some(max_s.to_string());
                }
            } else {
                // @range without ".." is invalid (e.g. @range(100) or @range(1, 5))
                return Err(ContextError::new());
            }
            Ok(())
        },
    ))
    .parse_next(input)
}

// ---------------------------------------------------------------------------
// Entity definition
// ---------------------------------------------------------------------------

fn parse_entity_def(input: &mut &str) -> PResult<EntityType> {
    literal("entity").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    let mut entity = EntityType {
        name: name.to_string(),
        parent: None,
        is_abstract: false,
        owns: Vec::new(),
        owns_order: Vec::new(),
        plays: Vec::new(),
        doc: None,
        meta: Default::default(),
    };

    loop {
        ws_comments(input);
        opt_comma(input);
        ws_comments(input);
        if opt(literal(";")).parse_next(input)?.is_some() {
            break;
        }
        parse_entity_clause(input, &mut entity)?;
    }

    Ok(entity)
}

fn parse_entity_clause<'a>(input: &mut &'a str, entity: &mut EntityType) -> PResult<()> {
    alt((
        |i: &mut &'a str| {
            literal("sub").parse_next(i)?;
            ws_comments_required(i)?;
            let parent = identifier(i)?;
            entity.parent = Some(parent.to_string());
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@abstract").parse_next(i)?;
            entity.is_abstract = true;
            Ok(())
        },
        |i: &mut &'a str| {
            let text = parse_doc_annotation(i)?;
            entity.doc = Some(text);
            Ok(())
        },
        |i: &mut &'a str| {
            let (key, value) = parse_meta_annotation(i)?;
            entity.meta.insert(key, value);
            Ok(())
        },
        |i: &mut &'a str| {
            let owned = parse_owns_statement(i)?;
            entity.owns_order.push(owned.name.clone());
            entity.owns.push(owned);
            Ok(())
        },
        |i: &mut &'a str| {
            let played = parse_plays_statement(i)?;
            entity.plays.push(played);
            Ok(())
        },
    ))
    .parse_next(input)
}

// ---------------------------------------------------------------------------
// Relation definition
// ---------------------------------------------------------------------------

fn parse_relation_def(input: &mut &str) -> PResult<RelationType> {
    literal("relation").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    let mut relation = RelationType {
        name: name.to_string(),
        parent: None,
        is_abstract: false,
        roles: Vec::new(),
        owns: Vec::new(),
        owns_order: Vec::new(),
        plays: Vec::new(),
        doc: None,
        meta: Default::default(),
    };

    loop {
        ws_comments(input);
        opt_comma(input);
        ws_comments(input);
        if opt(literal(";")).parse_next(input)?.is_some() {
            break;
        }
        parse_relation_clause(input, &mut relation)?;
    }

    Ok(relation)
}

fn parse_relation_clause<'a>(input: &mut &'a str, relation: &mut RelationType) -> PResult<()> {
    alt((
        |i: &mut &'a str| {
            literal("sub").parse_next(i)?;
            ws_comments_required(i)?;
            let parent = identifier(i)?;
            relation.parent = Some(parent.to_string());
            Ok(())
        },
        |i: &mut &'a str| {
            literal("@abstract").parse_next(i)?;
            relation.is_abstract = true;
            Ok(())
        },
        |i: &mut &'a str| {
            let text = parse_doc_annotation(i)?;
            relation.doc = Some(text);
            Ok(())
        },
        |i: &mut &'a str| {
            let (key, value) = parse_meta_annotation(i)?;
            relation.meta.insert(key, value);
            Ok(())
        },
        |i: &mut &'a str| {
            let role = parse_relates_statement(i)?;
            relation.roles.push(role);
            Ok(())
        },
        |i: &mut &'a str| {
            let owned = parse_owns_statement(i)?;
            relation.owns_order.push(owned.name.clone());
            relation.owns.push(owned);
            Ok(())
        },
        |i: &mut &'a str| {
            let played = parse_plays_statement(i)?;
            relation.plays.push(played);
            Ok(())
        },
    ))
    .parse_next(input)
}

// ---------------------------------------------------------------------------
// Shared statement parsers: owns, plays, relates
// ---------------------------------------------------------------------------

fn parse_owns_statement(input: &mut &str) -> PResult<OwnedAttribute> {
    literal("owns").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    // Parse the optional ordered-ownership marker `[]` immediately after the name.
    // TypeDB only accepts `@distinct` on ordered ownerships (`owns attr[]`); the
    // bare form `owns attr @distinct` is rejected by all server versions.
    let ordered = opt(literal("[]")).parse_next(input)?.is_some();

    let mut owned = OwnedAttribute {
        name: name.to_string(),
        is_key: false,
        is_unique: false,
        is_cascade: false,
        subkey_group: None,
        cardinality: None,
        ordered,
        distinct: false,
        doc: None,
        meta: Default::default(),
    };

    loop {
        ws_comments(input);
        let parsed = opt(alt((
            |i: &mut &str| {
                literal("@key").parse_next(i)?;
                owned.is_key = true;
                Ok(())
            },
            |i: &mut &str| {
                literal("@unique").parse_next(i)?;
                owned.is_unique = true;
                Ok(())
            },
            |i: &mut &str| {
                literal("@cascade").parse_next(i)?;
                owned.is_cascade = true;
                Ok(())
            },
            |i: &mut &str| {
                literal("@subkey").parse_next(i)?;
                ws_comments(i);
                let group: &str = delimited("(", padded(identifier), ")").parse_next(i)?;
                owned.subkey_group = Some(group.to_string());
                Ok(())
            },
            |i: &mut &str| {
                literal("@distinct").parse_next(i)?;
                owned.distinct = true;
                Ok(())
            },
            |i: &mut &str| {
                let card = parse_card_annotation(i)?;
                owned.cardinality = Some(card);
                Ok(())
            },
            |i: &mut &str| {
                let text = parse_doc_annotation(i)?;
                owned.doc = Some(text);
                Ok(())
            },
            |i: &mut &str| {
                let (key, value) = parse_meta_annotation(i)?;
                owned.meta.insert(key, value);
                Ok(())
            },
        )))
        .parse_next(input)?;
        if parsed.is_none() {
            break;
        }
    }

    Ok(owned)
}

fn parse_plays_statement(input: &mut &str) -> PResult<PlayedRole> {
    literal("plays").parse_next(input)?;
    ws_comments_required(input)?;
    let relation = identifier(input)?;
    ws_comments(input);
    literal(":").parse_next(input)?;
    ws_comments(input);
    let role = identifier(input)?;

    let role_ref = format!("{}:{}", relation, role);

    let mut played = PlayedRole {
        role_ref,
        cardinality: None,
        doc: None,
        meta: BTreeMap::new(),
    };

    // Parse optional annotations: @card(...), @doc(...), @meta(...) in any order.
    loop {
        ws_comments(input);
        let parsed = opt(alt((
            |i: &mut &str| {
                let card = parse_card_annotation(i)?;
                played.cardinality = Some(card);
                Ok(())
            },
            |i: &mut &str| {
                let text = parse_doc_annotation(i)?;
                played.doc = Some(text);
                Ok(())
            },
            |i: &mut &str| {
                let (key, value) = parse_meta_annotation(i)?;
                played.meta.insert(key, value);
                Ok(())
            },
        )))
        .parse_next(input)?;
        if parsed.is_none() {
            break;
        }
    }

    Ok(played)
}

fn parse_relates_statement(input: &mut &str) -> PResult<RoleSpec> {
    literal("relates").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?;

    // Parse the optional ordered-role marker `[]` immediately after the name.
    // TypeDB only accepts `@distinct` on ordered roles (e.g. `relates member[] @distinct`);
    // the bare form `relates member @distinct` is rejected by all server versions.
    let ordered = opt(literal("[]")).parse_next(input)?.is_some();

    let mut role = RoleSpec {
        name: name.to_string(),
        overrides: None,
        cardinality: None,
        distinct: false,
        ordered,
        is_abstract: false,
        doc: None,
        meta: Default::default(),
    };

    // Parse optional "as <parent_role>" — the override comes after the `[]` marker.
    ws_comments(input);
    if opt(literal("as")).parse_next(input)?.is_some() {
        ws_comments_required(input)?;
        let parent_role = identifier(input)?;
        role.overrides = Some(parent_role.to_string());
    }

    // Parse optional annotations: @abstract, @card(...), @distinct in any order.
    loop {
        ws_comments(input);
        let parsed = opt(alt((
            |i: &mut &str| {
                literal("@abstract").parse_next(i)?;
                role.is_abstract = true;
                Ok(())
            },
            |i: &mut &str| {
                literal("@distinct").parse_next(i)?;
                role.distinct = true;
                Ok(())
            },
            |i: &mut &str| {
                let card = parse_card_annotation(i)?;
                role.cardinality = Some(card);
                Ok(())
            },
            |i: &mut &str| {
                let text = parse_doc_annotation(i)?;
                role.doc = Some(text);
                Ok(())
            },
            |i: &mut &str| {
                let (key, value) = parse_meta_annotation(i)?;
                role.meta.insert(key, value);
                Ok(())
            },
        )))
        .parse_next(input)?;
        if parsed.is_none() {
            break;
        }
    }

    Ok(role)
}

// ---------------------------------------------------------------------------
// Annotation parsers
// ---------------------------------------------------------------------------

/// Parse a `@doc("...")` annotation (TypeDB 3.12+), returning the doc text.
fn parse_doc_annotation(input: &mut &str) -> PResult<String> {
    literal("@doc").parse_next(input)?;
    ws_comments(input);
    literal("(").parse_next(input)?;
    let text = string_literal(input)?;
    ws_comments(input);
    literal(")").parse_next(input)?;
    Ok(text)
}

/// Parse a `@meta("key", "value")` annotation (TypeDB 3.12+), returning the pair.
///
/// TypeDB requires both key and value to be quoted string literals.
fn parse_meta_annotation(input: &mut &str) -> PResult<(String, String)> {
    literal("@meta").parse_next(input)?;
    ws_comments(input);
    literal("(").parse_next(input)?;
    let key = string_literal(input)?;
    ws_comments(input);
    literal(",").parse_next(input)?;
    let value = string_literal(input)?;
    ws_comments(input);
    literal(")").parse_next(input)?;
    Ok((key, value))
}

fn parse_card_annotation(input: &mut &str) -> PResult<Cardinality> {
    literal("@card").parse_next(input)?;
    ws_comments(input);
    literal("(").parse_next(input)?;
    ws_comments(input);
    let min = parse_u32(input)?;
    ws_comments(input);

    let has_dotdot = opt(literal("..")).parse_next(input)?;
    if has_dotdot.is_some() {
        ws_comments(input);
        let max = opt(parse_u32).parse_next(input)?;
        ws_comments(input);
        literal(")").parse_next(input)?;
        Ok(Cardinality { min, max })
    } else {
        ws_comments(input);
        literal(")").parse_next(input)?;
        Ok(Cardinality {
            min,
            max: Some(min),
        })
    }
}

// ---------------------------------------------------------------------------
// Function / Struct definition parsers
// ---------------------------------------------------------------------------

/// Remove `fun` and `struct` definitions from released schema text,
/// keeping everything else byte-for-byte.
///
/// Definition extents are consumed by the same grammar the schema parser
/// applies, so released spellings (dummy `return { 1 };` bodies, the
/// `struct name, value field type;` form) strip exactly; a token that does
/// not parse as its definition is kept untouched.
pub fn strip_function_definitions(source: &str) -> String {
    let mut output = String::new();
    let mut rest = source;
    while let Some((position, keyword)) = find_definition_token(rest) {
        let mut cursor = &rest[position..];
        let consumed = match keyword {
            "fun" => parse_function_def(&mut cursor).is_ok(),
            _ => parse_struct_def(&mut cursor).is_ok(),
        };
        if consumed {
            output.push_str(&rest[..position]);
            rest = cursor;
        } else {
            output.push_str(&rest[..position + keyword.len()]);
            rest = &rest[position + keyword.len()..];
        }
    }
    output.push_str(rest);
    output
}

fn find_definition_token(source: &str) -> Option<(usize, &'static str)> {
    let fun = find_keyword(source, "fun");
    let structure = find_keyword(source, "struct");
    match (fun, structure) {
        (Some(fun), Some(structure)) if fun <= structure => Some((fun, "fun")),
        (Some(_) | None, Some(structure)) => Some((structure, "struct")),
        (Some(fun), None) => Some((fun, "fun")),
        (None, None) => None,
    }
}

fn find_keyword(source: &str, keyword: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut start = 0;
    while let Some(found) = source[start..].find(keyword) {
        let position = start + found;
        let before_ok = position == 0
            || !(bytes[position - 1].is_ascii_alphanumeric()
                || bytes[position - 1] == b'_'
                || bytes[position - 1] == b'-');
        let after = position + keyword.len();
        let after_ok = after >= bytes.len()
            || bytes[after].is_ascii_whitespace();
        if before_ok && after_ok {
            return Some(position);
        }
        start = position + keyword.len();
    }
    None
}

fn parse_function_def(input: &mut &str) -> PResult<FunctionType> {
    literal("fun").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?.to_string();
    ws_comments(input);
    literal("(").parse_next(input)?;
    let parameters = parse_param_list(input)?;
    ws_comments(input);
    literal(")").parse_next(input)?;
    ws_comments(input);
    literal("->").parse_next(input)?;
    let return_type = parse_return_type_clause(input)?;
    ws_comments(input);
    literal(":").parse_next(input)?;
    skip_function_body(input)?;
    Ok(FunctionType {
        name,
        parameters,
        return_type,
    })
}

fn parse_param_list(input: &mut &str) -> PResult<Vec<Parameter>> {
    let mut params = Vec::new();
    ws_comments(input);
    if input.starts_with(')') {
        return Ok(params); // empty param list `()`
    }
    params.push(parse_param(input)?);
    loop {
        ws_comments(input);
        if opt(literal::<_, _, ContextError>(","))
            .parse_next(input)?
            .is_some()
        {
            params.push(parse_param(input)?);
        } else {
            break;
        }
    }
    Ok(params)
}

fn parse_param(input: &mut &str) -> PResult<Parameter> {
    ws_comments(input);
    literal("$").parse_next(input)?;
    let name = identifier(input)?.to_string();
    ws_comments(input);
    literal(":").parse_next(input)?;
    ws_comments(input);
    // VALUE_TYPE_NAME or IDENTIFIER — both match the identifier pattern.
    let type_ = identifier(input)?.to_string();
    Ok(Parameter { name, type_ })
}

/// Build the STRUCTURED return type. `-> { a, b }` → is_stream=true,
/// types=["a","b"]; `-> a, b` → is_stream=false, types=["a","b"].
/// No string formatting — the renderer decides presentation.
fn parse_return_type_clause(input: &mut &str) -> PResult<ReturnType> {
    ws_comments(input);
    if input.starts_with('{') {
        literal("{").parse_next(input)?;
        let types = parse_return_type_list(input)?;
        ws_comments(input);
        literal("}").parse_next(input)?;
        Ok(ReturnType {
            is_stream: true,
            types,
        })
    } else {
        let types = parse_return_type_list(input)?;
        Ok(ReturnType {
            is_stream: false,
            types,
        })
    }
}

fn parse_return_type_list(input: &mut &str) -> PResult<Vec<ReturnTypeItem>> {
    let mut types = vec![parse_return_type(input)?];
    loop {
        ws_comments(input);
        if input.starts_with(',') {
            literal(",").parse_next(input)?;
            types.push(parse_return_type(input)?);
        } else {
            break;
        }
    }
    Ok(types)
}

/// One return-type item: identifier + structural optionality.
/// Optionality is a bool, NOT a `?`-suffix — the IR stays language-neutral.
fn parse_return_type(input: &mut &str) -> PResult<ReturnTypeItem> {
    ws_comments(input);
    let name = identifier(input)?.to_string();
    ws_comments(input);
    let optional = opt(literal::<_, _, ContextError>("?"))
        .parse_next(input)?
        .is_some();
    Ok(ReturnTypeItem { name, optional })
}

/// Skip the function body. A body runs from the signature to the first
/// `return` clause and its terminating `;` — match that pair and discard it.
fn skip_function_body(input: &mut &str) -> PResult<()> {
    if let Some(pos) = input.find("return") {
        *input = &input[pos + 6..];
        if let Some(semi) = input.find(';') {
            *input = &input[semi + 1..];
            return Ok(());
        }
    }
    Err(ContextError::new())
}

fn parse_struct_def(input: &mut &str) -> PResult<StructType> {
    literal("struct").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?.to_string();
    ws_comments(input);
    opt_comma(input); // optional comma after the struct name
    let mut fields = vec![parse_struct_field(input)?];
    loop {
        ws_comments(input);
        opt_comma(input);
        ws_comments(input);
        match opt(parse_struct_field).parse_next(input)? {
            Some(f) => fields.push(f),
            None => break,
        }
    }
    ws_comments(input);
    literal(";").parse_next(input)?;
    Ok(StructType { name, fields })
}

fn parse_struct_field(input: &mut &str) -> PResult<StructField> {
    ws_comments(input);
    literal("value").parse_next(input)?;
    ws_comments_required(input)?;
    let name = identifier(input)?.to_string();
    ws_comments_required(input)?;
    let value_type = value_type_name(input)?.to_string();
    ws_comments(input);
    let optional = opt(literal::<_, _, ContextError>("?"))
        .parse_next(input)?
        .is_some();
    Ok(StructField {
        name,
        value_type,
        optional,
    })
}

// ---------------------------------------------------------------------------
// Token-level parsers
// ---------------------------------------------------------------------------

/// Parse an identifier: `[a-zA-Z_][a-zA-Z0-9_-]*`
fn identifier<'a>(input: &mut &'a str) -> PResult<&'a str> {
    let ident: &str = take_while(1.., |c: char| {
        c.is_ascii_alphanumeric() || c == '_' || c == '-'
    })
    .parse_next(input)?;

    let first = ident.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(ContextError::new());
    }

    Ok(ident)
}

/// Parse a value type name keyword.
fn value_type_name<'a>(input: &mut &'a str) -> PResult<&'a str> {
    alt((
        literal("datetime-tz"),
        literal("datetime"),
        literal("boolean"),
        literal("decimal"),
        literal("duration"),
        literal("integer"),
        literal("string"),
        literal("double"),
        literal("date"),
        literal("long"),
        literal("bool"),
        literal("int"),
    ))
    .parse_next(input)
}

/// Parse a quoted string literal (double or single quotes).
fn string_literal(input: &mut &str) -> PResult<String> {
    ws_comments(input);
    let quote: &str = alt((literal("\""), literal("'"))).parse_next(input)?;
    let close = if quote == "\"" { "\"" } else { "'" };

    let mut result = String::new();
    loop {
        if input.is_empty() {
            return Err(ContextError::new());
        }
        if input.starts_with(close) {
            *input = &input[1..];
            break;
        }
        if input.starts_with('\\') {
            *input = &input[1..];
            if input.is_empty() {
                return Err(ContextError::new());
            }
            let ch = input.chars().next().unwrap();
            match ch {
                'n' => result.push('\n'),
                't' => result.push('\t'),
                'r' => result.push('\r'),
                '\\' => result.push('\\'),
                '"' => result.push('"'),
                '\'' => result.push('\''),
                other => {
                    // Preserve unknown escape sequences (e.g. \. in regex)
                    result.push('\\');
                    result.push(other);
                }
            };
            *input = &input[ch.len_utf8()..];
        } else {
            let ch = input.chars().next().unwrap();
            result.push(ch);
            *input = &input[ch.len_utf8()..];
        }
    }
    Ok(result)
}

/// Parse an unsigned 32-bit integer.
fn parse_u32(input: &mut &str) -> PResult<u32> {
    let digits: &str = take_while(1.., |c: char| c.is_ascii_digit()).parse_next(input)?;
    digits.parse::<u32>().map_err(|_| ContextError::new())
}

// ---------------------------------------------------------------------------
// Whitespace / comment helpers
// ---------------------------------------------------------------------------

/// Skip whitespace and comments (# ... and // ...).
fn ws_comments(input: &mut &str) {
    loop {
        let before = *input;
        let _: &str = take_while::<_, _, ContextError>(0.., |c: char| c.is_ascii_whitespace())
            .parse_next(input)
            .unwrap();

        if input.starts_with('#') || input.starts_with("//") {
            if let Some(pos) = input.find('\n') {
                *input = &input[pos + 1..];
            } else {
                *input = &input[input.len()..];
            }
            continue;
        }

        if input.starts_with("/*")
            && let Some(pos) = input.find("*/")
        {
            *input = &input[pos + 2..];
            continue;
        }

        if *input == before {
            break;
        }
    }
}

/// Require at least one whitespace character or comment.
fn ws_comments_required(input: &mut &str) -> PResult<()> {
    let before = *input;
    ws_comments(input);
    if *input == before {
        return Err(ContextError::new());
    }
    Ok(())
}

/// Skip an optional comma.
fn opt_comma(input: &mut &str) {
    let _ = opt(literal::<_, _, ContextError>(",")).parse_next(input);
}

/// Wrap a parser with surrounding whitespace/comment skipping.
fn padded<'a, O, P>(mut parser: P) -> impl FnMut(&mut &'a str) -> PResult<O>
where
    P: Parser<&'a str, O, ContextError>,
{
    move |input: &mut &'a str| {
        ws_comments(input);
        let result = parser.parse_next(input)?;
        ws_comments(input);
        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identifier_simple() {
        let mut input = "person";
        let result = identifier(&mut input).unwrap();
        assert_eq!(result, "person");
        assert_eq!(input, "");
    }

    #[test]
    fn test_identifier_with_hyphen() {
        let mut input = "isbn-13 rest";
        let result = identifier(&mut input).unwrap();
        assert_eq!(result, "isbn-13");
        assert_eq!(input, " rest");
    }

    #[test]
    fn test_identifier_with_underscore() {
        let mut input = "_private_name";
        let result = identifier(&mut input).unwrap();
        assert_eq!(result, "_private_name");
    }

    #[test]
    fn test_value_type_name() {
        for vt in &[
            "string",
            "integer",
            "double",
            "boolean",
            "date",
            "datetime",
            "datetime-tz",
            "decimal",
            "duration",
            "long",
            "bool",
            "int",
        ] {
            let mut input = *vt;
            let result = value_type_name(&mut input).unwrap();
            assert_eq!(result, *vt);
        }
    }

    #[test]
    fn test_string_literal_double_quotes() {
        let mut input = r#""hello world""#;
        let result = string_literal(&mut input).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_string_literal_single_quotes() {
        let mut input = "'hello'";
        let result = string_literal(&mut input).unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_string_literal_with_escapes() {
        let mut input = r#""line1\nline2""#;
        let result = string_literal(&mut input).unwrap();
        assert_eq!(result, "line1\nline2");
    }

    #[test]
    fn test_parse_u32() {
        let mut input = "42 rest";
        let result = parse_u32(&mut input).unwrap();
        assert_eq!(result, 42);
        assert_eq!(input, " rest");
    }

    #[test]
    fn test_card_annotation_bounded() {
        let mut input = "@card(1..3)";
        let card = parse_card_annotation(&mut input).unwrap();
        assert_eq!(
            card,
            Cardinality {
                min: 1,
                max: Some(3)
            }
        );
    }

    #[test]
    fn test_card_annotation_unbounded() {
        let mut input = "@card(1..)";
        let card = parse_card_annotation(&mut input).unwrap();
        assert_eq!(card, Cardinality { min: 1, max: None });
    }

    #[test]
    fn test_card_annotation_exact() {
        let mut input = "@card(5)";
        let card = parse_card_annotation(&mut input).unwrap();
        assert_eq!(
            card,
            Cardinality {
                min: 5,
                max: Some(5)
            }
        );
    }

    #[test]
    fn test_card_annotation_optional() {
        let mut input = "@card(0..1)";
        let card = parse_card_annotation(&mut input).unwrap();
        assert_eq!(
            card,
            Cardinality {
                min: 0,
                max: Some(1)
            }
        );
    }

    // --- Attribute definition tests ---

    #[test]
    fn test_parse_simple_attribute() {
        let schema = parse_typeql("define\nattribute name, value string;").unwrap();
        assert_eq!(schema.attributes.len(), 1);
        let attr = schema.attributes.get("name").unwrap();
        assert_eq!(attr.value_type, "string");
        assert!(!attr.is_abstract);
    }

    #[test]
    fn test_parse_attribute_all_value_types() {
        for vt in &[
            "string", "integer", "double", "boolean", "date", "datetime", "decimal", "duration",
        ] {
            let input = format!("define\nattribute test-attr, value {};", vt);
            let schema = parse_typeql(&input).unwrap();
            let attr = schema.attributes.get("test-attr").unwrap();
            assert_eq!(attr.value_type, *vt);
        }
    }

    #[test]
    fn test_parse_attribute_with_regex() {
        let schema = parse_typeql(
            "define\nattribute email, value string, @regex(\"^[a-z]+@[a-z]+\\.[a-z]+$\");",
        )
        .unwrap();
        let attr = schema.attributes.get("email").unwrap();
        assert_eq!(attr.regex.as_deref(), Some("^[a-z]+@[a-z]+\\.[a-z]+$"));
    }

    #[test]
    fn test_parse_attribute_with_values() {
        let schema = parse_typeql(
            "define\nattribute status, value string, @values(\"active\", \"inactive\", \"pending\");",
        )
        .unwrap();
        let attr = schema.attributes.get("status").unwrap();
        assert_eq!(
            attr.allowed_values,
            Some(vec!["active".into(), "inactive".into(), "pending".into()])
        );
    }

    #[test]
    fn test_parse_attribute_with_range() {
        let schema = parse_typeql("define\nattribute age, value integer, @range(0..150);").unwrap();
        let attr = schema.attributes.get("age").unwrap();
        assert_eq!(attr.range_min.as_deref(), Some("0"));
        assert_eq!(attr.range_max.as_deref(), Some("150"));
    }

    #[test]
    fn test_parse_attribute_abstract() {
        let schema = parse_typeql("define\nattribute content @abstract;").unwrap();
        assert!(schema.attributes.get("content").unwrap().is_abstract);
    }

    #[test]
    fn test_parse_attribute_independent() {
        let schema = parse_typeql("define\nattribute tag, value string, @independent;").unwrap();
        assert!(schema.attributes.get("tag").unwrap().is_independent);
    }

    #[test]
    fn test_parse_attribute_inheritance() {
        let schema = parse_typeql("define\nattribute isbn sub content, value string;").unwrap();
        let attr = schema.attributes.get("isbn").unwrap();
        assert_eq!(attr.parent.as_deref(), Some("content"));
        assert_eq!(attr.value_type, "string");
    }

    // --- Entity definition tests ---

    #[test]
    fn test_parse_simple_entity() {
        let schema = parse_typeql("define\nentity person;").unwrap();
        let entity = schema.entities.get("person").unwrap();
        assert_eq!(entity.name, "person");
        assert!(!entity.is_abstract);
    }

    #[test]
    fn test_parse_entity_with_owns() {
        let schema = parse_typeql("define\nentity person, owns name, owns age;").unwrap();
        let entity = schema.entities.get("person").unwrap();
        assert_eq!(entity.owns.len(), 2);
        assert_eq!(entity.owns_order, vec!["name", "age"]);
    }

    #[test]
    fn test_parse_entity_with_key() {
        let schema = parse_typeql("define\nentity person, owns name @key;").unwrap();
        assert!(schema.entities.get("person").unwrap().owns[0].is_key);
    }

    #[test]
    fn test_parse_entity_with_unique() {
        let schema = parse_typeql("define\nentity person, owns email @unique;").unwrap();
        assert!(schema.entities.get("person").unwrap().owns[0].is_unique);
    }

    #[test]
    fn test_parse_entity_with_cascade() {
        let schema = parse_typeql("define\nentity person, owns name @cascade;").unwrap();
        assert!(schema.entities.get("person").unwrap().owns[0].is_cascade);
    }

    #[test]
    fn test_parse_entity_with_subkey() {
        let schema = parse_typeql("define\nentity person, owns name @subkey(name_group);").unwrap();
        assert_eq!(
            schema.entities.get("person").unwrap().owns[0]
                .subkey_group
                .as_deref(),
            Some("name_group")
        );
    }

    #[test]
    fn test_parse_entity_with_card() {
        let schema = parse_typeql("define\nentity person, owns nickname @card(0..3);").unwrap();
        assert_eq!(
            schema.entities.get("person").unwrap().owns[0].cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(3)
            })
        );
    }

    #[test]
    fn test_parse_entity_with_plays() {
        let schema = parse_typeql("define\nentity person, plays friendship:friend;").unwrap();
        let entity = schema.entities.get("person").unwrap();
        assert_eq!(entity.plays[0].role_ref, "friendship:friend");
    }

    #[test]
    fn test_parse_entity_with_plays_cardinality() {
        let schema =
            parse_typeql("define\nentity person, plays friendship:friend @card(0..5);").unwrap();
        assert_eq!(
            schema.entities.get("person").unwrap().plays[0].cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(5)
            })
        );
    }

    #[test]
    fn test_parse_entity_abstract() {
        let schema = parse_typeql("define\nentity animal @abstract;").unwrap();
        assert!(schema.entities.get("animal").unwrap().is_abstract);
    }

    #[test]
    fn test_parse_entity_inheritance() {
        let schema = parse_typeql("define\nentity employee sub person;").unwrap();
        assert_eq!(
            schema.entities.get("employee").unwrap().parent.as_deref(),
            Some("person")
        );
    }

    // --- Relation definition tests ---

    #[test]
    fn test_parse_simple_relation() {
        let schema = parse_typeql("define\nrelation friendship, relates friend;").unwrap();
        let rel = schema.relations.get("friendship").unwrap();
        assert_eq!(rel.roles.len(), 1);
        assert_eq!(rel.roles[0].name, "friend");
    }

    #[test]
    fn test_parse_relation_with_multiple_roles() {
        let schema =
            parse_typeql("define\nrelation employment, relates employer, relates employee;")
                .unwrap();
        assert_eq!(schema.relations.get("employment").unwrap().roles.len(), 2);
    }

    #[test]
    fn test_parse_relation_role_override() {
        let schema = parse_typeql(
            "define\nrelation authoring sub contribution, relates author as contributor;",
        )
        .unwrap();
        let rel = schema.relations.get("authoring").unwrap();
        assert_eq!(rel.roles[0].name, "author");
        assert_eq!(rel.roles[0].overrides.as_deref(), Some("contributor"));
    }

    // Bare `relates member @distinct` (no `[]`) must be rejected: TypeDB servers
    // refuse this form with "Invalid ordering ''" (SVL21) on every known version.
    // Validation runs inside `from_typeql` (which calls `validate()` after parsing).
    #[test]
    fn test_parse_relation_role_distinct_bare_rejected() {
        let err = TypeSchema::from_typeql("define\nrelation team, relates member @distinct;")
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("@distinct"),
            "expected teaching message mentioning @distinct, got: {msg}"
        );
        assert!(
            msg.contains("[]"),
            "expected teaching message mentioning ordered-role syntax `[]`, got: {msg}"
        );
    }

    // `relates member[] @distinct` — the only form TypeDB accepts.
    #[test]
    fn test_parse_relation_role_ordered_distinct() {
        let schema = parse_typeql("define\nrelation team, relates member[] @distinct;").unwrap();
        let role = &schema.relations.get("team").unwrap().roles[0];
        assert!(role.ordered, "role should be marked ordered");
        assert!(role.distinct, "role should be marked distinct");
    }

    // Ordered without @distinct — valid.
    #[test]
    fn test_parse_relation_role_ordered_no_distinct() {
        let schema = parse_typeql("define\nrelation queue, relates item[];").unwrap();
        let role = &schema.relations.get("queue").unwrap().roles[0];
        assert!(role.ordered, "role should be marked ordered");
        assert!(!role.distinct, "role should not be marked distinct");
    }

    // Plain bare role — ordered and distinct both false.
    #[test]
    fn test_parse_relation_role_bare() {
        let schema = parse_typeql("define\nrelation friendship, relates friend;").unwrap();
        let role = &schema.relations.get("friendship").unwrap().roles[0];
        assert!(!role.ordered, "plain role should not be ordered");
        assert!(!role.distinct, "plain role should not be distinct");
    }

    // Ordered role with `as <parent>` override.
    #[test]
    fn test_parse_relation_role_ordered_with_override() {
        let schema = parse_typeql(
            "define\nrelation authoring sub contribution, relates author[] as contributor;",
        )
        .unwrap();
        let role = &schema.relations.get("authoring").unwrap().roles[0];
        assert_eq!(role.name, "author");
        assert!(role.ordered, "role should be ordered");
        assert_eq!(role.overrides.as_deref(), Some("contributor"));
    }

    // Ordered role with both @card and @distinct.
    #[test]
    fn test_parse_relation_role_ordered_card_and_distinct() {
        let schema =
            parse_typeql("define\nrelation team, relates member[] @card(0..) @distinct;").unwrap();
        let role = &schema.relations.get("team").unwrap().roles[0];
        assert!(role.ordered);
        assert!(role.distinct);
        assert_eq!(role.cardinality, Some(Cardinality { min: 0, max: None }));
    }

    #[test]
    fn test_parse_relation_role_cardinality() {
        let schema =
            parse_typeql("define\nrelation friendship, relates friend @card(2..2);").unwrap();
        assert_eq!(
            schema.relations.get("friendship").unwrap().roles[0].cardinality,
            Some(Cardinality {
                min: 2,
                max: Some(2)
            })
        );
    }

    // --- @abstract role annotation tests ---

    // `relates participant @abstract` on an @abstract relation.
    #[test]
    fn test_parse_role_abstract_on_abstract_relation() {
        let schema =
            parse_typeql("define\nrelation interaction @abstract, relates participant @abstract;")
                .unwrap();
        let role = &schema.relations.get("interaction").unwrap().roles[0];
        assert!(role.is_abstract, "role should be abstract");
        assert_eq!(role.name, "participant");
    }

    // Override + @abstract + @card in any order.
    #[test]
    fn test_parse_role_abstract_with_override_and_card() {
        let schema = parse_typeql(
            "define\nrelation task sub work, relates worker as assignee @abstract @card(0..1);",
        )
        .unwrap();
        let role = &schema.relations.get("task").unwrap().roles[0];
        assert_eq!(role.overrides.as_deref(), Some("assignee"));
        assert!(role.is_abstract, "role should be abstract");
        assert_eq!(
            role.cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(1)
            })
        );
    }

    // @card before @abstract — order must not matter.
    #[test]
    fn test_parse_role_abstract_card_then_abstract() {
        let schema =
            parse_typeql("define\nrelation work, relates assignee @card(0..2) @abstract;").unwrap();
        let role = &schema.relations.get("work").unwrap().roles[0];
        assert!(role.is_abstract, "role should be abstract");
        assert_eq!(
            role.cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(2)
            })
        );
    }

    // Plain role without @abstract — is_abstract defaults to false.
    #[test]
    fn test_parse_role_not_abstract_by_default() {
        let schema = parse_typeql("define\nrelation friendship, relates plain;").unwrap();
        let role = &schema.relations.get("friendship").unwrap().roles[0];
        assert!(!role.is_abstract, "plain role should not be abstract");
    }

    // Serde round-trip: JSON without the `is_abstract` field deserializes with false.
    #[test]
    fn test_role_spec_serde_default_is_abstract() {
        let json = r#"{
            "name": "participant",
            "overrides": null,
            "cardinality": null,
            "distinct": false,
            "ordered": false
        }"#;
        let role: RoleSpec = serde_json::from_str(json).unwrap();
        assert!(
            !role.is_abstract,
            "missing is_abstract field should default to false"
        );
    }

    // --- owns [] (ordered ownership) annotation tests ---

    // `owns nickname[]` — ordered true, distinct false.
    #[test]
    fn test_parse_owns_ordered_no_distinct() {
        let schema = parse_typeql(
            "define\nentity person, owns nickname[];\nattribute nickname, value string;",
        )
        .unwrap();
        let own = &schema.entities.get("person").unwrap().owns[0];
        assert!(own.ordered, "ownership should be marked ordered");
        assert!(!own.distinct, "ownership should not be marked distinct");
    }

    // `owns nickname[] @distinct` — both true.
    #[test]
    fn test_parse_owns_ordered_and_distinct() {
        let schema = parse_typeql(
            "define\nentity person, owns nickname[] @distinct;\nattribute nickname, value string;",
        )
        .unwrap();
        let own = &schema.entities.get("person").unwrap().owns[0];
        assert!(own.ordered, "ownership should be marked ordered");
        assert!(own.distinct, "ownership should be marked distinct");
    }

    // `owns nickname[] @card(0..5) @distinct` — annotations in any order.
    #[test]
    fn test_parse_owns_ordered_card_then_distinct() {
        let schema = parse_typeql(
            "define\nentity person, owns nickname[] @card(0..5) @distinct;\nattribute nickname, value string;",
        )
        .unwrap();
        let own = &schema.entities.get("person").unwrap().owns[0];
        assert!(own.ordered);
        assert!(own.distinct);
        assert_eq!(
            own.cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(5)
            })
        );
    }

    // `owns nickname[] @distinct @card(0..5)` — reversed annotation order.
    #[test]
    fn test_parse_owns_ordered_distinct_then_card() {
        let schema = parse_typeql(
            "define\nentity person, owns nickname[] @distinct @card(0..5);\nattribute nickname, value string;",
        )
        .unwrap();
        let own = &schema.entities.get("person").unwrap().owns[0];
        assert!(own.ordered);
        assert!(own.distinct);
        assert_eq!(
            own.cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(5)
            })
        );
    }

    // `owns pid[] @key` — ordered plus is_key.
    #[test]
    fn test_parse_owns_ordered_key() {
        let schema =
            parse_typeql("define\nentity person, owns pid[] @key;\nattribute pid, value string;")
                .unwrap();
        let own = &schema.entities.get("person").unwrap().owns[0];
        assert!(own.ordered, "ownership should be marked ordered");
        assert!(own.is_key, "ownership should be a key");
    }

    // Plain `owns x` — ordered and distinct both false.
    #[test]
    fn test_parse_owns_plain_both_false() {
        let schema =
            parse_typeql("define\nentity person, owns name;\nattribute name, value string;")
                .unwrap();
        let own = &schema.entities.get("person").unwrap().owns[0];
        assert!(!own.ordered, "plain ownership should not be ordered");
        assert!(!own.distinct, "plain ownership should not be distinct");
    }

    // Bare `owns x @distinct` (no `[]`) must be rejected by validation.
    // `from_typeql` runs validate() after parsing; the error message must mention
    // @distinct and the ordered `[]` syntax.
    #[test]
    fn test_parse_owns_distinct_bare_rejected() {
        let err = TypeSchema::from_typeql(
            "define\nentity person, owns name @distinct;\nattribute name, value string;",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("@distinct"),
            "expected message mentioning @distinct, got: {msg}"
        );
        assert!(
            msg.contains("[]"),
            "expected message mentioning ordered-ownership syntax `[]`, got: {msg}"
        );
    }

    // Serde round-trip: JSON without `ordered`/`distinct` deserializes to both false.
    #[test]
    fn test_owned_attribute_serde_default_ordered_distinct() {
        let json = r#"{
            "name": "nickname",
            "is_key": false,
            "is_unique": false,
            "is_cascade": false,
            "subkey_group": null,
            "cardinality": null
        }"#;
        let own: OwnedAttribute = serde_json::from_str(json).unwrap();
        assert!(
            !own.ordered,
            "missing ordered field should default to false"
        );
        assert!(
            !own.distinct,
            "missing distinct field should default to false"
        );
    }

    #[test]
    fn test_parse_relation_with_owns() {
        let schema = parse_typeql(
            "define\nrelation employment, relates employer, relates employee, owns start-date;",
        )
        .unwrap();
        let rel = schema.relations.get("employment").unwrap();
        assert_eq!(rel.owns[0].name, "start-date");
    }

    #[test]
    fn test_parse_relation_abstract() {
        let schema = parse_typeql(
            "define\nrelation contribution @abstract, relates contributor, relates work;",
        )
        .unwrap();
        assert!(schema.relations.get("contribution").unwrap().is_abstract);
    }

    #[test]
    fn test_parse_relation_inheritance() {
        let schema = parse_typeql("define\nrelation authoring sub contribution;").unwrap();
        assert_eq!(
            schema.relations.get("authoring").unwrap().parent.as_deref(),
            Some("contribution")
        );
    }

    // --- Comments, whitespace, optional commas ---

    #[test]
    fn test_comments_ignored() {
        let schema = parse_typeql(
            "# comment\ndefine\n# another\nattribute name, value string;\n// cpp\nentity person, owns name;\n",
        ).unwrap();
        assert_eq!(schema.attributes.len(), 1);
        assert_eq!(schema.entities.len(), 1);
    }

    #[test]
    fn test_block_comments_ignored() {
        let schema = parse_typeql("define\n/* block */\nattribute name, value string;").unwrap();
        assert_eq!(schema.attributes.len(), 1);
    }

    #[test]
    fn test_optional_commas() {
        let s1 = parse_typeql("define\nentity person owns name owns age;").unwrap();
        let s2 = parse_typeql("define\nentity person, owns name, owns age;").unwrap();
        assert_eq!(s1.entities.get("person").unwrap().owns.len(), 2);
        assert_eq!(s2.entities.get("person").unwrap().owns.len(), 2);
    }

    // --- Function/struct parsing ---

    #[test]
    fn test_function_parsed_alongside_other_defs() {
        let schema = parse_typeql(
            "define\nattribute name, value string;\nfun calc($d: date) -> integer:\n  match $x = 1;\n  return 30;\nentity person, owns name;\n",
        ).unwrap();
        assert_eq!(schema.attributes.len(), 1);
        assert_eq!(schema.entities.len(), 1);
        assert_eq!(schema.functions.len(), 1);
        let func = schema.functions.get("calc").unwrap();
        assert_eq!(func.parameters.len(), 1);
        assert_eq!(func.parameters[0].name, "d");
        assert_eq!(func.parameters[0].type_, "date");
        assert!(!func.return_type.is_stream);
        assert_eq!(func.return_type.types.len(), 1);
        assert_eq!(func.return_type.types[0].name, "integer");
        assert!(!func.return_type.types[0].optional);
    }

    #[test]
    fn test_struct_parsed_alongside_other_defs() {
        let schema = parse_typeql(
            "define\nattribute name, value string;\nstruct pn, value first string, value last string;\nentity person, owns name;\n",
        ).unwrap();
        assert_eq!(schema.attributes.len(), 1);
        assert_eq!(schema.entities.len(), 1);
        assert_eq!(schema.structs.len(), 1);
        let st = schema.structs.get("pn").unwrap();
        assert_eq!(st.fields.len(), 2);
        assert_eq!(st.fields[0].name, "first");
        assert_eq!(st.fields[1].name, "last");
    }

    // --- Structured return-type contract ---

    /// Assert a return type's items match `(name, optional)` pairs in order.
    fn assert_return_items(rt: &ReturnType, expected: &[(&str, bool)]) {
        assert_eq!(
            rt.types.len(),
            expected.len(),
            "item count mismatch: {:?}",
            rt.types
        );
        for (item, (name, optional)) in rt.types.iter().zip(expected) {
            assert_eq!(item.name, *name);
            assert_eq!(item.optional, *optional);
        }
    }

    #[test]
    fn test_function_return_stream_single() {
        let schema = parse_typeql("define\nfun f() -> { int }: return 1;").unwrap();
        let rt = &schema.functions.get("f").unwrap().return_type;
        assert!(rt.is_stream);
        assert_return_items(rt, &[("int", false)]);
    }

    #[test]
    fn test_function_return_stream_multi() {
        let schema = parse_typeql("define\nfun f() -> { user, phone, string }: return 1;").unwrap();
        let rt = &schema.functions.get("f").unwrap().return_type;
        assert!(rt.is_stream);
        assert_return_items(rt, &[("user", false), ("phone", false), ("string", false)]);
    }

    #[test]
    fn test_function_return_scalar_single() {
        let schema = parse_typeql("define\nfun f() -> integer: return 1;").unwrap();
        let rt = &schema.functions.get("f").unwrap().return_type;
        assert!(!rt.is_stream);
        assert_return_items(rt, &[("integer", false)]);
    }

    #[test]
    fn test_function_return_scalar_tuple() {
        let schema = parse_typeql("define\nfun f() -> integer, integer: return 1;").unwrap();
        let rt = &schema.functions.get("f").unwrap().return_type;
        assert!(!rt.is_stream);
        assert_return_items(rt, &[("integer", false), ("integer", false)]);
    }

    #[test]
    fn test_function_return_bool() {
        let schema = parse_typeql("define\nfun f() -> bool: return true;").unwrap();
        let rt = &schema.functions.get("f").unwrap().return_type;
        assert!(!rt.is_stream);
        assert_return_items(rt, &[("bool", false)]);
    }

    #[test]
    fn test_function_return_optional_token() {
        let schema = parse_typeql("define\nfun f() -> place, name?: return 1;").unwrap();
        let rt = &schema.functions.get("f").unwrap().return_type;
        assert!(!rt.is_stream);
        assert_return_items(rt, &[("place", false), ("name", true)]);
    }

    #[test]
    fn test_function_multi_param() {
        let schema =
            parse_typeql("define\nfun f($birth-date: date, $n: integer) -> bool: return true;")
                .unwrap();
        let func = schema.functions.get("f").unwrap();
        assert_eq!(func.parameters.len(), 2);
        assert_eq!(func.parameters[0].name, "birth-date");
        assert_eq!(func.parameters[0].type_, "date");
        assert_eq!(func.parameters[1].name, "n");
        assert_eq!(func.parameters[1].type_, "integer");
    }

    #[test]
    fn test_function_zero_params() {
        let schema = parse_typeql("define\nfun f() -> integer: return 1;").unwrap();
        let func = schema.functions.get("f").unwrap();
        assert!(func.parameters.is_empty());
    }

    #[test]
    fn test_struct_with_optional_field() {
        let schema =
            parse_typeql("define\nstruct person-name, value first string, value middle string?;")
                .unwrap();
        let st = schema.structs.get("person-name").unwrap();
        assert_eq!(st.fields.len(), 2);
        assert_eq!(st.fields[0].name, "first");
        assert_eq!(st.fields[0].value_type, "string");
        assert!(!st.fields[0].optional);
        assert_eq!(st.fields[1].name, "middle");
        assert_eq!(st.fields[1].value_type, "string");
        assert!(st.fields[1].optional);
    }

    #[test]
    fn test_function_param_type_serde_key() {
        let schema = parse_typeql("define\nfun f($x: integer) -> integer: return 1;").unwrap();
        let func = schema.functions.get("f").unwrap();
        let json = serde_json::to_string(&func.parameters[0]).unwrap();
        assert!(json.contains("\"type\""), "expected \"type\" key: {}", json);
        assert!(!json.contains("type_"), "type_ leaked into JSON: {}", json);
    }

    // --- Multiple define blocks ---

    #[test]
    fn test_multiple_define_blocks() {
        let schema = parse_typeql(
            "\ndefine\nattribute name, value string;\n\ndefine\nentity person, owns name;\n",
        )
        .unwrap();
        assert_eq!(schema.attributes.len(), 1);
        assert_eq!(schema.entities.len(), 1);
    }

    // --- Complex schema ---

    #[test]
    fn test_parse_complex_schema() {
        let schema = parse_typeql(
            "\
define

# Attributes
attribute name, value string;
attribute age, value integer;
attribute email, value string;
attribute isbn, value string;
attribute title, value string;
attribute pages, value integer;
attribute price, value double;
attribute published, value boolean;

# Entities
entity person @abstract
    , owns name @key
    , owns age
    , owns email @unique
    , plays friendship:friend
    , plays authoring:author;

entity employee sub person;

entity book @abstract
    , owns isbn @key
    , owns title
    , owns pages;

entity novel sub book
    , owns price;

# Relations
relation friendship
    , relates friend @card(2..2);

relation contribution @abstract
    , relates contributor
    , relates work;

relation authoring sub contribution
    , relates author as contributor
    , relates book as work;
",
        )
        .unwrap();

        assert_eq!(schema.attributes.len(), 8);
        assert_eq!(schema.entities.len(), 4);
        assert_eq!(schema.relations.len(), 3);

        let person = schema.entities.get("person").unwrap();
        assert!(person.is_abstract);
        assert_eq!(person.owns.len(), 3);
        assert!(person.owns.iter().any(|o| o.name == "name" && o.is_key));
        assert!(person.owns.iter().any(|o| o.name == "email" && o.is_unique));
        assert_eq!(person.plays.len(), 2);
    }

    // --- Error handling ---

    #[test]
    fn test_parse_error_no_define() {
        assert!(parse_typeql("attribute name, value string;").is_err());
    }

    #[test]
    fn test_parse_error_trailing_content() {
        assert!(parse_typeql("define\nattribute name, value string;\ngarbage").is_err());
    }

    // --- Full round-trip with inheritance ---

    #[test]
    fn test_parse_and_resolve() {
        let schema = TypeSchema::from_typeql(
            "\
define
attribute name, value string;
attribute age, value integer;

entity person
    , owns name @key
    , owns age
    , plays friendship:friend;

entity employee sub person;

relation friendship
    , relates friend @card(2..2);
",
        )
        .unwrap();

        let employee = schema.get_entity("employee").unwrap();
        assert_eq!(employee.owns.len(), 2);
        assert_eq!(employee.plays.len(), 1);

        let attrs = schema.get_all_owned_attributes("employee");
        assert_eq!(attrs.len(), 2);

        let roles = schema.get_all_plays_roles("employee");
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].role_ref, "friendship:friend");
    }

    #[test]
    fn test_empty_input() {
        let schema = parse_typeql("").unwrap();
        assert!(schema.entities.is_empty());
    }

    #[test]
    fn test_only_comments() {
        let schema = parse_typeql("# comment\n// another\n").unwrap();
        assert!(schema.entities.is_empty());
    }

    #[test]
    fn test_parse_multiline_entity() {
        let schema = parse_typeql(
            "\
define
entity person
    , owns name @key
    , owns age @card(0..1)
    , owns email @unique
    , plays friendship:friend @card(0..10)
    , plays employment:employee;
",
        )
        .unwrap();
        let person = schema.entities.get("person").unwrap();
        assert_eq!(person.owns.len(), 3);
        assert_eq!(person.plays.len(), 2);
        assert!(person.owns[0].is_key);
        assert_eq!(
            person.owns[1].cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(1)
            })
        );
        assert!(person.owns[2].is_unique);
        assert_eq!(
            person.plays[0].cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(10)
            })
        );
        assert!(person.plays[1].cardinality.is_none());
    }

    // --- @range validation tests ---

    #[test]
    fn test_range_no_dotdot_fails() {
        let result = parse_typeql("define\nattribute age, value integer, @range(100);");
        assert!(result.is_err());
    }

    #[test]
    fn test_range_comma_fails() {
        let result = parse_typeql("define\nattribute age, value integer, @range(1, 5);");
        assert!(result.is_err());
    }

    #[test]
    fn test_range_valid_passes() {
        let schema = parse_typeql("define\nattribute age, value integer, @range(0..150);").unwrap();
        let attr = schema.attributes.get("age").unwrap();
        assert_eq!(attr.range_min.as_deref(), Some("0"));
        assert_eq!(attr.range_max.as_deref(), Some("150"));
    }

    #[test]
    fn test_range_open_ended_min() {
        let schema = parse_typeql("define\nattribute age, value integer, @range(0..);").unwrap();
        let attr = schema.attributes.get("age").unwrap();
        assert_eq!(attr.range_min.as_deref(), Some("0"));
        assert!(attr.range_max.is_none());
    }

    #[test]
    fn test_range_open_ended_max() {
        let schema = parse_typeql("define\nattribute age, value integer, @range(..150);").unwrap();
        let attr = schema.attributes.get("age").unwrap();
        assert!(attr.range_min.is_none());
        assert_eq!(attr.range_max.as_deref(), Some("150"));
    }

    #[test]
    fn test_parse_standalone_plays() {
        let schema =
            parse_typeql("define\nentity person;\nperson plays friendship:friend;\n").unwrap();
        let person = schema.entities.get("person").unwrap();
        assert_eq!(person.plays.len(), 1);
        assert_eq!(person.plays[0].role_ref, "friendship:friend");
    }

    // -----------------------------------------------------------------------
    // @doc / @meta annotations (TypeDB 3.12+)
    // -----------------------------------------------------------------------

    #[test]
    fn test_doc_meta_on_type_headers() {
        let schema = parse_typeql(
            r#"define
attribute name @doc("a personal name") @meta("pii", "true"), value string;
entity person @doc("an individual client") @meta("icon", "silhouette.png"),
  owns name;
relation friendship @doc("a mutual bond") @meta("color", "blue"),
  relates friend;
"#,
        )
        .unwrap();
        let attr = schema.attributes.get("name").unwrap();
        assert_eq!(attr.doc.as_deref(), Some("a personal name"));
        assert_eq!(attr.meta.get("pii").map(String::as_str), Some("true"));
        let person = schema.entities.get("person").unwrap();
        assert_eq!(person.doc.as_deref(), Some("an individual client"));
        assert_eq!(
            person.meta.get("icon").map(String::as_str),
            Some("silhouette.png")
        );
        let friendship = schema.relations.get("friendship").unwrap();
        assert_eq!(friendship.doc.as_deref(), Some("a mutual bond"));
        assert_eq!(
            friendship.meta.get("color").map(String::as_str),
            Some("blue")
        );
    }

    #[test]
    fn test_doc_meta_on_capabilities() {
        let schema = parse_typeql(
            r#"define
attribute name, value string;
entity person,
  owns name @key @doc("full legal name") @meta("column", "name"),
  plays friendship:friend @doc("participation") @meta("edge", "true");
relation friendship,
  relates friend @card(0..2) @doc("one side of the bond") @meta("endpoint", "true");
"#,
        )
        .unwrap();
        let person = schema.entities.get("person").unwrap();
        let owns = &person.owns[0];
        assert!(owns.is_key);
        assert_eq!(owns.doc.as_deref(), Some("full legal name"));
        assert_eq!(owns.meta.get("column").map(String::as_str), Some("name"));
        let plays = &person.plays[0];
        assert_eq!(plays.doc.as_deref(), Some("participation"));
        assert_eq!(plays.meta.get("edge").map(String::as_str), Some("true"));
        let friendship = schema.relations.get("friendship").unwrap();
        let role = &friendship.roles[0];
        assert_eq!(
            role.cardinality,
            Some(Cardinality {
                min: 0,
                max: Some(2)
            })
        );
        assert_eq!(role.doc.as_deref(), Some("one side of the bond"));
        assert_eq!(role.meta.get("endpoint").map(String::as_str), Some("true"));
    }

    #[test]
    fn test_doc_meta_any_order_with_constraints() {
        // Annotations may appear in any order relative to @key/@card.
        let schema = parse_typeql(
            r#"define
attribute name, value string;
entity person,
  owns name @doc("d") @key @meta("k", "v") @card(1..1);
"#,
        )
        .unwrap();
        let owns = &schema.entities.get("person").unwrap().owns[0];
        assert!(owns.is_key);
        assert_eq!(owns.doc.as_deref(), Some("d"));
        assert_eq!(owns.meta.get("k").map(String::as_str), Some("v"));
        assert_eq!(
            owns.cardinality,
            Some(Cardinality {
                min: 1,
                max: Some(1)
            })
        );
    }

    #[test]
    fn test_multiple_meta_keys() {
        let schema =
            parse_typeql("define\nentity gadget @meta(\"k1\", \"v1\") @meta(\"k2\", \"v2\");\n")
                .unwrap();
        let gadget = schema.entities.get("gadget").unwrap();
        assert_eq!(gadget.meta.len(), 2);
        assert_eq!(gadget.meta.get("k1").map(String::as_str), Some("v1"));
        assert_eq!(gadget.meta.get("k2").map(String::as_str), Some("v2"));
    }

    #[test]
    fn test_doc_with_escapes() {
        // Matches the exact escape rendering of the TypeDB 3.12 schema export.
        let schema =
            parse_typeql("define\nentity escbox @doc(\"line1\\nline2 with \\\"quotes\\\"\");\n")
                .unwrap();
        let escbox = schema.entities.get("escbox").unwrap();
        assert_eq!(escbox.doc.as_deref(), Some("line1\nline2 with \"quotes\""));
    }

    #[test]
    fn test_doc_meta_live_312_export_shape() {
        // Exact shape captured from `databases.get(db).schema()` on TypeDB 3.12.0
        // (see tmp/probe-312-annotations.md): annotated attribute header with the
        // value clause on its own line, and capability annotations after @card.
        let schema = parse_typeql(
            "define\n\nattribute name @doc(\"a personal name\") @meta(\"pii\", \"true\"),\n value string;\nattribute nickname,\n value string;\nentity person,\n  owns name @meta(\"column\", \"name\"),\n  owns nickname,\n  plays friendship:friend @doc(\"participation in friendship\") @meta(\"edge\", \"true\");\nrelation friendship @doc(\"a mutual bond\") @meta(\"color\", \"blue\"),\n  relates friend @card(0..2) @doc(\"one side of the bond\") @meta(\"endpoint\", \"true\");\n",
        )
        .unwrap();
        assert_eq!(
            schema.attributes.get("name").unwrap().doc.as_deref(),
            Some("a personal name")
        );
        let person = schema.entities.get("person").unwrap();
        assert!(person.doc.is_none());
        assert_eq!(
            person.owns[0].meta.get("column").map(String::as_str),
            Some("name")
        );
        let friendship = schema.relations.get("friendship").unwrap();
        assert_eq!(
            friendship.roles[0].doc.as_deref(),
            Some("one side of the bond")
        );
    }

    #[test]
    fn test_doc_meta_split_definition_merge() {
        let schema = parse_typeql(
            "define\nentity person @meta(\"k1\", \"v1\");\nentity person @doc(\"d\") @meta(\"k2\", \"v2\");\n",
        )
        .unwrap();
        let person = schema.entities.get("person").unwrap();
        assert_eq!(person.doc.as_deref(), Some("d"));
        assert_eq!(person.meta.len(), 2);
    }
}
